mod renderer;
mod streaming;
mod styles;

pub use renderer::{
    render_markdown, render_markdown_with_highlighter, render_markdown_with_mode, Hyperlink,
    RenderedMarkdown, SyntaxHighlighter,
};
pub use streaming::StreamingMarkdown;
pub use styles::MarkdownStyles;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum MarkdownRenderMode {
    #[default]
    Rich,
    Raw,
}
