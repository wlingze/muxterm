//! 单行文本输入框的可编辑状态。
//!
//! 命令面板（`Cmd-Shift-P`）与统一面板（`Cmd-P`）的查询框以前只有
//! `String::push` / `pop`：没有光标、没有选区、`Cmd-A` 没反应、退格只能删
//! 末尾。这里补一个最小但完整的编辑模型，渲染层再按它画光标与选区。

/// 单行文本 + 光标 + 锚点（选区另一端）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextInput {
    chars: Vec<char>,
    /// 光标位置（字符下标，0..=len）。
    cursor: usize,
    /// 选区锚点。与 `cursor` 相等表示没有选区。
    anchor: usize,
}

impl TextInput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_text(text: &str) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let end = chars.len();
        Self {
            chars,
            cursor: end,
            anchor: end,
        }
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    pub fn len(&self) -> usize {
        self.chars.len()
    }

    /// 光标位置（字符下标），渲染光标用。
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// 选区范围（已归一化成 `start..end`）；没有选区时返回 `None`。
    pub fn selection(&self) -> Option<std::ops::Range<usize>> {
        if self.cursor == self.anchor {
            return None;
        }
        Some(self.cursor.min(self.anchor)..self.cursor.max(self.anchor))
    }

    pub fn selected_text(&self) -> Option<String> {
        let range = self.selection()?;
        Some(self.chars[range].iter().collect())
    }

    pub fn clear(&mut self) {
        self.chars.clear();
        self.cursor = 0;
        self.anchor = 0;
    }

    /// 整体替换（Enter 选中项后回填等）。光标落在末尾。
    pub fn set_text(&mut self, text: &str) {
        self.chars = text.chars().collect();
        self.cursor = self.chars.len();
        self.anchor = self.cursor;
    }

    /// `Cmd-A`：全选。
    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.cursor = self.chars.len();
    }

    /// 删掉选区；没有选区时删掉光标前一个字符（`backspace`）。
    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor > 0 {
            self.cursor -= 1;
            self.chars.remove(self.cursor);
        }
        self.anchor = self.cursor;
    }

    /// 删掉选区；没有选区时删掉光标后一个字符（`delete`）。
    pub fn delete_forward(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
        }
        self.anchor = self.cursor;
    }

    /// 删掉选区；没有选区时删掉光标前一个词（`Alt-Backspace` / `Ctrl-W`）。
    pub fn delete_word_backward(&mut self) {
        if self.delete_selection() {
            return;
        }
        let mut start = self.cursor;
        while start > 0 && self.chars[start - 1].is_whitespace() {
            start -= 1;
        }
        while start > 0 && !self.chars[start - 1].is_whitespace() {
            start -= 1;
        }
        self.chars.drain(start..self.cursor);
        self.cursor = start;
        self.anchor = start;
    }

    fn delete_selection(&mut self) -> bool {
        let Some(range) = self.selection() else {
            return false;
        };
        self.chars.drain(range.clone());
        self.cursor = range.start;
        self.anchor = range.start;
        true
    }

    /// 插入一段文本（替换当前选区）。
    pub fn insert(&mut self, text: &str) {
        self.delete_selection();
        for ch in text.chars().filter(|ch| !ch.is_control()) {
            self.chars.insert(self.cursor, ch);
            self.cursor += 1;
        }
        self.anchor = self.cursor;
    }

    /// ←/→；`extend` 为真时扩展选区（Shift）。
    pub fn move_cursor(&mut self, delta: isize, extend: bool) {
        let next = (self.cursor as isize + delta).clamp(0, self.chars.len() as isize) as usize;
        self.cursor = next;
        if !extend {
            self.anchor = next;
        }
    }

    /// Home / End（`Cmd-←` / `Cmd-→` 也走这里）。
    pub fn move_to(&mut self, end: bool, extend: bool) {
        self.cursor = if end { self.chars.len() } else { 0 };
        if !extend {
            self.anchor = self.cursor;
        }
    }

    /// 一个字符的移动步长：`Cmd-←/→` 走词边界。
    pub fn move_word(&mut self, forward: bool, extend: bool) {
        if forward {
            let mut index = self.cursor;
            while index < self.chars.len() && self.chars[index].is_whitespace() {
                index += 1;
            }
            while index < self.chars.len() && !self.chars[index].is_whitespace() {
                index += 1;
            }
            self.cursor = index;
        } else {
            let mut index = self.cursor;
            while index > 0 && self.chars[index - 1].is_whitespace() {
                index -= 1;
            }
            while index > 0 && !self.chars[index - 1].is_whitespace() {
                index -= 1;
            }
            self.cursor = index;
        }
        if !extend {
            self.anchor = self.cursor;
        }
    }

    /// 渲染用切片：`(选区前, 选区, 选区后)`。
    ///
    /// 没有选区时中间段为空，光标画在 `before` 末尾。
    pub fn segments(&self) -> (String, String, String) {
        let range = self.selection();
        let cursor = self.cursor;
        let before_end = range.as_ref().map(|range| range.start).unwrap_or(cursor);
        let (selected, after_start) = match range {
            Some(range) => (
                self.chars[range.clone()].iter().collect::<String>(),
                range.end,
            ),
            None => (String::new(), cursor),
        };
        let before: String = self.chars[..before_end].iter().collect();
        let after: String = self.chars[after_start.min(self.chars.len())..]
            .iter()
            .collect();
        (before, selected, after)
    }

    /// 光标在选区首尾哪一端（渲染时把光标画在对应位置）。
    pub fn cursor_at_start_of_selection(&self) -> bool {
        self.selection()
            .is_some_and(|range| self.cursor == range.start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_appends_and_keeps_the_cursor_at_the_end() {
        let mut input = TextInput::new();
        input.insert("ab");
        input.insert("中");
        assert_eq!(input.text(), "ab中");
        assert_eq!(input.cursor(), 3);
        assert_eq!(input.selection(), None);
    }

    #[test]
    fn cmd_a_selects_everything_and_typing_replaces_it() {
        let mut input = TextInput::from_text("alpha");
        input.select_all();
        assert_eq!(input.selection(), Some(0..5));
        assert_eq!(input.selected_text().as_deref(), Some("alpha"));
        input.insert("beta");
        assert_eq!(input.text(), "beta");
        assert_eq!(input.selection(), None);
        assert_eq!(input.cursor(), 4);
    }

    #[test]
    fn backspace_deletes_the_selection_or_the_char_before_the_cursor() {
        let mut input = TextInput::from_text("abc");
        input.select_all();
        input.backspace();
        assert_eq!(input.text(), "");
        input.insert("xyz");
        input.move_to(false, false);
        input.move_cursor(2, false);
        input.backspace();
        assert_eq!(input.text(), "xz");
        assert_eq!(input.cursor(), 1);
    }

    #[test]
    fn shift_arrows_extend_and_plain_arrows_collapse_the_selection() {
        let mut input = TextInput::from_text("hello");
        input.move_to(false, false);
        input.move_cursor(2, true);
        assert_eq!(input.selection(), Some(0..2));
        input.move_cursor(1, false);
        assert_eq!(input.selection(), None);
        assert_eq!(input.cursor(), 3);
    }

    #[test]
    fn moving_by_words_skips_whitespace() {
        let mut input = TextInput::from_text("open local dev");
        input.move_to(false, false);
        input.move_word(true, false);
        assert_eq!(input.cursor(), 4);
        input.move_word(true, false);
        assert_eq!(input.cursor(), 10);
        input.move_word(false, false);
        assert_eq!(input.cursor(), 5);
    }

    #[test]
    fn segments_split_around_the_selection_for_rendering() {
        let mut input = TextInput::from_text("abcdef");
        input.move_to(false, false);
        input.move_cursor(2, true);
        assert_eq!(input.segments(), ("".into(), "ab".into(), "cdef".into()));
        let plain = TextInput::from_text("abc");
        assert_eq!(plain.segments(), ("abc".into(), "".into(), "".into()));
    }

    #[test]
    fn deleting_words_removes_the_trailing_word() {
        let mut input = TextInput::from_text("open local ");
        input.delete_word_backward();
        assert_eq!(input.text(), "open ");
        input.delete_word_backward();
        assert_eq!(input.text(), "");
    }
}
