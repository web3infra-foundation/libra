#![allow(dead_code)]
//! Data-driven matrix runner for the Code UI Remote L2 test suite.
//!
//! Each JSON case in `tests/data/code_ui_remote/*.json` is mapped to a
//! sequence of typed [`Step`]s and run against a fresh [`CodeSession`]. The
//! runner is intentionally minimal: it knows about a fixed catalogue of
//! `op` / `auth` / `token` / `assertion` strings so a stale data fixture
//! fails loud at deserialization time rather than silently changing
//! behaviour.
//!
//! Per-case JSON shape (subset; see `docs/improvement/test.md`):
//!
//! ```jsonc
//! {
//!   "schemaVersion": 1,
//!   "defaults": {
//!     "fixture": { "path": "tests/fixtures/code_ui/basic_chat.json" },
//!     "options": { "control": "write", "browserControl": "off", "leaseDurationMs": null }
//!   },
//!   "cases": [
//!     {
//!       "name": "...",
//!       "priority": "P0",
//!       "options": { "leaseDurationMs": 500 },
//!       "steps": [ { "op": "attach", "kind": "automation", ... } ]
//!     }
//!   ]
//! }
//! ```

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Output,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::Value;

use super::{CodeSession, CodeSessionOptions, EventStream, SseEvent};

/// Loaded matrix file. Cases are kept as raw JSON values and only
/// deserialised into typed [`Case`]s on demand by [`find_case`].
///
/// Why lazy: each Wave (`docs/improvement/test.md`) lands new
/// `Step` variants alongside the runner code. If we deserialised
/// every case upfront, Wave 1's runner would refuse to load the
/// shared `sse_cases.json` file just because Wave 2's case relies
/// on a `collectEventsUntil` step the Wave 1 runner doesn't
/// implement yet. Per-case deserialisation lets each Wave run only
/// the cases it has wired up while leaving the JSON file as the
/// shared source of truth.
#[derive(Debug, Deserialize)]
pub struct CaseFile {
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    #[allow(dead_code)]
    pub matrix: String,
    pub defaults: Defaults,
    pub cases: Vec<Value>,
}

#[derive(Debug, Deserialize, Default)]
pub struct Defaults {
    pub fixture: FixtureRef,
    #[serde(default)]
    pub options: CaseOptions,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct FixtureRef {
    pub path: String,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct CaseOptions {
    #[serde(default)]
    pub control: Option<String>,
    #[serde(default, rename = "browserControl")]
    pub browser_control: Option<String>,
    #[serde(default, rename = "leaseDurationMs")]
    pub lease_duration_ms: Option<u64>,
    /// Wave 5 / PR 5 — operating context (`dev` / `review` /
    /// `research`). When set, the harness spawns
    /// `libra code --context <value>`; generation cases use `"dev"`
    /// so `apply_patch` survives the intent classifier's
    /// allow-list filter.
    #[serde(default)]
    pub context: Option<String>,
    /// Wave 5 / PR 5 — `--approval-policy` override. Generation
    /// cases use `"never"` so workspace-bounded apply_patch skips
    /// the human approval gate.
    #[serde(default, rename = "approvalPolicy")]
    pub approval_policy: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Case {
    pub name: String,
    #[allow(dead_code)]
    pub priority: String,
    /// Per-case fixture override. Wave 2's
    /// `sse_streaming_fixture_transcript_content_grows_monotonically`
    /// case in `sse_cases.json` requires the streaming fixture
    /// instead of the file's default basic-chat fixture. Codex
    /// pass-1 P3: surfacing it here lets `build_session_options`
    /// honour the override deterministically.
    #[serde(default)]
    pub fixture: Option<FixtureRef>,
    #[serde(default)]
    pub options: CaseOptions,
    pub steps: Vec<Step>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Step {
    Attach {
        name: String,
        kind: String,
        #[serde(rename = "clientId")]
        client_id: String,
        #[serde(default = "default_auth")]
        auth: AuthMode,
        expect: AttachExpect,
    },
    Detach {
        name: String,
        #[serde(rename = "clientId")]
        client_id: String,
        token: TokenSource,
        #[serde(default = "default_auth")]
        auth: AuthMode,
        expect: SimpleExpect,
    },
    Submit {
        name: String,
        text: String,
        token: TokenSource,
        #[serde(default = "default_auth")]
        auth: AuthMode,
        expect: SimpleExpect,
    },
    Sleep {
        name: String,
        #[serde(rename = "durationMs")]
        duration_ms: u64,
    },
    WaitSnapshot {
        name: String,
        expect: AssertionsExpect,
    },
    /// Open a new SSE subscription against `/api/code/events` and
    /// label it with `stream` so later steps can wait for events on
    /// it. `timeoutMs` is the wait budget for individual reads on
    /// this stream (NOT the open call itself).
    ///
    /// `closeImmediately` lets reconnect tests open a stream just
    /// to consume the initial replay and then drop it before any
    /// later submit fires; downstream Waves (SSE reconnect case)
    /// rely on this.
    OpenEvents {
        name: String,
        stream: String,
        #[serde(default = "default_event_timeout_ms", rename = "timeoutMs")]
        timeout_ms: u64,
        #[serde(default, rename = "closeImmediately")]
        close_immediately: bool,
    },
    /// Read the very next event off `stream` and assert it has the
    /// requested `event:` field plus all expected assertions. Use
    /// this when the next event is deterministic (e.g. SSE initial
    /// replay always emits `session_updated` first).
    ExpectEvent {
        name: String,
        stream: String,
        event: String,
        #[serde(default = "default_event_timeout_ms", rename = "timeoutMs")]
        timeout_ms: u64,
        expect: AssertionsExpect,
    },
    /// Wave 4 / PR 4 — drain events off `stream` until one with
    /// the requested `event:` field arrives. Intermediate events
    /// of other types are accepted and discarded silently. Used
    /// for cases where the order or count of supporting events
    /// (e.g. status_changed bursts) is not stable but the matrix
    /// still wants to assert the eventual state-change event
    /// matches its assertions.
    CollectEventsUntil {
        name: String,
        stream: String,
        event: String,
        #[serde(default = "default_event_timeout_ms", rename = "timeoutMs")]
        timeout_ms: u64,
        expect: AssertionsExpect,
    },
    /// Wave 4 / PR 4 — drain every `session_updated` event off
    /// `stream` until either:
    ///
    ///   * the snapshot contained in the latest event has
    ///     `status == "idle"` (terminal state), OR
    ///   * `timeoutMs` elapses (whichever comes first).
    ///
    /// Run multi-event assertions on the COLLECTED sequence — e.g.
    /// `assistant_content_monotonic` walks the assistant message
    /// content across each session_updated and asserts it grows
    /// monotonically (no truncation, no shrink).
    CollectSessionUpdates {
        name: String,
        stream: String,
        #[serde(default = "default_event_timeout_ms", rename = "timeoutMs")]
        timeout_ms: u64,
        expect: AssertionsExpect,
    },
    /// Wave 4 / PR 4 — submit then poll `/session` until the
    /// session is back to `status == "idle"`. Used by the SSE
    /// reconnect case where we need to ensure the assistant's
    /// reply is fully recorded BEFORE re-opening the stream so
    /// the new initial-replay snapshot contains it.
    SubmitAndWaitIdle {
        name: String,
        text: String,
        token: TokenSource,
        #[serde(default = "default_auth")]
        auth: AuthMode,
        expect: SimpleExpect,
    },
    /// Wave 5 / PR 5 — submit then poll `/session` until the
    /// session reaches a terminal state — either `idle` or `error`.
    /// Generation cases use this for the fault-injection branch
    /// (invalid patch) where the runtime is allowed to surface an
    /// error in the transcript without the test mistaking it for
    /// hang. Assertions on the FINAL snapshot (status / transcript)
    /// run via the `assertions` list, not via a follow-up
    /// `WaitSnapshot` step, so the matrix file stays terse.
    SubmitAndWaitTerminal {
        name: String,
        text: String,
        token: TokenSource,
        #[serde(default = "default_auth")]
        auth: AuthMode,
        expect: SimpleExpect,
    },
    /// Wave 5 / PR 5 — read a file from the spawned `libra code`
    /// working directory and run `file_contains:<needle>` /
    /// `file_contains_any:<a>|<b>` assertions over its contents.
    /// Used by the apply_patch generation cases to verify the patch
    /// landed and the produced source compiles when downstream
    /// `Step::RunRepoCommand` invokes `rustc`.
    ReadRepoFile {
        name: String,
        path: String,
        expect: AssertionsExpect,
    },
    /// Wave 5 / PR 5 — assert a path under the working directory
    /// does NOT exist. Used by the invalid-patch branch to prove
    /// the runtime did not leave a half-written file behind.
    RepoFileAbsent { name: String, path: String },
    /// Wave 5 / PR 5 — run a shell command inside the spawned
    /// working directory with a hard `timeout_ms` budget; stdout
    /// and stderr are captured so the matrix can assert on them via
    /// `stdout_or_stderr_contains:<needle>`. The expected exit code
    /// is matched against `expect.status` (default 0).
    RunRepoCommand {
        name: String,
        command: Vec<String>,
        #[serde(default = "default_run_command_timeout_ms", rename = "timeoutMs")]
        timeout_ms: u64,
        expect: RunCommandExpect,
    },
}

fn default_event_timeout_ms() -> u64 {
    5_000
}

/// 10 s default for `Step::RunRepoCommand` — matches the smoke
/// timeout used elsewhere for tool-driven workflows. JSON cases
/// override this when their command (e.g. `rustc --test`) needs a
/// larger budget.
fn default_run_command_timeout_ms() -> u64 {
    10_000
}

fn default_auth() -> AuthMode {
    AuthMode::None
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    None,
    /// Send the live process's `X-Libra-Control-Token` header.
    ValidControl,
    /// Omit `X-Libra-Control-Token` even on routes that require it.
    MissingControl,
    /// Send a clearly-bogus `X-Libra-Control-Token`.
    InvalidControl,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TokenSource {
    /// No `X-Code-Controller-Token` header.
    None,
    /// The most recently saved token under the `current` slot.
    Current,
    /// A token previously saved into the `stale` slot.
    Stale,
    /// A clearly-bogus controller token (4242…42).
    Forged,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AttachExpect {
    pub status: u16,
    #[serde(default, rename = "errorCode")]
    pub error_code: Option<String>,
    #[serde(default, rename = "saveToken")]
    pub save_token: Option<TokenSlot>,
    #[serde(default, rename = "saveLeaseExpiresAt")]
    pub save_lease_expires_at: Option<String>,
    #[serde(default)]
    pub assertions: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SimpleExpect {
    pub status: u16,
    #[serde(default, rename = "errorCode")]
    pub error_code: Option<String>,
    #[serde(default)]
    pub assertions: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssertionsExpect {
    #[serde(default)]
    pub assertions: Vec<String>,
}

/// Expectation envelope for `Step::RunRepoCommand`. `status`
/// defaults to 0 when absent so the common "must succeed" case
/// stays terse in JSON. Set it to `null` only when the command is
/// allowed to exit non-zero and only the captured output matters.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct RunCommandExpect {
    #[serde(default = "default_run_command_status")]
    pub status: Option<i32>,
    #[serde(default)]
    pub assertions: Vec<String>,
}

fn default_run_command_status() -> Option<i32> {
    Some(0)
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum TokenSlot {
    Current,
    Stale,
}

/// Shared state across the steps of a single case.
struct CaseRuntime<'a> {
    session: &'a mut CodeSession,
    case_name: &'a str,
    /// Saved `controllerToken` values, keyed by slot.
    tokens: HashMap<TokenSlot, String>,
    /// Saved `leaseExpiresAt` values, keyed by user-supplied label.
    lease_timestamps: HashMap<String, chrono::DateTime<chrono::Utc>>,
    /// Open SSE subscriptions, keyed by the user-supplied `stream`
    /// label. Streams persist across steps so a single case can
    /// `openEvents` early and `expectEvent` later.
    streams: HashMap<String, EventStream>,
}

const FORGED_CONTROLLER_TOKEN: &str = "42424242-4242-4242-4242-424242424242";

/// Resolve a case-file path under `CARGO_MANIFEST_DIR`.
pub fn data_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

/// Repo-root-relative fixture path → absolute path.
pub fn repo_root_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

/// Load and parse the matrix file at `path`.
pub fn load_case_file(path: &Path) -> Result<CaseFile> {
    let bytes = fs::read(path)
        .with_context(|| format!("failed to read matrix data file '{}'", path.display()))?;
    let parsed: CaseFile = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse matrix data file '{}'", path.display()))?;
    if parsed.schema_version != 1 {
        bail!(
            "unsupported matrix schemaVersion {} in '{}'",
            parsed.schema_version,
            path.display(),
        );
    }
    Ok(parsed)
}

/// Locate a case by name in the loaded file and deserialise it
/// into a typed [`Case`]. The returned value is owned because
/// each case in the file is stored as raw JSON until requested
/// (see the doc-comment on [`CaseFile`] for why).
pub fn find_case(file: &CaseFile, name: &str) -> Result<Case> {
    let raw = file
        .cases
        .iter()
        .find(|c| {
            c.get("name")
                .and_then(Value::as_str)
                .is_some_and(|n| n == name)
        })
        .ok_or_else(|| anyhow!("matrix case '{name}' not present in case file"))?;
    serde_json::from_value::<Case>(raw.clone())
        .with_context(|| format!("failed to deserialise matrix case '{name}'"))
}

/// Merge per-case options on top of the file-level defaults.
fn effective_options(defaults: &CaseOptions, case: &CaseOptions) -> CaseOptions {
    CaseOptions {
        control: case.control.clone().or_else(|| defaults.control.clone()),
        browser_control: case
            .browser_control
            .clone()
            .or_else(|| defaults.browser_control.clone()),
        lease_duration_ms: case.lease_duration_ms.or(defaults.lease_duration_ms),
        context: case.context.clone().or_else(|| defaults.context.clone()),
        approval_policy: case
            .approval_policy
            .clone()
            .or_else(|| defaults.approval_policy.clone()),
    }
}

/// Build a [`CodeSessionOptions`] from the matrix config, including
/// per-case overrides for `control`, `leaseDurationMs`, and
/// `fixture`. Codex pass-1 P3: case-level `fixture` overrides the
/// file's default, matching the JSON schema documented in
/// `docs/improvement/test.md`.
pub fn build_session_options(file: &CaseFile, case: &Case) -> CodeSessionOptions {
    let merged = effective_options(&file.defaults.options, &case.options);
    let fixture_path = case
        .fixture
        .as_ref()
        .unwrap_or(&file.defaults.fixture)
        .path
        .clone();
    let fixture = repo_root_path(&fixture_path);
    let mut options = CodeSessionOptions::new(case.name.clone(), fixture);
    if let Some(control) = merged.control.as_deref() {
        match control {
            "write" => {} // default
            "observe" => {
                options = options.with_control_observe();
            }
            other => panic!(
                "matrix case '{}' uses unsupported control mode '{other}'",
                case.name,
            ),
        }
    }
    if matches!(merged.browser_control.as_deref(), Some("loopback")) {
        options = options.with_browser_control_loopback();
    }
    if let Some(ms) = merged.lease_duration_ms {
        options = options.with_lease_duration_ms(ms);
    }
    if let Some(context) = merged.context.as_deref() {
        options = options.with_context(context);
    }
    if let Some(policy) = merged.approval_policy.as_deref() {
        options = options.with_approval_policy(policy);
    }
    options
}

/// Run an entire case top-to-bottom against a fresh session. Caller is
/// responsible for spawning + shutting down the session so the lifetime is
/// visible in `cargo test` output (each case becomes its own `#[test]`).
pub fn run_case(session: &mut CodeSession, case: &Case) -> Result<()> {
    let mut runtime = CaseRuntime {
        session,
        case_name: &case.name,
        tokens: HashMap::new(),
        lease_timestamps: HashMap::new(),
        streams: HashMap::new(),
    };
    for (idx, step) in case.steps.iter().enumerate() {
        let step_name = step_name(step);
        runtime
            .run_step(step)
            .with_context(|| format!("case '{}' step #{idx} ({step_name}) failed", case.name))?;
    }
    Ok(())
}

fn step_name(step: &Step) -> &str {
    match step {
        Step::Attach { name, .. } => name,
        Step::Detach { name, .. } => name,
        Step::Submit { name, .. } => name,
        Step::Sleep { name, .. } => name,
        Step::WaitSnapshot { name, .. } => name,
        Step::OpenEvents { name, .. } => name,
        Step::ExpectEvent { name, .. } => name,
        Step::CollectEventsUntil { name, .. } => name,
        Step::CollectSessionUpdates { name, .. } => name,
        Step::SubmitAndWaitIdle { name, .. } => name,
        Step::SubmitAndWaitTerminal { name, .. } => name,
        Step::ReadRepoFile { name, .. } => name,
        Step::RepoFileAbsent { name, .. } => name,
        Step::RunRepoCommand { name, .. } => name,
    }
}

impl CaseRuntime<'_> {
    fn run_step(&mut self, step: &Step) -> Result<()> {
        match step {
            Step::Attach {
                kind,
                client_id,
                auth,
                expect,
                ..
            } => self.run_attach(kind, client_id, auth, expect),
            Step::Detach {
                client_id,
                token,
                auth,
                expect,
                ..
            } => self.run_detach(client_id, token, auth, expect),
            Step::Submit {
                text,
                token,
                auth,
                expect,
                ..
            } => self.run_submit(text, token, auth, expect),
            Step::Sleep { duration_ms, .. } => {
                std::thread::sleep(Duration::from_millis(*duration_ms));
                Ok(())
            }
            Step::WaitSnapshot { expect, .. } => self.run_wait_snapshot(expect),
            Step::OpenEvents {
                stream,
                close_immediately,
                ..
            } => self.run_open_events(stream, *close_immediately),
            Step::ExpectEvent {
                stream,
                event,
                timeout_ms,
                expect,
                ..
            } => self.run_expect_event(stream, event, *timeout_ms, expect),
            Step::CollectEventsUntil {
                stream,
                event,
                timeout_ms,
                expect,
                ..
            } => self.run_collect_events_until(stream, event, *timeout_ms, expect),
            Step::CollectSessionUpdates {
                stream,
                timeout_ms,
                expect,
                ..
            } => self.run_collect_session_updates(stream, *timeout_ms, expect),
            Step::SubmitAndWaitIdle {
                text,
                token,
                auth,
                expect,
                ..
            } => self.run_submit_and_wait_idle(text, token, auth, expect),
            Step::SubmitAndWaitTerminal {
                text,
                token,
                auth,
                expect,
                ..
            } => self.run_submit_and_wait_terminal(text, token, auth, expect),
            Step::ReadRepoFile { path, expect, .. } => self.run_read_repo_file(path, expect),
            Step::RepoFileAbsent { path, .. } => self.run_repo_file_absent(path),
            Step::RunRepoCommand {
                command,
                timeout_ms,
                expect,
                ..
            } => self.run_run_repo_command(command, *timeout_ms, expect),
        }
    }

    fn run_open_events(&mut self, stream: &str, close_immediately: bool) -> Result<()> {
        // Open the SSE subscription. The downstream Wave 2 case
        // `sse_reconnect_initial_replay_contains_latest_transcript`
        // depends on `closeImmediately` to consume the initial
        // replay then drop the stream before any later submit.
        let mut event_stream = self
            .session
            .open_event_stream()
            .with_context(|| format!("failed to open SSE stream '{stream}'"))?;
        if close_immediately {
            event_stream.close();
            return Ok(());
        }
        if self
            .streams
            .insert(stream.to_string(), event_stream)
            .is_some()
        {
            bail!(
                "case '{}' opened SSE stream label '{stream}' twice",
                self.case_name
            );
        }
        Ok(())
    }

    fn run_expect_event(
        &mut self,
        stream: &str,
        event: &str,
        timeout_ms: u64,
        expect: &AssertionsExpect,
    ) -> Result<()> {
        let event_stream = self.streams.get_mut(stream).ok_or_else(|| {
            anyhow!(
                "case '{}' references SSE stream '{stream}' before openEvents",
                self.case_name
            )
        })?;
        let timeout = Duration::from_millis(timeout_ms);
        let received = event_stream
            .next_event(timeout)?
            .ok_or_else(|| anyhow!("timed out waiting for SSE event '{event}' on '{stream}'"))?;
        if received.event != event {
            bail!(
                "expected SSE event '{event}' on '{stream}', got '{}': {}",
                received.event,
                received.data
            );
        }
        let payload = parse_event_data(&received).with_context(|| {
            format!(
                "case '{}' SSE event '{}' had invalid JSON payload",
                self.case_name, received.event
            )
        })?;
        for assertion in &expect.assertions {
            evaluate_event_assertion(assertion, &received, &payload).with_context(|| {
                format!(
                    "case '{}' SSE assertion '{assertion}' failed; payload: {payload}",
                    self.case_name
                )
            })?;
        }
        Ok(())
    }

    fn run_collect_events_until(
        &mut self,
        stream: &str,
        target_event: &str,
        timeout_ms: u64,
        expect: &AssertionsExpect,
    ) -> Result<()> {
        let event_stream = self.streams.get_mut(stream).ok_or_else(|| {
            anyhow!(
                "case '{}' references SSE stream '{stream}' before openEvents",
                self.case_name
            )
        })?;
        // Per-event budget within an overall deadline so a stream
        // that drips one stale event per second can't quietly
        // consume the entire test budget without ever reaching
        // the target.
        //
        // Wave 4 fix: the initial-replay `session_updated` carries
        // the snapshot at SUBSCRIPTION time (typically idle, empty
        // transcript). For an assertion like
        // `event_transcript_contains:<reply>` the first matching
        // event won't satisfy it — the assistant hasn't streamed
        // yet. Treat assertion failure as "this isn't the event we
        // want, keep waiting" and only surface the LAST error on
        // timeout. The rule guarantees we don't silently lose a
        // genuinely failing assertion: if the deadline elapses,
        // the caller still sees the diagnostic from the most
        // recent matching event.
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let mut last_error: Option<anyhow::Error> = None;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                if let Some(error) = last_error {
                    return Err(error.context(format!(
                        "case '{}' timed out after {timeout_ms}ms waiting for matching '{target_event}' on '{stream}'",
                        self.case_name,
                    )));
                }
                bail!(
                    "case '{}' timed out after {timeout_ms}ms waiting for SSE event '{target_event}' on '{stream}'",
                    self.case_name,
                );
            }
            let event = match event_stream.next_event(remaining)? {
                Some(event) => event,
                None => continue,
            };
            if event.event != target_event {
                continue;
            }
            let payload = parse_event_data(&event).with_context(|| {
                format!(
                    "case '{}' SSE event '{}' had invalid JSON payload",
                    self.case_name, event.event
                )
            })?;
            let mut all_ok = true;
            for assertion in &expect.assertions {
                if let Err(error) = evaluate_event_assertion(assertion, &event, &payload) {
                    last_error = Some(error.context(format!(
                        "case '{}' SSE assertion '{assertion}' failed on payload: {payload}",
                        self.case_name
                    )));
                    all_ok = false;
                    break;
                }
            }
            if all_ok {
                return Ok(());
            }
        }
    }

    fn run_collect_session_updates(
        &mut self,
        stream: &str,
        timeout_ms: u64,
        expect: &AssertionsExpect,
    ) -> Result<()> {
        let event_stream = self.streams.get_mut(stream).ok_or_else(|| {
            anyhow!(
                "case '{}' references SSE stream '{stream}' before openEvents",
                self.case_name
            )
        })?;
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        // Wave 4 fix: the initial-replay `session_updated` is
        // always emitted with the snapshot at subscription time —
        // for a fresh session that's `status=idle, transcript=[]`.
        // Treating that initial idle as terminal would exit before
        // any streaming chunks arrive.
        //
        // Codex pass-1 P2 fix: the runtime emits `status_changed`
        // for status flips (see code_ui.rs `set_status`), NOT
        // `session_updated`. So the terminal "idle" signal we wait
        // for is a `status_changed` event whose snapshot has
        // `status == idle`, observed AFTER we've seen at least
        // one non-idle status_changed (which marks the start of
        // the turn). This avoids relying on timeout to terminate
        // the collector and unblocks fast/no-op runtimes too.
        let mut collected: Vec<Value> = Vec::new();
        let mut saw_non_idle = false;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let event = match event_stream.next_event(remaining)? {
                Some(event) => event,
                None => break,
            };
            if event.event != "session_updated" && event.event != "status_changed" {
                continue;
            }
            let payload = parse_event_data(&event).with_context(|| {
                format!(
                    "case '{}' SSE {} had invalid JSON payload",
                    self.case_name, event.event,
                )
            })?;
            let is_idle = payload
                .pointer("/data/status")
                .and_then(Value::as_str)
                .is_some_and(|status| status == "idle");
            // Track the turn lifecycle from BOTH event streams.
            // status_changed: thinking flips the gate; the
            // matching status_changed: idle (which fires after
            // every transcript mutation has already produced its
            // session_updated) closes the collection.
            if !is_idle {
                saw_non_idle = true;
            }
            // Only collect session_updated payloads — that's the
            // shape the multi-event assertions look at. The
            // status_changed events are observed purely for the
            // terminal-idle signal and dropped from the buffer.
            if event.event == "session_updated" {
                collected.push(payload);
            } else if is_idle && saw_non_idle {
                break;
            }
        }
        if collected.is_empty() {
            bail!(
                "case '{}' collected zero session_updated events on '{stream}' within {timeout_ms}ms",
                self.case_name,
            );
        }
        for assertion in &expect.assertions {
            evaluate_collected_assertion(assertion, &collected).with_context(|| {
                format!(
                    "case '{}' SSE multi-event assertion '{assertion}' failed across {} events",
                    self.case_name,
                    collected.len()
                )
            })?;
        }
        Ok(())
    }

    fn run_submit_and_wait_idle(
        &mut self,
        text: &str,
        token: &TokenSource,
        auth: &AuthMode,
        expect: &SimpleExpect,
    ) -> Result<()> {
        // Codex pass-1 P2 — capture the pre-submit transcript
        // length so the wait predicate AND the post-wait assertion
        // evaluator both ignore entries from prior turns. Without
        // this baseline a previously-completed assistant_message
        // would satisfy the predicate immediately, and any
        // `transcript_contains:<needle>` assertion could match a
        // stale entry instead of the response under test.
        let baseline_len = current_transcript_len(&self.session.snapshot()?);
        // Reuse the standard submit path so the response status /
        // error code semantics stay identical to `run_submit`.
        self.run_submit(text, token, auth, expect)?;
        // Then poll /session until the runtime drains the turn.
        // 10 s ceiling matches the lease/submit smoke timeout used
        // elsewhere in the harness.
        //
        // Wave 4 race fix (Codex pass-1 follow-up): a naive
        // "status == idle" wait can fire on the PRE-submit snapshot
        // — POST /messages returns before the runtime begins
        // processing, so the very next /session call may still
        // observe the initial idle state. To pin "the assistant
        // reply has actually landed", require BOTH:
        //   * status == idle (turn drained), AND
        //   * a NEW transcript entry (appended after `baseline_len`)
        //     is a completed assistant_message with non-empty
        //     content (the streaming completion marker the runtime
        //     sets when it flushes the final delta).
        //
        // Wave 5 fix: the assistant_message is no longer guaranteed
        // to be the LAST entry — apply_patch tool calls land
        // afterwards in their own transcript entry — so iterate the
        // post-baseline tail instead of inspecting only the
        // absolute last entry.
        let final_snapshot =
            self.session
                .wait_for_snapshot(Duration::from_secs(10), |snapshot| {
                    let status_idle = snapshot
                        .pointer("/status")
                        .and_then(Value::as_str)
                        .is_some_and(|status| status == "idle");
                    let assistant_complete =
                        new_transcript_entries(snapshot, baseline_len).any(|entry| {
                            let kind = entry.pointer("/kind").and_then(Value::as_str).unwrap_or("");
                            let content = entry
                                .pointer("/content")
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            let entry_status = entry
                                .pointer("/status")
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            kind == "assistant_message"
                                && !content.is_empty()
                                && entry_status == "completed"
                        });
                    status_idle && assistant_complete
                })?;
        for assertion in &expect.assertions {
            evaluate_post_submit_assertion(assertion, &final_snapshot, baseline_len)
                .with_context(|| {
                    format!(
                        "case '{}' SubmitAndWaitIdle assertion '{assertion}' failed; snapshot:\n{final_snapshot:#}",
                        self.case_name,
                    )
                })?;
        }
        Ok(())
    }

    fn run_submit_and_wait_terminal(
        &mut self,
        text: &str,
        token: &TokenSource,
        auth: &AuthMode,
        expect: &SimpleExpect,
    ) -> Result<()> {
        // Codex pass-1 P2 — same baseline trick as
        // `run_submit_and_wait_idle`: capture the transcript length
        // BEFORE submit so the predicate and assertion evaluator
        // both restrict their view to entries appended for THIS
        // turn. Otherwise a stale completed entry from a prior
        // submit could satisfy the wait, and a stale failure
        // marker could match `transcript_contains_any:` even when
        // the new turn is still streaming.
        let baseline_len = current_transcript_len(&self.session.snapshot()?);
        // POST /messages first; reuse the standard submit path so
        // status / error code semantics match `run_submit`.
        self.run_submit(text, token, auth, expect)?;
        // Then poll /session until either:
        //   * status == "error" (fault branch — invalid patch
        //     bubbled up to a session-level error), OR
        //   * status == "idle" AND a NEW transcript entry (after
        //     `baseline_len`) is either a completed
        //     assistant_message or a tool_call with completed /
        //     failed status (apply_patch failure lands as a
        //     tool_call entry with status="failed"; the fixture's
        //     follow-up text lands an assistant_message before).
        let final_snapshot =
            self.session
                .wait_for_snapshot(Duration::from_secs(15), |snapshot| {
                    let status = snapshot
                        .pointer("/status")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if status == "error" {
                        return true;
                    }
                    if status != "idle" {
                        return false;
                    }
                    new_transcript_entries(snapshot, baseline_len).any(|entry| {
                        let kind = entry.pointer("/kind").and_then(Value::as_str).unwrap_or("");
                        let entry_status = entry
                            .pointer("/status")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        (kind == "assistant_message" && entry_status == "completed")
                            || (kind == "tool_call"
                                && (entry_status == "completed" || entry_status == "failed"))
                            || kind == "tool_result"
                            || kind == "system_message"
                    })
                })?;
        for assertion in &expect.assertions {
            evaluate_post_submit_assertion(assertion, &final_snapshot, baseline_len)
                .with_context(|| {
                    format!(
                        "case '{}' SubmitAndWaitTerminal assertion '{assertion}' failed; snapshot:\n{final_snapshot:#}",
                        self.case_name,
                    )
                })?;
        }
        Ok(())
    }

    fn run_read_repo_file(&self, path: &str, expect: &AssertionsExpect) -> Result<()> {
        let contents = self.session.read_repo_file(path)?.ok_or_else(|| {
            anyhow!(
                "case '{}' expected repo file '{path}' to exist; not found under {}",
                self.case_name,
                self.session.repo_dir().display(),
            )
        })?;
        for assertion in &expect.assertions {
            evaluate_file_assertion(assertion, &contents).with_context(|| {
                format!(
                    "case '{}' file assertion '{assertion}' failed for '{path}'",
                    self.case_name
                )
            })?;
        }
        Ok(())
    }

    fn run_repo_file_absent(&self, path: &str) -> Result<()> {
        if let Some(contents) = self.session.read_repo_file(path)? {
            bail!(
                "case '{}' expected repo file '{path}' to be absent, but it exists ({} bytes)",
                self.case_name,
                contents.len(),
            );
        }
        Ok(())
    }

    fn run_run_repo_command(
        &self,
        command: &[String],
        timeout_ms: u64,
        expect: &RunCommandExpect,
    ) -> Result<()> {
        let output = self
            .session
            .run_repo_command(command, Duration::from_millis(timeout_ms))?;
        if let Some(expected_status) = expect.status
            && output.status.code() != Some(expected_status)
        {
            bail!(
                "case '{}' command {:?} exited with {:?} (expected {expected_status})\nstdout:\n{}\nstderr:\n{}",
                self.case_name,
                command,
                output.status.code(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
        for assertion in &expect.assertions {
            evaluate_command_output_assertion(assertion, &output).with_context(|| {
                format!(
                    "case '{}' command output assertion '{assertion}' failed for {:?}",
                    self.case_name, command
                )
            })?;
        }
        Ok(())
    }

    fn run_attach(
        &mut self,
        kind: &str,
        client_id: &str,
        auth: &AuthMode,
        expect: &AttachExpect,
    ) -> Result<()> {
        let (status, body) = self.session.matrix_attach(kind, client_id, auth)?;
        let expected_status = StatusCode::from_u16(expect.status).with_context(|| {
            format!(
                "invalid expected status {} in case '{}'",
                expect.status, self.case_name
            )
        })?;
        ensure_status(status, expected_status, &body)?;
        if let Some(code) = expect.error_code.as_deref() {
            ensure_error_code(&body, code)?;
        }
        if let Some(slot) = expect.save_token {
            let token = body
                .get("controllerToken")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("attach response did not include controllerToken: {body}"))?
                .to_string();
            self.tokens.insert(slot, token);
        }
        if let Some(label) = expect.save_lease_expires_at.as_deref() {
            let ts = body
                .get("leaseExpiresAt")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("attach response did not include leaseExpiresAt: {body}"))?;
            let parsed = chrono::DateTime::parse_from_rfc3339(ts)
                .map_err(|err| anyhow!("invalid leaseExpiresAt '{ts}': {err}"))?
                .with_timezone(&chrono::Utc);
            self.lease_timestamps.insert(label.to_string(), parsed);
        }
        for assertion in &expect.assertions {
            self.evaluate_attach_assertion(assertion, &body)?;
        }
        Ok(())
    }

    fn run_detach(
        &mut self,
        client_id: &str,
        token: &TokenSource,
        auth: &AuthMode,
        expect: &SimpleExpect,
    ) -> Result<()> {
        let (status, body) = self
            .session
            .matrix_detach(client_id, token, auth, &self.tokens)?;
        let expected_status = StatusCode::from_u16(expect.status).with_context(|| {
            format!(
                "invalid expected status {} in case '{}'",
                expect.status, self.case_name
            )
        })?;
        ensure_status(status, expected_status, &body)?;
        if let Some(code) = expect.error_code.as_deref() {
            ensure_error_code(&body, code)?;
        }
        Ok(())
    }

    fn run_submit(
        &mut self,
        text: &str,
        token: &TokenSource,
        auth: &AuthMode,
        expect: &SimpleExpect,
    ) -> Result<()> {
        let (status, body) = self
            .session
            .matrix_submit(text, token, auth, &self.tokens)?;
        let expected_status = StatusCode::from_u16(expect.status).with_context(|| {
            format!(
                "invalid expected status {} in case '{}'",
                expect.status, self.case_name
            )
        })?;
        ensure_status(status, expected_status, &body)?;
        if let Some(code) = expect.error_code.as_deref() {
            ensure_error_code(&body, code)?;
        }
        Ok(())
    }

    fn run_wait_snapshot(&mut self, expect: &AssertionsExpect) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut last_snapshot: Value;
        loop {
            let snapshot = self.session.snapshot()?;
            last_snapshot = snapshot.clone();
            if expect.assertions.iter().all(|a| {
                evaluate_snapshot_assertion(a, &snapshot, &self.lease_timestamps).unwrap_or(false)
            }) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                let mut errs = Vec::new();
                for assertion in &expect.assertions {
                    if let Err(err) = evaluate_snapshot_assertion(
                        assertion,
                        &last_snapshot,
                        &self.lease_timestamps,
                    )
                    .and_then(|ok| {
                        if ok {
                            Ok(())
                        } else {
                            Err(anyhow!("assertion '{assertion}' did not hold"))
                        }
                    }) {
                        errs.push(format!("- {err}"));
                    }
                }
                bail!(
                    "waitSnapshot timed out for case '{}'\nfailing assertions:\n{}\nlast snapshot:\n{last_snapshot:#}",
                    self.case_name,
                    errs.join("\n"),
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn evaluate_attach_assertion(&self, assertion: &str, body: &Value) -> Result<()> {
        match assertion {
            "controller_token_non_empty" => {
                let token = body
                    .get("controllerToken")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if token.is_empty() {
                    bail!("attach response controllerToken was empty: {body}");
                }
            }
            "lease_expires_at_future" => {
                let ts = body
                    .get("leaseExpiresAt")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("attach response missing leaseExpiresAt: {body}"))?;
                let parsed = chrono::DateTime::parse_from_rfc3339(ts)
                    .map_err(|err| anyhow!("invalid leaseExpiresAt '{ts}': {err}"))?;
                if parsed.with_timezone(&chrono::Utc) <= chrono::Utc::now() {
                    bail!("leaseExpiresAt {ts} is not in the future");
                }
            }
            "controller_kind_automation" => {
                let kind = body
                    .pointer("/controller/kind")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if kind != "automation" {
                    bail!("expected controller.kind == 'automation', got '{kind}' (body: {body})");
                }
            }
            other if other.starts_with("lease_expires_after:") => {
                let label = other.trim_start_matches("lease_expires_after:");
                let baseline = self
                    .lease_timestamps
                    .get(label)
                    .ok_or_else(|| anyhow!("no saved leaseExpiresAt under label '{label}'"))?;
                let ts = body
                    .get("leaseExpiresAt")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("attach response missing leaseExpiresAt: {body}"))?;
                let parsed = chrono::DateTime::parse_from_rfc3339(ts)
                    .map_err(|err| anyhow!("invalid leaseExpiresAt '{ts}': {err}"))?
                    .with_timezone(&chrono::Utc);
                if parsed <= *baseline {
                    bail!("renew leaseExpiresAt {parsed} did not extend past baseline {baseline}",);
                }
            }
            other => bail!("unsupported attach assertion '{other}'"),
        }
        Ok(())
    }
}

fn evaluate_snapshot_assertion(
    assertion: &str,
    snapshot: &Value,
    _lease_timestamps: &HashMap<String, chrono::DateTime<chrono::Utc>>,
) -> Result<bool> {
    match assertion {
        "controller_kind_tui_or_none" => {
            let kind = snapshot
                .pointer("/controller/kind")
                .and_then(Value::as_str)
                .unwrap_or("");
            Ok(kind == "tui" || kind == "none")
        }
        "controller_can_write_false" => {
            let can_write = snapshot
                .pointer("/controller/canWrite")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            Ok(!can_write)
        }
        "controller_kind_automation" => {
            let kind = snapshot
                .pointer("/controller/kind")
                .and_then(Value::as_str)
                .unwrap_or("");
            Ok(kind == "automation")
        }
        other => bail!("unsupported snapshot assertion '{other}'"),
    }
}

/// Parse an SSE event's `data:` field as JSON. The `/api/code/events`
/// handler always serialises a `CodeUiEventEnvelope` via
/// `Event::json_data`, so a deserialisation failure here means the
/// runtime emitted a malformed envelope — surface as an error rather
/// than silently empty.
fn parse_event_data(event: &SseEvent) -> Result<Value> {
    serde_json::from_str(&event.data)
        .with_context(|| format!("failed to parse SSE data as JSON: {}", event.data))
}

fn evaluate_event_assertion(assertion: &str, event: &SseEvent, payload: &Value) -> Result<()> {
    match assertion {
        "event_data_has_transcript_array" => {
            // Initial replay must include the snapshot's transcript
            // array so a fresh subscriber renders the room state.
            let transcript = payload
                .pointer("/data/transcript")
                .and_then(Value::as_array);
            if transcript.is_none() {
                bail!("payload missing /data/transcript array");
            }
        }
        "event_data_has_controller" => {
            let controller = payload.pointer("/data/controller");
            if !controller.is_some_and(Value::is_object) {
                bail!("payload missing /data/controller object");
            }
        }
        "event_data_status_thinking" => {
            let status = payload
                .pointer("/data/status")
                .and_then(Value::as_str)
                .unwrap_or("");
            if status != "thinking" {
                bail!("expected /data/status == 'thinking', got '{status}'");
            }
        }
        "event_data_status_idle" => {
            let status = payload
                .pointer("/data/status")
                .and_then(Value::as_str)
                .unwrap_or("");
            if status != "idle" {
                bail!("expected /data/status == 'idle', got '{status}'");
            }
        }
        "event_data_controller_kind_automation" => {
            let kind = payload
                .pointer("/data/controller/kind")
                .and_then(Value::as_str)
                .unwrap_or("");
            if kind != "automation" {
                bail!("expected /data/controller.kind == 'automation', got '{kind}'");
            }
        }
        "event_data_controller_kind_tui_or_none" => {
            let kind = payload
                .pointer("/data/controller/kind")
                .and_then(Value::as_str)
                .unwrap_or("");
            if kind != "tui" && kind != "none" {
                bail!("expected /data/controller.kind in {{tui, none}}, got '{kind}'");
            }
        }
        other if other.starts_with("event_transcript_contains:") => {
            let needle = other.trim_start_matches("event_transcript_contains:");
            let transcript = payload
                .pointer("/data/transcript")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("payload missing /data/transcript array"))?;
            let haystack = serde_json::to_string(transcript).unwrap_or_default();
            if !haystack.contains(needle) {
                bail!("transcript did not contain '{needle}'; serialised transcript:\n{haystack}");
            }
        }
        other => bail!(
            "unsupported SSE event assertion '{other}' (event '{}')",
            event.event
        ),
    }
    Ok(())
}

/// Evaluate an assertion across a sequence of collected SSE
/// payloads. Used by `Step::CollectSessionUpdates`. Each payload
/// is the full envelope (`{ seq, type, at, data: snapshot }`).
///
/// New cross-event assertions live here; per-event assertions
/// stay on `evaluate_event_assertion`. Suffix assertions of the
/// form `event_transcript_contains:NEEDLE` are also accepted —
/// they must hold against the FINAL collected payload.
fn evaluate_collected_assertion(assertion: &str, collected: &[Value]) -> Result<()> {
    match assertion {
        "assistant_content_monotonic" => {
            // Walk every collected snapshot, extract the LAST
            // assistant_message entry's `content`, and assert each
            // observation is a prefix of the next (or equal). Any
            // shrink or non-prefix change is a regression in the
            // streaming pipeline.
            let mut prev: Option<String> = None;
            for (idx, payload) in collected.iter().enumerate() {
                let transcript = payload
                    .pointer("/data/transcript")
                    .and_then(Value::as_array)
                    .ok_or_else(|| anyhow!("payload #{idx} missing /data/transcript array"))?;
                let assistant_content = transcript.iter().rev().find_map(|entry| {
                    let kind = entry.pointer("/kind").and_then(Value::as_str).unwrap_or("");
                    if kind == "assistant_message" {
                        entry
                            .pointer("/content")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    } else {
                        None
                    }
                });
                let Some(current) = assistant_content else {
                    // Snapshots before the first assistant chunk
                    // legitimately have no assistant message yet.
                    // Skip but keep `prev` so the next observation
                    // still has to extend the previous run.
                    continue;
                };
                if let Some(prev_content) = &prev
                    && !current.starts_with(prev_content)
                {
                    bail!(
                        "assistant content #{idx} is not a prefix-extension of the previous; \
                         prev: {prev_content:?}, current: {current:?}",
                    );
                }
                prev = Some(current);
            }
            if prev.is_none() {
                bail!("collected sessions had no assistant_message entries");
            }
        }
        other if other.starts_with("event_transcript_contains:") => {
            // Apply the per-event assertion to the FINAL collected
            // payload — by then the streamed reply has fully
            // landed in the snapshot.
            let last = collected.last().ok_or_else(|| {
                anyhow!("event_transcript_contains assertion needs at least one collected event")
            })?;
            let needle = other.trim_start_matches("event_transcript_contains:");
            let transcript = last
                .pointer("/data/transcript")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("payload missing /data/transcript array"))?;
            let haystack = serde_json::to_string(transcript).unwrap_or_default();
            if !haystack.contains(needle) {
                bail!("transcript did not contain '{needle}'; serialised transcript:\n{haystack}");
            }
        }
        other => bail!("unsupported collected-events assertion '{other}'"),
    }
    Ok(())
}

fn ensure_status(actual: StatusCode, expected: StatusCode, body: &Value) -> Result<()> {
    if actual == expected {
        Ok(())
    } else {
        bail!("expected HTTP {expected}, got {actual}: {body}")
    }
}

fn ensure_error_code(body: &Value, expected: &str) -> Result<()> {
    let code = body
        .pointer("/error/code")
        .and_then(Value::as_str)
        .or_else(|| body.get("code").and_then(Value::as_str))
        .unwrap_or("");
    if code == expected {
        Ok(())
    } else {
        bail!("expected error.code == '{expected}', got '{code}' (body: {body})")
    }
}

/// Helper exposed to tests: forged controller token literal.
pub fn forged_controller_token() -> &'static str {
    FORGED_CONTROLLER_TOKEN
}

/// Length of the snapshot's transcript array, or `0` if the
/// snapshot has no transcript field. Used by SubmitAndWait* steps
/// to pin a per-turn baseline so subsequent assertions can ignore
/// stale entries appended by earlier submits in the same case.
fn current_transcript_len(snapshot: &Value) -> usize {
    snapshot
        .pointer("/transcript")
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0)
}

/// Iterator over transcript entries appended after `baseline_len`.
/// Returns an empty iterator when the snapshot has no transcript or
/// when the runtime trimmed the array below the baseline (defensive
/// — not expected in production cases).
fn new_transcript_entries(snapshot: &Value, baseline_len: usize) -> impl Iterator<Item = &Value> {
    snapshot
        .pointer("/transcript")
        .and_then(Value::as_array)
        .map(|entries| {
            if baseline_len >= entries.len() {
                [].iter()
            } else {
                entries[baseline_len..].iter()
            }
        })
        .into_iter()
        .flatten()
}

/// Evaluate an assertion against the snapshot returned after a
/// `Step::SubmitAndWaitIdle` / `Step::SubmitAndWaitTerminal`.
/// Snapshot-wide assertions (e.g. `snapshot_status_idle`) inspect
/// the whole snapshot; transcript needle assertions
/// (`transcript_contains:`, `transcript_contains_any:`) restrict
/// their search to entries appended after `baseline_len` so a
/// stale entry from a prior submit can't satisfy a needle intended
/// for the current turn — Codex pass-1 P2 follow-up.
fn evaluate_post_submit_assertion(
    assertion: &str,
    snapshot: &Value,
    baseline_len: usize,
) -> Result<()> {
    match assertion {
        "snapshot_status_idle" => {
            let status = snapshot
                .pointer("/status")
                .and_then(Value::as_str)
                .unwrap_or("");
            if status != "idle" {
                bail!("expected /status == 'idle', got '{status}'");
            }
            Ok(())
        }
        "snapshot_status_error" => {
            let status = snapshot
                .pointer("/status")
                .and_then(Value::as_str)
                .unwrap_or("");
            if status != "error" {
                bail!("expected /status == 'error', got '{status}'");
            }
            Ok(())
        }
        "snapshot_status_error_or_idle" => {
            let status = snapshot
                .pointer("/status")
                .and_then(Value::as_str)
                .unwrap_or("");
            if status != "error" && status != "idle" {
                bail!("expected /status in {{error, idle}}, got '{status}'");
            }
            Ok(())
        }
        other if other.starts_with("transcript_contains:") => {
            let needle = other.trim_start_matches("transcript_contains:");
            let new_entries: Vec<&Value> = new_transcript_entries(snapshot, baseline_len).collect();
            if new_entries.is_empty() {
                bail!(
                    "expected at least one transcript entry appended after baseline_len={baseline_len}, but the runtime appended none",
                );
            }
            let haystack = serde_json::to_string(&new_entries).unwrap_or_default();
            if !haystack.contains(needle) {
                bail!("new transcript entries did not contain '{needle}'; entries:\n{haystack}",);
            }
            Ok(())
        }
        other if other.starts_with("transcript_contains_any:") => {
            let raw = other.trim_start_matches("transcript_contains_any:");
            let needles: Vec<&str> = raw.split('|').filter(|s| !s.is_empty()).collect();
            if needles.is_empty() {
                bail!("'transcript_contains_any:' assertion needs at least one needle");
            }
            let new_entries: Vec<&Value> = new_transcript_entries(snapshot, baseline_len).collect();
            if new_entries.is_empty() {
                bail!(
                    "expected at least one transcript entry appended after baseline_len={baseline_len}, but the runtime appended none",
                );
            }
            let haystack = serde_json::to_string(&new_entries).unwrap_or_default();
            if !needles.iter().any(|needle| haystack.contains(needle)) {
                bail!("new transcript entries matched none of {needles:?}; entries:\n{haystack}",);
            }
            Ok(())
        }
        other => bail!("unsupported post-submit assertion '{other}'"),
    }
}

/// Evaluate an assertion against the contents of a file read with
/// `Step::ReadRepoFile`. Generation cases use this to pin the exact
/// strings the apply_patch fixture was supposed to produce.
fn evaluate_file_assertion(assertion: &str, contents: &str) -> Result<()> {
    if let Some(needle) = assertion.strip_prefix("file_contains:") {
        if !contents.contains(needle) {
            bail!("file did not contain '{needle}'; full contents:\n{contents}");
        }
        return Ok(());
    }
    if let Some(raw) = assertion.strip_prefix("file_contains_any:") {
        let needles: Vec<&str> = raw.split('|').filter(|s| !s.is_empty()).collect();
        if needles.is_empty() {
            bail!("'file_contains_any:' assertion needs at least one needle");
        }
        if !needles.iter().any(|needle| contents.contains(needle)) {
            bail!("file matched none of {needles:?}; full contents:\n{contents}");
        }
        return Ok(());
    }
    bail!("unsupported file assertion '{assertion}'")
}

/// Evaluate an assertion against the captured stdout / stderr of a
/// `Step::RunRepoCommand`. The combined-stream form lets tests stay
/// agnostic to which channel `rustc` / `cargo` use for "test result:
/// ok"-style summaries (rustc emits to stdout; some wrappers send
/// it to stderr).
fn evaluate_command_output_assertion(assertion: &str, output: &Output) -> Result<()> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if let Some(needle) = assertion.strip_prefix("stdout_contains:") {
        if !stdout.contains(needle) {
            bail!("stdout did not contain '{needle}'\nstdout:\n{stdout}\nstderr:\n{stderr}",);
        }
        return Ok(());
    }
    if let Some(needle) = assertion.strip_prefix("stderr_contains:") {
        if !stderr.contains(needle) {
            bail!("stderr did not contain '{needle}'\nstdout:\n{stdout}\nstderr:\n{stderr}",);
        }
        return Ok(());
    }
    if let Some(needle) = assertion.strip_prefix("stdout_or_stderr_contains:") {
        if !stdout.contains(needle) && !stderr.contains(needle) {
            bail!(
                "neither stdout nor stderr contained '{needle}'\nstdout:\n{stdout}\nstderr:\n{stderr}",
            );
        }
        return Ok(());
    }
    bail!("unsupported command output assertion '{assertion}'")
}
