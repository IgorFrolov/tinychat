mod api;
mod app;
mod config;
mod event;
mod input;
mod layout;
pub mod markdown;
mod model;
mod proxy;
mod scroll;
mod ui;

use std::{
    io::{self, Stdout},
    panic,
    sync::atomic::{AtomicBool, Ordering},
};

use anyhow::{Context, Result};
use api::ApiClient;
use app::App;
use clap::Parser;
use config::{AppConfig, Cli};
use crossterm::{
    cursor::{Hide, Show},
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        EventStream, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures_util::StreamExt;
use ratatui::{backend::CrosstermBackend, layout::Rect, Terminal};
use tokio::{sync::mpsc, task::JoinHandle, time};

static KEYBOARD_ENHANCEMENT_PUSHED: AtomicBool = AtomicBool::new(false);

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    enhanced_keys_supported: bool,
}

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode().context("failed to enable terminal raw mode")?;
        let mut stdout = io::stdout();

        let enhanced_keys_supported = matches!(
            crossterm::terminal::supports_keyboard_enhancement(),
            Ok(true)
        );
        if enhanced_keys_supported {
            KEYBOARD_ENHANCEMENT_PUSHED.store(true, Ordering::SeqCst);
            if let Err(error) = execute!(
                stdout,
                PushKeyboardEnhancementFlags(
                    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                        | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
                        | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
                        | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                )
            ) {
                restore_terminal();
                return Err(error).context("failed to enable enhanced keyboard input");
            }
        }

        if let Err(error) = execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste,
            Hide
        ) {
            restore_terminal();
            return Err(error).context("failed to enter alternate screen");
        }
        let terminal = match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => terminal,
            Err(error) => {
                restore_terminal();
                return Err(error).context("failed to initialize TUI");
            }
        };
        Ok(Self {
            terminal,
            enhanced_keys_supported,
        })
    }

    fn set_cursor_visible(&mut self, visible: bool) -> io::Result<()> {
        if visible {
            execute!(self.terminal.backend_mut(), Show)
        } else {
            execute!(self.terminal.backend_mut(), Hide)
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

fn restore_terminal() {
    let mut stdout = io::stdout();
    if KEYBOARD_ENHANCEMENT_PUSHED.swap(false, Ordering::SeqCst) {
        let _ = execute!(stdout, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(
        stdout,
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen,
        Show
    );
    let _ = disable_raw_mode();
}

fn install_panic_hook() {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        restore_terminal();
        previous(info);
    }));
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = AppConfig::load(Cli::parse())?;
    let api_client = ApiClient::new(&config)?;
    install_panic_hook();

    let mut terminal = TerminalGuard::enter()?;
    let mut app = App::new(config);
    app.set_enhanced_keys_supported(terminal.enhanced_keys_supported);
    let result = run(&mut terminal, app, api_client).await;
    drop(terminal);
    result
}

async fn run(terminal: &mut TerminalGuard, mut app: App, api_client: ApiClient) -> Result<()> {
    let mut terminal_events = EventStream::new();
    let (api_tx, mut api_rx) = mpsc::unbounded_channel();
    let mut tick = time::interval(time::Duration::from_millis(200));
    tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
    let mut network_task: Option<JoinHandle<()>> = None;

    loop {
        let area = terminal.terminal.size()?;
        let rect = Rect::new(0, 0, area.width, area.height);
        app.refresh_history_layout(usize::from(area.width));
        app.clamp_scroll(ui::history_metrics(&app, rect));
        terminal.set_cursor_visible(
            !app.model_selector_open
                && area.width >= ui::MIN_WIDTH
                && area.height >= ui::MIN_HEIGHT,
        )?;
        terminal.terminal.draw(|frame| ui::render(frame, &app))?;

        tokio::select! {
            event = terminal_events.next() => {
                match event {
                    Some(Ok(event)) => {
                        let area = terminal.terminal.size()?;
                        let rect = Rect::new(0, 0, area.width, area.height);
                        app.refresh_history_layout(usize::from(area.width));
                        let metrics = ui::history_metrics(&app, rect);
                        if let Some((request, cancellation)) =
                            app.handle_terminal_event(event, metrics)
                        {
                            if let Some(task) = network_task.take() {
                                let _ = task.await;
                            }
                            let client = api_client.clone();
                            let sender = api_tx.clone();
                            network_task = Some(tokio::spawn(async move {
                                client.run(request, cancellation, sender).await;
                            }));
                        }
                    }
                    Some(Err(error)) => return Err(error).context("terminal event error"),
                    None => {
                        app.should_quit = true;
                    }
                }
            }
            event = api_rx.recv() => {
                match event {
                    Some(event) => {
                        let area = terminal.terminal.size()?;
                        let rect = Rect::new(0, 0, area.width, area.height);
                        let before = ui::history_metrics(&app, rect);
                        let preserve_position = !app.scroll.follow_output;
                        app.handle_api_event(event);
                        app.refresh_history_layout(usize::from(area.width));
                        let after = ui::history_metrics(&app, rect);
                        if preserve_position {
                            let added_lines = after.total_lines.saturating_sub(before.total_lines);
                            app.scroll.offset_from_bottom =
                                app.scroll.offset_from_bottom.saturating_add(added_lines);
                            app.clamp_scroll(after);
                        }
                    }
                    None => return Err(anyhow::anyhow!("API event channel closed")),
                }
            }
            _ = tick.tick() => {}
        }

        if network_task.as_ref().is_some_and(JoinHandle::is_finished) {
            if let Some(task) = network_task.take() {
                task.await.context("network task failed")?;
            }
        }
        if app.should_quit {
            break;
        }
    }

    app.cancel_active_request();
    if let Some(task) = network_task {
        task.await.context("network task failed during shutdown")?;
    }
    Ok(())
}
