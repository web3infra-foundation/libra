//! Terminal management for the TUI.
//!
//! Provides terminal initialization, restoration, and event streaming.

use std::{
    io::{self, IsTerminal, Result, Stdout, stdin, stdout},
    panic,
    pin::Pin,
    time::Duration,
};

use crossterm::{
    event::{
        DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
        EnableFocusChange, EnableMouseCapture, KeyEvent, KeyboardEnhancementFlags, MouseEvent,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute, terminal,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::broadcast;
use tokio_stream::{Stream, StreamExt};

/// Target frame interval for UI redraw scheduling.
pub const TARGET_FRAME_INTERVAL: Duration = Duration::from_millis(16); // ~60 FPS

/// A type alias for the terminal type used in this application.
pub type TerminalType = Terminal<CrosstermBackend<Stdout>>;

/// Events from the terminal.
#[derive(Debug, Clone)]
pub enum TuiEvent {
    /// Key press event.
    Key(KeyEvent),
    /// Paste event.
    Paste(String),
    /// Mouse event.
    Mouse(MouseEvent),
    /// Request to draw a frame.
    Draw,
    /// Terminal resize event.
    Resize,
}

/// Initialize the terminal for TUI mode.
pub fn init() -> Result<TerminalType> {
    if !stdin().is_terminal() {
        return Err(io::Error::other("stdin is not a terminal"));
    }
    if !stdout().is_terminal() {
        return Err(io::Error::other("stdout is not a terminal"));
    }

    set_modes()?;
    set_panic_hook();

    let backend = CrosstermBackend::new(stdout());
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

/// Set up terminal modes for TUI.
fn set_modes() -> Result<()> {
    execute!(stdout(), EnableBracketedPaste)?;
    execute!(stdout(), EnableMouseCapture)?;

    terminal::enable_raw_mode()?;

    // Enable keyboard enhancement flags for better key event handling.
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
pub fn restore() -> Result<()> {
    // Pop may fail on platforms that didn't support the push; ignore errors.
    let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    execute!(stdout(), DisableBracketedPaste)?;
    let _ = execute!(stdout(), DisableMouseCapture);
    let _ = execute!(stdout(), DisableFocusChange);
    terminal::disable_raw_mode()?;
    let _ = execute!(stdout(), crossterm::cursor::Show);
    Ok(())
}

fn set_panic_hook() {
    let hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        let _ = restore(); // ignore any errors as we are already failing
        hook(panic_info);
    }));
}

/// The TUI wrapper that manages terminal and event streaming.
pub struct Tui {
    terminal: TerminalType,
    draw_tx: broadcast::Sender<()>,
    event_rx: Option<crossterm::event::EventStream>,
}

impl Tui {
    /// Create a new TUI instance.
    pub fn new(terminal: TerminalType) -> Self {
        let (draw_tx, _) = broadcast::channel(1);
        Self {
            terminal,
            draw_tx,
            event_rx: Some(crossterm::event::EventStream::new()),
        }
    }

    /// Get a frame requester to schedule redraws.
    pub fn frame_requester(&self) -> broadcast::Sender<()> {
        self.draw_tx.clone()
    }

    /// Get the event stream for terminal events.
    pub fn event_stream(&mut self) -> Pin<Box<dyn Stream<Item = TuiEvent> + Send + 'static>> {
        let draw_rx = self.draw_tx.subscribe();
        let event_rx = self.event_rx.take();

        Box::pin(async_stream::stream! {
            let mut event_rx = event_rx;
            let mut draw_rx = draw_rx;

            loop {
                tokio::select! {
                    // Handle terminal events
                    Some(Ok(event)) = async {
                        match &mut event_rx {
                            Some(rx) => rx.next().await,
                            None => None,
                        }
                    } => {
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

                    // Handle draw requests
                    Ok(()) = draw_rx.recv() => {
                        yield TuiEvent::Draw;
                    }
                }
            }
        })
    }

    /// Draw a frame to the terminal.
    pub fn draw<F>(&mut self, f: F) -> Result<()>
    where
        F: FnOnce(&mut ratatui::Frame),
    {
        self.terminal.draw(f)?;
        Ok(())
    }

    /// Clear the terminal.
    pub fn clear(&mut self) -> Result<()> {
        self.terminal.clear()?;
        Ok(())
    }

    /// Enter alternate screen mode.
    pub fn enter_alt_screen(&mut self) -> Result<()> {
        execute!(self.terminal.backend_mut(), EnterAlternateScreen)?;
        Ok(())
    }

    /// Leave alternate screen mode.
    pub fn leave_alt_screen(&mut self) -> Result<()> {
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
        Ok(())
    }

    /// Get the terminal size.
    pub fn size(&self) -> Result<ratatui::layout::Rect> {
        let size = self.terminal.size()?;
        Ok(ratatui::layout::Rect::new(0, 0, size.width, size.height))
    }
}
