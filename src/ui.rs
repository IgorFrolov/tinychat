use std::time::Duration;

use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Widget},
    Frame,
};
use unicode_width::UnicodeWidthStr;

use crate::{
    app::{App, RequestStatus, UiMetrics},
    layout::VisualLine,
    model::MessageState,
};

pub const MIN_WIDTH: u16 = 30;
pub const MIN_HEIGHT: u16 = 8;
pub const WELCOME_BANNER_HEIGHT: u16 = 5;

const LIVE_PREFIX_COLS: u16 = 2;
const COMPOSER_PADDING_ROWS: u16 = 2;
const FOOTER_GAP_ROWS: u16 = 1;
const SHORTCUT_ROWS: u16 = 5;
const MIN_HISTORY_HEIGHT: u16 = 2;
const USER_MESSAGE_STYLE: Style = Style::new().bg(Color::Rgb(38, 38, 38));

pub fn render(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        frame.render_widget(
            Paragraph::new("Terminal is too small")
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::Yellow)),
            area,
        );
        return;
    }

    let (history, panel) = chat_areas(app, area);

    render_history(frame, app, history);
    render_panel(frame, app, panel);

    if app.model_selector_open {
        render_model_selector(frame, app, area);
    } else {
        place_input_cursor(frame, app, panel);
    }
}

pub fn ui_metrics(area: Rect) -> UiMetrics {
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        return UiMetrics::default();
    }
    UiMetrics {
        input_width: input_text_width(area.width),
    }
}

fn chat_areas(app: &App, area: Rect) -> (Rect, Rect) {
    let panel_height = panel_height(app, area).min(area.height);
    let desired_history_height = u16::try_from(app.history_layout.lines.len()).unwrap_or(u16::MAX);
    let content_height = panel_height
        .saturating_add(desired_history_height)
        .min(area.height);
    let history_height = content_height.saturating_sub(panel_height);
    let history = Rect::new(area.x, area.y, area.width, history_height);
    let panel = Rect::new(
        area.x,
        history.bottom(),
        area.width,
        panel_height.min(area.bottom().saturating_sub(history.bottom())),
    );
    (history, panel)
}

fn render_history(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let lines = &app.history_layout.lines;
    let viewport = usize::from(area.height);
    let start = lines.len().saturating_sub(viewport);
    let end = (start + viewport).min(lines.len());
    let visible = &lines[start..end];

    for (index, line) in visible.iter().enumerate() {
        let y = area
            .y
            .saturating_add(u16::try_from(index).unwrap_or(u16::MAX));
        if y >= area.bottom() {
            break;
        }
        let row = Rect::new(area.x, y, area.width, 1);
        if is_user_line(line) {
            frame.render_widget(Block::default().style(USER_MESSAGE_STYLE), row);
        }
    }

    let rendered_lines = visible.iter().map(visual_line).collect::<Vec<_>>();
    // HistoryLayout and the Markdown renderer already produce width-bounded
    // visual rows. A second Paragraph wrap would create rows unknown to the
    // layout and could put the actual tail behind the composer.
    frame.render_widget(Paragraph::new(Text::from(rendered_lines)), area);
}

pub fn render_transcript_buffer(buffer: &mut Buffer, lines: &[VisualLine]) {
    for (index, line) in lines.iter().enumerate() {
        let y = u16::try_from(index).unwrap_or(u16::MAX);
        if y >= buffer.area.height {
            break;
        }
        let row = Rect::new(
            buffer.area.x,
            buffer.area.y.saturating_add(y),
            buffer.area.width,
            1,
        );
        if is_user_line(line) {
            Block::default()
                .style(USER_MESSAGE_STYLE)
                .render(row, buffer);
        }
    }
    Paragraph::new(Text::from(
        lines.iter().map(visual_line).collect::<Vec<_>>(),
    ))
    .render(buffer.area, buffer);
}

pub fn render_welcome_banner(buffer: &mut Buffer, model: &str) {
    let area = Rect::new(
        buffer.area.x,
        buffer.area.y,
        buffer.area.width.min(64),
        WELCOME_BANNER_HEIGHT.saturating_sub(1),
    );
    let title = Line::from(vec![
        Span::styled(">_ ", Style::default().fg(Color::Cyan)),
        Span::styled(
            "tinychat",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    let content = vec![
        Line::from(Span::styled(
            "Welcome to tinychat",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled("model: ", Style::default().add_modifier(Modifier::DIM)),
            Span::raw(model.to_owned()),
        ]),
    ];
    Paragraph::new(content)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .render(area, buffer);
}

fn is_user_line(line: &VisualLine) -> bool {
    matches!(line, VisualLine::UserPadding | VisualLine::UserText { .. })
}

fn visual_line(line: &VisualLine) -> Line<'static> {
    match line {
        VisualLine::UserPadding => Line::default().style(USER_MESSAGE_STYLE),
        VisualLine::UserText { text, first } => Line::from(vec![
            Span::styled(
                if *first { "› " } else { "  " },
                USER_MESSAGE_STYLE.add_modifier(Modifier::BOLD | Modifier::DIM),
            ),
            Span::styled(text.clone(), USER_MESSAGE_STYLE),
        ]),
        VisualLine::AssistantText { line, first } => {
            let mut rendered = line.clone();
            rendered.spans.insert(
                0,
                Span::styled(
                    if *first { "• " } else { "  " },
                    Style::default().add_modifier(Modifier::DIM),
                ),
            );
            rendered
        }
        VisualLine::SystemText { line, first } => {
            let base = Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::DIM);
            let mut rendered = line.clone();
            for span in &mut rendered.spans {
                span.style = base.patch(span.style);
            }
            rendered
                .spans
                .insert(0, Span::styled(if *first { "• " } else { "  " }, base));
            rendered
        }
        VisualLine::QrText { text, first } => Line::from(vec![
            Span::styled(
                if *first { "• " } else { "  " },
                Style::default().add_modifier(Modifier::DIM),
            ),
            Span::styled(
                text.clone(),
                Style::default().fg(Color::Black).bg(Color::White),
            ),
        ]),
        VisualLine::MessageState(state) => match state {
            MessageState::Complete => Line::default(),
            MessageState::Streaming => Line::default(),
            MessageState::Cancelled => Line::from(Span::styled(
                "  ■ Interrupted",
                Style::default().add_modifier(Modifier::DIM),
            )),
            MessageState::Failed => Line::from(Span::styled(
                "  × Response failed",
                Style::default().fg(Color::Red),
            )),
        },
        VisualLine::Blank => Line::default(),
    }
}

fn render_panel(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let status_height = status_height(app);
    if let Some(line) = status_line(app) {
        let status = Rect::new(area.x, area.y, area.width, status_height.min(area.height));
        frame.render_widget(Paragraph::new(line), status);
    }

    let footer_height = footer_content_height(app, frame.area().height);
    let composer_y = area.y.saturating_add(status_height);
    let composer_height = area
        .height
        .saturating_sub(status_height)
        .saturating_sub(FOOTER_GAP_ROWS)
        .saturating_sub(footer_height)
        .max(COMPOSER_PADDING_ROWS + 1);
    let composer = Rect::new(area.x, composer_y, area.width, composer_height);
    render_composer(frame, app, composer);

    let footer_y = composer
        .bottom()
        .saturating_add(FOOTER_GAP_ROWS)
        .min(area.bottom());
    let footer = Rect::new(
        area.x,
        footer_y,
        area.width,
        area.bottom().saturating_sub(footer_y),
    );
    render_footer(frame, app, footer);
}

fn render_composer(frame: &mut Frame<'_>, app: &App, area: Rect) {
    frame.render_widget(Block::default().style(USER_MESSAGE_STYLE), area);

    let inner = Rect::new(
        area.x.saturating_add(LIVE_PREFIX_COLS),
        area.y.saturating_add(1),
        area.width.saturating_sub(LIVE_PREFIX_COLS + 1),
        area.height.saturating_sub(COMPOSER_PADDING_ROWS),
    );
    if inner.is_empty() {
        return;
    }

    let layout = app
        .input
        .layout(input_text_width(area.width), usize::from(inner.height));
    let prefix = Span::styled("› ", USER_MESSAGE_STYLE.add_modifier(Modifier::BOLD));
    let input = if app.input.is_empty() {
        vec![Line::from(vec![
            prefix,
            Span::styled(
                "Ask anything or use /qr <text>",
                USER_MESSAGE_STYLE.add_modifier(Modifier::DIM),
            ),
        ])]
    } else {
        layout
            .lines
            .iter()
            .enumerate()
            .map(|(index, text)| {
                let visual_line = layout.first_visual_line + index;
                Line::from(vec![
                    Span::styled(
                        if visual_line == 0 { "› " } else { "  " },
                        USER_MESSAGE_STYLE.add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(text, USER_MESSAGE_STYLE),
                ])
            })
            .collect()
    };
    let input_area = Rect::new(area.x, inner.y, area.width.saturating_sub(1), inner.height);
    frame.render_widget(Paragraph::new(input), input_area);
}

fn status_line(app: &App) -> Option<Line<'static>> {
    if app.quit_confirmation_active() {
        return Some(Line::from(vec![
            Span::styled(
                "Press ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::DIM),
            ),
            Span::styled(
                "ctrl+c",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " again to exit",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::DIM),
            ),
        ]));
    }

    match &app.request_status {
        RequestStatus::Connecting | RequestStatus::Streaming => {
            let elapsed = app.elapsed().unwrap_or(Duration::ZERO);
            let label = if matches!(app.request_status, RequestStatus::Connecting) {
                "Connecting"
            } else {
                "Working"
            };
            let spans = vec![
                Span::styled("• ", Style::default().fg(Color::Cyan)),
                Span::styled(label, Style::default().fg(Color::Cyan)),
                Span::styled(
                    format!(" ({} • ", format_elapsed_compact(elapsed)),
                    Style::default().add_modifier(Modifier::DIM),
                ),
                Span::styled("esc", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(
                    " to interrupt)",
                    Style::default().add_modifier(Modifier::DIM),
                ),
            ];
            Some(Line::from(spans))
        }
        RequestStatus::Failed(message) => Some(Line::from(vec![
            Span::styled("× ", Style::default().fg(Color::Red)),
            Span::styled(message.clone(), Style::default().fg(Color::Red)),
        ])),
        RequestStatus::Idle | RequestStatus::Completed | RequestStatus::Cancelled => None,
    }
}

fn render_footer(frame: &mut Frame<'_>, app: &App, area: Rect) {
    if area.is_empty() {
        return;
    }
    if app.shortcuts_open && area.height >= SHORTCUT_ROWS {
        frame.render_widget(Paragraph::new(shortcut_lines(app)), area);
        return;
    }

    frame.render_widget(
        Paragraph::new(app.selected_model().to_owned())
            .alignment(Alignment::Right)
            .style(Style::default().add_modifier(Modifier::DIM)),
        Rect::new(
            area.x,
            area.y,
            area.width.saturating_sub(2),
            area.height.min(1),
        ),
    );
}

fn shortcut_lines(app: &App) -> Vec<Line<'static>> {
    let newline = if app.enhanced_keys_supported {
        "shift+enter"
    } else {
        "ctrl+j"
    };
    vec![
        shortcut_line("enter", "send message"),
        shortcut_line(newline, "insert newline"),
        shortcut_line("alt+m", "select model"),
        shortcut_line("esc", "interrupt response"),
        shortcut_line(
            "ctrl+c",
            "press twice to quit  ·  ctrl+l clear  ·  terminal scrollback",
        ),
    ]
}

fn shortcut_line(key: &str, label: &str) -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!("{key:<12}"),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            label.to_owned(),
            Style::default().add_modifier(Modifier::DIM),
        ),
    ])
}

fn place_input_cursor(frame: &mut Frame<'_>, app: &App, panel: Rect) {
    let status_height = status_height(app);
    let footer_height = footer_content_height(app, frame.area().height);
    let input_height = panel
        .height
        .saturating_sub(status_height)
        .saturating_sub(COMPOSER_PADDING_ROWS)
        .saturating_sub(FOOTER_GAP_ROWS)
        .saturating_sub(footer_height)
        .max(1);
    let width = input_text_width(panel.width);
    let layout = app.input.layout(width, usize::from(input_height));
    let x = panel
        .x
        .saturating_add(LIVE_PREFIX_COLS)
        .saturating_add(u16::try_from(layout.cursor_column).unwrap_or(u16::MAX))
        .min(panel.right().saturating_sub(2));
    let y = panel
        .y
        .saturating_add(status_height)
        .saturating_add(1)
        .saturating_add(u16::try_from(layout.cursor_row).unwrap_or(u16::MAX))
        .min(panel.bottom().saturating_sub(2));
    frame.set_cursor_position((x, y));
}

fn panel_height(app: &App, area: Rect) -> u16 {
    let footer_height = footer_content_height(app, area.height);
    let chrome = status_height(app)
        .saturating_add(COMPOSER_PADDING_ROWS)
        .saturating_add(FOOTER_GAP_ROWS)
        .saturating_add(footer_height);
    let max_panel = area.height.saturating_sub(MIN_HISTORY_HEIGHT);
    let max_input = max_panel.saturating_sub(chrome).max(1);
    let desired_input = u16::try_from(app.input.visual_line_count(input_text_width(area.width)))
        .unwrap_or(u16::MAX)
        .clamp(1, max_input);
    chrome.saturating_add(desired_input).min(max_panel)
}

fn status_height(app: &App) -> u16 {
    u16::from(status_line(app).is_some())
}

fn footer_content_height(app: &App, terminal_height: u16) -> u16 {
    if app.shortcuts_open && terminal_height >= 14 {
        SHORTCUT_ROWS
    } else {
        1
    }
}

fn input_text_width(width: u16) -> usize {
    usize::from(width.saturating_sub(LIVE_PREFIX_COLS + 1)).max(1)
}

fn format_elapsed_compact(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!(
            "{}h {:02}m {:02}s",
            seconds / 3600,
            (seconds % 3600) / 60,
            seconds % 60
        )
    }
}

fn render_model_selector(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let desired_width = app
        .models
        .iter()
        .map(|model| UnicodeWidthStr::width(model.as_str()))
        .max()
        .unwrap_or(1)
        .saturating_add(8)
        .max(28);
    let width = u16::try_from(desired_width)
        .unwrap_or(u16::MAX)
        .min(area.width.saturating_sub(4))
        .max(1);
    let desired_height = app.models.len().saturating_add(2);
    let height = u16::try_from(desired_height)
        .unwrap_or(u16::MAX)
        .min(area.height.saturating_sub(2))
        .max(1);
    let modal = centered_rect(width, height, area);

    frame.render_widget(Clear, modal);
    let items = app
        .models
        .iter()
        .map(|model| ListItem::new(format!("  {model}")))
        .collect::<Vec<_>>();
    let list = List::new(items)
        .block(
            Block::default()
                .title(" Select model ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .highlight_symbol("› ")
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    let mut state = ListState::default().with_selected(Some(app.model_selector_index));
    frame.render_stateful_widget(list, modal, &mut state);
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(area.width.saturating_sub(width) / 2),
        y: area
            .y
            .saturating_add(area.height.saturating_sub(height) / 2),
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::AppConfig,
        model::{Message, MessageKind, Role},
    };
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{backend::TestBackend, buffer::Buffer, Terminal, TerminalOptions, Viewport};

    #[test]
    fn compact_elapsed_matches_codex_shape() {
        assert_eq!(format_elapsed_compact(Duration::ZERO), "0s");
        assert_eq!(format_elapsed_compact(Duration::from_secs(65)), "1m 05s");
        assert_eq!(
            format_elapsed_compact(Duration::from_secs(3661)),
            "1h 01m 01s"
        );
    }

    #[test]
    fn renders_codex_style_chat_and_composer() {
        let mut app = test_app();
        app.messages = vec![
            Message {
                id: 1,
                role: Role::User,
                content: "hello".into(),
                state: MessageState::Complete,
                kind: MessageKind::Chat,
            },
            Message {
                id: 2,
                role: Role::Assistant,
                content: "hi there".into(),
                state: MessageState::Complete,
                kind: MessageKind::Chat,
            },
        ];
        app.refresh_history_layout(60);

        let mut terminal = Terminal::new(TestBackend::new(60, 20)).expect("terminal");
        terminal.draw(|frame| render(frame, &app)).expect("render");
        let buffer = terminal.backend().buffer();
        let rows = buffer_rows(buffer);

        let user_row = rows
            .iter()
            .position(|row| row.contains("› hello"))
            .expect("user row");
        assert_eq!(
            buffer[(0, user_row as u16)].style().bg,
            Some(Color::Rgb(38, 38, 38))
        );
        assert!(rows.iter().any(|row| row.contains("• hi there")));
        assert!(rows.iter().any(|row| row.contains("› Ask anything")));
        assert!(rows.iter().all(|row| !row.contains("for shortcuts")));
        assert!(rows.iter().any(|row| row.contains("gpt-4.1-mini")));
    }

    #[test]
    fn renders_h3_as_styled_heading_in_transcript() {
        let mut app = test_app();
        app.messages = vec![Message {
            id: 1,
            role: Role::Assistant,
            content: "### Заголовок".into(),
            state: MessageState::Complete,
            kind: MessageKind::Chat,
        }];
        app.refresh_history_layout(60);

        let mut terminal = Terminal::new(TestBackend::new(60, 20)).expect("terminal");
        terminal.draw(|frame| render(frame, &app)).expect("render");
        let buffer = terminal.backend().buffer();
        let rows = buffer_rows(buffer);
        let heading_row = rows
            .iter()
            .position(|row| row.contains("• Заголовок"))
            .expect("styled heading row");

        assert!(!rows[heading_row].contains("###"));
        let heading_cell = &buffer[(2, heading_row as u16)];
        assert_eq!(heading_cell.style().fg, Some(Color::Cyan));
        assert!(heading_cell.style().add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn transcript_tail_stays_immediately_before_composer_after_resize() {
        let mut app = test_app();
        app.input
            .set_text("draft one\ndraft two\ndraft three".to_owned());
        app.messages = vec![Message {
            id: 1,
            role: Role::Assistant,
            content: concat!(
                "## Intro\n\n",
                "A long Markdown paragraph that reflows when the terminal becomes narrower. ",
                "It is deliberately verbose enough to occupy several visual rows.\n\n",
                "### LAST_VISIBLE"
            )
            .into(),
            state: MessageState::Complete,
            kind: MessageKind::Chat,
        }];

        for (width, height) in [(60, 20), (34, 10), (80, 15)] {
            app.refresh_history_layout(usize::from(width));
            let area = Rect::new(0, 0, width, height);
            let (history, panel) = chat_areas(&app, area);
            assert_eq!(panel.y, history.bottom());
            assert!(panel.bottom() <= area.bottom());

            let mut terminal =
                Terminal::new(TestBackend::new(width, height)).expect("terminal backend");
            terminal.draw(|frame| render(frame, &app)).expect("render");
            let rows = buffer_rows(terminal.backend().buffer());
            let tail_row = rows
                .iter()
                .position(|row| row.contains("LAST_VISIBLE"))
                .unwrap_or_else(|| panic!("missing transcript tail at {width}x{height}: {rows:?}"));

            assert!(
                (tail_row as u16) < panel.y,
                "transcript tail entered panel at {width}x{height}"
            );
            assert!(
                rows[usize::from(panel.y)..]
                    .iter()
                    .all(|row| !row.contains("LAST_VISIBLE")),
                "panel overwrote transcript geometry at {width}x{height}"
            );
        }
    }

    #[test]
    fn empty_composer_is_not_pinned_to_terminal_bottom() {
        let app = test_app();
        let area = Rect::new(0, 0, 60, 20);
        let (history, panel) = chat_areas(&app, area);

        assert_eq!(history.height, 0);
        assert_eq!(panel.y, 0);
        assert!(panel.bottom() < area.bottom());

        let mut terminal = Terminal::new(TestBackend::new(60, 20)).expect("terminal");
        terminal.draw(|frame| render(frame, &app)).expect("render");
        let rows = buffer_rows(terminal.backend().buffer());
        let composer_row = rows
            .iter()
            .position(|row| row.contains("› Ask anything"))
            .expect("composer");
        assert!(composer_row < 4);
    }

    #[test]
    fn committed_message_moves_above_inline_composer() {
        let mut app = test_app();
        app.messages = vec![Message {
            id: 1,
            role: Role::User,
            content: "kept in terminal scrollback".into(),
            state: MessageState::Complete,
            kind: MessageKind::Chat,
        }];
        app.refresh_history_layout(60);
        let committed_lines = app.history_layout.lines.clone();

        let mut terminal = Terminal::with_options(
            TestBackend::new(60, 20),
            TerminalOptions {
                viewport: Viewport::Inline(12),
            },
        )
        .expect("inline terminal");
        terminal
            .insert_before(
                u16::try_from(committed_lines.len()).expect("line count"),
                |buffer| render_transcript_buffer(buffer, &committed_lines),
            )
            .expect("insert history");

        app.mark_transcript_committed(1);
        app.refresh_history_layout(60);
        terminal.draw(|frame| render(frame, &app)).expect("render");
        let rows = buffer_rows(terminal.backend().buffer());
        let message_row = rows
            .iter()
            .position(|row| row.contains("kept in terminal scrollback"))
            .expect("committed message");
        let composer_row = rows
            .iter()
            .position(|row| row.contains("› Ask anything"))
            .expect("composer");

        assert!(message_row < composer_row);
    }

    #[test]
    fn welcome_banner_contains_product_and_model() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 80, WELCOME_BANNER_HEIGHT));
        render_welcome_banner(&mut buffer, "gpt-4.1-mini");
        let rows = buffer_rows(&buffer);

        assert!(rows.iter().any(|row| row.contains(">_ tinychat")));
        assert!(rows.iter().any(|row| row.contains("Welcome to tinychat")));
        assert!(rows.iter().any(|row| row.contains("model: gpt-4.1-mini")));
    }

    #[test]
    fn first_quit_shortcut_renders_confirmation() {
        let mut app = test_app();
        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            UiMetrics::default(),
        );

        let mut terminal = Terminal::new(TestBackend::new(60, 20)).expect("terminal");
        terminal.draw(|frame| render(frame, &app)).expect("render");
        let rows = buffer_rows(terminal.backend().buffer());
        assert!(rows
            .iter()
            .any(|row| row.contains("Press ctrl+c again to exit")));
    }

    #[test]
    fn shortcut_overlay_uses_shift_enter_when_supported() {
        let mut app = test_app();
        app.shortcuts_open = true;
        app.enhanced_keys_supported = true;

        let mut terminal = Terminal::new(TestBackend::new(60, 20)).expect("terminal");
        terminal.draw(|frame| render(frame, &app)).expect("render");
        let rows = buffer_rows(terminal.backend().buffer());

        assert!(rows.iter().any(|row| row.contains("shift+enter")));
        assert!(rows.iter().any(|row| row.contains("insert newline")));
        assert!(rows.iter().any(|row| row.contains("alt+m")));
    }

    fn test_app() -> App {
        App::new(AppConfig {
            base_url: "https://api.openai.com/v1".into(),
            api_key: String::new(),
            model: "gpt-4.1-mini".into(),
            models: vec!["gpt-4.1-mini".into()],
            system_prompt: String::new(),
            temperature: 0.7,
            max_tokens: 4096,
            timeout: Duration::from_secs(120),
        })
    }

    fn buffer_rows(buffer: &Buffer) -> Vec<String> {
        (0..buffer.area.height)
            .map(|y| {
                let mut row = String::new();
                for x in 0..buffer.area.width {
                    row.push_str(buffer[(x, y)].symbol());
                }
                row
            })
            .collect()
    }
}
