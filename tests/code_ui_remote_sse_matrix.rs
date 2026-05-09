//! Data-driven SSE matrix runner (Wave 1 closing case).
//!
//! Each `#[test]` here loads `tests/data/code_ui_remote/sse_cases.json`
//! and runs the named case end-to-end against a fresh `libra code` PTY
//! session, subscribing to `/api/code/events` through the new
//! `tests/harness/event_stream.rs` blocking client.
//!
//! Wave 1's exit criteria from `docs/improvement/test.md` is to prove
//! one SSE case end-to-end so the harness, matrix step variants, and
//! event-stream client are all wired correctly. Subsequent Waves
//! (PR 4) flesh out the remaining six cases without changing this
//! runner — adding a new case becomes one extra `sse_case!` line.

#[cfg(feature = "test-provider")]
mod harness;

#[cfg(feature = "test-provider")]
use anyhow::Result;
#[cfg(feature = "test-provider")]
use harness::CodeSession;
#[cfg(feature = "test-provider")]
use harness::matrix::{Case, CaseFile, build_session_options, find_case, load_case_file};
#[cfg(feature = "test-provider")]
use serial_test::serial;

#[cfg(feature = "test-provider")]
const CASE_FILE_PATH: &str = "tests/data/code_ui_remote/sse_cases.json";

#[cfg(feature = "test-provider")]
fn run_sse_case(case_name: &str) -> Result<()> {
    let file_path = harness::matrix::data_path(CASE_FILE_PATH);
    let file: CaseFile = load_case_file(&file_path)?;
    let case: Case = find_case(&file, case_name)?;
    let options = build_session_options(&file, &case);
    let mut session = CodeSession::spawn(options)?;
    let outcome = harness::matrix::run_case(&mut session, &case);
    let shutdown = session.shutdown();
    outcome?;
    shutdown
}

#[cfg(feature = "test-provider")]
macro_rules! sse_case {
    ($name:ident) => {
        #[test]
        #[serial]
        fn $name() -> Result<()> {
            run_sse_case(stringify!($name))
        }
    };
}

// Wave 1 closing case — exercises:
//   * `EventStream::open` against `/api/code/events`
//   * SSE block parser via the worker thread
//   * `Step::OpenEvents` + `Step::ExpectEvent` matrix variants
//   * `event_data_has_transcript_array` / `event_data_has_controller`
//     assertion vocabulary
// Subsequent Waves add the remaining six cases by uncommenting the
// matching macro lines below; they are intentionally left commented
// so PR 1 lands with just the single proof-of-life case.
#[cfg(feature = "test-provider")]
sse_case!(sse_initial_connect_replays_session_updated_with_full_snapshot);

// Wave 2 (PR 4) wires the rest:
// sse_case!(sse_emits_status_changed_when_submit_starts_thinking);
// sse_case!(sse_emits_session_updated_after_assistant_completion);
// sse_case!(sse_emits_controller_changed_on_attach_and_detach);
// sse_case!(sse_two_concurrent_subscribers_receive_status_changed);
// sse_case!(sse_reconnect_initial_replay_contains_latest_transcript);
// sse_case!(sse_streaming_fixture_transcript_content_grows_monotonically);

#[cfg(not(feature = "test-provider"))]
#[test]
fn sse_matrix_requires_test_provider_feature() {
    eprintln!("skipping SSE matrix; enable --features test-provider");
}
