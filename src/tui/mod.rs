mod app;
mod forms;
mod log_layer;
mod runner;
mod ui;

use std::io;
use std::sync::{Arc, Mutex};
use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use app::App;
use log_layer::{ChannelLayer, SharedLogSender};

/// Entry point for TUI mode. Sets up the terminal, runs the event loop,
/// and restores the terminal on exit.
pub async fn run() -> Result<()> {
    // Create a shared log sender for routing tracing events to the TUI.
    let shared_sender: SharedLogSender = Arc::new(Mutex::new(None));

    // Install a global tracing subscriber with our channel layer so that
    // tracing events from the Host are captured into the TUI log panel.
    tracing_subscriber::registry()
        .with(ChannelLayer::new(shared_sender.clone()))
        .init();

    // Install a panic hook that restores the terminal before printing the panic.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original_hook(panic_info);
    }));

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(shared_sender);
    let result = run_event_loop(&mut terminal, &mut app).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

async fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    loop {
        terminal.draw(|frame| ui::render(frame, app))?;

        // Poll for events with a small timeout to allow async updates
        if event::poll(std::time::Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                // Only handle key press events (not release/repeat)
                if key.kind == KeyEventKind::Press {
                    if app.handle_key(key.code, key.modifiers).await? {
                        return Ok(());
                    }
                }
            }
        }

        // Tick: update any async state (e.g., log buffer from running host)
        app.tick().await;
    }
}
