use std::time::{Duration, Instant};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};
use tokio_util::sync::CancellationToken;

use crate::{
    api::ApiRequest,
    config::AppConfig,
    event::ApiEvent,
    input::InputState,
    layout::HistoryLayout,
    model::{messages_for_request, Message, MessageState, Role, TokenUsage},
    scroll::ScrollState,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RequestStatus {
    Idle,
    Connecting,
    Streaming,
    Completed,
    Cancelled,
    Failed(String),
}

#[derive(Debug)]
pub struct ActiveRequest {
    pub id: u64,
    pub started_at: Instant,
    pub cancellation: CancellationToken,
    pub assistant_message_id: u64,
    pub chunks_received: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HistoryMetrics {
    pub total_lines: usize,
    pub viewport_lines: usize,
}

pub struct App {
    pub messages: Vec<Message>,
    pub history_layout: HistoryLayout,
    pub input: InputState,
    pub input_history: Vec<String>,
    pub input_history_index: Option<usize>,
    input_history_draft: String,
    pub scroll: ScrollState,
    pub models: Vec<String>,
    pub selected_model_index: usize,
    pub model_selector_open: bool,
    pub model_selector_index: usize,
    pub request_status: RequestStatus,
    pub active_request: Option<ActiveRequest>,
    pub next_message_id: u64,
    pub next_request_id: u64,
    pub received_chunks: u64,
    pub usage: Option<TokenUsage>,
    pub new_output_while_scrolled: bool,
    pub should_quit: bool,
    pub config: AppConfig,
}

impl App {
    pub fn new(config: AppConfig) -> Self {
        let selected_model_index = config
            .models
            .iter()
            .position(|model| model == &config.model)
            .unwrap_or(0);
        Self {
            messages: Vec::new(),
            history_layout: HistoryLayout::default(),
            input: InputState::default(),
            input_history: Vec::new(),
            input_history_index: None,
            input_history_draft: String::new(),
            scroll: ScrollState::default(),
            models: config.models.clone(),
            selected_model_index,
            model_selector_open: false,
            model_selector_index: selected_model_index,
            request_status: RequestStatus::Idle,
            active_request: None,
            next_message_id: 1,
            next_request_id: 1,
            received_chunks: 0,
            usage: None,
            new_output_while_scrolled: false,
            should_quit: false,
            config,
        }
    }

    pub fn selected_model(&self) -> &str {
        self.models
            .get(self.selected_model_index)
            .map(String::as_str)
            .unwrap_or(self.config.model.as_str())
    }

    pub fn elapsed(&self) -> Option<Duration> {
        self.active_request
            .as_ref()
            .map(|request| request.started_at.elapsed())
    }

    pub fn active_task_count(&self) -> usize {
        usize::from(self.active_request.is_some())
    }

    pub fn handle_terminal_event(
        &mut self,
        event: Event,
        metrics: HistoryMetrics,
    ) -> Option<(ApiRequest, CancellationToken)> {
        match event {
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                self.handle_key(key, metrics)
            }
            Event::Mouse(mouse) => {
                self.handle_mouse(mouse, metrics);
                None
            }
            Event::Resize(_, _) => {
                self.clamp_scroll(metrics);
                None
            }
            _ => None,
        }
    }

    pub fn handle_api_event(&mut self, event: ApiEvent) {
        let request_id = event.request_id();
        if self.active_request.as_ref().map(|request| request.id) != Some(request_id) {
            return;
        }

        match event {
            ApiEvent::Started { .. } => {
                self.request_status = RequestStatus::Connecting;
            }
            ApiEvent::Delta { content, .. } => {
                if content.is_empty() {
                    return;
                }
                let Some(active) = self.active_request.as_mut() else {
                    return;
                };
                active.chunks_received = active.chunks_received.saturating_add(1);
                self.received_chunks = active.chunks_received;
                if let Some(message) = self
                    .messages
                    .iter_mut()
                    .find(|message| message.id == active.assistant_message_id)
                {
                    message.content.push_str(&content);
                }
                self.history_layout.invalidate();
                self.request_status = RequestStatus::Streaming;
                if self.scroll.follow_output {
                    self.scroll.offset_from_bottom = 0;
                } else {
                    self.new_output_while_scrolled = true;
                }
            }
            ApiEvent::Usage {
                prompt_tokens,
                completion_tokens,
                total_tokens,
                ..
            } => {
                self.usage = Some(TokenUsage {
                    prompt_tokens,
                    completion_tokens,
                    total_tokens,
                });
            }
            ApiEvent::Finished { .. } => {
                self.finish_message(MessageState::Complete);
                self.active_request = None;
                self.request_status = RequestStatus::Completed;
            }
            ApiEvent::Cancelled { .. } => {
                self.finish_message(MessageState::Cancelled);
                self.active_request = None;
                self.request_status = RequestStatus::Cancelled;
            }
            ApiEvent::Failed { message, .. } => {
                self.finish_message(MessageState::Failed);
                self.active_request = None;
                self.request_status = RequestStatus::Failed(message);
            }
        }
    }

    pub fn clamp_scroll(&mut self, metrics: HistoryMetrics) {
        self.scroll
            .clamp(metrics.total_lines, metrics.viewport_lines);
        if self.scroll.follow_output {
            self.new_output_while_scrolled = false;
        }
    }

    pub fn refresh_history_layout(&mut self, width: usize) {
        self.history_layout.refresh(&self.messages, width);
    }

    pub fn cancel_active_request(&mut self) {
        let Some(active) = self.active_request.take() else {
            return;
        };
        active.cancellation.cancel();
        if let Some(message) = self
            .messages
            .iter_mut()
            .find(|message| message.id == active.assistant_message_id)
        {
            message.state = MessageState::Cancelled;
        }
        self.history_layout.invalidate();
        self.request_status = RequestStatus::Cancelled;
    }

    fn handle_key(
        &mut self,
        key: KeyEvent,
        metrics: HistoryMetrics,
    ) -> Option<(ApiRequest, CancellationToken)> {
        if self.model_selector_open {
            self.handle_model_selector_key(key);
            return None;
        }

        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        if control && matches!(key.code, KeyCode::Char('c' | 'C')) {
            if self.active_request.is_some() {
                self.cancel_active_request();
            } else {
                self.should_quit = true;
            }
            return None;
        }
        if key.code == KeyCode::Esc {
            self.cancel_active_request();
            return None;
        }
        if control && matches!(key.code, KeyCode::Char('m' | 'M')) {
            self.model_selector_index = self.selected_model_index;
            self.model_selector_open = true;
            return None;
        }
        if control && matches!(key.code, KeyCode::Char('l' | 'L')) {
            self.clear_session();
            return None;
        }

        match key.code {
            KeyCode::PageUp => self.scroll_up(metrics.viewport_lines, metrics),
            KeyCode::PageDown => self.scroll_down(metrics.viewport_lines),
            KeyCode::Home if control => {
                self.scroll.top(metrics.total_lines, metrics.viewport_lines);
            }
            KeyCode::End if control => self.scroll_to_bottom(),
            KeyCode::Char('u' | 'U') if control && self.input.is_empty() => {
                self.scroll_up(half_viewport(metrics.viewport_lines), metrics);
            }
            KeyCode::Char('d' | 'D') if control => {
                self.scroll_down(half_viewport(metrics.viewport_lines));
            }
            KeyCode::Char('a' | 'A') if control => self.input.move_home(),
            KeyCode::Char('e' | 'E') if control => self.input.move_end(),
            KeyCode::Char('w' | 'W') if control => {
                self.leave_history_navigation();
                self.input.delete_previous_word();
            }
            KeyCode::Char('k' | 'K') if control => {
                self.leave_history_navigation();
                self.input.delete_to_end();
            }
            KeyCode::Char('u' | 'U') if control => {
                self.leave_history_navigation();
                self.input.delete_to_start();
            }
            KeyCode::Enter => return self.start_request(),
            KeyCode::Left => self.input.move_left(),
            KeyCode::Right => self.input.move_right(),
            KeyCode::Home => self.input.move_home(),
            KeyCode::End => self.input.move_end(),
            KeyCode::Backspace => {
                self.leave_history_navigation();
                self.input.backspace();
            }
            KeyCode::Delete => {
                self.leave_history_navigation();
                self.input.delete();
            }
            KeyCode::Up => self.history_previous(),
            KeyCode::Down => self.history_next(),
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.leave_history_navigation();
                self.input.insert(character);
            }
            _ => {}
        }
        None
    }

    fn handle_model_selector_key(&mut self, key: KeyEvent) {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::Esc || (control && matches!(key.code, KeyCode::Char('m' | 'M'))) {
            self.model_selector_open = false;
            return;
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.model_selector_index = self.model_selector_index.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.model_selector_index =
                    (self.model_selector_index + 1).min(self.models.len().saturating_sub(1));
            }
            KeyCode::Enter => {
                self.selected_model_index = self.model_selector_index;
                self.model_selector_open = false;
            }
            _ => {}
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, metrics: HistoryMetrics) {
        if self.model_selector_open {
            return;
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => self.scroll_up(3, metrics),
            MouseEventKind::ScrollDown => self.scroll_down(3),
            _ => {}
        }
    }

    fn start_request(&mut self) -> Option<(ApiRequest, CancellationToken)> {
        if self.active_request.is_some() || self.input.text().trim().is_empty() {
            return None;
        }

        let text = self.input.clear();
        if self.input_history.last() != Some(&text) {
            self.input_history.push(text.clone());
        }
        self.input_history_index = None;
        self.input_history_draft.clear();

        let user_message_id = self.take_message_id();
        self.messages.push(Message {
            id: user_message_id,
            role: Role::User,
            content: text,
            state: MessageState::Complete,
        });
        let assistant_message_id = self.take_message_id();
        self.messages.push(Message {
            id: assistant_message_id,
            role: Role::Assistant,
            content: String::new(),
            state: MessageState::Streaming,
        });
        self.history_layout.invalidate();

        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        let cancellation = CancellationToken::new();
        self.active_request = Some(ActiveRequest {
            id: request_id,
            started_at: Instant::now(),
            cancellation: cancellation.clone(),
            assistant_message_id,
            chunks_received: 0,
        });
        self.request_status = RequestStatus::Connecting;
        self.received_chunks = 0;
        self.usage = None;
        self.scroll.bottom();
        self.new_output_while_scrolled = false;

        let request = ApiRequest {
            id: request_id,
            model: self.selected_model().to_owned(),
            messages: messages_for_request(&self.messages, &self.config.system_prompt),
        };
        Some((request, cancellation))
    }

    fn take_message_id(&mut self) -> u64 {
        let id = self.next_message_id;
        self.next_message_id = self.next_message_id.saturating_add(1);
        id
    }

    fn finish_message(&mut self, state: MessageState) {
        let Some(message_id) = self
            .active_request
            .as_ref()
            .map(|request| request.assistant_message_id)
        else {
            return;
        };
        if let Some(message) = self
            .messages
            .iter_mut()
            .find(|message| message.id == message_id)
        {
            message.state = state;
        }
        self.history_layout.invalidate();
    }

    fn clear_session(&mut self) {
        self.cancel_active_request();
        self.messages.clear();
        self.history_layout.invalidate();
        self.scroll = ScrollState::default();
        self.request_status = RequestStatus::Idle;
        self.received_chunks = 0;
        self.usage = None;
        self.new_output_while_scrolled = false;
    }

    fn scroll_up(&mut self, amount: usize, metrics: HistoryMetrics) {
        self.scroll
            .up(amount, metrics.total_lines, metrics.viewport_lines);
    }

    fn scroll_down(&mut self, amount: usize) {
        self.scroll.down(amount);
        if self.scroll.follow_output {
            self.new_output_while_scrolled = false;
        }
    }

    fn scroll_to_bottom(&mut self) {
        self.scroll.bottom();
        self.new_output_while_scrolled = false;
    }

    fn history_previous(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        let index = match self.input_history_index {
            None => {
                self.input_history_draft = self.input.text().to_owned();
                self.input_history.len() - 1
            }
            Some(index) => index.saturating_sub(1),
        };
        self.input_history_index = Some(index);
        self.input.set_text(self.input_history[index].clone());
    }

    fn history_next(&mut self) {
        let Some(index) = self.input_history_index else {
            return;
        };
        if index + 1 < self.input_history.len() {
            let next = index + 1;
            self.input_history_index = Some(next);
            self.input.set_text(self.input_history[next].clone());
        } else {
            self.input_history_index = None;
            self.input
                .set_text(std::mem::take(&mut self.input_history_draft));
        }
    }

    fn leave_history_navigation(&mut self) {
        if self.input_history_index.take().is_some() {
            self.input_history_draft.clear();
        }
    }
}

fn half_viewport(viewport: usize) -> usize {
    (viewport / 2).max(1)
}

pub fn format_elapsed(duration: Duration) -> String {
    let total = duration.as_secs();
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_elapsed_time() {
        assert_eq!(format_elapsed(Duration::from_secs(0)), "00:00");
        assert_eq!(format_elapsed(Duration::from_secs(87)), "01:27");
        assert_eq!(format_elapsed(Duration::from_secs(3861)), "01:04:21");
    }
}
