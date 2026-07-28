use textwrap::Options;

use crate::model::{Message, MessageState, Role};

#[derive(Clone, Debug)]
pub enum VisualLine {
    UserPadding,
    UserText { text: String, first: bool },
    AssistantText { text: String, first: bool },
    SystemText { text: String, first: bool },
    MessageState(MessageState),
    Blank,
}

#[derive(Debug)]
pub struct HistoryLayout {
    width: usize,
    dirty: bool,
    pub lines: Vec<VisualLine>,
}

impl Default for HistoryLayout {
    fn default() -> Self {
        Self {
            width: 0,
            dirty: true,
            lines: Vec::new(),
        }
    }
}

impl HistoryLayout {
    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    pub fn refresh(&mut self, messages: &[Message], width: usize) {
        let width = width.max(1);
        if !self.dirty && self.width == width {
            return;
        }

        self.width = width;
        self.dirty = false;
        self.lines.clear();

        for message in messages {
            if message.role == Role::Assistant
                && message.content.is_empty()
                && message.state == MessageState::Streaming
            {
                continue;
            }

            let wrap_width = match message.role {
                Role::User => width.saturating_sub(3).max(1),
                Role::Assistant | Role::System => width.saturating_sub(2).max(1),
            };
            let mut first = true;

            if message.role == Role::User {
                self.lines.push(VisualLine::UserPadding);
            }

            if !message.content.is_empty() {
                for physical_line in message.content.trim_end_matches('\n').split('\n') {
                    let wrapped = if physical_line.is_empty() {
                        vec![String::new()]
                    } else {
                        textwrap::wrap(physical_line, Options::new(wrap_width).break_words(true))
                            .into_iter()
                            .map(|line| line.into_owned())
                            .collect()
                    };
                    for text in wrapped {
                        self.lines.push(match message.role {
                            Role::User => VisualLine::UserText { text, first },
                            Role::Assistant => VisualLine::AssistantText { text, first },
                            Role::System => VisualLine::SystemText { text, first },
                        });
                        first = false;
                    }
                }
            }

            if message.role == Role::User {
                self.lines.push(VisualLine::UserPadding);
            } else if message.state != MessageState::Complete {
                self.lines.push(VisualLine::MessageState(message.state));
            }

            if !matches!(self.lines.last(), Some(VisualLine::Blank)) {
                self.lines.push(VisualLine::Blank);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_codex_style_message_shapes() {
        let messages = vec![
            Message {
                id: 1,
                role: Role::User,
                content: "hello".into(),
                state: MessageState::Complete,
            },
            Message {
                id: 2,
                role: Role::Assistant,
                content: "hi there".into(),
                state: MessageState::Complete,
            },
        ];
        let mut layout = HistoryLayout::default();
        layout.refresh(&messages, 40);

        assert!(matches!(layout.lines[0], VisualLine::UserPadding));
        assert!(matches!(
            layout.lines[1],
            VisualLine::UserText { first: true, .. }
        ));
        assert!(matches!(layout.lines[2], VisualLine::UserPadding));
        assert!(matches!(
            layout.lines[4],
            VisualLine::AssistantText { first: true, .. }
        ));
    }
}
