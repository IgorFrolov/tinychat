use textwrap::Options;

use crate::model::{Message, MessageState, Role};

#[derive(Clone, Debug)]
pub enum VisualLine {
    EmptyHistory,
    Metadata { role: Role, state: MessageState },
    Text(String),
    StreamingPlaceholder,
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
        if messages.is_empty() {
            self.lines.push(VisualLine::EmptyHistory);
            return;
        }

        for message in messages {
            self.lines.push(VisualLine::Metadata {
                role: message.role,
                state: message.state,
            });
            if message.content.is_empty() {
                if message.state == MessageState::Streaming {
                    self.lines.push(VisualLine::StreamingPlaceholder);
                }
            } else {
                for physical_line in message.content.split('\n') {
                    if physical_line.is_empty() {
                        self.lines.push(VisualLine::Blank);
                        continue;
                    }
                    self.lines.extend(
                        textwrap::wrap(physical_line, Options::new(width).break_words(true))
                            .into_iter()
                            .map(|line| VisualLine::Text(line.into_owned())),
                    );
                }
            }
            self.lines.push(VisualLine::Blank);
        }
    }
}
