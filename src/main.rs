mod api;
mod app;
mod config;
mod event;
mod input;
mod layout;
pub mod markdown;
mod model;
mod proxy;
mod ui;

use std::{
    io::{self, Stdout},
    panic,
};

use anyhow::{Context, Result};
use api::ApiClient;
use app::App;
use clap::Parser;
use config::{AppConfig, Cli};
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EventStream},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode},
};
use futures_util::StreamExt;
use ratatui::{backend::CrosstermBackend, Terminal, TerminalOptions, Viewport};
use tokio::{sync::mpsc, task::JoinHandle, time};

const INLINE_VIEWPORT_HEIGHT: u16 = 12;
const API_EVENT_CHANNEL_CAPACITY: usize = 64;

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode().context("failed to enable terminal raw mode")?;
        let mut stdout = io::stdout();

        if let Err(error) = execute!(
            stdout,
            // Leave mouse input to the terminal so users can select any
            // visible part of the conversation and copy it normally.
            DisableMouseCapture,
            EnableBracketedPaste,
            Hide
        ) {
            restore_terminal();
            return Err(error).context("failed to configure terminal input");
        }
        let screen_height = crossterm::terminal::size()
            .map(|(_, height)| height)
            .unwrap_or(INLINE_VIEWPORT_HEIGHT);
        let terminal = match Terminal::with_options(
            CrosstermBackend::new(stdout),
            TerminalOptions {
                viewport: Viewport::Inline(INLINE_VIEWPORT_HEIGHT.min(screen_height)),
            },
        ) {
            Ok(terminal) => terminal,
            Err(error) => {
                restore_terminal();
                return Err(error).context("failed to initialize TUI");
            }
        };
        Ok(Self { terminal })
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
        let area = self.terminal.get_frame().area();
        let _ = self.terminal.clear();
        let _ = execute!(self.terminal.backend_mut(), MoveTo(0, area.y), Show);
        restore_terminal();
    }
}

fn restore_terminal() {
    let mut stdout = io::stdout();
    let _ = execute!(stdout, DisableBracketedPaste, DisableMouseCapture, Show);
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
    let app = App::new(config);
    let result = run(&mut terminal, app, api_client).await;
    drop(terminal);
    result
}

async fn run(terminal: &mut TerminalGuard, mut app: App, api_client: ApiClient) -> Result<()> {
    let mut terminal_events = EventStream::new();
    let (api_tx, mut api_rx) = mpsc::channel(API_EVENT_CHANNEL_CAPACITY);
    let mut tick = time::interval(time::Duration::from_millis(200));
    tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
    let mut network_task: Option<JoinHandle<()>> = None;
    let mut render_pending = true;

    insert_welcome_banner(terminal, app.selected_model())?;

    loop {
        if render_pending {
            flush_stable_transcript(terminal, &mut app)?;
            let area = terminal.terminal.get_frame().area();
            app.refresh_history_layout(usize::from(area.width));
            terminal.set_cursor_visible(
                !app.model_selector_open
                    && area.width >= ui::MIN_WIDTH
                    && area.height >= ui::MIN_HEIGHT,
            )?;
            terminal.terminal.draw(|frame| ui::render(frame, &app))?;
            render_pending = false;
        }

        tokio::select! {
            event = terminal_events.next() => {
                match event {
                    Some(Ok(event)) => {
                        let area = terminal.terminal.get_frame().area();
                        app.refresh_history_layout(usize::from(area.width));
                        let metrics = ui::ui_metrics(area);
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
                        render_pending = true;
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
                        app.handle_api_event(event);
                        while let Ok(event) = api_rx.try_recv() {
                            app.handle_api_event(event);
                        }
                    }
                    None => return Err(anyhow::anyhow!("API event channel closed")),
                }
            }
            _ = tick.tick() => {
                render_pending = true;
            }
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
    flush_stable_transcript(terminal, &mut app)?;
    if let Some(task) = network_task {
        task.await.context("network task failed during shutdown")?;
    }
    Ok(())
}

fn insert_welcome_banner(terminal: &mut TerminalGuard, model: &str) -> Result<()> {
    terminal
        .terminal
        .insert_before(ui::WELCOME_BANNER_HEIGHT, |buffer| {
            ui::render_welcome_banner(buffer, model)
        })?;
    Ok(())
}

fn flush_stable_transcript(terminal: &mut TerminalGuard, app: &mut App) -> Result<()> {
    let end = app.stable_transcript_end();
    if end <= app.transcript_start {
        return Ok(());
    }

    let width = usize::from(terminal.terminal.size()?.width);
    let mut layout = layout::HistoryLayout::default();
    layout.refresh(&app.messages[app.transcript_start..end], width);
    let height = u16::try_from(layout.lines.len()).unwrap_or(u16::MAX);
    if height > 0 {
        terminal.terminal.insert_before(height, |buffer| {
            ui::render_transcript_buffer(buffer, &layout.lines)
        })?;
    }
    app.mark_transcript_committed(end);
    Ok(())
}
