use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::{
    app::{format_elapsed, App, HistoryMetrics, RequestStatus},
    layout::VisualLine,
    model::{MessageState, Role},
};

pub const MIN_WIDTH: u16 = 30;
pub const MIN_HEIGHT: u16 = 6;

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

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(3)])
        .split(area);
    render_history(frame, app, sections[0]);
    render_panel(frame, app, sections[1]);

    if app.model_selector_open {
        render_model_selector(frame, app, area);
    } else {
        place_input_cursor(frame, app, sections[1]);
    }
}

pub fn history_metrics(app: &App, area: Rect) -> HistoryMetrics {
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        return HistoryMetrics::default();
    }
    let history_height = area.height.saturating_sub(3);
    let viewport_lines = usize::from(history_height.saturating_sub(1));
    HistoryMetrics {
        total_lines: app.history_layout.lines.len(),
        viewport_lines,
    }
}

fn render_history(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = &app.history_layout.lines;
    let viewport = usize::from(inner.height);
    let maximum_offset = lines.len().saturating_sub(viewport);
    let offset = app.scroll.offset_from_bottom.min(maximum_offset);
    let start = lines.len().saturating_sub(viewport.saturating_add(offset));
    let end = (start + viewport).min(lines.len());
    let visible = &lines[start..end];
    let top_padding = viewport.saturating_sub(visible.len());
    let mut display = Vec::with_capacity(viewport);
    display.extend((0..top_padding).map(|_| Line::default()));
    display.extend(visible.iter().map(visual_line));
    frame.render_widget(Paragraph::new(display), inner);
}

fn visual_line(line: &VisualLine) -> Line<'_> {
    match line {
        VisualLine::EmptyHistory => Line::from(Span::styled(
            "Enter a message to start",
            Style::default().fg(Color::DarkGray),
        )),
        VisualLine::Metadata { role, state } => {
            let (role, color) = match role {
                Role::User => ("You", Color::Cyan),
                Role::Assistant => ("Assistant", Color::Green),
                Role::System => ("System", Color::Yellow),
            };
            let suffix = match state {
                MessageState::Complete => "",
                MessageState::Streaming => " · streaming",
                MessageState::Cancelled => " · cancelled",
                MessageState::Failed => " · failed",
            };
            let state_color = match state {
                MessageState::Failed => Color::Red,
                MessageState::Cancelled => Color::DarkGray,
                _ => color,
            };
            Line::from(vec![
                Span::styled(
                    role,
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(suffix, Style::default().fg(state_color)),
            ])
        }
        VisualLine::Text(text) => Line::from(text.as_str()),
        VisualLine::StreamingPlaceholder => {
            Line::from(Span::styled("…", Style::default().fg(Color::DarkGray)))
        }
        VisualLine::Blank => Line::default(),
    }
}

fn render_panel(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    frame.render_widget(Paragraph::new(status_line(app)), rows[0]);

    let input_width = usize::from(rows[1].width.saturating_sub(2)).max(1);
    let input = if app.input.is_empty() {
        Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::Cyan)),
            Span::styled("Введите сообщение...", Style::default().fg(Color::DarkGray)),
        ])
    } else {
        let viewport = app.input.viewport(input_width);
        Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::Cyan)),
            Span::raw(viewport.text),
        ])
    };
    frame.render_widget(Paragraph::new(input), rows[1]);

    let left = format!("model: {}", app.selected_model());
    let hint = "Ctrl+M сменить модель";
    let model_line = align_edges(&left, hint, usize::from(rows[2].width));
    frame.render_widget(
        Paragraph::new(model_line).style(Style::default().fg(Color::DarkGray)),
        rows[2],
    );
}

fn status_line(app: &App) -> Line<'static> {
    let mut spans = vec![Span::styled(
        format!("● задач: {}", app.active_task_count()),
        Style::default().fg(if app.active_request.is_some() {
            Color::Cyan
        } else {
            Color::DarkGray
        }),
    )];

    if let Some(elapsed) = app.elapsed() {
        spans.push(Span::raw(format!(
            "   ожидание: {}   chunks: {}",
            format_elapsed(elapsed),
            app.received_chunks
        )));
    } else if app.received_chunks > 0 {
        spans.push(Span::raw(format!("   chunks: {}", app.received_chunks)));
    }

    let (label, color) = match &app.request_status {
        RequestStatus::Idle => ("idle".to_owned(), Color::DarkGray),
        RequestStatus::Connecting => ("connecting".to_owned(), Color::Cyan),
        RequestStatus::Streaming => ("streaming".to_owned(), Color::Cyan),
        RequestStatus::Completed => ("completed".to_owned(), Color::Green),
        RequestStatus::Cancelled => ("cancelled".to_owned(), Color::Yellow),
        RequestStatus::Failed(message) => (format!("error: {message}"), Color::Red),
    };
    spans.push(Span::styled(
        format!("   {label}"),
        Style::default().fg(color),
    ));
    if app.new_output_while_scrolled {
        spans.push(Span::styled(
            "   ↓ новый вывод",
            Style::default().fg(Color::Cyan),
        ));
    }
    Line::from(spans)
}

fn place_input_cursor(frame: &mut Frame<'_>, app: &App, panel: Rect) {
    let width = usize::from(panel.width.saturating_sub(2)).max(1);
    let viewport = app.input.viewport(width);
    let x = panel
        .x
        .saturating_add(2)
        .saturating_add(u16::try_from(viewport.cursor_column).unwrap_or(u16::MAX))
        .min(panel.right().saturating_sub(1));
    let y = panel.y.saturating_add(1);
    frame.set_cursor_position((x, y));
}

fn render_model_selector(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let desired_width = app
        .models
        .iter()
        .map(|model| UnicodeWidthStr::width(model.as_str()))
        .max()
        .unwrap_or(1)
        .saturating_add(6)
        .max(24);
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
        .map(|model| ListItem::new(model.as_str()))
        .collect::<Vec<_>>();
    let list = List::new(items)
        .block(
            Block::default()
                .title(" Select model ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .highlight_symbol("> ")
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

fn align_edges(left: &str, right: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let left_width = UnicodeWidthStr::width(left);
    let right_width = UnicodeWidthStr::width(right);
    if left_width + right_width < width {
        return format!(
            "{left}{}{right}",
            " ".repeat(width - left_width - right_width)
        );
    }
    truncate_width(left, width)
}

fn truncate_width(value: &str, width: usize) -> String {
    let mut result = String::new();
    let mut used = 0;
    for grapheme in value.graphemes(true) {
        let grapheme_width = UnicodeWidthStr::width(grapheme);
        if used + grapheme_width > width {
            break;
        }
        result.push_str(grapheme);
        used += grapheme_width;
    }
    result
}
