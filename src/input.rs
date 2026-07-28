use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Debug, Default)]
pub struct InputState {
    text: String,
    cursor: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputViewport {
    pub text: String,
    pub cursor_column: usize,
}

impl InputState {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn insert(&mut self, character: char) {
        self.text.insert(self.cursor, character);
        self.cursor += character.len_utf8();
    }

    pub fn move_left(&mut self) {
        if let Some((index, _)) = self.text[..self.cursor].grapheme_indices(true).next_back() {
            self.cursor = index;
        }
    }

    pub fn move_right(&mut self) {
        if let Some(grapheme) = self.text[self.cursor..].graphemes(true).next() {
            self.cursor += grapheme.len();
        }
    }

    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.text.len();
    }

    pub fn backspace(&mut self) {
        if let Some((index, _)) = self.text[..self.cursor].grapheme_indices(true).next_back() {
            self.text.drain(index..self.cursor);
            self.cursor = index;
        }
    }

    pub fn delete(&mut self) {
        if let Some(grapheme) = self.text[self.cursor..].graphemes(true).next() {
            self.text.drain(self.cursor..self.cursor + grapheme.len());
        }
    }

    pub fn delete_to_end(&mut self) {
        self.text.truncate(self.cursor);
    }

    pub fn delete_to_start(&mut self) {
        self.text.drain(..self.cursor);
        self.cursor = 0;
    }

    pub fn delete_previous_word(&mut self) {
        let prefix = &self.text[..self.cursor];
        let mut boundary = prefix.len();

        for (index, grapheme) in prefix.grapheme_indices(true).rev() {
            if grapheme.chars().all(char::is_whitespace) {
                boundary = index;
            } else {
                break;
            }
        }
        for (index, grapheme) in prefix[..boundary].grapheme_indices(true).rev() {
            if grapheme.chars().all(char::is_whitespace) {
                break;
            }
            boundary = index;
        }

        self.text.drain(boundary..self.cursor);
        self.cursor = boundary;
    }

    pub fn clear(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    pub fn set_text(&mut self, text: String) {
        self.cursor = text.len();
        self.text = text;
    }

    pub fn viewport(&self, width: usize) -> InputViewport {
        if width == 0 {
            return InputViewport {
                text: String::new(),
                cursor_column: 0,
            };
        }

        let before = &self.text[..self.cursor];
        let mut start = self.cursor;
        let mut cursor_width = 0;
        for (index, grapheme) in before.grapheme_indices(true).rev() {
            let grapheme_width = UnicodeWidthStr::width(grapheme);
            if cursor_width + grapheme_width >= width {
                break;
            }
            start = index;
            cursor_width += grapheme_width;
        }

        let mut end = self.cursor;
        let mut used = cursor_width;
        for grapheme in self.text[self.cursor..].graphemes(true) {
            let grapheme_width = UnicodeWidthStr::width(grapheme);
            if used + grapheme_width > width {
                break;
            }
            used += grapheme_width;
            end += grapheme.len();
        }

        InputViewport {
            text: self.text[start..end].to_owned(),
            cursor_column: cursor_width.min(width.saturating_sub(1)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_cursor_moves_by_grapheme() {
        let mut input = InputState::default();
        input.set_text("a👨‍👩‍👧‍👦я".into());
        input.move_left();
        assert_eq!(input.cursor, "a👨‍👩‍👧‍👦".len());
        input.move_left();
        assert_eq!(input.cursor, 1);
        input.move_right();
        assert_eq!(input.cursor, "a👨‍👩‍👧‍👦".len());
    }

    #[test]
    fn backspace_removes_unicode_grapheme() {
        let mut input = InputState::default();
        input.set_text("a👍🏽".into());
        input.backspace();
        assert_eq!(input.text(), "a");
        assert_eq!(input.cursor, 1);
    }

    #[test]
    fn delete_removes_unicode_grapheme() {
        let mut input = InputState::default();
        input.set_text("я😊z".into());
        input.move_home();
        input.move_right();
        input.delete();
        assert_eq!(input.text(), "яz");
    }

    #[test]
    fn viewport_keeps_cursor_visible_and_respects_width() {
        let mut input = InputState::default();
        input.set_text("ab界cd".into());
        let viewport = input.viewport(4);
        assert!(viewport.cursor_column < 4);
        assert!(UnicodeWidthStr::width(viewport.text.as_str()) <= 4);

        input.move_left();
        let viewport = input.viewport(4);
        assert_eq!(viewport.text, "界cd");
        assert_eq!(viewport.cursor_column, 3);

        input.move_home();
        input.move_right();
        let viewport = input.viewport(4);
        assert_eq!(viewport.cursor_column, 1);
        assert!(viewport.text.starts_with('a'));
    }
}
