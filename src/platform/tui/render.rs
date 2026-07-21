//! 纯函数渲染器：把 `State` 快照渲染成 ASCII 文本帧。
//!
//! 不做任何 I/O，输入是 `&dyn State` + 终端尺寸，输出是 `Vec<String>`（每行一行）。
//! 方便单元测试：构造 mock state → render → 断言输出行。
//!
//! 渲染布局（自顶向下）：
//! ```text
//! ┌ tab 栏：[1:main*] [2:dev] ─────────────────────────────┐
//! ├ pane 标题栏：@1 bash | @2 zsh ─────────────────────────┤
//! │ pane 内容区（每个 pane 最近 N 行输出，按布局分割）        │
//! └ 状态栏：connected | 2 panes | Ctrl-Q quit ─────────────┘
//! ```

use crate::core::model::state::{BackendStatus, State};
use crate::core::types::{PaneId, WindowId};

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
            max_output_lines: 8,
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

    // ── tab 栏 ──────────────────────────────────────────────
    let tab_bar = render_tab_bar(state, cols);
    lines.push(tab_bar);

    // ── pane 标题栏 ─────────────────────────────────────────
    let active_win = state.active_window();
    let pane_titles = render_pane_titles(state, active_win.map(|w| w.id), cols);
    lines.push(pane_titles);

    // ── pane 内容区 ─────────────────────────────────────────
    let panes: Vec<PaneId> = active_win
        .and_then(|w| state.layout(&w.id))
        .map(|wl| wl.tree.leaves())
        .unwrap_or_default();

    let remaining = rows.saturating_sub(4); // 减去 tab/标题/状态栏 + 边距
    let pane_area = remaining.max(1);
    let per_pane = if panes.is_empty() {
        pane_area
    } else {
        (pane_area / panes.len()).max(1)
    };

    for pid in &panes {
        let out = state.pane_output(pid).unwrap_or(&[]);
        let text = String::from_utf8_lossy(out);
        let mut shown = 0;
        for raw_line in text.lines().rev() {
            if shown >= per_pane || shown as usize >= opts.max_output_lines {
                break;
            }
            let truncated = truncate(raw_line, cols);
            lines.push(truncated);
            shown += 1;
        }
        // 补空行对齐
        while shown < per_pane {
            lines.push(String::new());
            shown += 1;
        }
    }
    if panes.is_empty() {
        for _ in 0..pane_area {
            lines.push(String::new());
        }
    }

    // ── 状态栏 ──────────────────────────────────────────────
    let status_bar = render_status_bar(state, cols);
    lines.push(status_bar);

    // 截断到 rows
    lines.truncate(rows);
    lines
}

/// 渲染 tab 栏：`[1:name*] [2:name] ...`
fn render_tab_bar(state: &dyn State, cols: usize) -> String {
    let mut s = String::new();
    for w in state.sessions().iter().flat_map(|sess| {
        // 没有 windows() 接口，用 active_window + sessions 模拟
        // 简化：只显示 active window
        std::iter::once((sess.id, sess.name.clone(), true))
    }) {
        let _ = w;
    }
    // 实际上 State 没有 all_windows()；用 active_window 单个 tab
    if let Some(w) = state.active_window() {
        s.push_str(&format!(
            "[{}:{}{}]",
            w.id.0,
            w.name,
            if w.active { "*" } else { "" }
        ));
    } else {
        s.push_str("[no window]");
    }
    truncate(&s, cols)
}

/// 渲染 pane 标题栏：`@1 bash | @2 zsh`
fn render_pane_titles(state: &dyn State, window: Option<WindowId>, cols: usize) -> String {
    let mut parts = Vec::new();
    if let Some(wid) = window {
        for p in state.panes(&wid) {
            let mark = if p.active { "*" } else { " " };
            parts.push(format!("{}@{} {}{}", mark, p.id.0, p.title, mark));
        }
    }
    let s = parts.join(" | ");
    truncate(if s.is_empty() { "(no pane)" } else { &s }, cols)
}

/// 渲染状态栏：`connected | 2 panes | Ctrl-Q quit`
fn render_status_bar(state: &dyn State, cols: usize) -> String {
    let status = match state.status() {
        BackendStatus::Disconnected => "disconnected",
        BackendStatus::Connecting => "connecting",
        BackendStatus::Connected => "connected",
        BackendStatus::Error => "error",
        BackendStatus::Exited => "exited",
    };
    let n_panes = state
        .active_window()
        .map(|w| state.panes(&w.id).len())
        .unwrap_or(0);
    let s = format!("{status} | {n_panes} panes | Ctrl-Q quit");
    truncate(&s, cols)
}

/// 截断字符串到指定列数（按 char count 粗略）。
fn truncate(s: &str, cols: usize) -> String {
    if s.chars().count() <= cols {
        return s.to_string();
    }
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i >= cols {
            break;
        }
        out.push(c);
    }
    if out.chars().count() == cols && s.chars().count() > cols {
        // 末尾省略号（如果还有空间）
        if cols >= 3 {
            let chars: Vec<char> = out.chars().collect();
            out = chars[..cols - 1].iter().collect();
            out.push('…');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::backend::mock::MockBackend;
    use crate::core::model::layout::{LayoutNode, SplitDir, WindowLayout};
    use crate::core::model::state::{PaneInfo, SessionInfo, WindowInfo};
    use crate::core::types::{PaneId, SessionId, WindowId};

    fn mock_with_two_panes() -> MockBackend {
        let mut b = MockBackend::with_single_pane();
        // 手动加第二个 pane
        b.panes.push(PaneInfo {
            id: PaneId(2),
            window: WindowId(1),
            active: false,
            title: "zsh".into(),
            cols: 40,
            rows: 24,
        });
        b.layouts.clear();
        let mut tree = LayoutNode::leaf(PaneId(1));
        tree.split_at(PaneId(1), PaneId(2), SplitDir::Horizontal);
        b.layouts.push(WindowLayout {
            window: WindowId(1),
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
        assert!(lines[0].starts_with('[')); // tab 栏
    }

    #[test]
    fn render_single_pane_has_tab_bar() {
        let b = MockBackend::with_single_pane();
        let lines = render_frame(&b, RenderOpts::default());
        assert!(lines[0].contains("1:w1") || lines[0].contains("w1"));
    }

    #[test]
    fn render_two_panes_shows_both_titles() {
        let b = mock_with_two_panes();
        let lines = render_frame(&b, RenderOpts::default());
        // pane 标题栏（第二行）应含两个 pane
        let title_line = &lines[1];
        assert!(title_line.contains("@1"));
        assert!(title_line.contains("@2"));
    }

    #[test]
    fn render_status_bar_shows_connected() {
        let b = MockBackend::with_single_pane();
        let lines = render_frame(&b, RenderOpts::default());
        let last = lines.last().unwrap();
        assert!(last.contains("connected"));
        assert!(last.contains("Ctrl-Q"));
    }

    #[test]
    fn render_includes_pane_output() {
        let b = mock_with_two_panes();
        let lines = render_frame(&b, RenderOpts::default());
        // 至少有一行含 "hello" 或 "line1"（pane 输出）
        let joined = lines.join("\n");
        assert!(joined.contains("hello") || joined.contains("line1"));
    }

    #[test]
    fn render_respects_max_rows() {
        let b = mock_with_two_panes();
        let opts = RenderOpts {
            cols: 80,
            rows: 5,
            max_output_lines: 2,
        };
        let lines = render_frame(&b, opts);
        assert!(lines.len() <= 5);
    }

    #[test]
    fn truncate_short_unchanged() {
        assert_eq!(truncate("abc", 10), "abc");
    }

    #[test]
    fn truncate_long_cut() {
        assert_eq!(truncate("abcdefgh", 4), "abc…");
    }

    #[test]
    fn render_exited_status() {
        let mut b = MockBackend::with_single_pane();
        b.status = BackendStatus::Exited;
        let lines = render_frame(&b, RenderOpts::default());
        let last = lines.last().unwrap();
        assert!(last.contains("exited"));
    }
}
