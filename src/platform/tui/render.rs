//! 纯函数渲染器：把 `FrameSnapshot` 渲染成 ASCII 文本帧。
//!
//! 不做任何 I/O，输入是桥接快照 + 终端尺寸，输出是 `Vec<String>`（每行一行）。
//! 方便单元测试：构造 snapshot → render → 断言输出行。
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
//! │ 状态栏：connected | 2 panes | Alt+T new tab | ...        │
//! └─────────────────────────────────────────────────────────┘
//! ```

use std::collections::HashMap;

use crate::platform::tui::ffi_bridge::{BridgeLayout, BridgePane, BridgeTab, FrameSnapshot};

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

/// 把快照渲染成一帧文本（每行一个 String，不含换行）。
///
/// 返回的行数 <= rows。
pub fn render_frame(snap: &FrameSnapshot, opts: RenderOpts) -> Vec<String> {
    let cols = opts.cols.max(1) as usize;
    let rows = opts.rows.max(1) as usize;

    let mut lines: Vec<String> = Vec::with_capacity(rows);

    // ── 顶部边框 ────────────────────────────────────────────
    lines.push(border_top(cols));

    // ── tab 栏 ──────────────────────────────────────────────
    let tab_bar = render_tab_bar(&snap.tabs);
    let inner_cols = cols.saturating_sub(2);
    lines.push(format!("│{}│", pad(&tab_bar, inner_cols)));

    // ── 分隔线 ──────────────────────────────────────────────
    lines.push(border_mid(cols));

    // ── pane 标题栏（递归布局树）────────────────────────────
    let pane_titles = render_pane_titles(snap.layout.as_ref(), &snap.panes, inner_cols);
    lines.push(format!("│{}│", pad(&pane_titles, inner_cols)));

    // ── 分隔线 ──────────────────────────────────────────────
    lines.push(border_mid(cols));

    // ── pane 内容区（递归布局树）────────────────────────────
    // 固定行：top + tab + mid + titles + mid + mid(content后) + status + bottom = 8
    let used = 8;
    let content_rows = rows.saturating_sub(used).max(1);
    let content_cols = inner_cols;

    if let Some(ref layout) = snap.layout {
        let mut grid: Vec<Vec<char>> = vec![vec![' '; content_cols]; content_rows];
        render_node(
            &mut grid,
            0,
            content_rows,
            0,
            content_cols,
            layout,
            &snap.outputs,
        );
        for row in &grid {
            let line: String = row.iter().collect();
            lines.push(format!("│{}│", line));
        }
    } else {
        for _ in 0..content_rows {
            lines.push(format!("│{}│", pad("", content_cols)));
        }
    }

    // ── 分隔线 ──────────────────────────────────────────────
    lines.push(border_mid(cols));

    // ── 状态栏 ──────────────────────────────────────────────
    let status_bar = render_status_bar(snap);
    lines.push(format!("│{}│", pad(&status_bar, cols - 2)));

    // ── 底部边框 ────────────────────────────────────────────
    lines.push(border_bottom(cols));

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
fn render_tab_bar(tabs: &[BridgeTab]) -> String {
    if tabs.is_empty() {
        return " (no tab) ".to_string();
    }
    let parts: Vec<String> = tabs
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let mark = if t.is_active { "*" } else { " " };
            format!("{}:{}{}", i + 1, t.name, mark)
        })
        .collect();
    format!(" {} ", parts.join("  "))
}

/// 渲染 pane 标题栏（递归布局树）。
fn render_pane_titles(layout: Option<&BridgeLayout>, panes: &[BridgePane], cols: usize) -> String {
    if cols == 0 {
        return String::new();
    }
    let mut buf: Vec<char> = vec![' '; cols];
    if let Some(tree) = layout {
        render_title_node(&mut buf, 0, cols, tree, panes);
    }
    if buf.iter().all(|&c| c == ' ') {
        return "(no pane)".to_string();
    }
    buf.iter().collect()
}

fn pane_title(panes: &[BridgePane], pane_id: u32) -> String {
    let p = panes.iter().find(|p| p.id == pane_id);
    match p {
        Some(p) => {
            let mark = if p.is_active { "*" } else { " " };
            let title = if p.title.is_empty() {
                String::new()
            } else {
                format!(" {}", p.title)
            };
            format!("{}@{}{}{}", mark, pane_id, title, mark)
        }
        None => format!(" @{} ", pane_id),
    }
}

/// 递归填充标题栏字符缓冲。
fn render_title_node(
    buf: &mut [char],
    col0: usize,
    col1: usize,
    node: &BridgeLayout,
    panes: &[BridgePane],
) {
    match node {
        BridgeLayout::Leaf { pane_id } => {
            let title = pane_title(panes, *pane_id);
            for (i, ch) in title.chars().enumerate() {
                let c = col0 + i;
                if c >= col1 {
                    break;
                }
                buf[c] = ch;
            }
        }
        BridgeLayout::Split {
            horizontal,
            ratio,
            first,
            second,
        } => {
            if *horizontal {
                let total = col1.saturating_sub(col0);
                if total < 3 {
                    render_title_node(buf, col0, col1, first, panes);
                    return;
                }
                let usable = total - 1;
                let first_cols = ((usable * *ratio as usize) / 1000)
                    .max(1)
                    .min(usable.saturating_sub(1));
                let mid = col0 + first_cols;
                render_title_node(buf, col0, mid, first, panes);
                if mid < col1 {
                    buf[mid] = '│';
                }
                render_title_node(buf, mid + 1, col1, second, panes);
            } else {
                // 上下分割：两个 pane 共享同一列范围。
                render_title_node(buf, col0, col1, first, panes);
                let mut end = col0;
                for (c, &ch) in buf[col0..col1].iter().enumerate() {
                    if ch != ' ' {
                        end = col0 + c + 1;
                    }
                }
                let start2 = (end + 1).min(col1);
                if start2 < col1 {
                    render_title_node(buf, start2, col1, second, panes);
                }
            }
        }
    }
}

/// 递归填充内容区字符网格。
fn render_node(
    grid: &mut [Vec<char>],
    row0: usize,
    row1: usize,
    col0: usize,
    col1: usize,
    node: &BridgeLayout,
    outputs: &HashMap<u32, Vec<u8>>,
) {
    match node {
        BridgeLayout::Leaf { pane_id } => {
            let avail_rows = row1.saturating_sub(row0);
            let avail_cols = col1.saturating_sub(col0);
            if avail_rows == 0 || avail_cols == 0 {
                return;
            }
            let out = outputs.get(pane_id).map(|v| v.as_slice()).unwrap_or(&[]);
            let text = String::from_utf8_lossy(out);
            let all_lines: Vec<&str> = text.lines().collect();
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
        BridgeLayout::Split {
            horizontal,
            ratio,
            first,
            second,
        } => {
            if *horizontal {
                let total_cols = col1.saturating_sub(col0);
                if total_cols < 3 {
                    render_node(grid, row0, row1, col0, col1, first, outputs);
                    return;
                }
                let sep = 1;
                let usable = total_cols - sep;
                let first_cols = ((usable * *ratio as usize) / 1000)
                    .max(1)
                    .min(usable.saturating_sub(1));
                let mid = col0 + first_cols;
                render_node(grid, row0, row1, col0, mid, first, outputs);
                for r in row0..row1 {
                    if r < grid.len() && mid < grid[r].len() {
                        grid[r][mid] = '│';
                    }
                }
                render_node(grid, row0, row1, mid + 1, col1, second, outputs);
            } else {
                let total_rows = row1.saturating_sub(row0);
                if total_rows < 3 {
                    render_node(grid, row0, row1, col0, col1, first, outputs);
                    return;
                }
                let sep = 1;
                let usable = total_rows - sep;
                let first_rows = ((usable * *ratio as usize) / 1000)
                    .max(1)
                    .min(usable.saturating_sub(1));
                let mid = row0 + first_rows;
                render_node(grid, row0, mid, col0, col1, first, outputs);
                if mid < grid.len() {
                    for c in col0..col1 {
                        if c < grid[mid].len() {
                            grid[mid][c] = '─';
                        }
                    }
                }
                render_node(grid, mid + 1, row1, col0, col1, second, outputs);
            }
        }
    }
}

/// 渲染状态栏。
fn render_status_bar(snap: &FrameSnapshot) -> String {
    let status = if snap.status.is_empty() {
        "unknown"
    } else {
        snap.status.as_str()
    };
    let n_panes = snap.panes.len();
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
    use crate::platform::tui::ffi_bridge::{BridgeLayout, BridgePane, BridgeTab, FrameSnapshot};
    use std::collections::HashMap;

    fn snap_empty() -> FrameSnapshot {
        FrameSnapshot {
            status: "disconnected".into(),
            ..Default::default()
        }
    }

    fn snap_single_pane() -> FrameSnapshot {
        let mut outputs = HashMap::new();
        outputs.insert(1, b"hello\nworld\n".to_vec());
        FrameSnapshot {
            tabs: vec![BridgeTab {
                id: 1,
                name: "t1".into(),
                is_active: true,
            }],
            panes: vec![BridgePane {
                id: 1,
                cols: 80,
                rows: 24,
                is_active: true,
                title: "bash".into(),
            }],
            layout: Some(BridgeLayout::Leaf { pane_id: 1 }),
            outputs,
            status: "connected".into(),
            active_tab: 1,
            active_pane: 1,
        }
    }

    fn snap_two_panes() -> FrameSnapshot {
        let mut outputs = HashMap::new();
        outputs.insert(1, b"hello\nworld\n".to_vec());
        outputs.insert(2, b"line1\nline2\n".to_vec());
        FrameSnapshot {
            tabs: vec![
                BridgeTab {
                    id: 1,
                    name: "t1".into(),
                    is_active: true,
                },
                BridgeTab {
                    id: 2,
                    name: "t2".into(),
                    is_active: false,
                },
            ],
            panes: vec![
                BridgePane {
                    id: 1,
                    cols: 40,
                    rows: 24,
                    is_active: true,
                    title: "bash".into(),
                },
                BridgePane {
                    id: 2,
                    cols: 40,
                    rows: 24,
                    is_active: false,
                    title: "zsh".into(),
                },
            ],
            layout: Some(BridgeLayout::Split {
                horizontal: true,
                ratio: 500,
                first: Box::new(BridgeLayout::Leaf { pane_id: 1 }),
                second: Box::new(BridgeLayout::Leaf { pane_id: 2 }),
            }),
            outputs,
            status: "connected".into(),
            active_tab: 1,
            active_pane: 1,
        }
    }

    fn snap_nested_split() -> FrameSnapshot {
        let mut outputs = HashMap::new();
        outputs.insert(1, b"left\n".to_vec());
        outputs.insert(2, b"top-right\n".to_vec());
        outputs.insert(3, b"bottom-right\n".to_vec());
        FrameSnapshot {
            tabs: vec![BridgeTab {
                id: 1,
                name: "t1".into(),
                is_active: true,
            }],
            panes: vec![
                BridgePane {
                    id: 1,
                    cols: 40,
                    rows: 24,
                    is_active: true,
                    title: "bash".into(),
                },
                BridgePane {
                    id: 2,
                    cols: 40,
                    rows: 12,
                    is_active: false,
                    title: "zsh".into(),
                },
                BridgePane {
                    id: 3,
                    cols: 40,
                    rows: 12,
                    is_active: false,
                    title: "fish".into(),
                },
            ],
            layout: Some(BridgeLayout::Split {
                horizontal: true,
                ratio: 500,
                first: Box::new(BridgeLayout::Leaf { pane_id: 1 }),
                second: Box::new(BridgeLayout::Split {
                    horizontal: false,
                    ratio: 500,
                    first: Box::new(BridgeLayout::Leaf { pane_id: 2 }),
                    second: Box::new(BridgeLayout::Leaf { pane_id: 3 }),
                }),
            }),
            outputs,
            status: "connected".into(),
            active_tab: 1,
            active_pane: 1,
        }
    }

    #[test]
    fn render_empty_state() {
        let lines = render_frame(&snap_empty(), RenderOpts::default());
        assert!(!lines.is_empty());
        assert!(lines[0].starts_with('┌'));
    }

    #[test]
    fn render_has_top_and_bottom_border() {
        let lines = render_frame(&snap_single_pane(), RenderOpts::default());
        assert!(lines[0].starts_with('┌'));
        assert!(lines.last().unwrap().starts_with('└'));
    }

    #[test]
    fn render_tab_bar_shows_window_name() {
        let lines = render_frame(&snap_single_pane(), RenderOpts::default());
        let tab_line = &lines[1];
        assert!(
            tab_line.contains("1:") && (tab_line.contains("t1") || tab_line.contains('*')),
            "tab bar should show tabs, got: {tab_line}"
        );
    }

    #[test]
    fn render_two_panes_shows_both_titles() {
        let lines = render_frame(&snap_two_panes(), RenderOpts::default());
        let title_line = &lines[3];
        assert!(title_line.contains("@1"));
        assert!(title_line.contains("@2"));
    }

    #[test]
    fn render_status_bar_shows_connected() {
        let lines = render_frame(&snap_single_pane(), RenderOpts::default());
        let status_line = &lines[lines.len() - 2];
        assert!(status_line.contains("connected"));
        assert!(status_line.contains("Ctrl-Q"));
    }

    #[test]
    fn render_includes_pane_output() {
        let lines = render_frame(&snap_two_panes(), RenderOpts::default());
        let joined = lines.join("\n");
        assert!(joined.contains("hello") || joined.contains("line1"));
    }

    #[test]
    fn render_respects_max_rows() {
        let opts = RenderOpts {
            cols: 80,
            rows: 8,
            max_output_lines: 2,
        };
        let lines = render_frame(&snap_two_panes(), opts);
        assert!(lines.len() <= 8);
    }

    #[test]
    fn render_has_pane_separator_between_panes() {
        let lines = render_frame(&snap_two_panes(), RenderOpts::default());
        let content_lines: Vec<&String> = lines
            .iter()
            .filter(|l| l.starts_with('│') && l.contains('│') && !l.starts_with('├'))
            .collect();
        assert!(
            content_lines.iter().any(|l| l.matches('│').count() >= 3),
            "内容区应有 pane 分隔符 │"
        );
    }

    #[test]
    fn render_nested_split_shows_all_three_pane_titles() {
        let lines = render_frame(&snap_nested_split(), RenderOpts::default());
        let title_line = &lines[3];
        assert!(title_line.contains("@1"), "标题栏应有 @1: {title_line}");
        assert!(title_line.contains("@2"), "标题栏应有 @2: {title_line}");
        assert!(title_line.contains("@3"), "标题栏应有 @3: {title_line}");
    }

    #[test]
    fn render_nested_split_left_pane_content_on_left() {
        let lines = render_frame(
            &snap_nested_split(),
            RenderOpts {
                cols: 80,
                rows: 24,
                max_output_lines: 20,
            },
        );
        let joined = lines.join("\n");
        assert!(joined.contains("left"), "应包含 @1 的 left 输出: {joined}");
    }

    #[test]
    fn render_nested_split_right_panes_stacked_vertically() {
        let lines = render_frame(
            &snap_nested_split(),
            RenderOpts {
                cols: 80,
                rows: 24,
                max_output_lines: 20,
            },
        );
        let joined = lines.join("\n");
        assert!(
            joined.contains("top-right"),
            "应包含 @2 的 top-right 输出: {joined}"
        );
        assert!(
            joined.contains("bottom-right"),
            "应包含 @3 的 bottom-right 输出: {joined}"
        );
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
        let lines = render_frame(&snap_nested_split(), RenderOpts::default());
        let has_h_sep = lines.iter().any(|l| l.contains('─'));
        assert!(has_h_sep, "内容区应有水平分隔线 ─ (垂直分割): {lines:?}");
    }

    #[test]
    fn render_nested_split_has_vertical_separator() {
        let lines = render_frame(&snap_nested_split(), RenderOpts::default());
        let has_v_sep = lines.iter().any(|l| l.matches('│').count() >= 2);
        assert!(has_v_sep, "内容区应有垂直分隔线 │ (水平分割): {lines:?}");
    }

    #[test]
    fn render_status_bar_shows_alt_t_hint() {
        let lines = render_frame(&snap_single_pane(), RenderOpts::default());
        let status_line = &lines[lines.len() - 2];
        assert!(status_line.contains("Alt+T"));
    }

    #[test]
    fn render_status_bar_shows_split_hints() {
        let lines = render_frame(&snap_single_pane(), RenderOpts::default());
        let status_line = &lines[lines.len() - 2];
        assert!(status_line.contains("Alt+S"), "状态栏应提示 Alt+S 水平分割");
        assert!(status_line.contains("Alt+V"), "状态栏应提示 Alt+V 垂直分割");
    }

    #[test]
    fn render_exited_status() {
        let mut snap = snap_single_pane();
        snap.status = "exited".into();
        let lines = render_frame(&snap, RenderOpts::default());
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
