use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tokio_util::sync::CancellationToken;

use crate::{
    api::ApiRequest,
    config::AppConfig,
    event::ApiEvent,
    input::InputState,
    layout::HistoryLayout,
    model::{messages_for_request, Message, MessageState, Role, TokenUsage},
};

const QUIT_CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(2);

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
pub struct UiMetrics {
    pub input_width: usize,
}

pub struct App {
    pub messages: Vec<Message>,
    pub transcript_start: usize,
    pub history_layout: HistoryLayout,
    pub input: InputState,
    pub input_history: Vec<String>,
    pub input_history_index: Option<usize>,
    input_history_draft: String,
    pub models: Vec<String>,
    pub selected_model_index: usize,
    pub model_selector_open: bool,
    pub model_selector_index: usize,
    pub shortcuts_open: bool,
    pub enhanced_keys_supported: bool,
    pub request_status: RequestStatus,
    pub active_request: Option<ActiveRequest>,
    pub next_message_id: u64,
    pub next_request_id: u64,
    pub received_chunks: u64,
    pub usage: Option<TokenUsage>,
    quit_requested_at: Option<Instant>,
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
            transcript_start: 0,
            history_layout: HistoryLayout::default(),
            input: InputState::default(),
            input_history: Vec::new(),
            input_history_index: None,
            input_history_draft: String::new(),
            models: config.models.clone(),
            selected_model_index,
            model_selector_open: false,
            model_selector_index: selected_model_index,
            shortcuts_open: false,
            enhanced_keys_supported: false,
            request_status: RequestStatus::Idle,
            active_request: None,
            next_message_id: 1,
            next_request_id: 1,
            received_chunks: 0,
            usage: None,
            quit_requested_at: None,
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

    pub fn quit_confirmation_active(&self) -> bool {
        self.quit_requested_at
            .is_some_and(|requested_at| requested_at.elapsed() < QUIT_CONFIRMATION_TIMEOUT)
    }

    pub fn handle_terminal_event(
        &mut self,
        event: Event,
        metrics: UiMetrics,
    ) -> Option<(ApiRequest, CancellationToken)> {
        match event {
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                self.handle_key(key, metrics)
            }
            Event::Paste(text) => {
                self.quit_requested_at = None;
                self.leave_history_navigation();
                self.input.insert_str(&normalize_paste(&text));
                None
            }
            Event::Resize(_, _) => None,
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

    pub fn refresh_history_layout(&mut self, width: usize) {
        self.history_layout
            .refresh(&self.messages[self.transcript_start..], width);
    }

    pub fn stable_transcript_end(&self) -> usize {
        self.messages
            .len()
            .saturating_sub(usize::from(self.active_request.is_some()))
    }

    pub fn mark_transcript_committed(&mut self, end: usize) {
        self.transcript_start = end.min(self.messages.len());
        self.history_layout.invalidate();
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
        metrics: UiMetrics,
    ) -> Option<(ApiRequest, CancellationToken)> {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if control && matches!(key.code, KeyCode::Char('c' | 'C' | 'с' | 'С')) {
            if key.kind == KeyEventKind::Press {
                self.handle_quit_shortcut();
            }
            return None;
        }
        self.quit_requested_at = None;

        if self.model_selector_open {
            self.handle_model_selector_key(key);
            return None;
        }

        if key.code == KeyCode::Esc && self.shortcuts_open {
            self.shortcuts_open = false;
            return None;
        }
        if key.code == KeyCode::Esc {
            self.cancel_active_request();
            return None;
        }
        if alt && matches!(key.code, KeyCode::Char('m' | 'M')) {
            self.shortcuts_open = false;
            self.model_selector_index = self.selected_model_index;
            self.model_selector_open = true;
            return None;
        }
        if control && matches!(key.code, KeyCode::Char('l' | 'L')) {
            self.clear_session();
            return None;
        }
        if !control && !alt && self.input.is_empty() && matches!(key.code, KeyCode::Char('?')) {
            self.shortcuts_open = !self.shortcuts_open;
            return None;
        }
        self.shortcuts_open = false;

        match key.code {
            KeyCode::Char('d' | 'D') if control => {
                self.leave_history_navigation();
                self.input.delete();
            }
            KeyCode::Char('d' | 'D') if alt => {
                self.leave_history_navigation();
                self.input.delete_next_word();
            }
            KeyCode::Char('a' | 'A') if control => self.input.move_home(),
            KeyCode::Char('b' | 'B') if control => self.input.move_left(),
            KeyCode::Char('e' | 'E') if control => self.input.move_end(),
            KeyCode::Char('f' | 'F') if control => self.input.move_right(),
            KeyCode::Char('p' | 'P') if control => {
                self.move_input_or_history_up(metrics.input_width);
            }
            KeyCode::Char('n' | 'N') if control => {
                self.move_input_or_history_down(metrics.input_width);
            }
            KeyCode::Char('j' | 'J' | 'm' | 'M') if control => {
                self.leave_history_navigation();
                self.input.insert_newline();
            }
            KeyCode::Char('h' | 'H') if control => {
                self.leave_history_navigation();
                self.input.backspace();
            }
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
            KeyCode::Char('y' | 'Y') if control => {
                self.leave_history_navigation();
                self.input.yank();
            }
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) =>
            {
                self.leave_history_navigation();
                self.input.insert_newline();
            }
            KeyCode::Enter if !control => return self.start_request(),
            KeyCode::Left
                if key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) =>
            {
                self.input.move_word_left();
            }
            KeyCode::Right
                if key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) =>
            {
                self.input.move_word_right();
            }
            KeyCode::Left => self.input.move_left(),
            KeyCode::Right => self.input.move_right(),
            KeyCode::Home => self.input.move_home(),
            KeyCode::End => self.input.move_end(),
            KeyCode::Backspace
                if key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) =>
            {
                self.leave_history_navigation();
                self.input.delete_previous_word();
            }
            KeyCode::Backspace => {
                self.leave_history_navigation();
                self.input.backspace();
            }
            KeyCode::Delete
                if key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) =>
            {
                self.leave_history_navigation();
                self.input.delete_next_word();
            }
            KeyCode::Delete => {
                self.leave_history_navigation();
                self.input.delete();
            }
            KeyCode::Up => self.move_input_or_history_up(metrics.input_width),
            KeyCode::Down => self.move_input_or_history_down(metrics.input_width),
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

    fn handle_quit_shortcut(&mut self) {
        if self.quit_confirmation_active() {
            self.should_quit = true;
            return;
        }

        self.cancel_active_request();
        self.model_selector_open = false;
        self.shortcuts_open = false;
        self.quit_requested_at = Some(Instant::now());
    }

    fn handle_model_selector_key(&mut self, key: KeyEvent) {
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if key.code == KeyCode::Esc || (alt && matches!(key.code, KeyCode::Char('m' | 'M'))) {
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

    fn start_request(&mut self) -> Option<(ApiRequest, CancellationToken)> {
        if self.active_request.is_some() || self.input.text().trim().is_empty() {
            return None;
        }

        let text = self.input.clear().trim().to_owned();
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
        self.transcript_start = 0;
        self.history_layout.invalidate();
        self.request_status = RequestStatus::Idle;
        self.shortcuts_open = false;
        self.received_chunks = 0;
        self.usage = None;
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

    fn move_input_or_history_up(&mut self, width: usize) {
        if self.input_history_index.is_some() || !self.input.move_up(width.max(1)) {
            self.history_previous();
        }
    }

    fn move_input_or_history_down(&mut self, width: usize) {
        if self.input_history_index.is_some() || !self.input.move_down(width.max(1)) {
            self.history_next();
        }
    }
}

fn normalize_paste(value: &str) -> String {
    value.replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn shift_enter_inserts_newline_and_plain_enter_submits() {
        let mut app = test_app();
        let metrics = UiMetrics { input_width: 40 };
        app.input.set_text("first".into());

        let newline = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
        assert!(app.handle_key(newline, metrics).is_none());
        assert_eq!(app.input.text(), "first\n");

        app.input.insert_str("second");
        let submit = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let request = app.handle_key(submit, metrics);
        assert!(request.is_some());
        assert_eq!(app.messages[0].content, "first\nsecond");
    }

    #[test]
    fn codex_newline_shortcuts_insert_instead_of_submit() {
        let shortcuts = [
            KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT),
            KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT),
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('m'), KeyModifiers::CONTROL),
        ];
        for shortcut in shortcuts {
            let mut app = test_app();
            app.input.set_text("line".into());
            assert!(app.handle_key(shortcut, UiMetrics::default()).is_none());
            assert_eq!(app.input.text(), "line\n");
            assert!(app.messages.is_empty());
        }
    }

    #[test]
    fn question_mark_toggles_shortcuts_only_for_an_empty_draft() {
        let mut app = test_app();
        let metrics = UiMetrics::default();
        let question_mark = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT);

        app.handle_key(question_mark, metrics);
        assert!(app.shortcuts_open);
        app.handle_key(question_mark, metrics);
        assert!(!app.shortcuts_open);

        app.input.set_text("draft".into());
        app.handle_key(question_mark, metrics);
        assert_eq!(app.input.text(), "draft?");
        assert!(!app.shortcuts_open);
    }

    #[test]
    fn paste_normalizes_newlines_and_inserts_at_cursor() {
        let mut app = test_app();
        app.input.set_text("ac".into());
        app.input.move_left();
        app.handle_terminal_event(Event::Paste("b\r\nline\rnext".into()), UiMetrics::default());
        assert_eq!(app.input.text(), "ab\nline\nnextc");
    }

    #[test]
    fn only_stable_messages_are_ready_for_terminal_scrollback() {
        let mut app = test_app();
        app.input.set_text("hello".into());
        let submit = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.handle_key(submit, UiMetrics::default()).is_some());

        assert_eq!(app.messages.len(), 2);
        assert_eq!(app.stable_transcript_end(), 1);
        app.mark_transcript_committed(1);
        app.refresh_history_layout(40);
        assert!(app.history_layout.lines.is_empty());

        let request_id = app.active_request.as_ref().expect("active request").id;
        app.handle_api_event(ApiEvent::Delta {
            request_id,
            content: "hi".into(),
        });
        assert_eq!(app.stable_transcript_end(), 1);

        app.handle_api_event(ApiEvent::Finished { request_id });
        assert_eq!(app.stable_transcript_end(), 2);
        app.mark_transcript_committed(2);
        app.refresh_history_layout(40);
        assert!(app.history_layout.lines.is_empty());
    }

    #[test]
    fn ctrl_c_requires_confirmation_before_quitting() {
        let mut app = test_app();
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);

        app.handle_key(ctrl_c, UiMetrics::default());
        assert!(app.quit_confirmation_active());
        assert!(!app.should_quit);

        app.handle_key(ctrl_c, UiMetrics::default());
        assert!(app.should_quit);
    }

    #[test]
    fn russian_layout_ctrl_c_requires_confirmation_before_quitting() {
        let mut app = test_app();
        let ctrl_c = KeyEvent::new(KeyCode::Char('с'), KeyModifiers::CONTROL);

        app.handle_key(ctrl_c, UiMetrics::default());
        assert!(app.quit_confirmation_active());
        assert!(!app.should_quit);

        app.handle_key(ctrl_c, UiMetrics::default());
        assert!(app.should_quit);
    }

    #[test]
    fn holding_ctrl_c_does_not_confirm_exit_via_key_repeat() {
        let mut app = test_app();
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let repeated_ctrl_c = KeyEvent::new_with_kind(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
            KeyEventKind::Repeat,
        );

        app.handle_key(ctrl_c, UiMetrics::default());
        app.handle_key(repeated_ctrl_c, UiMetrics::default());

        assert!(app.quit_confirmation_active());
        assert!(!app.should_quit);
    }

    #[test]
    fn expired_quit_confirmation_requires_two_fresh_presses() {
        let mut app = test_app();
        app.quit_requested_at = Some(
            Instant::now()
                .checked_sub(QUIT_CONFIRMATION_TIMEOUT + Duration::from_millis(1))
                .expect("past instant"),
        );
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);

        app.handle_key(ctrl_c, UiMetrics::default());

        assert!(app.quit_confirmation_active());
        assert!(!app.should_quit);
    }

    #[test]
    fn ctrl_c_cancels_active_request_then_requires_confirmation() {
        let mut app = test_app();
        app.input.set_text("hello".into());
        let submit = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.handle_key(submit, UiMetrics::default()).is_some());
        assert!(app.active_request.is_some());

        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        app.handle_key(ctrl_c, UiMetrics::default());
        assert!(app.active_request.is_none());
        assert!(app.quit_confirmation_active());
        assert!(!app.should_quit);

        app.handle_key(ctrl_c, UiMetrics::default());
        assert!(app.should_quit);
    }

    #[test]
    fn ctrl_c_closes_model_selector_and_still_requests_exit() {
        let mut app = test_app();
        app.model_selector_open = true;

        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        app.handle_key(ctrl_c, UiMetrics::default());

        assert!(!app.model_selector_open);
        assert!(app.quit_confirmation_active());
        assert!(!app.should_quit);
    }
}
