use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
};

/// Theme-owned styles used by the native Markdown renderer.
///
/// `version` is deliberately explicit: callers can cheaply invalidate render
/// caches when a theme changes without comparing every `Style`.
#[derive(Clone, Debug)]
pub struct MarkdownStyles {
    pub h1: Style,
    pub h2: Style,
    pub h3: Style,
    pub h4: Style,
    pub h5: Style,
    pub h6: Style,
    pub strong: Style,
    pub emphasis: Style,
    pub strikethrough: Style,
    pub inline_code: Style,
    pub code_block: Style,
    pub code_label: Style,
    pub ordered_list_marker: Style,
    pub unordered_list_marker: Style,
    pub blockquote: Style,
    pub link: Style,
    pub rule: Style,
    pub table_header: Style,
    pub table_border: Style,
    version: u64,
}

impl MarkdownStyles {
    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn with_version(mut self, version: u64) -> Self {
        self.version = version;
        self
    }

    pub(crate) fn heading(&self, level: pulldown_cmark::HeadingLevel) -> Style {
        use pulldown_cmark::HeadingLevel;

        match level {
            HeadingLevel::H1 => self.h1,
            HeadingLevel::H2 => self.h2,
            HeadingLevel::H3 => self.h3,
            HeadingLevel::H4 => self.h4,
            HeadingLevel::H5 => self.h5,
            HeadingLevel::H6 => self.h6,
        }
    }

    pub(crate) fn list_marker(&self, ordered: bool, marker: String) -> Span<'static> {
        Span::styled(
            marker,
            if ordered {
                self.ordered_list_marker
            } else {
                self.unordered_list_marker
            },
        )
    }
}

impl Default for MarkdownStyles {
    fn default() -> Self {
        Self {
            h1: Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            h2: Style::default().add_modifier(Modifier::BOLD),
            h3: Style::default().add_modifier(Modifier::BOLD | Modifier::ITALIC),
            h4: Style::default().add_modifier(Modifier::ITALIC),
            h5: Style::default().add_modifier(Modifier::ITALIC),
            h6: Style::default().add_modifier(Modifier::ITALIC | Modifier::DIM),
            strong: Style::default().add_modifier(Modifier::BOLD),
            emphasis: Style::default().add_modifier(Modifier::ITALIC),
            strikethrough: Style::default().add_modifier(Modifier::CROSSED_OUT),
            inline_code: Style::default().fg(Color::Cyan),
            code_block: Style::default().fg(Color::LightCyan),
            code_label: Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::DIM),
            ordered_list_marker: Style::default().fg(Color::LightBlue),
            unordered_list_marker: Style::default().fg(Color::LightBlue),
            blockquote: Style::default().fg(Color::Green),
            link: Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::UNDERLINED),
            rule: Style::default().add_modifier(Modifier::DIM),
            table_header: Style::default().add_modifier(Modifier::BOLD),
            table_border: Style::default().add_modifier(Modifier::DIM),
            version: 0,
        }
    }
}
