#![allow(dead_code)]
//! Server-Sent Events client for the L2 SSE matrix.
//!
//! Wave 1 (`docs/improvement/test.md`) calls this out as the only hard
//! blocking item: the `tests/harness/code_session.rs` PTY harness has
//! always driven `/api/code/messages` and `/session`, but the matrix
//! roadmap needs a blocking SSE reader before any of the SSE / state /
//! generation / approval matrices can run.
//!
//! Design notes:
//!
//! * Use `reqwest::blocking` so the matrix runner stays synchronous
//!   like the rest of the L2 harness. The Worker UI's SSE wire format
//!   is plain `text/event-stream`, not framed JSON, so we own the
//!   parser instead of depending on an SSE crate.
//! * Read the underlying response on a worker thread that pushes
//!   parsed [`SseEvent`]s into an `mpsc::sync_channel`. Each call to
//!   [`EventStream::next_event`] then becomes a `recv_timeout` —
//!   per-call timeouts decouple cleanly from the request-level
//!   timeout we hand `reqwest`.
//! * Bound the parsed line buffer at 1 MiB so a runaway server can't
//!   exhaust the test process. Lines bigger than that abort the
//!   stream and surface as an `Err` from the next call to
//!   `next_event`; the parser thread shuts down at the same time.
//! * EOF and timeout are distinct outcomes:
//!   * `Ok(None)` — no event arrived within the requested timeout
//!     (the underlying connection may still be open).
//!   * `Err(...)` — the connection ended (server closed, network
//!     error, line-too-long, etc.). Once an `Err` is returned the
//!     stream is poisoned and subsequent calls will keep returning
//!     `Err`.
//! * `keep-alive` heartbeats from axum's `Sse::keep_alive(...)` are
//!   surfaced as comments (`:` lines) and silently dropped; only
//!   real `event:`+`data:` blocks reach the channel.
//!
//! The public surface is intentionally small. Tests that care about
//! a specific event type use [`EventStream::next_event`] in a loop;
//! the matrix runner builds higher-level steps (`expect_event`,
//! `collect_events_until`) on top.

use std::{
    io::{BufRead, BufReader},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError, TrySendError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use reqwest::blocking::{Client, Response};

/// Maximum number of bytes a single SSE line may carry. axum's
/// `Sse::keep_alive` produces 1-byte comment lines and our snapshots
/// fit well under this; a 1 MiB ceiling prevents a buggy server from
/// growing the test process unboundedly.
pub const MAX_SSE_LINE_BYTES: usize = 1024 * 1024;

/// Decoded `event:` + `data:` block. Multi-line `data:` payloads are
/// joined with `\n` per the SSE spec, mirroring the EventSource API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: String,
    pub data: String,
}

/// Blocking SSE client wrapper. The Drop impl signals the worker
/// thread to stop and DETACHES it (it does not join). Joining
/// would deadlock teardown if the worker were parked in a long
/// socket read or a blocked `SyncSender::send`. Detaching plus
/// dropping the receiver is enough: the next `try_send` from the
/// worker fails with `Disconnected`, the loop exits, and the
/// underlying socket close lets the OS reap the thread.
pub struct EventStream {
    receiver: mpsc::Receiver<EventOrError>,
    cancel: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    /// Sticky error: once the parser shuts down with an error, we
    /// keep returning it on subsequent calls so callers don't
    /// silently treat "connection died" as "no event yet".
    poisoned: Option<String>,
}

enum EventOrError {
    Event(SseEvent),
    Err(String),
}

impl EventStream {
    /// Open `url` with an optional bearer token and start the
    /// background reader. The connection itself is established
    /// synchronously; per-event timeouts apply to subsequent
    /// [`Self::next_event`] calls.
    pub fn open(client: &Client, url: &str, bearer_token: Option<&str>) -> Result<Self> {
        let mut request = client.get(url).header("Accept", "text/event-stream");
        if let Some(token) = bearer_token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .with_context(|| format!("failed to open SSE stream at '{url}'"))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            bail!("SSE stream open returned HTTP {status}: {body}");
        }
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        if !content_type.starts_with("text/event-stream") {
            // The handler always sets text/event-stream; refusing
            // any other Content-Type guards against routing changes
            // that would otherwise let an HTML 404 sneak through.
            bail!("SSE stream returned non-event Content-Type '{content_type}'");
        }

        let (tx, rx) = mpsc::sync_channel::<EventOrError>(64);
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_for_thread = Arc::clone(&cancel);
        let worker = thread::Builder::new()
            .name("libra-test-sse-reader".to_string())
            .spawn(move || {
                run_reader(response, tx, cancel_for_thread);
            })
            .context("failed to spawn SSE reader thread")?;
        Ok(Self {
            receiver: rx,
            cancel,
            worker: Some(worker),
            poisoned: None,
        })
    }

    /// Wait up to `timeout` for the next event.
    ///
    /// * `Ok(Some(event))` — an event arrived.
    /// * `Ok(None)` — timeout elapsed without an event; the
    ///   connection may still be alive.
    /// * `Err(_)` — the connection ended (with a sticky error). All
    ///   subsequent calls keep returning the same error so callers
    ///   can `expect()` once and propagate.
    pub fn next_event(&mut self, timeout: Duration) -> Result<Option<SseEvent>> {
        if let Some(error) = self.poisoned.as_ref() {
            return Err(anyhow!("SSE stream poisoned: {error}"));
        }
        match self.receiver.recv_timeout(timeout) {
            Ok(EventOrError::Event(event)) => Ok(Some(event)),
            Ok(EventOrError::Err(error)) => {
                self.poisoned = Some(error.clone());
                Err(anyhow!("SSE stream poisoned: {error}"))
            }
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => {
                let message = "SSE reader thread exited without surfacing an event".to_string();
                self.poisoned = Some(message.clone());
                Err(anyhow!(message))
            }
        }
    }

    /// Signal the reader thread to stop. Codex pass-1 P2: do NOT
    /// `join()` here — the reader can be parked in either a long
    /// `read()` on the underlying socket OR a blocked
    /// `mpsc::SyncSender::send` if the matrix consumer fell behind.
    /// Both block until the corresponding peer wakes them up, so
    /// joining would deadlock test teardown for an unbounded period.
    /// Instead we drop the receiver and the join handle: dropping
    /// the receiver makes the worker's next send fail with
    /// `Disconnected` (it uses `try_send`), and dropping the join
    /// handle detaches the thread. The OS reaps it once the socket
    /// read returns (typically within milliseconds, when the
    /// `Response` itself is dropped on this side).
    pub fn close(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        // Detach the worker so a stuck I/O read can't hang teardown.
        // We intentionally do not join.
        let _ = self.worker.take();
    }
}

impl Drop for EventStream {
    fn drop(&mut self) {
        self.close();
    }
}

fn run_reader(response: Response, tx: mpsc::SyncSender<EventOrError>, cancel: Arc<AtomicBool>) {
    let mut reader = BufReader::new(response);
    let mut event_field = String::new();
    let mut data_lines: Vec<String> = Vec::new();
    loop {
        if cancel.load(Ordering::SeqCst) {
            return;
        }
        let mut line_bytes: Vec<u8> = Vec::new();
        // Codex pass-1 P2: bound bytes DURING the read so a malformed
        // SSE source without newlines cannot grow the buffer beyond
        // the documented cap before we surface the error.
        match read_line_bounded(&mut reader, &mut line_bytes, MAX_SSE_LINE_BYTES) {
            Ok(BoundedRead::Eof) => {
                send_or_drop(
                    &tx,
                    EventOrError::Err("SSE stream closed by server".to_string()),
                );
                return;
            }
            Ok(BoundedRead::Line) => {}
            Ok(BoundedRead::OverLimit) => {
                send_or_drop(
                    &tx,
                    EventOrError::Err(format!(
                        "SSE line exceeded {MAX_SSE_LINE_BYTES} bytes before newline"
                    )),
                );
                return;
            }
            Err(error) => {
                send_or_drop(
                    &tx,
                    EventOrError::Err(format!("SSE stream read error: {error}")),
                );
                return;
            }
        }
        // Strip the trailing newline pair (`\r\n` or `\n`) without
        // allocating: the parser pre-strips so we never carry it
        // through the line classification below.
        if line_bytes.ends_with(b"\n") {
            line_bytes.pop();
            if line_bytes.ends_with(b"\r") {
                line_bytes.pop();
            }
        }

        // Empty line → dispatch the buffered event (if any).
        if line_bytes.is_empty() {
            if !event_field.is_empty() || !data_lines.is_empty() {
                let event = SseEvent {
                    event: if event_field.is_empty() {
                        "message".to_string()
                    } else {
                        std::mem::take(&mut event_field)
                    },
                    data: data_lines.join("\n"),
                };
                data_lines.clear();
                if !send_or_drop(&tx, EventOrError::Event(event)) {
                    // Receiver gone — quit instead of looping
                    // forever pushing events into a dropped channel.
                    return;
                }
            }
            continue;
        }

        // SSE comment lines start with ':' and must be ignored.
        if line_bytes.starts_with(b":") {
            continue;
        }
        // Non-UTF-8 lines are not part of the spec; surface as an
        // error rather than corrupt the parser state.
        let line = match std::str::from_utf8(&line_bytes) {
            Ok(s) => s,
            Err(error) => {
                send_or_drop(
                    &tx,
                    EventOrError::Err(format!("SSE line not UTF-8: {error}")),
                );
                return;
            }
        };

        if let Some(rest) = line.strip_prefix("event:") {
            event_field = rest.trim_start().to_string();
        } else if let Some(rest) = line.strip_prefix("event") {
            // `event\n` (no colon) is not technically valid; treat
            // the entire line as a malformed signal but keep going.
            if !rest.is_empty() {
                send_or_drop(
                    &tx,
                    EventOrError::Err(format!(
                        "malformed SSE field, expected ':' after 'event': {line:?}"
                    )),
                );
                return;
            }
        } else if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.trim_start().to_string());
        }
        // `id:` and `retry:` ignored — Libra doesn't use them.
    }
}

/// Outcome of a single bounded line read.
enum BoundedRead {
    /// Read up to and including a newline (the newline IS appended
    /// to `out`, mirroring `BufRead::read_until`'s contract so the
    /// parser can keep its existing trailing-`\n` strip logic).
    Line,
    /// EOF before any bytes for this call.
    Eof,
    /// `out` reached `cap` bytes without seeing a newline; the
    /// caller must surface the over-limit error and stop reading.
    OverLimit,
}

/// Read up to one line (terminated by `\n`) from `reader`, but stop
/// as soon as `out` reaches `cap` bytes. This caps memory growth
/// even when the upstream sends an unterminated, ever-growing line.
fn read_line_bounded<R: BufRead>(
    reader: &mut R,
    out: &mut Vec<u8>,
    cap: usize,
) -> std::io::Result<BoundedRead> {
    let mut byte = [0_u8; 1];
    loop {
        if out.len() >= cap {
            return Ok(BoundedRead::OverLimit);
        }
        match reader.read(&mut byte)? {
            0 => {
                if out.is_empty() {
                    return Ok(BoundedRead::Eof);
                }
                // EOF mid-line: treat as a complete line so the
                // parser still classifies it; the next iteration
                // will return Eof and exit cleanly.
                return Ok(BoundedRead::Line);
            }
            1 => {
                out.push(byte[0]);
                if byte[0] == b'\n' {
                    return Ok(BoundedRead::Line);
                }
            }
            _ => unreachable!("Read::read with a 1-byte buf returns 0 or 1"),
        }
    }
}

/// Non-blocking enqueue. Returns `true` if the receiver accepted
/// the value, `false` if either:
///
/// * the receiver was dropped (`Disconnected`); or
/// * the bounded channel is full (`Full`).
///
/// Codex pass-2 P2-1: the previous version fell back to a blocking
/// `send()` on Full so the reader wouldn't silently drop events.
/// That was wrong: a *live but slow* consumer holding the
/// `EventStream` without calling `next_event` would hold the
/// channel full and stall the reader thread for the entire test
/// duration. We now treat Full identically to Disconnected — both
/// signal "the consumer is gone or hopelessly behind". The reader
/// loop returns immediately, and the next `EventStream::next_event`
/// call surfaces the channel-disconnected error so the test fails
/// loud rather than hanging quietly.
fn send_or_drop(tx: &mpsc::SyncSender<EventOrError>, value: EventOrError) -> bool {
    match tx.try_send(value) {
        Ok(()) => true,
        Err(TrySendError::Disconnected(_)) | Err(TrySendError::Full(_)) => false,
    }
}

#[cfg(test)]
mod tests {
    //! L0 unit tests for the SSE block parser. These run in-process
    //! against an `mpsc::channel` instead of a real TCP socket so
    //! the parser logic is verifiable without spinning up a Worker.

    use std::io::Cursor;

    use super::*;

    /// Drive `run_reader` against an in-memory cursor and collect
    /// every event/error the worker produces. Uses a 1024-slot
    /// sync_channel (see [`large_buffer_test_channel`]) — wide
    /// enough that any plausible future L0 fixture won't fill it,
    /// but still backpressured so the type matches the production
    /// `SyncSender<EventOrError>` signature on
    /// `run_reader_into_with_cap`.
    fn drive(input: &str) -> Vec<EventOrError> {
        let cursor = Cursor::new(input.as_bytes().to_vec());
        let (tx, rx) = large_buffer_test_channel();
        let cancel = Arc::new(AtomicBool::new(false));
        // We can't reuse `run_reader` directly (it expects a
        // `Response`); inline the same loop against a generic
        // BufRead so the unit test exercises the same parsing
        // logic. Keep this implementation in lock-step with
        // `run_reader` above when adding new field handling.
        let cancel_for_thread = Arc::clone(&cancel);
        thread::scope(|scope| {
            scope.spawn(move || {
                run_reader_into::<Cursor<Vec<u8>>>(cursor, tx, cancel_for_thread);
            });
        });
        rx.try_iter().collect()
    }

    /// Build a 1024-slot bounded `sync_channel` for L0 tests.
    ///
    /// We intentionally don't use `mpsc::channel` (genuinely
    /// unbounded) because `run_reader_into_with_cap` takes the
    /// same `SyncSender<EventOrError>` type the production reader
    /// uses, so the test surface has to stay
    /// `SyncSender`-compatible. 1024 slots is large enough that
    /// every L0 fixture (which currently emits 1 event each, with
    /// a worst-case bound of "a few" for future high-fan-out
    /// tests) drains synchronously into `try_iter()` without ever
    /// hitting backpressure. Backpressure DOES still exist if a
    /// future L0 case generates more than 1024 events without
    /// draining — surface that as a test failure rather than a
    /// silent hang by relying on the production `send_or_drop`
    /// contract (Full == Disconnected).
    fn large_buffer_test_channel() -> (mpsc::SyncSender<EventOrError>, mpsc::Receiver<EventOrError>)
    {
        mpsc::sync_channel::<EventOrError>(1024)
    }

    fn run_reader_into<R: std::io::Read>(
        reader: R,
        tx: mpsc::SyncSender<EventOrError>,
        cancel: Arc<AtomicBool>,
    ) {
        run_reader_into_with_cap(reader, tx, cancel, MAX_SSE_LINE_BYTES);
    }

    /// Variant that exposes the byte cap so the over-limit
    /// regression test can saturate it without allocating MiB
    /// fixtures. Mirrors `run_reader` so the parser stays in lock-
    /// step with the production path.
    fn run_reader_into_with_cap<R: std::io::Read>(
        reader: R,
        tx: mpsc::SyncSender<EventOrError>,
        cancel: Arc<AtomicBool>,
        cap: usize,
    ) {
        let mut reader = BufReader::new(reader);
        let mut event_field = String::new();
        let mut data_lines: Vec<String> = Vec::new();
        loop {
            if cancel.load(Ordering::SeqCst) {
                return;
            }
            let mut line_bytes: Vec<u8> = Vec::new();
            match read_line_bounded(&mut reader, &mut line_bytes, cap) {
                Ok(BoundedRead::Eof) => return,
                Ok(BoundedRead::Line) => {}
                Ok(BoundedRead::OverLimit) => {
                    let _ = tx.try_send(EventOrError::Err(format!(
                        "SSE line exceeded {cap} bytes before newline"
                    )));
                    return;
                }
                Err(error) => {
                    let _ = tx.try_send(EventOrError::Err(format!("read error: {error}")));
                    return;
                }
            }
            if line_bytes.ends_with(b"\n") {
                line_bytes.pop();
                if line_bytes.ends_with(b"\r") {
                    line_bytes.pop();
                }
            }
            if line_bytes.is_empty() {
                if !event_field.is_empty() || !data_lines.is_empty() {
                    let event = SseEvent {
                        event: if event_field.is_empty() {
                            "message".to_string()
                        } else {
                            std::mem::take(&mut event_field)
                        },
                        data: data_lines.join("\n"),
                    };
                    data_lines.clear();
                    if tx.try_send(EventOrError::Event(event)).is_err() {
                        return;
                    }
                }
                continue;
            }
            if line_bytes.starts_with(b":") {
                continue;
            }
            let line = std::str::from_utf8(&line_bytes).expect("UTF-8 in fixture");
            if let Some(rest) = line.strip_prefix("event:") {
                event_field = rest.trim_start().to_string();
            } else if let Some(rest) = line.strip_prefix("data:") {
                data_lines.push(rest.trim_start().to_string());
            }
        }
    }

    fn events_only(items: Vec<EventOrError>) -> Vec<SseEvent> {
        items
            .into_iter()
            .filter_map(|e| match e {
                EventOrError::Event(ev) => Some(ev),
                EventOrError::Err(_) => None,
            })
            .collect()
    }

    #[test]
    fn parses_single_event_block() {
        let body = "event: status_changed\ndata: {\"status\":\"thinking\"}\n\n";
        let events = events_only(drive(body));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "status_changed");
        assert_eq!(events[0].data, "{\"status\":\"thinking\"}");
    }

    #[test]
    fn ignores_comment_keepalive_lines() {
        let body = ":keep-alive\nevent: ping\ndata: 1\n\n";
        let events = events_only(drive(body));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "ping");
        assert_eq!(events[0].data, "1");
    }

    #[test]
    fn joins_multiple_data_lines_with_newline() {
        let body = "event: chunk\ndata: line1\ndata: line2\n\n";
        let events = events_only(drive(body));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "line1\nline2");
    }

    #[test]
    fn defaults_event_name_to_message_when_only_data_present() {
        let body = "data: payload\n\n";
        let events = events_only(drive(body));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "message");
        assert_eq!(events[0].data, "payload");
    }

    #[test]
    fn handles_crlf_line_endings() {
        let body = "event: status_changed\r\ndata: idle\r\n\r\n";
        let events = events_only(drive(body));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "status_changed");
        assert_eq!(events[0].data, "idle");
    }

    /// Codex pass-1 P2 / pass-2 P2-2 regression: the byte cap MUST
    /// be enforced during the read, not after `read_until` has
    /// already grown the buffer past it. We test the bounded
    /// helper directly so the assertion is precise: when
    /// `read_line_bounded` returns `OverLimit`, the line buffer
    /// MUST contain exactly `cap` bytes (no more, no less). A
    /// naive `read_until` implementation would have grown the
    /// buffer to the full 200-byte payload before bailing.
    #[test]
    fn read_line_bounded_caps_buffer_growth_at_cap_bytes() {
        let cap = 32;
        let body = "x".repeat(200);
        let mut reader = BufReader::new(Cursor::new(body.into_bytes()));
        let mut out: Vec<u8> = Vec::new();
        let outcome = read_line_bounded(&mut reader, &mut out, cap)
            .expect("bounded read should not surface IO errors against an in-memory cursor");
        match outcome {
            BoundedRead::OverLimit => {}
            BoundedRead::Line => panic!("expected OverLimit, got Line (out.len()={})", out.len()),
            BoundedRead::Eof => panic!("expected OverLimit, got Eof (out.len()={})", out.len()),
        }
        assert_eq!(
            out.len(),
            cap,
            "line buffer grew past cap; got {} bytes, expected exactly {cap}",
            out.len(),
        );
    }

    /// And a complementary regression: the worker loop wrapping
    /// the bounded helper still surfaces the OverLimit as a single
    /// `EventOrError::Err` with a descriptive message, with no
    /// stray `SseEvent`s leaking into the channel.
    #[test]
    fn read_aborts_when_line_exceeds_cap_before_newline() {
        let cap = 32;
        let body = "x".repeat(200);
        let cursor = Cursor::new(body.into_bytes());
        let (tx, rx) = large_buffer_test_channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_for_thread = Arc::clone(&cancel);
        thread::scope(|scope| {
            scope.spawn(move || {
                run_reader_into_with_cap::<Cursor<Vec<u8>>>(cursor, tx, cancel_for_thread, cap);
            });
        });
        let items: Vec<EventOrError> = rx.try_iter().collect();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EventOrError::Err(message) => {
                assert!(
                    message.contains("32 bytes before newline"),
                    "unexpected error message: {message}",
                );
            }
            EventOrError::Event(event) => panic!("expected Err, got Event {event:?}"),
        }
    }
}
