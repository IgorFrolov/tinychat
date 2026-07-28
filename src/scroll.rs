#[derive(Clone, Debug)]
pub struct ScrollState {
    pub offset_from_bottom: usize,
    pub follow_output: bool,
}

impl Default for ScrollState {
    fn default() -> Self {
        Self {
            offset_from_bottom: 0,
            follow_output: true,
        }
    }
}

impl ScrollState {
    pub fn max_offset(total_lines: usize, viewport_lines: usize) -> usize {
        total_lines.saturating_sub(viewport_lines)
    }

    pub fn clamp(&mut self, total_lines: usize, viewport_lines: usize) {
        self.offset_from_bottom = self
            .offset_from_bottom
            .min(Self::max_offset(total_lines, viewport_lines));
        if self.offset_from_bottom == 0 {
            self.follow_output = true;
        }
    }

    pub fn up(&mut self, amount: usize, total_lines: usize, viewport_lines: usize) {
        let maximum = Self::max_offset(total_lines, viewport_lines);
        self.offset_from_bottom = self.offset_from_bottom.saturating_add(amount).min(maximum);
        self.follow_output = self.offset_from_bottom == 0;
    }

    pub fn down(&mut self, amount: usize) {
        self.offset_from_bottom = self.offset_from_bottom.saturating_sub(amount);
        self.follow_output = self.offset_from_bottom == 0;
    }

    pub fn top(&mut self, total_lines: usize, viewport_lines: usize) {
        self.offset_from_bottom = Self::max_offset(total_lines, viewport_lines);
        self.follow_output = self.offset_from_bottom == 0;
    }

    pub fn bottom(&mut self) {
        self.offset_from_bottom = 0;
        self.follow_output = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_is_always_limited() {
        let mut scroll = ScrollState {
            offset_from_bottom: 999,
            follow_output: false,
        };
        scroll.clamp(20, 8);
        assert_eq!(scroll.offset_from_bottom, 12);

        scroll.clamp(4, 8);
        assert_eq!(scroll.offset_from_bottom, 0);
        assert!(scroll.follow_output);
    }
}
