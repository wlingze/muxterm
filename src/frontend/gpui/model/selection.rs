//! 终端选区：单元格坐标 + 文本提取。
//!
//! 语义对齐 iTerm2 / Ghostty 的默认行为：
//! - 拖选按**阅读顺序**取区间，跨行时中间行整行选中；
//! - 双击选词走 `model::word_selection`（三层：identifier → word → path）；
//! - 三击整行；
//! - 复制时逐行去尾部空白，行之间用 `\n` 连接，末尾不额外加换行。

use super::word_selection;

/// 网格坐标（视口内，0 基）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CellPos {
    pub row: usize,
    pub col: usize,
}

impl CellPos {
    pub fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}

/// 选区的来源：决定拖拽时怎么扩展。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// 普通拖选。
    Simple,
    /// 双击选词；拖拽按 token 扩展（不会在 `-` / `/` 处切开）。
    Word,
    /// 三击整行。
    Line,
}

/// 一次选区。`anchor` 是按下鼠标的格子，`focus` 是当前拖到的格子。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: CellPos,
    pub focus: CellPos,
    pub kind: Kind,
}

impl Selection {
    pub fn new(anchor: CellPos, focus: CellPos, kind: Kind) -> Self {
        Self {
            anchor,
            focus,
            kind,
        }
    }

    /// 起止按阅读顺序排好（往后拖、往前拖结果一致）。
    pub fn normalized(&self) -> (CellPos, CellPos) {
        if self.anchor <= self.focus {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }

    /// 单个格子（点一下没拖）不算选区，复制时应当忽略。
    pub fn is_single_cell(&self) -> bool {
        self.anchor == self.focus
    }

    /// 渲染用：这个格子是否在选区内。
    pub fn contains(&self, row: usize, col: usize) -> bool {
        let (start, end) = self.normalized();
        if row < start.row || row > end.row {
            return false;
        }
        if row == start.row && col < start.col {
            return false;
        }
        if row == end.row && col > end.col {
            return false;
        }
        true
    }

    /// 同一选区换焦点（拖拽）。
    pub fn with_focus(self, focus: CellPos) -> Self {
        Self { focus, ..self }
    }

    /// 选区文本。`line` 返回某一行的格子字符（`None` 表示没有这行）。
    ///
    /// 宽字符占位格（`\0`）跳过；每行去尾部空白。
    pub fn text(&self, line: impl Fn(usize) -> Option<Vec<char>>) -> String {
        let (start, end) = self.normalized();
        let mut rows: Vec<String> = Vec::new();
        for row in start.row..=end.row {
            let Some(cells) = line(row) else {
                continue;
            };
            let from = if row == start.row { start.col } else { 0 };
            let to = if row == end.row {
                end.col.min(cells.len().saturating_sub(1))
            } else {
                cells.len().saturating_sub(1)
            };
            let mut text = String::new();
            if from <= to {
                for ch in &cells[from..=to.min(cells.len().saturating_sub(1))] {
                    if *ch != '\0' {
                        text.push(*ch);
                    }
                }
            }
            rows.push(text.trim_end().to_string());
        }
        rows.join("\n")
    }
}

/// 双击：把点击点扩成整词；扩不出来就退化成单格。
pub fn word_selection(cells: &[char], pos: CellPos) -> Selection {
    let span = word_selection::select(cells, pos.col, None);
    match span {
        Some(span) => Selection::new(
            CellPos::new(pos.row, span.start),
            CellPos::new(pos.row, span.end.saturating_sub(1)),
            Kind::Word,
        ),
        None => Selection::new(pos, pos, Kind::Word),
    }
}

/// 三击：整行。
pub fn line_selection(row: usize, cols: usize) -> Selection {
    Selection::new(
        CellPos::new(row, 0),
        CellPos::new(row, cols.saturating_sub(1)),
        Kind::Line,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(lines: &[&str], cols: usize) -> Vec<Vec<char>> {
        lines
            .iter()
            .map(|line| {
                let mut cells: Vec<char> = line.chars().collect();
                cells.resize(cols, ' ');
                cells
            })
            .collect()
    }

    fn text_of(selection: &Selection, grid: &[Vec<char>]) -> String {
        selection.text(|row| grid.get(row).cloned())
    }

    /// 往前拖和往后拖选中的是同一批格子。
    #[test]
    fn drag_direction_does_not_change_the_range() {
        let forward = Selection::new(CellPos::new(0, 2), CellPos::new(0, 6), Kind::Simple);
        let backward = Selection::new(CellPos::new(0, 6), CellPos::new(0, 2), Kind::Simple);
        for col in 0..10 {
            assert_eq!(
                forward.contains(0, col),
                backward.contains(0, col),
                "col {col}"
            );
        }
        assert!(forward.contains(0, 2) && forward.contains(0, 6));
        assert!(!forward.contains(0, 1) && !forward.contains(0, 7));
    }

    /// 跨行时中间行整行选中，首行从起点开始，末行到终点结束。
    #[test]
    fn intermediate_rows_are_selected_end_to_end() {
        let selection = Selection::new(CellPos::new(1, 4), CellPos::new(3, 2), Kind::Simple);
        assert!(!selection.contains(1, 3));
        assert!(selection.contains(1, 4));
        assert!(selection.contains(2, 0), "中间行从头就有");
        assert!(selection.contains(2, 79), "中间行到尾也有");
        assert!(selection.contains(3, 2));
        assert!(!selection.contains(3, 3));
        assert!(!selection.contains(4, 0));
    }

    /// 多行复制：逐行去尾空白，`\n` 连接。
    #[test]
    fn copied_text_trims_trailing_spaces_per_line() {
        let grid = grid(&["hello   ", "world  ", "tail"], 10);
        let selection = Selection::new(CellPos::new(0, 0), CellPos::new(2, 3), Kind::Simple);
        assert_eq!(text_of(&selection, &grid), "hello\nworld\ntail");
    }

    /// 宽字符的第二个格子（`\0`）不能进剪贴板。
    #[test]
    fn wide_char_fillers_are_skipped() {
        let grid = vec![vec!['中', '\0', '文', '\0', 'a', ' ', ' ', ' ']];
        let selection = Selection::new(CellPos::new(0, 0), CellPos::new(0, 4), Kind::Simple);
        assert_eq!(text_of(&selection, &grid), "中文a");
    }

    /// 双击选词用 word_selection 的三层规则。
    #[test]
    fn double_click_expands_to_the_word() {
        let cells: Vec<char> = "git checkout feature/dogfood".chars().collect();
        let selection = word_selection(&cells, CellPos::new(0, 16));
        assert_eq!(selection.kind, Kind::Word);
        let (start, end) = selection.normalized();
        // 第一次落在 identifier 层：至少包含 `feature`（`/` 是 path 分隔）。
        assert!(start.col <= 13 && end.col >= 19, "{start:?}..{end:?}");
        assert!(selection.contains(0, 13));
    }

    /// 三击整行。
    #[test]
    fn triple_click_selects_the_whole_row() {
        let selection = line_selection(2, 80);
        assert_eq!(selection.normalized().0, CellPos::new(2, 0));
        assert_eq!(selection.normalized().1, CellPos::new(2, 79));
        assert!(selection.contains(2, 0) && selection.contains(2, 79));
        assert!(!selection.contains(1, 0));
    }

    /// 空格上双击也必须有选区（退化成单格），不能 panic。
    #[test]
    fn double_click_on_blank_cells_is_safe() {
        let cells = vec![' '; 10];
        let selection = word_selection(&cells, CellPos::new(0, 3));
        assert!(selection.contains(0, 3));
    }

    /// 单格点击不算选区。
    #[test]
    fn a_bare_click_is_not_a_selection() {
        let selection = Selection::new(CellPos::new(1, 1), CellPos::new(1, 1), Kind::Simple);
        assert!(selection.is_single_cell());
    }
}
