use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Debug, Default)]
pub struct InputState {
    text: String,
    cursor: usize,
    preferred_column: Option<usize>,
    kill_buffer: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputLayout {
    pub lines: Vec<String>,
    pub cursor_row: usize,
    pub cursor_column: usize,
    pub first_visual_line: usize,
    pub total_visual_lines: usize,
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
        self.preferred_column = None;
    }

    pub fn insert_str(&mut self, text: &str) {
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.preferred_column = None;
    }

    pub fn insert_newline(&mut self) {
        self.insert('\n');
    }

    pub fn move_left(&mut self) {
        if let Some((index, _)) = self.text[..self.cursor].grapheme_indices(true).next_back() {
            self.cursor = index;
        }
        self.preferred_column = None;
    }

    pub fn move_right(&mut self) {
        if let Some(grapheme) = self.text[self.cursor..].graphemes(true).next() {
            self.cursor += grapheme.len();
        }
        self.preferred_column = None;
    }

    pub fn move_home(&mut self) {
        self.cursor = self.line_start();
        self.preferred_column = None;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.line_end();
        self.preferred_column = None;
    }

    pub fn move_word_left(&mut self) {
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

        self.cursor = boundary;
        self.preferred_column = None;
    }

    pub fn move_word_right(&mut self) {
        let mut boundary = self.cursor;
        let mut saw_word = false;
        for grapheme in self.text[self.cursor..].graphemes(true) {
            let whitespace = grapheme.chars().all(char::is_whitespace);
            if saw_word && whitespace {
                break;
            }
            boundary += grapheme.len();
            saw_word |= !whitespace;
        }
        self.cursor = boundary;
        self.preferred_column = None;
    }

    pub fn move_up(&mut self, width: usize) -> bool {
        let lines = self.wrapped_lines(width);
        let index = visual_line_index(&lines, self.cursor);
        let current = &lines[index];
        let target_column = self
            .preferred_column
            .unwrap_or_else(|| UnicodeWidthStr::width(&self.text[current.start..self.cursor]));

        if index == 0 {
            if self.cursor == 0 {
                return false;
            }
            self.cursor = 0;
            self.preferred_column = None;
            return true;
        }

        if self.preferred_column.is_none() {
            self.preferred_column = Some(target_column);
        }
        self.cursor = cursor_at_column(&self.text, &lines[index - 1], target_column);
        true
    }

    pub fn move_down(&mut self, width: usize) -> bool {
        let lines = self.wrapped_lines(width);
        let index = visual_line_index(&lines, self.cursor);
        let current = &lines[index];
        let target_column = self
            .preferred_column
            .unwrap_or_else(|| UnicodeWidthStr::width(&self.text[current.start..self.cursor]));

        if index + 1 >= lines.len() {
            if self.cursor == self.text.len() {
                return false;
            }
            self.cursor = self.text.len();
            self.preferred_column = None;
            return true;
        }

        if self.preferred_column.is_none() {
            self.preferred_column = Some(target_column);
        }
        self.cursor = cursor_at_column(&self.text, &lines[index + 1], target_column);
        true
    }

    pub fn backspace(&mut self) {
        if let Some((index, _)) = self.text[..self.cursor].grapheme_indices(true).next_back() {
            self.text.drain(index..self.cursor);
            self.cursor = index;
        }
        self.preferred_column = None;
    }

    pub fn delete(&mut self) {
        if let Some(grapheme) = self.text[self.cursor..].graphemes(true).next() {
            self.text.drain(self.cursor..self.cursor + grapheme.len());
        }
        self.preferred_column = None;
    }

    pub fn delete_to_end(&mut self) {
        let mut end = self.line_end();
        if end == self.cursor && end < self.text.len() {
            end += 1;
        }
        self.kill_buffer = self.text[self.cursor..end].to_owned();
        self.text.drain(self.cursor..end);
        self.preferred_column = None;
    }

    pub fn delete_to_start(&mut self) {
        let start = self.line_start();
        self.kill_buffer = self.text[start..self.cursor].to_owned();
        self.text.drain(start..self.cursor);
        self.cursor = start;
        self.preferred_column = None;
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
        self.preferred_column = None;
    }

    pub fn delete_next_word(&mut self) {
        let start = self.cursor;
        self.move_word_right();
        let end = self.cursor;
        self.text.drain(start..end);
        self.cursor = start;
        self.preferred_column = None;
    }

    pub fn yank(&mut self) {
        if !self.kill_buffer.is_empty() {
            let killed = self.kill_buffer.clone();
            self.insert_str(&killed);
        }
    }

    pub fn clear(&mut self) -> String {
        self.cursor = 0;
        self.preferred_column = None;
        std::mem::take(&mut self.text)
    }

    pub fn set_text(&mut self, text: String) {
        self.cursor = text.len();
        self.text = text;
        self.preferred_column = None;
    }

    pub fn visual_line_count(&self, width: usize) -> usize {
        self.wrapped_lines(width).len()
    }

    pub fn layout(&self, width: usize, height: usize) -> InputLayout {
        let width = width.max(1);
        let lines = self.wrapped_lines(width);
        let cursor_visual_line = visual_line_index(&lines, self.cursor);
        let visible_height = height.max(1).min(lines.len());
        let first_visual_line = cursor_visual_line
            .saturating_add(1)
            .saturating_sub(visible_height);
        let end = (first_visual_line + visible_height).min(lines.len());
        let visible = lines[first_visual_line..end]
            .iter()
            .map(|range| self.text[range.clone()].to_owned())
            .collect();
        let current = &lines[cursor_visual_line];

        InputLayout {
            lines: visible,
            cursor_row: cursor_visual_line.saturating_sub(first_visual_line),
            cursor_column: UnicodeWidthStr::width(&self.text[current.start..self.cursor])
                .min(width.saturating_sub(1)),
            first_visual_line,
            total_visual_lines: lines.len(),
        }
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor]
            .rfind('\n')
            .map(|index| index + 1)
            .unwrap_or(0)
    }

    fn line_end(&self) -> usize {
        self.text[self.cursor..]
            .find('\n')
            .map(|index| self.cursor + index)
            .unwrap_or(self.text.len())
    }

    fn wrapped_lines(&self, width: usize) -> Vec<Range<usize>> {
        let width = width.max(1);
        if self.text.is_empty() {
            return std::iter::once(0..0).collect();
        }

        let mut lines = Vec::new();
        let mut start = 0;
        let mut used = 0usize;
        for (index, grapheme) in self.text.grapheme_indices(true) {
            if grapheme == "\n" {
                let wrapped_line_ends_here = lines
                    .last()
                    .is_some_and(|line: &Range<usize>| line.end == index);
                if start < index || !wrapped_line_ends_here {
                    lines.push(start..index);
                }
                start = index + 1;
                used = 0;
                continue;
            }

            let grapheme_width = UnicodeWidthStr::width(grapheme);
            if used > 0 && used.saturating_add(grapheme_width) > width {
                lines.push(start..index);
                start = index;
                used = 0;
            }
            used = used.saturating_add(grapheme_width);
            if used >= width {
                let end = index + grapheme.len();
                lines.push(start..end);
                start = end;
                used = 0;
            }
        }

        if start < self.text.len() || self.text.ends_with('\n') {
            lines.push(start..self.text.len());
        }
        if lines.is_empty() {
            lines.push(0..0);
        }
        lines
    }
}

fn visual_line_index(lines: &[Range<usize>], cursor: usize) -> usize {
    lines
        .partition_point(|line| line.start <= cursor)
        .saturating_sub(1)
        .min(lines.len().saturating_sub(1))
}

fn cursor_at_column(text: &str, line: &Range<usize>, column: usize) -> usize {
    let mut cursor = line.start;
    let mut used = 0usize;
    for grapheme in text[line.clone()].graphemes(true) {
        let width = UnicodeWidthStr::width(grapheme);
        if used.saturating_add(width) > column {
            break;
        }
        used = used.saturating_add(width);
        cursor += grapheme.len();
    }
    cursor
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
    fn wraps_multiline_input_and_keeps_cursor_visible() {
        let mut input = InputState::default();
        input.set_text("ab界cd\nlast".into());
        let layout = input.layout(4, 2);
        assert_eq!(layout.lines, vec!["cd", "last"]);
        assert_eq!(layout.cursor_row, 1);
        assert_eq!(layout.cursor_column, 3);
        assert_eq!(layout.total_visual_lines, 3);
    }

    #[test]
    fn moves_vertically_across_wrapped_and_explicit_lines() {
        let mut input = InputState::default();
        input.set_text("abcd\nx".into());
        assert!(input.move_up(4));
        assert_eq!(input.cursor, 1);
        assert!(input.move_down(4));
        assert_eq!(input.cursor, input.text.len());
    }

    #[test]
    fn home_end_and_kill_commands_are_line_local() {
        let mut input = InputState::default();
        input.set_text("first\nsecond".into());
        input.move_home();
        assert_eq!(input.cursor, "first\n".len());
        input.move_right();
        input.delete_to_end();
        assert_eq!(input.text(), "first\ns");
        input.yank();
        assert_eq!(input.text(), "first\nsecond");
    }

    #[test]
    fn inserts_multiline_paste_at_cursor() {
        let mut input = InputState::default();
        input.set_text("ac".into());
        input.move_left();
        input.insert_str("b\n");
        assert_eq!(input.text(), "ab\nc");
    }
}
