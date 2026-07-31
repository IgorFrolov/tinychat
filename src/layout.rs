use std::collections::HashMap;

use ratatui::text::Line;
use textwrap::Options;

use crate::{
    markdown::{
        Hyperlink, MarkdownRenderMode, MarkdownStyles, RenderedMarkdown, StreamingMarkdown,
    },
    model::{Message, MessageKind, MessageState, Role},
};

#[derive(Clone, Debug)]
pub enum VisualLine {
    UserPadding,
    UserText { text: String, first: bool },
    AssistantText { line: Line<'static>, first: bool },
    SystemText { line: Line<'static>, first: bool },
    QrText { text: String, first: bool },
    MessageState(MessageState),
    Blank,
}

#[derive(Clone, Debug)]
struct MarkdownCacheEntry {
    source: String,
    width: u16,
    theme_version: u64,
    mode: MarkdownRenderMode,
    streaming: StreamingMarkdown,
    render_count: usize,
}

impl MarkdownCacheEntry {
    fn new(source: &str, width: u16, styles: &MarkdownStyles, mode: MarkdownRenderMode) -> Self {
        let mut streaming = StreamingMarkdown::new(width);
        streaming.set_mode(mode, styles);
        streaming.append(source, styles);
        Self {
            source: source.to_owned(),
            width,
            theme_version: styles.version(),
            mode,
            streaming,
            render_count: 1,
        }
    }

    fn render(
        &mut self,
        source: &str,
        width: u16,
        styles: &MarkdownStyles,
        mode: MarkdownRenderMode,
    ) -> RenderedMarkdown {
        let exact_cache_hit = self.source == source
            && self.width == width
            && self.theme_version == styles.version()
            && self.mode == mode;
        if exact_cache_hit {
            return self.streaming.rendered();
        }

        self.streaming.set_mode(mode, styles);
        if self.width != width || self.theme_version != styles.version() {
            self.streaming.resize(width, styles);
        }

        if source != self.source {
            if let Some(chunk) = source.strip_prefix(&self.source) {
                self.streaming.append(chunk, styles);
            } else {
                self.streaming.clear();
                self.streaming.append(source, styles);
            }
        } else {
            // Theme-only invalidation is handled even when there is no text
            // delta because append observes the theme version.
            self.streaming.append("", styles);
        }

        self.source.clear();
        self.source.push_str(source);
        self.width = width;
        self.theme_version = styles.version();
        self.mode = mode;
        self.render_count = self.render_count.saturating_add(1);
        self.streaming.rendered()
    }
}

#[derive(Debug)]
pub struct HistoryLayout {
    width: usize,
    dirty: bool,
    styles: MarkdownStyles,
    mode: MarkdownRenderMode,
    markdown_cache: HashMap<u64, MarkdownCacheEntry>,
    pub lines: Vec<VisualLine>,
    pub hyperlinks: Vec<Hyperlink>,
}

impl Default for HistoryLayout {
    fn default() -> Self {
        Self {
            width: 0,
            dirty: true,
            styles: MarkdownStyles::default(),
            mode: MarkdownRenderMode::Rich,
            markdown_cache: HashMap::new(),
            lines: Vec::new(),
            hyperlinks: Vec::new(),
        }
    }
}

impl HistoryLayout {
    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    #[allow(dead_code)] // Public hook for a future UI/config toggle.
    pub fn set_markdown_mode(&mut self, mode: MarkdownRenderMode) {
        if self.mode != mode {
            self.mode = mode;
            self.invalidate();
        }
    }

    #[allow(dead_code)] // Theme owners invalidate the cache through this hook.
    pub fn set_markdown_styles(&mut self, styles: MarkdownStyles) {
        if self.styles.version() != styles.version() {
            self.styles = styles;
            self.invalidate();
        }
    }

    pub fn refresh(&mut self, messages: &[Message], width: usize) {
        let width = width.max(1);
        if !self.dirty && self.width == width {
            return;
        }

        self.width = width;
        self.dirty = false;
        self.lines.clear();
        self.hyperlinks.clear();
        self.markdown_cache
            .retain(|message_id, _| messages.iter().any(|message| message.id == *message_id));

        for message in messages {
            if message.role == Role::Assistant
                && message.content.is_empty()
                && message.state == MessageState::Streaming
            {
                continue;
            }

            if message.kind == MessageKind::Qr {
                self.push_qr_message(message);
            } else if message.role == Role::User {
                self.push_user_message(message, width);
            } else {
                self.push_markdown_message(message, width);
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

    fn push_qr_message(&mut self, message: &Message) {
        for (index, text) in message.content.lines().enumerate() {
            self.lines.push(VisualLine::QrText {
                text: text.to_owned(),
                first: index == 0,
            });
        }
    }

    fn push_user_message(&mut self, message: &Message, width: usize) {
        self.lines.push(VisualLine::UserPadding);
        let wrap_width = width.saturating_sub(3).max(1);
        let mut first = true;
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
                    self.lines.push(VisualLine::UserText { text, first });
                    first = false;
                }
            }
        }
    }

    fn push_markdown_message(&mut self, message: &Message, width: usize) {
        if message.content.is_empty() {
            return;
        }
        let markdown_width = u16::try_from(width.saturating_sub(2).max(1)).unwrap_or(u16::MAX);
        let rendered = match self.markdown_cache.get_mut(&message.id) {
            Some(cache) => cache.render(&message.content, markdown_width, &self.styles, self.mode),
            None => {
                let cache = MarkdownCacheEntry::new(
                    &message.content,
                    markdown_width,
                    &self.styles,
                    self.mode,
                );
                let rendered = cache.streaming.rendered();
                self.markdown_cache.insert(message.id, cache);
                rendered
            }
        };

        let base_row = self.lines.len();
        self.hyperlinks
            .extend(rendered.hyperlinks.into_iter().map(|mut hyperlink| {
                hyperlink.row += base_row;
                hyperlink.columns = hyperlink.columns.start.saturating_add(2)
                    ..hyperlink.columns.end.saturating_add(2);
                hyperlink
            }));

        for (index, line) in rendered.lines.into_iter().enumerate() {
            self.lines.push(match message.role {
                Role::Assistant => VisualLine::AssistantText {
                    line,
                    first: index == 0,
                },
                Role::System => VisualLine::SystemText {
                    line,
                    first: index == 0,
                },
                Role::User => unreachable!("user messages use plain layout"),
            });
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
                kind: MessageKind::Chat,
            },
            Message {
                id: 2,
                role: Role::Assistant,
                content: "**hi** there".into(),
                state: MessageState::Complete,
                kind: MessageKind::Chat,
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

    #[test]
    fn repeated_identical_text_hits_full_cache() {
        let messages = vec![Message {
            id: 7,
            role: Role::Assistant,
            content: "same **text**".into(),
            state: MessageState::Streaming,
            kind: MessageKind::Chat,
        }];
        let mut layout = HistoryLayout::default();
        layout.refresh(&messages, 40);
        assert_eq!(layout.markdown_cache[&7].render_count, 1);

        layout.invalidate();
        layout.refresh(&messages, 40);
        assert_eq!(layout.markdown_cache[&7].render_count, 1);
    }

    #[test]
    fn non_append_change_rebuilds_cached_source() {
        let mut messages = vec![Message {
            id: 7,
            role: Role::Assistant,
            content: "original".into(),
            state: MessageState::Streaming,
            kind: MessageKind::Chat,
        }];
        let mut layout = HistoryLayout::default();
        layout.refresh(&messages, 40);
        messages[0].content = "replacement".into();
        layout.invalidate();
        layout.refresh(&messages, 40);

        let cache = &layout.markdown_cache[&7];
        assert_eq!(cache.source, "replacement");
        assert_eq!(cache.streaming.source(), "replacement");
    }

    #[test]
    fn transcript_hyperlinks_include_chat_prefix_columns() {
        let messages = vec![Message {
            id: 7,
            role: Role::Assistant,
            content: "[site](https://example.com)".into(),
            state: MessageState::Complete,
            kind: MessageKind::Chat,
        }];
        let mut layout = HistoryLayout::default();
        layout.refresh(&messages, 40);

        assert_eq!(layout.hyperlinks.len(), 1);
        assert_eq!(layout.hyperlinks[0].columns, 2..6);
    }

    #[test]
    fn theme_version_and_mode_invalidate_cache() {
        let messages = vec![Message {
            id: 7,
            role: Role::Assistant,
            content: "**text**".into(),
            state: MessageState::Complete,
            kind: MessageKind::Chat,
        }];
        let mut layout = HistoryLayout::default();
        layout.refresh(&messages, 40);
        assert_eq!(layout.markdown_cache[&7].render_count, 1);

        layout.set_markdown_styles(MarkdownStyles::default().with_version(1));
        layout.refresh(&messages, 40);
        assert_eq!(layout.markdown_cache[&7].render_count, 2);

        layout.set_markdown_mode(MarkdownRenderMode::Raw);
        layout.refresh(&messages, 40);
        assert_eq!(layout.markdown_cache[&7].render_count, 3);
        let rendered = match &layout.lines[0] {
            VisualLine::AssistantText { line, .. } => line.to_string(),
            other => panic!("expected assistant text, got {other:?}"),
        };
        assert_eq!(rendered, "**text**");
    }

    #[test]
    fn keeps_qr_rows_verbatim_without_markdown_wrapping() {
        let messages = vec![Message {
            id: 8,
            role: Role::Assistant,
            content: " ▀█ \n ▄█ ".into(),
            state: MessageState::Complete,
            kind: MessageKind::Qr,
        }];
        let mut layout = HistoryLayout::default();
        layout.refresh(&messages, 3);

        assert!(matches!(
            &layout.lines[0],
            VisualLine::QrText { text, first: true } if text == " ▀█ "
        ));
        assert!(matches!(
            &layout.lines[1],
            VisualLine::QrText { text, first: false } if text == " ▄█ "
        ));
    }
}
