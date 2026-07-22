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

use crate::core::model::state::{BackendStatus, State};
use crate::core::types::{PaneId, TabId};

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

    // ── pane 标题栏 ─────────────────────────────────────────
    let pane_titles = render_pane_titles(state, state.active_tab().map(|t| t.id), cols);
    lines.push(format!("│{}│", pad(&pane_titles, cols - 2)));

    // ── 分隔线 ──────────────────────────────────────────────
    lines.push(border_mid(cols));

    // ── pane 内容区 ─────────────────────────────────────────
    let active_tab = state.active_tab();
    let panes: Vec<PaneId> = active_tab
        .and_then(|t| state.layout(&t.id))
        .map(|tl| tl.tree.leaves())
        .unwrap_or_default();

    // 固定行：top + tab + mid + titles + mid + mid(content后) + status + bottom = 8
    let used = 8;
    let content_rows = rows.saturating_sub(used).max(1);

    if panes.is_empty() {
        // 无 pane：填空行
        for _ in 0..content_rows {
            lines.push(format!("│{}│", pad("", cols - 2)));
        }
    } else {
        // 每个 pane 平均分配行数
        let per_pane = (content_rows / panes.len()).max(1);
        // 每个 pane 的列宽
        let pane_cols = ((cols - 2) / panes.len()).max(1);
        // 收集每个 pane 的输出行
        let pane_outputs: Vec<Vec<String>> = panes
            .iter()
            .map(|pid| {
                let out = state.pane_output(pid).unwrap_or(&[]);
                let text = String::from_utf8_lossy(out);
                let mut all_lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();
                // 取最后 per_pane 行，正序显示
                if all_lines.len() > per_pane {
                    let start = all_lines.len() - per_pane;
                    all_lines = all_lines[start..].to_vec();
                }
                // pad/truncate 到 pane_cols
                all_lines.iter().map(|l| truncate(l, pane_cols)).collect()
            })
            .collect();

        // 按 per_pane 行逐行拼接
        for row in 0..per_pane {
            let mut row_parts = Vec::new();
            for pane_out in &pane_outputs {
                let line = pane_out.get(row).map(|s| s.as_str()).unwrap_or("");
                row_parts.push(pad(line, pane_cols));
            }
            let content = row_parts.join("│");
            let content = pad(&content, cols - 2);
            lines.push(format!("│{}│", content));
        }
        // 补足剩余行
        let drawn = per_pane;
        if drawn < content_rows {
            for _ in drawn..content_rows {
                lines.push(format!("│{}│", pad("", cols - 2)));
            }
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
    if let Some(w) = state.active_window() {
        format!(" {}:{} ", w.id.0, w.name)
    } else {
        " (no window) ".to_string()
    }
}

/// 渲染 pane 标题栏：`@1 bash | @2 zsh`
fn render_pane_titles(state: &dyn State, tab: Option<TabId>, _cols: usize) -> String {
    let mut parts = Vec::new();
    if let Some(tid) = tab {
        for p in state.panes(&tid) {
            let mark = if p.active { "*" } else { " " };
            parts.push(format!("{}@{} {}{}", mark, p.id.0, p.title, mark));
        }
    }
    if parts.is_empty() {
        "(no pane)".to_string()
    } else {
        parts.join(" | ")
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
