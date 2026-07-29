use std::ops::Range;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::{MarkdownRenderMode, MarkdownStyles};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hyperlink {
    pub destination: String,
    pub row: usize,
    pub columns: Range<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct RenderedMarkdown {
    pub lines: Vec<Line<'static>>,
    pub hyperlinks: Vec<Hyperlink>,
}

/// Optional syntax coloring hook. The Markdown renderer owns layout and code
/// fences; an implementation only colors one verbatim source line at a time.
pub trait SyntaxHighlighter {
    fn highlight_line(
        &self,
        language: Option<&str>,
        source: &str,
        base_style: Style,
    ) -> Vec<Span<'static>>;
}

pub fn render_markdown(source: &str, width: u16, styles: &MarkdownStyles) -> RenderedMarkdown {
    render_markdown_mode(source, width, styles, MarkdownRenderMode::Rich, None)
}

pub fn render_markdown_with_highlighter(
    source: &str,
    width: u16,
    styles: &MarkdownStyles,
    highlighter: &dyn SyntaxHighlighter,
) -> RenderedMarkdown {
    render_markdown_mode(
        source,
        width,
        styles,
        MarkdownRenderMode::Rich,
        Some(highlighter),
    )
}

pub fn render_markdown_with_mode(
    source: &str,
    width: u16,
    styles: &MarkdownStyles,
    mode: MarkdownRenderMode,
) -> RenderedMarkdown {
    render_markdown_mode(source, width, styles, mode, None)
}

pub(crate) fn render_markdown_mode(
    source: &str,
    width: u16,
    styles: &MarkdownStyles,
    mode: MarkdownRenderMode,
    highlighter: Option<&dyn SyntaxHighlighter>,
) -> RenderedMarkdown {
    if mode == MarkdownRenderMode::Raw {
        return render_raw(source, width);
    }

    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    let parser = Parser::new_ext(source, options);
    Writer::new(width.max(1), styles, highlighter)
        .render(parser)
        .finish()
}

#[derive(Clone, Debug)]
struct Segment {
    text: String,
    style: Style,
    link: Option<String>,
}

impl Segment {
    fn new(text: impl Into<String>, style: Style, link: Option<String>) -> Self {
        Self {
            text: text.into(),
            style,
            link,
        }
    }

    fn from_span(span: Span<'static>) -> Self {
        Self::new(span.content.into_owned(), span.style, None)
    }
}

#[derive(Clone, Copy, Debug)]
enum WrapKind {
    Words,
    Hard,
}

#[derive(Clone, Debug)]
struct LogicalLine {
    initial_prefix: Vec<Span<'static>>,
    subsequent_prefix: Vec<Span<'static>>,
    segments: Vec<Segment>,
    wrap: WrapKind,
}

#[derive(Clone, Debug)]
struct ItemContext {
    marker: Span<'static>,
    continuation_width: usize,
    first_line_pending: bool,
}

#[derive(Clone, Debug)]
struct ListContext {
    next: Option<u64>,
}

#[derive(Clone, Debug)]
struct LinkContext {
    destination: String,
}

#[derive(Clone, Debug, Default)]
struct TableCell {
    segments: Vec<Segment>,
}

#[derive(Clone, Debug)]
struct TableState {
    alignments: Vec<Alignment>,
    header: Vec<TableCell>,
    rows: Vec<Vec<TableCell>>,
    current_row: Vec<TableCell>,
    current_cell: Option<TableCell>,
    in_header: bool,
}

impl TableState {
    fn new(alignments: Vec<Alignment>) -> Self {
        Self {
            alignments,
            header: Vec::new(),
            rows: Vec::new(),
            current_row: Vec::new(),
            current_cell: None,
            in_header: false,
        }
    }
}

struct Writer<'styles, 'highlight> {
    width: u16,
    styles: &'styles MarkdownStyles,
    highlighter: Option<&'highlight dyn SyntaxHighlighter>,
    logical_lines: Vec<LogicalLine>,
    current: Option<LogicalLine>,
    inline_styles: Vec<Style>,
    lists: Vec<ListContext>,
    items: Vec<ItemContext>,
    blockquotes: usize,
    block_depth: usize,
    link: Option<LinkContext>,
    in_code_block: bool,
    code_language: Option<String>,
    code_buffer: String,
    table: Option<TableState>,
}

impl<'styles, 'highlight> Writer<'styles, 'highlight> {
    fn new(
        width: u16,
        styles: &'styles MarkdownStyles,
        highlighter: Option<&'highlight dyn SyntaxHighlighter>,
    ) -> Self {
        Self {
            width,
            styles,
            highlighter,
            logical_lines: Vec::new(),
            current: None,
            inline_styles: Vec::new(),
            lists: Vec::new(),
            items: Vec::new(),
            blockquotes: 0,
            block_depth: 0,
            link: None,
            in_code_block: false,
            code_language: None,
            code_buffer: String::new(),
            table: None,
        }
    }

    fn render<'input, I>(mut self, events: I) -> Self
    where
        I: IntoIterator<Item = Event<'input>>,
    {
        for event in events {
            self.event(event);
        }
        self.flush_current();
        self
    }

    fn finish(self) -> RenderedMarkdown {
        wrap_logical_lines(self.logical_lines, self.width)
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text),
            Event::Code(code) => self.inline_code(&code),
            Event::SoftBreak => self.soft_break(),
            Event::HardBreak => self.hard_break(),
            Event::Rule => self.rule(),
            Event::Html(html) | Event::InlineHtml(html) => self.text(&html),
            Event::InlineMath(math) | Event::DisplayMath(math) => self.text(&math),
            Event::FootnoteReference(label) => self.text(&format!("[^{label}]")),
            Event::TaskListMarker(checked) => {
                self.text(if checked { "[x] " } else { "[ ] " });
            }
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        let is_block = is_block_tag(&tag);
        if is_block && self.block_depth == 0 {
            self.begin_top_level_block();
        }
        if is_block {
            self.block_depth += 1;
        }

        match tag {
            Tag::Paragraph => {}
            Tag::Heading { level, .. } => {
                let style = self.styles.heading(level);
                self.inline_styles.push(style);
                self.push_segment(Segment::new(
                    format!("{} ", "#".repeat(heading_number(level))),
                    style,
                    None,
                ));
            }
            Tag::BlockQuote(_) => {
                self.blockquotes += 1;
            }
            Tag::CodeBlock(kind) => {
                self.flush_current();
                self.in_code_block = true;
                self.code_language = match kind {
                    CodeBlockKind::Fenced(language) if !language.trim().is_empty() => {
                        Some(language.trim().to_owned())
                    }
                    _ => None,
                };
                self.code_buffer.clear();
            }
            Tag::List(start) => {
                // Tight list items may contain text events directly, without a
                // surrounding Paragraph tag. A nested list therefore has to
                // terminate the parent's current logical line explicitly.
                self.flush_current();
                self.lists.push(ListContext { next: start });
            }
            Tag::Item => {
                let list = self.lists.last_mut();
                let (ordered, marker) = match list {
                    Some(ListContext {
                        next: Some(next), ..
                    }) => {
                        let marker = format!("{next}. ");
                        *next = next.saturating_add(1);
                        (true, marker)
                    }
                    _ => (false, "• ".to_owned()),
                };
                let continuation_width = UnicodeWidthStr::width(marker.as_str());
                self.items.push(ItemContext {
                    marker: self.styles.list_marker(ordered, marker),
                    continuation_width,
                    first_line_pending: true,
                });
            }
            Tag::Emphasis => self.inline_styles.push(self.styles.emphasis),
            Tag::Strong => self.inline_styles.push(self.styles.strong),
            Tag::Strikethrough => self.inline_styles.push(self.styles.strikethrough),
            Tag::Link { dest_url, .. } => {
                self.inline_styles.push(self.styles.link);
                self.link = Some(LinkContext {
                    destination: dest_url.into_string(),
                });
            }
            Tag::Table(alignments) => {
                self.flush_current();
                self.table = Some(TableState::new(alignments));
            }
            Tag::TableHead => {
                if let Some(table) = &mut self.table {
                    table.in_header = true;
                }
            }
            Tag::TableRow => {
                if let Some(table) = &mut self.table {
                    table.current_row.clear();
                }
            }
            Tag::TableCell => {
                if let Some(table) = &mut self.table {
                    table.current_cell = Some(TableCell::default());
                }
            }
            Tag::HtmlBlock
            | Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Image { .. }
            | Tag::MetadataBlock(_) => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.flush_current(),
            TagEnd::Heading(_) => {
                self.flush_current();
                self.inline_styles.pop();
            }
            TagEnd::BlockQuote(_) => {
                self.flush_current();
                self.blockquotes = self.blockquotes.saturating_sub(1);
            }
            TagEnd::CodeBlock => self.end_code_block(),
            TagEnd::List(_) => {
                self.flush_current();
                self.lists.pop();
            }
            TagEnd::Item => {
                self.flush_current();
                self.items.pop();
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.inline_styles.pop();
            }
            TagEnd::Link => {
                self.inline_styles.pop();
                self.link = None;
            }
            TagEnd::Table => self.end_table(),
            TagEnd::TableHead => {
                if let Some(table) = &mut self.table {
                    if !table.current_row.is_empty() {
                        table.header = std::mem::take(&mut table.current_row);
                    }
                    table.in_header = false;
                }
            }
            TagEnd::TableRow => {
                if let Some(table) = &mut self.table {
                    let row = std::mem::take(&mut table.current_row);
                    if table.in_header {
                        table.header = row;
                    } else {
                        table.rows.push(row);
                    }
                }
            }
            TagEnd::TableCell => {
                if let Some(table) = &mut self.table {
                    if let Some(cell) = table.current_cell.take() {
                        table.current_row.push(cell);
                    }
                }
            }
            TagEnd::HtmlBlock
            | TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::Image
            | TagEnd::MetadataBlock(_) => {}
        }

        if is_block_end(tag) {
            self.block_depth = self.block_depth.saturating_sub(1);
        }
    }

    fn begin_top_level_block(&mut self) {
        self.flush_current();
        if !self.logical_lines.is_empty()
            && !self
                .logical_lines
                .last()
                .is_some_and(|line| line.segments.is_empty() && line.initial_prefix.is_empty())
        {
            self.logical_lines.push(LogicalLine {
                initial_prefix: Vec::new(),
                subsequent_prefix: Vec::new(),
                segments: Vec::new(),
                wrap: WrapKind::Words,
            });
        }
    }

    fn text(&mut self, text: &str) {
        if self.in_code_block {
            self.code_buffer.push_str(text);
            return;
        }

        let style = self.current_style();
        let link = self.link.as_ref().map(|link| link.destination.clone());
        if let Some(table) = &mut self.table {
            if let Some(cell) = &mut table.current_cell {
                cell.segments
                    .push(Segment::new(text.to_owned(), style, link));
            }
            return;
        }

        let mut parts = text.split('\n').peekable();
        while let Some(part) = parts.next() {
            if !part.is_empty() {
                self.push_segment(Segment::new(part.to_owned(), style, link.clone()));
            }
            if parts.peek().is_some() {
                self.flush_current();
            }
        }
    }

    fn inline_code(&mut self, code: &str) {
        let style = self.current_style().patch(self.styles.inline_code);
        let link = self.link.as_ref().map(|link| link.destination.clone());
        if let Some(table) = &mut self.table {
            if let Some(cell) = &mut table.current_cell {
                cell.segments
                    .push(Segment::new(code.to_owned(), style, link));
            }
            return;
        }
        self.push_segment(Segment::new(code.to_owned(), style, link));
    }

    fn soft_break(&mut self) {
        if self.in_code_block {
            self.code_buffer.push('\n');
            return;
        }
        self.push_segment(Segment::new(" ", self.current_style(), None));
    }

    fn hard_break(&mut self) {
        if self.in_code_block {
            self.code_buffer.push('\n');
        } else if let Some(table) = &mut self.table {
            if let Some(cell) = &mut table.current_cell {
                cell.segments
                    .push(Segment::new(" ", Style::default(), None));
            }
        } else {
            self.flush_current();
        }
    }

    fn rule(&mut self) {
        if self.block_depth == 0 {
            self.begin_top_level_block();
        }
        let (initial_prefix, subsequent_prefix) = self.take_prefixes();
        let prefix_width = spans_width(&initial_prefix);
        let rule_width = usize::from(self.width).saturating_sub(prefix_width).max(3);
        self.logical_lines.push(LogicalLine {
            initial_prefix,
            subsequent_prefix,
            segments: vec![Segment::new("─".repeat(rule_width), self.styles.rule, None)],
            wrap: WrapKind::Hard,
        });
    }

    fn current_style(&self) -> Style {
        self.inline_styles
            .iter()
            .copied()
            .fold(Style::default(), Style::patch)
    }

    fn push_segment(&mut self, segment: Segment) {
        self.ensure_current(WrapKind::Words).segments.push(segment);
    }

    fn ensure_current(&mut self, wrap: WrapKind) -> &mut LogicalLine {
        if self.current.is_none() {
            let (initial_prefix, subsequent_prefix) = self.take_prefixes();
            self.current = Some(LogicalLine {
                initial_prefix,
                subsequent_prefix,
                segments: Vec::new(),
                wrap,
            });
        }
        self.current.as_mut().expect("current line initialized")
    }

    fn flush_current(&mut self) {
        if let Some(line) = self.current.take() {
            self.logical_lines.push(line);
        }
    }

    fn take_prefixes(&mut self) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
        let mut initial = Vec::new();
        let mut subsequent = Vec::new();

        for _ in 0..self.blockquotes {
            initial.push(Span::styled("> ", self.styles.blockquote));
            subsequent.push(Span::styled("> ", self.styles.blockquote));
        }
        for item in &mut self.items {
            if item.first_line_pending {
                initial.push(item.marker.clone());
                item.first_line_pending = false;
            } else {
                initial.push(Span::raw(" ".repeat(item.continuation_width)));
            }
            subsequent.push(Span::raw(" ".repeat(item.continuation_width)));
        }

        (initial, subsequent)
    }

    fn end_code_block(&mut self) {
        self.flush_current();
        self.in_code_block = false;

        let label = self.code_language.as_deref().unwrap_or("code").to_owned();
        let (initial_prefix, subsequent_prefix) = self.take_prefixes();
        self.logical_lines.push(LogicalLine {
            initial_prefix,
            subsequent_prefix,
            segments: vec![Segment::new(
                format!("┌─ {label}"),
                self.styles.code_label,
                None,
            )],
            wrap: WrapKind::Hard,
        });

        let normalized = self.code_buffer.replace("\r\n", "\n");
        let code_lines: Vec<&str> = if normalized.is_empty() {
            vec![""]
        } else {
            normalized
                .strip_suffix('\n')
                .unwrap_or(&normalized)
                .split('\n')
                .collect()
        };
        for source_line in code_lines {
            let (mut initial_prefix, mut subsequent_prefix) = self.take_prefixes();
            initial_prefix.push(Span::styled("│ ", self.styles.code_label));
            subsequent_prefix.push(Span::styled("│ ", self.styles.code_label));
            let segments = if let Some(highlighter) = self.highlighter {
                highlighter
                    .highlight_line(
                        self.code_language.as_deref(),
                        source_line,
                        self.styles.code_block,
                    )
                    .into_iter()
                    .map(Segment::from_span)
                    .collect()
            } else {
                vec![Segment::new(
                    source_line.to_owned(),
                    self.styles.code_block,
                    None,
                )]
            };
            self.logical_lines.push(LogicalLine {
                initial_prefix,
                subsequent_prefix,
                segments,
                wrap: WrapKind::Hard,
            });
        }
        self.code_buffer.clear();
        self.code_language = None;
    }

    fn end_table(&mut self) {
        self.flush_current();
        let Some(table) = self.table.take() else {
            return;
        };
        let prefix_width = {
            let mut width = self.blockquotes * 2;
            width += self
                .items
                .iter()
                .map(|item| item.continuation_width)
                .sum::<usize>();
            width
        };
        let table_width = usize::from(self.width).saturating_sub(prefix_width).max(1);
        for segments in render_table(table, table_width, self.styles) {
            let (initial_prefix, subsequent_prefix) = self.take_prefixes();
            self.logical_lines.push(LogicalLine {
                initial_prefix,
                subsequent_prefix,
                segments,
                wrap: WrapKind::Hard,
            });
        }
    }
}

fn heading_number(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
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

fn render_raw(source: &str, width: u16) -> RenderedMarkdown {
    if source.is_empty() {
        return RenderedMarkdown::default();
    }
    let logical_lines = source
        .split('\n')
        .map(|line| LogicalLine {
            initial_prefix: Vec::new(),
            subsequent_prefix: Vec::new(),
            segments: vec![Segment::new(line.to_owned(), Style::default(), None)],
            wrap: WrapKind::Hard,
        })
        .collect();
    wrap_logical_lines(logical_lines, width.max(1))
}

#[derive(Clone, Debug)]
struct Unit {
    text: String,
    style: Style,
    link: Option<String>,
}

impl Unit {
    fn width(&self) -> usize {
        UnicodeWidthStr::width(self.text.as_str())
    }
}

fn segments_to_units(segments: &[Segment]) -> Vec<Unit> {
    segments
        .iter()
        .flat_map(|segment| {
            UnicodeSegmentation::graphemes(segment.text.as_str(), true).map(|grapheme| Unit {
                text: grapheme.to_owned(),
                style: segment.style,
                link: segment.link.clone(),
            })
        })
        .collect()
}

fn spans_to_units(spans: &[Span<'static>]) -> Vec<Unit> {
    spans
        .iter()
        .flat_map(|span| {
            UnicodeSegmentation::graphemes(span.content.as_ref(), true).map(|grapheme| Unit {
                text: grapheme.to_owned(),
                style: span.style,
                link: None,
            })
        })
        .collect()
}

fn wrap_logical_lines(lines: Vec<LogicalLine>, width: u16) -> RenderedMarkdown {
    let mut rendered = RenderedMarkdown::default();
    let width = usize::from(width.max(1));

    for logical in lines {
        match logical.wrap {
            WrapKind::Words => wrap_words(logical, width, &mut rendered),
            WrapKind::Hard => wrap_hard(logical, width, &mut rendered),
        }
    }
    rendered
}

fn wrap_words(logical: LogicalLine, width: usize, rendered: &mut RenderedMarkdown) {
    let mut words: Vec<Vec<Unit>> = Vec::new();
    let mut word = Vec::new();
    for unit in segments_to_units(&logical.segments) {
        if unit.text.chars().all(char::is_whitespace) {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else {
            word.push(unit);
        }
    }
    if !word.is_empty() {
        words.push(word);
    }

    let mut first = true;
    let mut content: Vec<Unit> = Vec::new();
    let mut prefix = spans_to_units(&logical.initial_prefix);
    let mut available = width.saturating_sub(units_width(&prefix)).max(1);

    if words.is_empty() {
        push_units_line(prefix, rendered);
        return;
    }

    for mut next_word in words {
        let word_width = units_width(&next_word);
        let separator_width = usize::from(!content.is_empty());
        if !content.is_empty() && units_width(&content) + separator_width + word_width <= available
        {
            let left = content.last();
            let right = next_word.first();
            let same_style = left
                .zip(right)
                .is_some_and(|(left, right)| left.style == right.style && left.link == right.link);
            content.push(Unit {
                text: " ".to_owned(),
                style: if same_style {
                    right.map(|unit| unit.style).unwrap_or_default()
                } else {
                    Style::default()
                },
                link: if same_style {
                    right.and_then(|unit| unit.link.clone())
                } else {
                    None
                },
            });
            content.append(&mut next_word);
            continue;
        }
        if content.is_empty() && word_width <= available {
            content.append(&mut next_word);
            continue;
        }

        if !content.is_empty() {
            let mut line = prefix;
            line.append(&mut content);
            push_units_line(line, rendered);
            first = false;
            prefix = spans_to_units(&logical.subsequent_prefix);
            available = width.saturating_sub(units_width(&prefix)).max(1);
        }

        while units_width(&next_word) > available {
            let split = fitting_unit_count(&next_word, available);
            let mut chunk: Vec<Unit> = next_word.drain(..split.max(1)).collect();
            let mut line = prefix.clone();
            line.append(&mut chunk);
            push_units_line(line, rendered);
            first = false;
            prefix = spans_to_units(&logical.subsequent_prefix);
            available = width.saturating_sub(units_width(&prefix)).max(1);
        }
        content = next_word;
    }

    if !content.is_empty() || first {
        let mut line = prefix;
        line.append(&mut content);
        push_units_line(line, rendered);
    }
}

fn wrap_hard(logical: LogicalLine, width: usize, rendered: &mut RenderedMarkdown) {
    let mut remaining = segments_to_units(&logical.segments);
    let mut prefix = spans_to_units(&logical.initial_prefix);
    if remaining.is_empty() {
        push_units_line(prefix, rendered);
        return;
    }

    while !remaining.is_empty() {
        let available = width.saturating_sub(units_width(&prefix)).max(1);
        let split = fitting_unit_count(&remaining, available).max(1);
        let mut chunk: Vec<Unit> = remaining.drain(..split.min(remaining.len())).collect();
        let mut line = prefix;
        line.append(&mut chunk);
        push_units_line(line, rendered);
        prefix = spans_to_units(&logical.subsequent_prefix);
    }
}

fn fitting_unit_count(units: &[Unit], maximum_width: usize) -> usize {
    let mut width = 0;
    let mut count = 0;
    for unit in units {
        let next = unit.width();
        if count > 0 && width + next > maximum_width {
            break;
        }
        if count == 0 && next > maximum_width {
            return 1;
        }
        width += next;
        count += 1;
    }
    count
}

fn push_units_line(units: Vec<Unit>, rendered: &mut RenderedMarkdown) {
    let row = rendered.lines.len();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut column = 0;
    let mut active_link: Option<(String, usize)> = None;

    for unit in units {
        if active_link
            .as_ref()
            .map(|(destination, _)| destination.as_str())
            != unit.link.as_deref()
        {
            if let Some((destination, start)) = active_link.take() {
                rendered.hyperlinks.push(Hyperlink {
                    destination,
                    row,
                    columns: start..column,
                });
            }
            if let Some(destination) = &unit.link {
                active_link = Some((destination.clone(), column));
            }
        }

        let unit_width = unit.width();
        if let Some(last) = spans.last_mut() {
            if last.style == unit.style {
                last.content.to_mut().push_str(&unit.text);
            } else {
                spans.push(Span::styled(unit.text, unit.style));
            }
        } else {
            spans.push(Span::styled(unit.text, unit.style));
        }
        column += unit_width;
    }
    if let Some((destination, start)) = active_link {
        rendered.hyperlinks.push(Hyperlink {
            destination,
            row,
            columns: start..column,
        });
    }
    rendered.lines.push(Line::from(spans));
}

fn render_table(mut table: TableState, width: usize, styles: &MarkdownStyles) -> Vec<Vec<Segment>> {
    let columns = table
        .alignments
        .len()
        .max(table.header.len())
        .max(table.rows.iter().map(Vec::len).max().unwrap_or(0));
    if columns == 0 {
        return Vec::new();
    }
    table.alignments.resize(columns, Alignment::None);
    normalize_cells(&mut table.header, columns);
    for row in &mut table.rows {
        normalize_cells(row, columns);
    }

    let border_width = columns.saturating_add(1) + columns * 2;
    let content_budget = width.saturating_sub(border_width).max(columns);
    let mut widths = vec![1usize; columns];
    for column in 0..columns {
        widths[column] = std::iter::once(&table.header)
            .chain(table.rows.iter())
            .map(|row| segments_width(&row[column].segments))
            .max()
            .unwrap_or(1)
            .max(1);
    }
    while widths.iter().sum::<usize>() > content_budget {
        let Some((index, widest)) = widths
            .iter()
            .copied()
            .enumerate()
            .max_by_key(|(_, value)| *value)
        else {
            break;
        };
        if widest <= 1 {
            break;
        }
        widths[index] -= 1;
    }

    let mut output = Vec::new();
    render_table_row(
        &table.header,
        &widths,
        &table.alignments,
        styles,
        true,
        &mut output,
    );
    let mut separator = vec![Segment::new("├", styles.table_border, None)];
    for (index, column_width) in widths.iter().enumerate() {
        separator.push(Segment::new(
            "─".repeat(column_width + 2),
            styles.table_border,
            None,
        ));
        separator.push(Segment::new(
            if index + 1 == columns { "┤" } else { "┼" },
            styles.table_border,
            None,
        ));
    }
    output.push(separator);
    for row in &table.rows {
        render_table_row(row, &widths, &table.alignments, styles, false, &mut output);
    }
    output
}

fn normalize_cells(cells: &mut Vec<TableCell>, columns: usize) {
    cells.resize_with(columns, TableCell::default);
    cells.truncate(columns);
}

fn render_table_row(
    cells: &[TableCell],
    widths: &[usize],
    alignments: &[Alignment],
    styles: &MarkdownStyles,
    header: bool,
    output: &mut Vec<Vec<Segment>>,
) {
    let wrapped: Vec<Vec<Vec<Unit>>> = cells
        .iter()
        .zip(widths)
        .map(|(cell, width)| wrap_cell(&cell.segments, *width))
        .collect();
    let height = wrapped.iter().map(Vec::len).max().unwrap_or(1);
    for row_index in 0..height {
        let mut row = vec![Segment::new("│", styles.table_border, None)];
        for (column, column_width) in widths.iter().enumerate() {
            let units = wrapped[column].get(row_index).cloned().unwrap_or_default();
            let content_width = units_width(&units);
            let missing = column_width.saturating_sub(content_width);
            let (left, right) = match alignments[column] {
                Alignment::Right => (missing, 0),
                Alignment::Center => (missing / 2, missing - missing / 2),
                Alignment::Left | Alignment::None => (0, missing),
            };
            row.push(Segment::new(" ", Style::default(), None));
            row.push(Segment::new(" ".repeat(left), Style::default(), None));
            for unit in units {
                row.push(Segment::new(
                    unit.text,
                    if header {
                        unit.style.patch(styles.table_header)
                    } else {
                        unit.style
                    },
                    unit.link,
                ));
            }
            row.push(Segment::new(" ".repeat(right), Style::default(), None));
            row.push(Segment::new(" ", Style::default(), None));
            row.push(Segment::new("│", styles.table_border, None));
        }
        output.push(row);
    }
}

fn wrap_cell(segments: &[Segment], width: usize) -> Vec<Vec<Unit>> {
    let mut remaining = segments_to_units(segments);
    if remaining.is_empty() {
        return vec![Vec::new()];
    }
    let mut lines = Vec::new();
    while !remaining.is_empty() {
        let split = fitting_unit_count(&remaining, width.max(1)).max(1);
        lines.push(remaining.drain(..split.min(remaining.len())).collect());
    }
    lines
}

fn spans_width(spans: &[Span<'static>]) -> usize {
    spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum()
}

fn segments_width(segments: &[Segment]) -> usize {
    segments
        .iter()
        .map(|segment| UnicodeWidthStr::width(segment.text.as_str()))
        .sum()
}

fn units_width(units: &[Unit]) -> usize {
    units.iter().map(Unit::width).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    fn strings(rendered: &RenderedMarkdown) -> Vec<String> {
        rendered
            .lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn renders_inline_styles_and_link_metadata() {
        let rendered = render_markdown(
            "A **bold _nested_** [link](https://example.com).",
            80,
            &MarkdownStyles::default(),
        );
        assert_eq!(strings(&rendered), vec!["A bold nested link.".to_owned()]);
        assert!(rendered.lines[0].spans.iter().any(|span| {
            span.content.contains("nested")
                && span
                    .style
                    .add_modifier
                    .contains(Modifier::BOLD | Modifier::ITALIC)
        }));
        assert_eq!(
            rendered.hyperlinks,
            vec![Hyperlink {
                destination: "https://example.com".to_owned(),
                row: 0,
                columns: 14..18,
            }]
        );
    }

    #[test]
    fn wraps_unicode_by_terminal_cell_width() {
        let rendered = render_markdown("ab🙂界cd", 5, &MarkdownStyles::default());
        assert_eq!(strings(&rendered), vec!["ab🙂", "界cd"]);
        assert!(rendered
            .lines
            .iter()
            .all(|line| UnicodeWidthStr::width(line.to_string().as_str()) <= 5));
    }

    #[test]
    fn nested_list_uses_continuation_indents() {
        let rendered = render_markdown(
            "- outer item with words\n    1. nested item with more words\n    2. second\n- done",
            20,
            &MarkdownStyles::default(),
        );
        let lines = strings(&rendered);
        assert!(
            lines.iter().any(|line| line.starts_with("  1. nested")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.starts_with("     with")),
            "{lines:?}"
        );
        assert!(lines.iter().any(|line| line == "• done"), "{lines:?}");
    }

    #[test]
    fn code_fence_shows_language_and_preserves_spaces() {
        let rendered = render_markdown(
            "```rust\nfn main() {\n    println!(\"x\");\n}\n```",
            80,
            &MarkdownStyles::default(),
        );
        assert_eq!(
            strings(&rendered),
            vec!["┌─ rust", "│ fn main() {", "│     println!(\"x\");", "│ }"]
        );
    }

    #[test]
    fn renders_gfm_table_to_bounded_lines() {
        let rendered = render_markdown(
            "| Name | Value |\n|:--|--:|\n| alpha | **42** |",
            24,
            &MarkdownStyles::default(),
        );
        let lines = strings(&rendered);
        assert!(lines.iter().any(|line| line.contains("Name")), "{lines:?}");
        assert!(lines.iter().any(|line| line.contains("alpha")));
        assert!(lines
            .iter()
            .all(|line| UnicodeWidthStr::width(line.as_str()) <= 24));
    }

    #[test]
    fn raw_mode_keeps_markdown_syntax() {
        let rendered = render_markdown_mode(
            "**bold**",
            80,
            &MarkdownStyles::default(),
            MarkdownRenderMode::Raw,
            None,
        );
        assert_eq!(strings(&rendered), vec!["**bold**"]);
    }

    #[test]
    fn headings_blockquotes_rules_and_breaks_render_natively() {
        let rendered = render_markdown(
            "# H1\n\n###### H6\n\n> quoted **text**\n\n---\n\nsoft\nbreak  \nhard",
            40,
            &MarkdownStyles::default(),
        );
        let lines = strings(&rendered);
        assert!(lines.iter().any(|line| line == "# H1"));
        assert!(lines.iter().any(|line| line == "###### H6"));
        assert!(lines.iter().any(|line| line == "> quoted text"));
        assert!(lines.iter().any(|line| line.starts_with("────")));
        assert!(lines.iter().any(|line| line == "soft break"));
        assert!(lines.iter().any(|line| line == "hard"));
    }

    #[test]
    fn multiword_link_has_one_range_per_visual_line() {
        let rendered = render_markdown(
            "[click here](https://example.com)",
            40,
            &MarkdownStyles::default(),
        );
        assert_eq!(rendered.hyperlinks.len(), 1);
        assert_eq!(rendered.hyperlinks[0].columns, 0..10);
    }

    #[test]
    fn raw_mode_preserves_repeated_spaces() {
        let rendered = render_markdown_with_mode(
            "a  **b**",
            80,
            &MarkdownStyles::default(),
            MarkdownRenderMode::Raw,
        );
        assert_eq!(strings(&rendered), vec!["a  **b**"]);
    }
}
