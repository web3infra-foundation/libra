//! Terminal management for the TUI.
//!
//! Owns the crossterm bootstrap (raw mode, bracketed paste, keyboard
//! enhancement flags, focus change, alternate screen), the matching teardown,
//! and a unified [`TuiEvent`] stream that merges:
//! - Terminal events (key / paste / mouse / resize) from `crossterm::EventStream`.
//! - Draw requests scheduled through a tokio broadcast channel.
//!
//! The merged stream is the single input source for [`super::app::App`]. By
//! collapsing the two sources into one enum the event loop can be a flat
//! `while let Some(ev) = stream.next().await` with no dual-source bookkeeping.

use std::{
    io::{self, IsTerminal, Result, Stdout, stdin, stdout},
    panic,
    pin::Pin,
    time::Duration,
};

use crossterm::{
    event::{
        DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
        EnableFocusChange, KeyEvent, KeyboardEnhancementFlags, MouseEvent,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute, terminal,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::broadcast;
use tokio_stream::{Stream, StreamExt};

/// Target frame interval for UI redraw scheduling. Approximately 60 FPS, used
/// by the App to coalesce rapid state changes into a single redraw rather
/// than thrashing the terminal.
pub const TARGET_FRAME_INTERVAL: Duration = Duration::from_millis(16); // ~60 FPS

/// A type alias for the terminal type used in this application.
///
/// Always crossterm-backed because the TUI binds platform behaviour (raw
/// mode, alt-screen, keyboard flags) directly to crossterm primitives.
pub type TerminalType = Terminal<CrosstermBackend<Stdout>>;

/// Events from the terminal.
///
/// Carries the raw crossterm payloads for key / paste / mouse plus the two
/// synthetic events (`Draw`, `Resize`) that the App needs to drive layout.
/// `Resize` deliberately drops the new dimensions because the App always
/// re-queries the terminal size during the next draw to avoid drifting from
/// reality.
#[derive(Debug, Clone)]
pub enum TuiEvent {
    /// Key press event.
    Key(KeyEvent),
    /// Bracketed-paste event with the full pasted text.
    Paste(String),
    /// Mouse event (clicks, scroll, etc.).
    Mouse(MouseEvent),
    /// Request to draw a frame; coalesces redraws onto the frame budget.
    Draw,
    /// Terminal resize; new dimensions are re-queried at draw time.
    Resize,
}

/// Initialize the terminal for TUI mode.
///
/// Functional scope: validates that both stdin and stdout are TTYs, then
/// installs the modes required by the TUI (raw mode, bracketed paste,
/// keyboard enhancement flags, focus change). Also registers a panic hook so
/// a panicking task does not leave the terminal in raw mode.
///
/// Boundary conditions:
/// - Returns an `io::Error` if either stream is not a terminal — running
///   `libra code` while piped is unsupported and would silently misbehave.
/// - Keyboard enhancement flags and focus change are best-effort; some
///   terminals reject them silently and the TUI still works without them.
pub fn init() -> Result<TerminalType> {
    if !stdin().is_terminal() {
        return Err(io::Error::other("stdin is not a terminal"));
    }
    if !stdout().is_terminal() {
        return Err(io::Error::other("stdout is not a terminal"));
    }

    // Once-per-process: redirect fd 2 (stderr) to either the libra log file
    // or /dev/null so external child processes (e.g. `mount_macfuse` from
    // rfuse3) cannot scribble error text onto the alternate-screen TUI.
    // Tracing-subscriber configured by main.rs writes through its own
    // file/non-stderr path when LIBRA_LOG_FILE is set, so this redirect does
    // not lose libra's own diagnostics.
    let _ = redirect_stderr_for_tui();

    set_modes()?;
    set_panic_hook();

    let backend = CrosstermBackend::new(stdout());
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

/// Redirect process-wide fd 2 to a sink that the TUI does not render.
///
/// Functional scope: the TUI runs in alternate-screen mode, but fd 2
/// belongs to the controlling tty regardless. External child processes
/// (e.g. `mount_macfuse` invoked deep inside rfuse3) write diagnostic text
/// directly to fd 2, which then bleeds through the alternate-screen and
/// corrupts the rendered frame. We pick a destination once at TUI startup:
/// the `LIBRA_LOG_FILE` path if the user set it, otherwise `/dev/null`. The
/// redirect is best-effort — any failure is logged via tracing and the TUI
/// proceeds with the original stderr.
///
/// Boundary conditions:
/// - Idempotent across repeated calls; subsequent calls are cheap dups.
/// - We deliberately leave the saved fd dangling because TUI mode runs
///   until process exit; restoring stderr on `tui_restore` would mean
///   plumbing a global guard through every panic path with no real benefit.
#[cfg(unix)]
fn redirect_stderr_for_tui() -> io::Result<()> {
    use std::{fs::OpenOptions, os::fd::AsRawFd};

    let target_path = std::env::var_os("LIBRA_LOG_FILE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/dev/null"));

    let target = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&target_path)?;

    // SAFETY: libc::dup2 is FFI; we pass valid file descriptors and ignore
    // EINTR-like transient errors via the syscall-loop convention.
    let rc = unsafe { libc::dup2(target.as_raw_fd(), libc::STDERR_FILENO) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    // Intentionally leak the file handle — the kernel keeps fd 2 alive
    // referencing the same inode until process exit.
    std::mem::forget(target);
    Ok(())
}

#[cfg(not(unix))]
fn redirect_stderr_for_tui() -> io::Result<()> {
    // Windows-side TUI does not face the same FUSE child-process leak; if
    // a future Windows backend needs equivalent protection, reuse the
    // same `LIBRA_LOG_FILE` convention here.
    Ok(())
}

/// Set up terminal modes for TUI.
///
/// Functional scope: enables bracketed paste, raw mode, keyboard enhancement
/// flags, and focus-change reporting. Mouse capture is intentionally left
/// disabled so the host terminal's native text-selection still works.
///
/// Boundary conditions: enhancement flag pushes and focus change use
/// `let _ = execute!(...)` because terminals that don't support them return
/// errors that are not actionable — the TUI continues to function with
/// reduced fidelity.
fn set_modes() -> Result<()> {
    execute!(stdout(), EnableBracketedPaste)?;

    // Leave mouse capture disabled so the terminal can provide native text
    // selection and mouse-driven paste. The TUI still accepts mouse events if a
    // terminal sends them without capture, but selection must take priority.
    terminal::enable_raw_mode()?;

    // Enable keyboard enhancement flags for better key event handling
    // (disambiguate Esc, surface key release events). Best-effort.
    let _ = execute!(
        stdout(),
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
        )
    );

    let _ = execute!(stdout(), EnableFocusChange);
    Ok(())
}

/// Restore the terminal to its original state.
///
/// Functional scope: reverses every change made by [`set_modes`] (and a few
/// extras like leaving alt-screen and showing the cursor) so the user's shell
/// is left in a clean state on exit.
///
/// Boundary conditions:
/// - Pop / disable calls use `let _ = execute!` because the matching enables
///   may have failed silently; we still attempt cleanup to avoid stranding
///   the terminal in a partial state.
/// - Called from [`set_panic_hook`] *before* the previous panic hook runs, so
///   panic backtraces print into a sane terminal.
pub fn restore() -> Result<()> {
    // Pop may fail on platforms that didn't support the push; ignore errors.
    let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    execute!(stdout(), DisableBracketedPaste)?;
    let _ = execute!(stdout(), DisableMouseCapture);
    let _ = execute!(stdout(), DisableFocusChange);
    let _ = execute!(stdout(), LeaveAlternateScreen);
    terminal::disable_raw_mode()?;
    let _ = execute!(stdout(), crossterm::cursor::Show);
    Ok(())
}

/// Install a panic hook that restores the terminal before delegating to the
/// previous hook.
///
/// Without this, a panic mid-frame would leave the user staring at a terminal
/// in raw mode with no echo and no cursor.
fn set_panic_hook() {
    let hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        let _ = restore(); // ignore any errors as we are already failing
        hook(panic_info);
    }));
}

/// The TUI wrapper that manages terminal and event streaming.
///
/// Holds the ratatui `Terminal`, a broadcast channel that lets the App
/// schedule redraws, and a stash of the crossterm event stream which is
/// `take()`-ed exactly once when [`Tui::event_stream`] is called.
pub struct Tui {
    /// Underlying ratatui terminal — owns stdout for the duration of the TUI.
    terminal: TerminalType,
    /// Sender side of the redraw broadcast channel; clones are returned by
    /// [`Tui::frame_requester`] so any task can request a frame.
    draw_tx: broadcast::Sender<()>,
    /// Crossterm event stream stashed until consumed by `event_stream`.
    /// Wrapped in `Option` so `event_stream` can move it into the async
    /// generator without leaving a dangling reference behind.
    event_rx: Option<crossterm::event::EventStream>,
}

impl Tui {
    /// Create a new TUI instance from an initialised ratatui terminal.
    ///
    /// Functional scope: builds the redraw broadcast channel (capacity 1; we
    /// only need to know that *some* redraw is queued) and seeds the event
    /// stream stash.
    pub fn new(terminal: TerminalType) -> Self {
        let (draw_tx, _) = broadcast::channel(1);
        Self {
            terminal,
            draw_tx,
            event_rx: Some(crossterm::event::EventStream::new()),
        }
    }

    /// Get a frame requester to schedule redraws.
    ///
    /// Functional scope: returns a clone of the broadcast `Sender`; any task
    /// holding one can call `.send(())` to wake the event loop and trigger a
    /// `TuiEvent::Draw`.
    pub fn frame_requester(&self) -> broadcast::Sender<()> {
        self.draw_tx.clone()
    }

    /// Get the event stream for terminal events.
    ///
    /// Functional scope: merges the crossterm event stream and the redraw
    /// broadcast into a single `Stream<Item = TuiEvent>` so the App's main
    /// loop can `select!` on a single source.
    ///
    /// Boundary conditions:
    /// - Calling `event_stream` more than once is unsupported because the
    ///   crossterm stream is `take()`-ed; subsequent calls would yield a
    ///   stream that immediately stops on terminal events while still
    ///   relaying draw requests.
    /// - When the underlying crossterm stream errors or ends, the source is
    ///   set to `None` so the merged stream stops emitting terminal events
    ///   but continues delivering redraws (useful in tests).
    /// - Lagged broadcast errors are converted into a single `Draw` event —
    ///   we already know we need to redraw, the exact count doesn't matter.
    /// - When the broadcast sender is dropped (`RecvError::Closed`) the draw
    ///   branch is permanently disabled and the loop falls through `else =>
    ///   break` once the terminal stream also ends.
    pub fn event_stream(&mut self) -> Pin<Box<dyn Stream<Item = TuiEvent> + Send + 'static>> {
        let draw_rx = self.draw_tx.subscribe();
        let event_rx = self.event_rx.take();

        Box::pin(async_stream::stream! {
            let mut event_rx = event_rx;
            let mut draw_rx = draw_rx;
            let mut draw_open = true;

            loop {
                tokio::select! {
                    // Handle terminal events. The inner async block awaits the
                    // next crossterm event, but only if we still have a stream
                    // — otherwise the branch is disabled via `if event_rx.is_some()`.
                    terminal_event = async {
                        match &mut event_rx {
                            Some(rx) => rx.next().await,
                            None => None,
                        }
                    }, if event_rx.is_some() => {
                        match terminal_event {
                            Some(Ok(event)) => {
                                // Translate crossterm's heterogeneous Event
                                // enum into our flat TuiEvent variants.
                                match event {
                                    crossterm::event::Event::Key(key) => {
                                        yield TuiEvent::Key(key);
                                    }
                                    crossterm::event::Event::Paste(s) => {
                                        yield TuiEvent::Paste(s);
                                    }
                                    crossterm::event::Event::Mouse(mouse) => {
                                        yield TuiEvent::Mouse(mouse);
                                    }
                                    crossterm::event::Event::Resize(_, _) => {
                                        yield TuiEvent::Resize;
                                    }
                                    _ => {}
                                }
                            }
                            Some(Err(_)) | None => {
                                // Disable the terminal branch on stream end
                                // or unrecoverable error. Tests rely on the
                                // draw channel still working afterwards.
                                event_rx = None;
                            }
                        }
                    }

                    // Handle draw requests. The branch is disabled once the
                    // sender is dropped.
                    draw_event = draw_rx.recv(), if draw_open => {
                        match draw_event {
                            Ok(()) => {
                                yield TuiEvent::Draw;
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                // We dropped intermediate draw signals but a
                                // single redraw is sufficient to catch up.
                                yield TuiEvent::Draw;
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                draw_open = false;
                            }
                        }
                    }
                    else => break,
                }
            }
        })
    }

    /// Draw a frame to the terminal.
    ///
    /// Functional scope: forwards to ratatui's `Terminal::draw` so the App
    /// can render its widget tree without depending directly on ratatui.
    pub fn draw<F>(&mut self, f: F) -> Result<()>
    where
        F: FnOnce(&mut ratatui::Frame),
    {
        self.terminal.draw(f)?;
        Ok(())
    }

    /// Clear the terminal.
    ///
    /// Used after switching alt-screen state so the next `draw` repaints from
    /// scratch instead of layering over leftover ratatui buffers.
    pub fn clear(&mut self) -> Result<()> {
        self.terminal.clear()?;
        Ok(())
    }

    /// Enter alternate screen mode so the TUI gets a clean canvas and the
    /// user's scrollback is preserved.
    pub fn enter_alt_screen(&mut self) -> Result<()> {
        execute!(self.terminal.backend_mut(), EnterAlternateScreen)?;
        Ok(())
    }

    /// Leave alternate screen mode, restoring the user's prior shell content.
    pub fn leave_alt_screen(&mut self) -> Result<()> {
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
        Ok(())
    }

    /// Get the terminal size.
    ///
    /// Functional scope: returns a `Rect` rooted at the origin so callers
    /// can use it directly as the layout root. Always re-queries — even
    /// during a resize storm — so dimensions are never stale.
    pub fn size(&self) -> Result<ratatui::layout::Rect> {
        let size = self.terminal.size()?;
        Ok(ratatui::layout::Rect::new(0, 0, size.width, size.height))
    }
}
