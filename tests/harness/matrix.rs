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
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::Value;

use super::{CodeSession, CodeSessionOptions};

#[derive(Debug, Deserialize)]
pub struct CaseFile {
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    #[allow(dead_code)]
    pub matrix: String,
    pub defaults: Defaults,
    pub cases: Vec<Case>,
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
}

#[derive(Debug, Deserialize)]
pub struct Case {
    pub name: String,
    #[allow(dead_code)]
    pub priority: String,
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

/// Locate a case by name in the loaded file.
pub fn find_case<'a>(file: &'a CaseFile, name: &str) -> Result<&'a Case> {
    file.cases
        .iter()
        .find(|c| c.name == name)
        .ok_or_else(|| anyhow!("matrix case '{name}' not present in case file"))
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
    }
}

/// Build a [`CodeSessionOptions`] from the matrix config, including
/// per-case overrides for `control` and `leaseDurationMs`.
pub fn build_session_options(file: &CaseFile, case: &Case) -> CodeSessionOptions {
    let merged = effective_options(&file.defaults.options, &case.options);
    let fixture = repo_root_path(&file.defaults.fixture.path);
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
        }
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
