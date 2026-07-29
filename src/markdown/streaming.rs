use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::text::Line;

use super::{
    renderer::render_markdown_mode, Hyperlink, MarkdownRenderMode, MarkdownStyles, RenderedMarkdown,
};

/// Incremental Markdown renderer using a stable prefix and one mutable
/// top-level tail block.
#[derive(Clone, Debug)]
pub struct StreamingMarkdown {
    source: String,
    stable_source_len: usize,
    stable_lines: Vec<Line<'static>>,
    mutable_lines: Vec<Line<'static>>,
    width: u16,
    stable_hyperlinks: Vec<Hyperlink>,
    mutable_hyperlinks: Vec<Hyperlink>,
    theme_version: u64,
    mode: MarkdownRenderMode,
}

impl StreamingMarkdown {
    pub fn new(width: u16) -> Self {
        Self {
            source: String::new(),
            stable_source_len: 0,
            stable_lines: Vec::new(),
            mutable_lines: Vec::new(),
            width: width.max(1),
            stable_hyperlinks: Vec::new(),
            mutable_hyperlinks: Vec::new(),
            theme_version: 0,
            mode: MarkdownRenderMode::Rich,
        }
    }

    pub fn append(&mut self, chunk: &str, styles: &MarkdownStyles) {
        let theme_changed = self.theme_version != styles.version();
        if !chunk.is_empty() {
            self.source.push_str(chunk);
        }
        if chunk.is_empty() && !theme_changed {
            return;
        }
        self.theme_version = styles.version();

        if theme_changed || contains_reference_definition(&self.source) {
            self.rebuild(styles);
            return;
        }

        let tail = &self.source[self.stable_source_len..];
        if let Some(boundary) = last_top_level_block_start(tail).filter(|offset| *offset > 0) {
            let completed =
                render_markdown_mode(&tail[..boundary], self.width, styles, self.mode, None);
            self.promote(completed);
            self.stable_source_len += boundary;
        }
        self.render_mutable(styles);
    }

    pub fn resize(&mut self, width: u16, styles: &MarkdownStyles) {
        let width = width.max(1);
        if self.width == width && self.theme_version == styles.version() {
            return;
        }
        self.width = width;
        self.theme_version = styles.version();
        self.rebuild(styles);
    }

    pub fn lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::with_capacity(self.stable_lines.len() + self.mutable_lines.len() + 1);
        lines.extend(self.stable_lines.iter().cloned());
        if !self.stable_lines.is_empty() && !self.mutable_lines.is_empty() {
            lines.push(Line::default());
        }
        lines.extend(self.mutable_lines.iter().cloned());
        lines
    }

    pub fn clear(&mut self) {
        self.source.clear();
        self.stable_source_len = 0;
        self.stable_lines.clear();
        self.mutable_lines.clear();
        self.stable_hyperlinks.clear();
        self.mutable_hyperlinks.clear();
    }

    pub fn set_mode(&mut self, mode: MarkdownRenderMode, styles: &MarkdownStyles) {
        if self.mode != mode {
            self.mode = mode;
            self.rebuild(styles);
        }
    }

    #[cfg(test)]
    pub(crate) fn source(&self) -> &str {
        &self.source
    }

    pub(crate) fn rendered(&self) -> RenderedMarkdown {
        let lines = self.lines();
        let separator_rows =
            usize::from(!self.stable_lines.is_empty() && !self.mutable_lines.is_empty());
        let mutable_row_offset = self.stable_lines.len() + separator_rows;
        let mut hyperlinks = self.stable_hyperlinks.clone();
        hyperlinks.extend(
            self.mutable_hyperlinks
                .iter()
                .cloned()
                .map(|mut hyperlink| {
                    hyperlink.row += mutable_row_offset;
                    hyperlink
                }),
        );
        RenderedMarkdown { lines, hyperlinks }
    }

    fn rebuild(&mut self, styles: &MarkdownStyles) {
        self.stable_source_len = 0;
        self.stable_lines.clear();
        self.mutable_lines.clear();
        self.stable_hyperlinks.clear();
        self.mutable_hyperlinks.clear();

        if self.source.is_empty() {
            return;
        }
        if contains_reference_definition(&self.source) {
            self.render_mutable(styles);
            return;
        }

        if let Some(boundary) =
            last_top_level_block_start(&self.source).filter(|offset| *offset > 0)
        {
            let stable = render_markdown_mode(
                &self.source[..boundary],
                self.width,
                styles,
                self.mode,
                None,
            );
            self.stable_lines = stable.lines;
            self.stable_hyperlinks = stable.hyperlinks;
            self.stable_source_len = boundary;
        }
        self.render_mutable(styles);
    }

    fn render_mutable(&mut self, styles: &MarkdownStyles) {
        let rendered = render_markdown_mode(
            &self.source[self.stable_source_len..],
            self.width,
            styles,
            self.mode,
            None,
        );
        self.mutable_lines = rendered.lines;
        self.mutable_hyperlinks = rendered.hyperlinks;
    }

    fn promote(&mut self, rendered: RenderedMarkdown) {
        let separator_rows =
            usize::from(!self.stable_lines.is_empty() && !rendered.lines.is_empty());
        if separator_rows == 1 {
            self.stable_lines.push(Line::default());
        }
        let row_offset = self.stable_lines.len();
        self.stable_lines.extend(rendered.lines);
        self.stable_hyperlinks
            .extend(rendered.hyperlinks.into_iter().map(|mut hyperlink| {
                hyperlink.row += row_offset;
                hyperlink
            }));
    }
}

fn parser_options() -> Options {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options
}

fn last_top_level_block_start(source: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut last_start = None;
    for (event, range) in Parser::new_ext(source, parser_options()).into_offset_iter() {
        match event {
            Event::Start(tag) if is_block_tag(&tag) => {
                if depth == 0 {
                    last_start = Some(range.start);
                }
                depth += 1;
            }
            Event::End(tag) if is_block_end(tag) => {
                depth = depth.saturating_sub(1);
            }
            Event::Rule if depth == 0 => {
                last_start = Some(range.start);
            }
            _ => {}
        }
    }
    last_start
}

fn is_block_tag(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::Paragraph
            | Tag::Heading { .. }
            | Tag::BlockQuote(_)
            | Tag::CodeBlock(_)
            | Tag::List(_)
            | Tag::Item
            | Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Table(_)
            | Tag::TableHead
            | Tag::TableRow
            | Tag::TableCell
            | Tag::HtmlBlock
    )
}

fn is_block_end(tag: TagEnd) -> bool {
    matches!(
        tag,
        TagEnd::Paragraph
            | TagEnd::Heading(_)
            | TagEnd::BlockQuote(_)
            | TagEnd::CodeBlock
            | TagEnd::List(_)
            | TagEnd::Item
            | TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::Table
            | TagEnd::TableHead
            | TagEnd::TableRow
            | TagEnd::TableCell
            | TagEnd::HtmlBlock
    )
}

fn contains_reference_definition(source: &str) -> bool {
    source.lines().any(|line| {
        let line = line.trim_start_matches(' ');
        if !line.starts_with('[') {
            return false;
        }
        let Some(close) = line.find("]:") else {
            return false;
        };
        close > 1 && !line[1..close].starts_with('^')
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    fn strings(markdown: &StreamingMarkdown) -> Vec<String> {
        markdown
            .lines()
            .iter()
            .map(|line| line.to_string())
            .collect()
    }

    #[test]
    fn unclosed_code_fence_stays_mutable_and_does_not_render_backticks() {
        let styles = MarkdownStyles::default();
        let mut markdown = StreamingMarkdown::new(40);
        markdown.append("```rust\nfn main() {", &styles);
        let before = strings(&markdown);
        assert_eq!(markdown.stable_source_len, 0);
        assert!(before.iter().any(|line| line == "┌─ rust"));

        markdown.append("\n}\n```", &styles);
        let after = strings(&markdown);
        assert_eq!(markdown.stable_source_len, 0);
        assert_eq!(before[0], after[0]);
        assert!(!after.iter().any(|line| line.contains("```")));
    }

    #[test]
    fn table_can_form_incrementally() {
        let styles = MarkdownStyles::default();
        let mut markdown = StreamingMarkdown::new(40);
        markdown.append("| A | B |\n", &styles);
        assert!(!strings(&markdown).iter().any(|line| line.contains('┼')));
        markdown.append("|---|---|\n", &styles);
        markdown.append("| 1 | 2 |", &styles);
        let lines = strings(&markdown);
        assert!(lines.iter().any(|line| line.contains('┼')));
        assert!(lines
            .iter()
            .any(|line| line.contains('1') && line.contains('2')));
        assert_eq!(markdown.stable_source_len, 0);
    }

    #[test]
    fn streaming_setext_heading_remains_mutable() {
        let styles = MarkdownStyles::default();
        let mut markdown = StreamingMarkdown::new(40);
        markdown.append("A heading", &styles);
        markdown.append("\n---------", &styles);
        assert_eq!(markdown.stable_source_len, 0);
        assert_eq!(strings(&markdown)[0], "A heading");
    }

    #[test]
    fn atx_h3_is_recognized_at_every_chunk_boundary() {
        use ratatui::style::Modifier;

        let styles = MarkdownStyles::default();
        let source = "### Заголовок";
        let split_points = source
            .char_indices()
            .map(|(index, _)| index)
            .chain(std::iter::once(source.len()))
            .filter(|index| *index > 0);

        for split in split_points {
            let mut markdown = StreamingMarkdown::new(40);
            markdown.append(&source[..split], &styles);
            markdown.append(&source[split..], &styles);

            let lines = markdown.lines();
            assert_eq!(lines.len(), 1, "split at byte {split}");
            assert_eq!(lines[0].to_string(), "Заголовок", "split at byte {split}");
            assert!(
                lines[0]
                    .spans
                    .iter()
                    .any(|span| span.style.add_modifier.contains(Modifier::BOLD)),
                "H3 style was lost at byte {split}: {:?}",
                lines[0]
            );
        }
    }

    #[test]
    fn atx_h3_stays_formatted_after_stable_prefix() {
        use ratatui::{style::Color, style::Modifier};

        let styles = MarkdownStyles::default();
        let mut markdown = StreamingMarkdown::new(40);
        markdown.append("Введение\n\n##", &styles);
        markdown.append("# Заголовок", &styles);

        assert!(markdown.stable_source_len > 0);
        let heading = markdown
            .lines()
            .into_iter()
            .find(|line| line.to_string() == "Заголовок")
            .expect("rendered H3");
        assert!(heading
            .spans
            .iter()
            .any(|span| span.style.add_modifier.contains(Modifier::BOLD)
                && span.style.fg == Some(Color::Cyan)));
    }

    #[test]
    fn unicode_emoji_survive_resize() {
        let styles = MarkdownStyles::default();
        let mut markdown = StreamingMarkdown::new(8);
        markdown.append("Привет 🙂 世界", &styles);
        assert!(markdown.lines().iter().all(|line| line.width() <= 8));
        markdown.resize(5, &styles);
        assert!(markdown.lines().iter().all(|line| line.width() <= 5));
        assert!(strings(&markdown).join("").contains('🙂'));
    }

    #[test]
    fn resize_performs_full_reflow() {
        let styles = MarkdownStyles::default();
        let mut markdown = StreamingMarkdown::new(20);
        markdown.append("one two three four five", &styles);
        let wide = markdown.lines().len();
        markdown.resize(8, &styles);
        assert!(markdown.lines().len() > wide);
    }

    #[test]
    fn completed_top_level_blocks_become_stable() {
        let styles = MarkdownStyles::default();
        let mut markdown = StreamingMarkdown::new(40);
        markdown.append("first paragraph", &styles);
        assert_eq!(markdown.stable_source_len, 0);
        markdown.append("\n\nsecond paragraph", &styles);
        let stable_len = markdown.stable_source_len;
        let stable_lines = markdown.stable_lines.clone();
        assert!(stable_len > 0);

        markdown.append(" grows", &styles);
        assert_eq!(markdown.stable_source_len, stable_len);
        assert_eq!(markdown.stable_lines, stable_lines);
    }

    #[test]
    fn reference_definition_forces_full_mutable_render() {
        let styles = MarkdownStyles::default();
        let mut markdown = StreamingMarkdown::new(80);
        markdown.append("[site][home]\n\nanother block", &styles);
        assert!(markdown.stable_source_len > 0);
        markdown.append("\n\n[home]: https://example.com", &styles);
        assert_eq!(markdown.stable_source_len, 0);
        assert_eq!(markdown.rendered().hyperlinks.len(), 1);
        assert_eq!(
            markdown.rendered().hyperlinks[0].destination,
            "https://example.com"
        );
    }

    #[test]
    fn empty_chunks_and_identical_resize_are_noops() {
        let styles = MarkdownStyles::default();
        let mut markdown = StreamingMarkdown::new(20);
        markdown.append("same text", &styles);
        let before = markdown.lines();
        markdown.append("", &styles);
        markdown.resize(20, &styles);
        assert_eq!(markdown.lines(), before);
    }

    #[test]
    fn raw_mode_displays_source() {
        let styles = MarkdownStyles::default();
        let mut markdown = StreamingMarkdown::new(80);
        markdown.set_mode(MarkdownRenderMode::Raw, &styles);
        markdown.append("**bold**", &styles);
        assert_eq!(strings(&markdown), vec!["**bold**"]);
    }

    #[test]
    fn nested_list_is_width_bounded() {
        let styles = MarkdownStyles::default();
        let mut markdown = StreamingMarkdown::new(18);
        markdown.append("- outer\n  - nested content that wraps", &styles);
        assert!(markdown
            .lines()
            .iter()
            .all(|line| UnicodeWidthStr::width(line.to_string().as_str()) <= 18));
    }
}
