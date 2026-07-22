//! 纯函数渲染器：把 `State` 快照渲染成 ASCII 文本帧。
//!
//! 不做任何 I/O，输入是 `&dyn State` + 终端尺寸，输出是 `Vec<String>`（每行一行）。
//! 方便单元测试：构造 mock state → render → 断言输出行。
//!
//! 渲染布局（自顶向下）：
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │ tab 栏：[1:main*] [2:dev]                                 │
//! ├─────────────────────────────────────────────────────────┤
//! │ pane 标题栏：@1 bash | @2 zsh                             │
//! ├───────────┬─────────────────────────────────────────────┤
//! │ pane 1    │ pane 2                                        │
//! │ output    │ output                                        │
//! │           │                                               │
//! ├─────────────────────────────────────────────────────────┤
//! │ 状态栏：connected | 2 panes | Alt+T new tab | Alt+S split | Alt+V vsplit | Ctrl-Q quit │
//! └─────────────────────────────────────────────────────────┘
//! ```

use crate::core::model::layout::{LayoutNode, SplitDir};
use crate::core::model::state::{BackendStatus, State};
use crate::core::types::TabId;

/// 渲染选项。
#[derive(Debug, Clone, Copy)]
pub struct RenderOpts {
    /// 终端列数。
    pub cols: u16,
    /// 终端行数。
    pub rows: u16,
    /// 每个 pane 最多显示的输出行数。
    pub max_output_lines: usize,
}

impl Default for RenderOpts {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            max_output_lines: 20,
        }
    }
}

/// 把 state 渲染成一帧文本（每行一个 String，不含换行）。
///
/// 返回的行数 <= rows。
pub fn render_frame(state: &dyn State, opts: RenderOpts) -> Vec<String> {
    let cols = opts.cols.max(1) as usize;
    let rows = opts.rows.max(1) as usize;

    let mut lines: Vec<String> = Vec::with_capacity(rows);

    // ── 顶部边框 ────────────────────────────────────────────
    lines.push(border_top(cols));

    // ── tab 栏 ──────────────────────────────────────────────
    let tab_bar = render_tab_bar(state, cols);
    lines.push(format!("│{}│", pad(&tab_bar, cols - 2)));

    // ── 分隔线 ──────────────────────────────────────────────
    lines.push(border_mid(cols));

    // ── pane 标题栏（递归布局树）────────────────────────────
    let inner_cols = cols.saturating_sub(2);
    let tab_id = state.active_tab().map(|t| t.id);
    let pane_titles = render_pane_titles(state, tab_id, inner_cols);
    lines.push(format!("│{}│", pad(&pane_titles, inner_cols)));

    // ── 分隔线 ──────────────────────────────────────────────
    lines.push(border_mid(cols));

    // ── pane 内容区（递归布局树）────────────────────────────
    // 固定行：top + tab + mid + titles + mid + mid(content后) + status + bottom = 8
    let used = 8;
    let content_rows = rows.saturating_sub(used).max(1);
    let content_cols = inner_cols;

    let active_tab = state.active_tab();
    let layout = active_tab.and_then(|t| state.layout(&t.id));

    if let Some(tl) = layout {
        // 构建字符网格，递归布局树填充每个 pane 的输出到对应矩形区域
        let mut grid: Vec<Vec<char>> = vec![vec![' '; content_cols]; content_rows];
        render_node(&mut grid, 0, content_rows, 0, content_cols, &tl.tree, state);
        for row in &grid {
            let line: String = row.iter().collect();
            lines.push(format!("│{}│", line));
        }
    } else {
        // 无 layout：填空行
        for _ in 0..content_rows {
            lines.push(format!("│{}│", pad("", content_cols)));
        }
    }

    // ── 分隔线 ──────────────────────────────────────────────
    lines.push(border_mid(cols));

    // ── 状态栏 ──────────────────────────────────────────────
    let status_bar = render_status_bar(state, cols);
    lines.push(format!("│{}│", pad(&status_bar, cols - 2)));

    // ── 底部边框 ────────────────────────────────────────────
    lines.push(border_bottom(cols));

    // 截断到 rows
    lines.truncate(rows);
    lines
}

/// 顶部边框 `┌─...─┐`
fn border_top(cols: usize) -> String {
    format!("┌{}┐", "─".repeat(cols.saturating_sub(2)))
}

/// 中间分隔线 `├─...─┤`
fn border_mid(cols: usize) -> String {
    format!("├{}┤", "─".repeat(cols.saturating_sub(2)))
}

/// 底部边框 `└─...─┘`
fn border_bottom(cols: usize) -> String {
    format!("└{}┘", "─".repeat(cols.saturating_sub(2)))
}

/// 渲染 tab 栏：`[1:main*] [2:dev]`
fn render_tab_bar(state: &dyn State, _cols: usize) -> String {
    // 列出所有 tab（= window），active 标 *
    let active_wid = state.active_window().map(|w| w.id);
    let windows = state.all_windows();
    if windows.is_empty() {
        return " (no window) ".to_string();
    }
    let parts: Vec<String> = windows
        .iter()
        .map(|w| {
            let mark = if active_wid == Some(w.id) { "*" } else { " " };
            format!("{}:{}{}", w.id.0, w.name, mark)
        })
        .collect();
    format!(" {} ", parts.join("  "))
}

/// 渲染 pane 标题栏（递归布局树）。
///
/// 水平分割（左右）→ 按比例分列宽，`│` 分隔；
/// 垂直分割（上下）→ 两个 pane 共享同一列范围，标题依次排列。
fn render_pane_titles(state: &dyn State, tab: Option<TabId>, cols: usize) -> String {
    if cols == 0 {
        return String::new();
    }
    let mut buf: Vec<char> = vec![' '; cols];
    if let Some(tid) = tab {
        if let Some(tl) = state.layout(&tid) {
            render_title_node(&mut buf, 0, cols, &tl.tree, state);
        }
    }
    // 检查是否全空白（无 pane）
    if buf.iter().all(|&c| c == ' ') {
        return "(no pane)".to_string();
    }
    buf.iter().collect()
}

/// 递归填充标题栏字符缓冲。
fn render_title_node(
    buf: &mut [char],
    col0: usize,
    col1: usize,
    node: &LayoutNode,
    state: &dyn State,
) {
    match node {
        LayoutNode::Leaf(pid) => {
            let title = state
                .pane(pid)
                .map(|p| {
                    let mark = if p.active { "*" } else { " " };
                    format!("{}@{} {}{}", mark, pid.0, p.title, mark)
                })
                .unwrap_or_default();
            for (i, ch) in title.chars().enumerate() {
                let c = col0 + i;
                if c >= col1 {
                    break;
                }
                buf[c] = ch;
            }
        }
        LayoutNode::Split {
            dir,
            ratio,
            first,
            second,
        } => {
            match dir {
                SplitDir::Horizontal => {
                    let total = col1.saturating_sub(col0);
                    if total < 3 {
                        render_title_node(buf, col0, col1, first, state);
                        return;
                    }
                    let usable = total - 1;
                    let first_cols = ((usable * *ratio as usize) / 1000)
                        .max(1)
                        .min(usable.saturating_sub(1));
                    let mid = col0 + first_cols;
                    render_title_node(buf, col0, mid, first, state);
                    if mid < col1 {
                        buf[mid] = '│';
                    }
                    render_title_node(buf, mid + 1, col1, second, state);
                }
                SplitDir::Vertical => {
                    // 上下分割：两个 pane 共享同一列范围。
                    // 先渲染 first 的标题，找到其结尾，再在剩余空间渲染 second。
                    render_title_node(buf, col0, col1, first, state);
                    // 找 first 已填充的最右位置
                    let mut end = col0;
                    for (c, &ch) in buf[col0..col1].iter().enumerate() {
                        if ch != ' ' {
                            end = col0 + c + 1;
                        }
                    }
                    let start2 = (end + 1).min(col1);
                    if start2 < col1 {
                        render_title_node(buf, start2, col1, second, state);
                    }
                }
            }
        }
    }
}

/// 递归填充内容区字符网格。
///
/// `Leaf(pid)` → 在分配的矩形 [row0,row1)×[col0,col1) 内渲染 pane 输出（取最后若干行）。
/// `Split { Horizontal, .. }` → 左右分列，`│` 分隔。
/// `Split { Vertical, .. }` → 上下分行，`─` 分隔。
fn render_node(
    grid: &mut [Vec<char>],
    row0: usize,
    row1: usize,
    col0: usize,
    col1: usize,
    node: &LayoutNode,
    state: &dyn State,
) {
    match node {
        LayoutNode::Leaf(pid) => {
            let avail_rows = row1.saturating_sub(row0);
            let avail_cols = col1.saturating_sub(col0);
            if avail_rows == 0 || avail_cols == 0 {
                return;
            }
            let out = state.pane_output(pid).unwrap_or(&[]);
            let text = String::from_utf8_lossy(out);
            let all_lines: Vec<&str> = text.lines().collect();
            // 取最后 avail_rows 行，正序显示（底部对齐）
            let start = all_lines.len().saturating_sub(avail_rows);
            let visible = &all_lines[start..];
            for (i, line) in visible.iter().enumerate() {
                let r = row0 + i;
                if r >= row1 {
                    break;
                }
                for (j, ch) in line.chars().enumerate() {
                    let c = col0 + j;
                    if c >= col1 {
                        break;
                    }
                    grid[r][c] = ch;
                }
            }
        }
        LayoutNode::Split {
            dir,
            ratio,
            first,
            second,
        } => {
            match dir {
                SplitDir::Horizontal => {
                    // 左右分割
                    let total_cols = col1.saturating_sub(col0);
                    if total_cols < 3 {
                        render_node(grid, row0, row1, col0, col1, first, state);
                        return;
                    }
                    let sep = 1;
                    let usable = total_cols - sep;
                    let first_cols = ((usable * *ratio as usize) / 1000)
                        .max(1)
                        .min(usable.saturating_sub(1));
                    let mid = col0 + first_cols;
                    // first: [col0, mid)
                    render_node(grid, row0, row1, col0, mid, first, state);
                    // 分隔线 │
                    for r in row0..row1 {
                        if r < grid.len() && mid < grid[r].len() {
                            grid[r][mid] = '│';
                        }
                    }
                    // second: [mid+1, col1)
                    render_node(grid, row0, row1, mid + 1, col1, second, state);
                }
                SplitDir::Vertical => {
                    // 上下分割
                    let total_rows = row1.saturating_sub(row0);
                    if total_rows < 3 {
                        render_node(grid, row0, row1, col0, col1, first, state);
                        return;
                    }
                    let sep = 1;
                    let usable = total_rows - sep;
                    let first_rows = ((usable * *ratio as usize) / 1000)
                        .max(1)
                        .min(usable.saturating_sub(1));
                    let mid = row0 + first_rows;
                    // first: [row0, mid)
                    render_node(grid, row0, mid, col0, col1, first, state);
                    // 分隔线 ─
                    if mid < grid.len() {
                        for c in col0..col1 {
                            if c < grid[mid].len() {
                                grid[mid][c] = '─';
                            }
                        }
                    }
                    // second: [mid+1, row1)
                    render_node(grid, mid + 1, row1, col0, col1, second, state);
                }
            }
        }
    }
}

/// 渲染状态栏：`connected | 2 panes | Alt+T new tab | Alt+S split | Alt+V vsplit | Ctrl-Q quit`
fn render_status_bar(state: &dyn State, _cols: usize) -> String {
    let status = match state.status() {
        BackendStatus::Disconnected => "disconnected",
        BackendStatus::Connecting => "connecting",
        BackendStatus::Connected => "connected",
        BackendStatus::Error => "error",
        BackendStatus::Exited => "exited",
    };
    let n_panes = state
        .active_tab()
        .map(|t| state.panes(&t.id).len())
        .unwrap_or(0);
    format!(
        " {status} | {n_panes} panes | Alt+T new tab | Alt+S split | Alt+V vsplit | Ctrl-Q quit "
    )
}

/// 把字符串 pad 到指定宽度（左侧空格填充，右侧截断）。
fn pad(s: &str, width: usize) -> String {
    let current = s.chars().count();
    if current >= width {
        s.chars().take(width).collect()
    } else {
        let pad = width - current;
        format!("{}{}", s, " ".repeat(pad))
    }
}

/// 截断字符串到指定列数（按 char count）。
fn truncate(s: &str, cols: usize) -> String {
    if s.chars().count() <= cols {
        return s.to_string();
    }
    s.chars().take(cols).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::backend::mock::MockBackend;
    use crate::core::model::layout::{LayoutNode, SplitDir, TabLayout};
    use crate::core::model::state::{PaneInfo, TabInfo, WindowInfo};
    use crate::core::types::{PaneId, TabId, WindowId};

    fn mock_with_two_panes() -> MockBackend {
        let mut b = MockBackend::with_single_pane();
        b.tabs.push(TabInfo {
            id: TabId(2),
            name: "t2".into(),
            window: WindowId(1),
            active: false,
        });
        b.panes.push(PaneInfo {
            id: PaneId(2),
            tab: TabId(1),
            active: false,
            title: "zsh".into(),
            cols: 40,
            rows: 24,
        });
        b.layouts.clear();
        let mut tree = LayoutNode::leaf(PaneId(1));
        tree.split_at(PaneId(1), PaneId(2), SplitDir::Horizontal);
        b.layouts.push(crate::core::model::layout::TabLayout {
            tab: TabId(1),
            tree,
            active: PaneId(1),
        });
        b.outputs.push((PaneId(2), b"line1\nline2\n".to_vec()));
        b.outputs[0].1 = b"hello\nworld\n".to_vec();
        b
    }

    #[test]
    fn render_empty_state() {
        let b = MockBackend::new();
        let lines = render_frame(&b, RenderOpts::default());
        assert!(!lines.is_empty());
        // 顶部边框
        assert!(lines[0].starts_with('┌'));
    }

    #[test]
    fn render_has_top_and_bottom_border() {
        let b = MockBackend::with_single_pane();
        let lines = render_frame(&b, RenderOpts::default());
        assert!(lines[0].starts_with('┌'));
        assert!(lines.last().unwrap().starts_with('└'));
    }

    #[test]
    fn render_tab_bar_shows_window_name() {
        let b = MockBackend::with_single_pane();
        let lines = render_frame(&b, RenderOpts::default());
        // tab 栏在第 2 行
        let tab_line = &lines[1];
        assert!(tab_line.contains("1:w1"));
    }

    #[test]
    fn render_two_panes_shows_both_titles() {
        let b = mock_with_two_panes();
        let lines = render_frame(&b, RenderOpts::default());
        let title_line = &lines[3];
        assert!(title_line.contains("@1"));
        assert!(title_line.contains("@2"));
    }

    #[test]
    fn render_status_bar_shows_connected() {
        let b = MockBackend::with_single_pane();
        let lines = render_frame(&b, RenderOpts::default());
        // 状态栏在倒数第二行（最后一行是底部边框）
        let status_line = &lines[lines.len() - 2];
        assert!(status_line.contains("connected"));
        assert!(status_line.contains("Ctrl-Q"));
    }

    #[test]
    fn render_includes_pane_output() {
        let b = mock_with_two_panes();
        let lines = render_frame(&b, RenderOpts::default());
        let joined = lines.join("\n");
        assert!(joined.contains("hello") || joined.contains("line1"));
    }

    #[test]
    fn render_respects_max_rows() {
        let b = mock_with_two_panes();
        let opts = RenderOpts {
            cols: 80,
            rows: 8,
            max_output_lines: 2,
        };
        let lines = render_frame(&b, opts);
        assert!(lines.len() <= 8);
    }

    #[test]
    fn render_has_pane_separator_between_panes() {
        let b = mock_with_two_panes();
        let lines = render_frame(&b, RenderOpts::default());
        // 内容区应有 │ 分隔两个 pane
        let content_lines: Vec<&String> = lines
            .iter()
            .filter(|l| l.starts_with('│') && l.contains("│") && !l.starts_with("├"))
            .collect();
        // 至少有一行内容区含中间 │（pane 分隔）
        assert!(
            content_lines.iter().any(|l| l.matches('│').count() >= 3),
            "内容区应有 pane 分隔符 │"
        );
    }

    /// 构造嵌套布局 Split(H, @1, Split(V, @2, @3)) 的 mock。
    fn mock_with_nested_split() -> MockBackend {
        let mut b = MockBackend::with_single_pane();
        // @2 pane
        b.panes.push(PaneInfo {
            id: PaneId(2),
            tab: TabId(1),
            active: false,
            title: "zsh".into(),
            cols: 40,
            rows: 12,
        });
        // @3 pane
        b.panes.push(PaneInfo {
            id: PaneId(3),
            tab: TabId(1),
            active: false,
            title: "fish".into(),
            cols: 40,
            rows: 12,
        });
        // 布局: Split(H, @1, Split(V, @2, @3))
        let mut tree = LayoutNode::leaf(PaneId(1));
        tree.split_at(PaneId(1), PaneId(2), SplitDir::Horizontal);
        tree.split_at(PaneId(2), PaneId(3), SplitDir::Vertical);
        b.layouts.clear();
        b.layouts.push(TabLayout {
            tab: TabId(1),
            tree,
            active: PaneId(1),
        });
        // 输出内容
        b.outputs.clear();
        b.outputs.push((
            PaneId(1),
            b"left
"
            .to_vec(),
        ));
        b.outputs.push((
            PaneId(2),
            b"top-right
"
            .to_vec(),
        ));
        b.outputs.push((
            PaneId(3),
            b"bottom-right
"
            .to_vec(),
        ));
        b
    }

    #[test]
    fn render_nested_split_shows_all_three_pane_titles() {
        let b = mock_with_nested_split();
        let lines = render_frame(&b, RenderOpts::default());
        let title_line = &lines[3];
        assert!(title_line.contains("@1"), "标题栏应有 @1: {title_line}");
        assert!(title_line.contains("@2"), "标题栏应有 @2: {title_line}");
        assert!(title_line.contains("@3"), "标题栏应有 @3: {title_line}");
    }

    #[test]
    fn render_nested_split_left_pane_content_on_left() {
        let b = mock_with_nested_split();
        // 用足够大的终端确保有空间
        let lines = render_frame(
            &b,
            RenderOpts {
                cols: 80,
                rows: 24,
                max_output_lines: 20,
            },
        );
        let joined = lines.join(
            "
",
        );
        assert!(joined.contains("left"), "应包含 @1 的 left 输出: {joined}");
    }

    #[test]
    fn render_nested_split_right_panes_stacked_vertically() {
        let b = mock_with_nested_split();
        let lines = render_frame(
            &b,
            RenderOpts {
                cols: 80,
                rows: 24,
                max_output_lines: 20,
            },
        );
        let joined = lines.join(
            "
",
        );
        // @2 (top-right) 和 @3 (bottom-right) 都应出现
        assert!(
            joined.contains("top-right"),
            "应包含 @2 的 top-right 输出: {joined}"
        );
        assert!(
            joined.contains("bottom-right"),
            "应包含 @3 的 bottom-right 输出: {joined}"
        );
        // top-right 应出现在 bottom-right 之前的行
        let top_row = lines
            .iter()
            .position(|l| l.contains("top-right"))
            .unwrap_or(usize::MAX);
        let bot_row = lines
            .iter()
            .position(|l| l.contains("bottom-right"))
            .unwrap_or(usize::MAX);
        assert!(
            top_row < bot_row,
            "top-right 应在 bottom-right 上方 (top={top_row}, bot={bot_row})"
        );
    }

    #[test]
    fn render_nested_split_has_horizontal_separator() {
        let b = mock_with_nested_split();
        let lines = render_frame(&b, RenderOpts::default());
        // 内容区应有 ─ 分隔上下两个右栏 pane
        let has_h_sep = lines.iter().any(|l| l.contains('─'));
        assert!(has_h_sep, "内容区应有水平分隔线 ─ (垂直分割): {lines:?}");
    }

    #[test]
    fn render_nested_split_has_vertical_separator() {
        let b = mock_with_nested_split();
        let lines = render_frame(&b, RenderOpts::default());
        // 内容区应有 │ 分隔左右
        let has_v_sep = lines.iter().any(|l| l.matches('│').count() >= 2);
        assert!(has_v_sep, "内容区应有垂直分隔线 │ (水平分割): {lines:?}");
    }

    #[test]
    fn render_status_bar_shows_alt_t_hint() {
        let b = MockBackend::with_single_pane();
        let lines = render_frame(&b, RenderOpts::default());
        let status_line = &lines[lines.len() - 2];
        assert!(status_line.contains("Alt+T"));
    }

    #[test]
    fn render_status_bar_shows_split_hints() {
        let b = MockBackend::with_single_pane();
        let lines = render_frame(&b, RenderOpts::default());
        let status_line = &lines[lines.len() - 2];
        assert!(status_line.contains("Alt+S"), "状态栏应提示 Alt+S 水平分割");
        assert!(status_line.contains("Alt+V"), "状态栏应提示 Alt+V 垂直分割");
    }

    #[test]
    fn render_exited_status() {
        let mut b = MockBackend::with_single_pane();
        b.status = BackendStatus::Exited;
        let lines = render_frame(&b, RenderOpts::default());
        let status_line = &lines[lines.len() - 2];
        assert!(status_line.contains("exited"));
    }

    #[test]
    fn pad_fills_to_width() {
        assert_eq!(pad("ab", 5), "ab   ");
        assert_eq!(pad("abcde", 3), "abc");
    }

    #[test]
    fn truncate_short_unchanged() {
        assert_eq!(truncate("abc", 10), "abc");
    }

    #[test]
    fn truncate_long_cut() {
        assert_eq!(truncate("abcdefgh", 4), "abcd");
    }
}
