//! 纯函数渲染器：把 `FrameSnapshot` 渲染进 ratatui `Buffer`。
//!
//! 不做任何 I/O，输入是桥接快照 + 终端尺寸，输出是写入一帧 ratatui Buffer。
//! 方便单元测试：构造 snapshot → render → 断言 Buffer 里的文本。
//!
//! 布局（自顶向下）：
//! ```text
//! ┌─────────── 标题栏（muxterm · <backend status>）───────────┐
//! │ Tab 栏：[1:main*] [2:dev]                                  │
//! ├──────────────────────────────────────────────────────────┤
//! │ pane 内容区（递归布局树）                                   │
//! │                                                            │
//! ├──────────────────────────────────────────────────────────┤
//! │ 状态栏：connected | 2 panes | Alt+P palette · Alt+S ...   │
//! └──────────────────────────────────────────────────────────┘
//! ```
//!
//! 命令面板（opencode 风格）作为悬浮层覆盖在上层。

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, StatefulWidget, Widget};

use crate::platform::tui::ffi_bridge::{BridgeLayout, FrameSnapshot};
use crate::platform::tui::palette::PaletteState;
use crate::platform::tui::theme::Theme;

/// 渲染选项。
#[derive(Debug, Clone, Copy)]
pub struct RenderOpts {
    /// 终端列数。
    pub cols: u16,
    /// 终端行数。
    pub rows: u16,
    /// 每个 pane 最多显示的输出行数。
    pub max_output_lines: usize,
    /// 是否显示命令面板。
    pub palette_open: bool,
}

impl Default for RenderOpts {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            max_output_lines: 20,
            palette_open: false,
        }
    }
}

/// 去除 ANSI 转义序列（颜色 / 光标 / 样式），返回纯文本。
///
/// pane 输出包含 tmux/shell 的颜色与光标控制码（如 `\x1b[31m`、`\x1b[38;5;242m`），
/// 直接渲染会变成乱码。渲染前必须剥掉这些转义。
pub fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // ESC 开头的 CSI/OSC 序列，剥到停止符
            if chars.peek() == Some(&'[') {
                chars.next(); // [
                              // 读到最终字节（@-~ 或 0x40-0x7e）
                for cc in chars.by_ref() {
                    if ('@'..='~').contains(&cc) {
                        break;
                    }
                }
            } else if chars.peek() == Some(&']') {
                chars.next(); // ]
                for cc in chars.by_ref() {
                    if cc == '\x07' || cc == '\x1b' {
                        break;
                    }
                }
            } else if chars.peek() == Some(&'(') || chars.peek() == Some(&')') {
                chars.next();
                let _ = chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// 渲染入口：把快照画进 `buf`。
///
/// `screens`：pane_id → 该 pane 的终端屏幕网格（由 `TerminalState` 生成）。
/// 用它渲染 pane 内容，而不是直接打印累计原始输出。
pub fn render_frame(
    buf: &mut Buffer,
    snap: &FrameSnapshot,
    screens: &std::collections::HashMap<u32, Vec<String>>,
    palette: Option<&PaletteState>,
    opts: RenderOpts,
) {
    let theme = Theme::default();
    let cols = opts.cols.max(1);
    let rows = opts.rows.max(1);

    // 全屏背景
    let bg_block = Block::default().style(Style::default().bg(theme.bg));
    bg_block.render(Rect::new(0, 0, cols, rows), buf);

    // 主布局：标题栏 / tab 栏 / 内容区 / 状态栏
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(Rect::new(0, 0, cols, rows));

    draw_title_bar(buf, chunks[0], snap, &theme);
    draw_tab_bar(buf, chunks[1], &snap.tabs, &theme);
    draw_content(buf, chunks[2], snap, screens, &theme);
    draw_status_bar(buf, chunks[3], snap, &theme);

    if opts.palette_open {
        if let Some(p) = palette {
            draw_palette(buf, p, &theme);
        }
    }
}

fn draw_title_bar(buf: &mut Buffer, area: Rect, snap: &FrameSnapshot, theme: &Theme) {
    let title = format!(
        " muxterm · {} · Ctrl-Q quit",
        if snap.status.is_empty() {
            "unknown"
        } else {
            &snap.status
        }
    );
    Paragraph::new(title)
        .style(theme.accent_style())
        .render(area, buf);
}

fn draw_tab_bar(
    buf: &mut Buffer,
    area: Rect,
    tabs: &[crate::platform::tui::ffi_bridge::BridgeTab],
    theme: &Theme,
) {
    if tabs.is_empty() {
        Paragraph::new(" (no tab) ")
            .style(theme.dim_style())
            .render(area, buf);
        return;
    }
    let mut spans: Vec<Span> = Vec::new();
    for (i, t) in tabs.iter().enumerate() {
        let label = format!(" {}:{} ", i + 1, t.name);
        if t.is_active {
            spans.push(Span::styled(label, theme.active_bg()));
        } else {
            spans.push(Span::styled(label, theme.dim_style()));
        }
        spans.push(Span::raw(" "));
    }
    Paragraph::new(Line::from(spans)).render(area, buf);
}

fn draw_content(
    buf: &mut Buffer,
    area: Rect,
    snap: &FrameSnapshot,
    screens: &std::collections::HashMap<u32, Vec<String>>,
    theme: &Theme,
) {
    let Some(layout) = snap.layout.as_ref() else {
        Paragraph::new("(no pane)")
            .style(theme.dim_style())
            .render(area, buf);
        return;
    };
    draw_layout_node(buf, area, layout, snap, screens, theme);
}

fn draw_layout_node(
    buf: &mut Buffer,
    area: Rect,
    node: &BridgeLayout,
    snap: &FrameSnapshot,
    screens: &std::collections::HashMap<u32, Vec<String>>,
    theme: &Theme,
) {
    match node {
        BridgeLayout::Leaf { pane_id } => draw_leaf(buf, area, *pane_id, snap, screens, theme),
        BridgeLayout::Split {
            horizontal,
            ratio,
            first,
            second,
        } => {
            if *horizontal {
                let total = area.width;
                if total < 3 {
                    draw_layout_node(buf, area, first, snap, screens, theme);
                    return;
                }
                let usable = total - 1;
                let first_w = ((usable as u32 * *ratio) / 1000).max(1) as u16;
                let first_w = first_w.min(usable.saturating_sub(1));
                let left = Rect {
                    width: first_w,
                    ..area
                };
                let right = Rect {
                    x: area.x + left.width + 1,
                    width: area.width - left.width - 1,
                    ..area
                };
                draw_layout_node(buf, left, first, snap, screens, theme);
                for y in area.y..area.y + area.height {
                    if let Some(cell) = buf.cell_mut((area.x + left.width, y)) {
                        cell.set_symbol("│");
                        cell.set_fg(theme.dim);
                    }
                }
                draw_layout_node(buf, right, second, snap, screens, theme);
            } else {
                let total = area.height;
                if total < 3 {
                    draw_layout_node(buf, area, first, snap, screens, theme);
                    return;
                }
                let usable = total - 1;
                let first_h = ((usable as u32 * *ratio) / 1000).max(1) as u16;
                let first_h = first_h.min(usable.saturating_sub(1));
                let top = Rect {
                    height: first_h,
                    ..area
                };
                let bottom = Rect {
                    y: area.y + top.height + 1,
                    height: area.height - top.height - 1,
                    ..area
                };
                draw_layout_node(buf, top, first, snap, screens, theme);
                for x in area.x..area.x + area.width {
                    if let Some(cell) = buf.cell_mut((x, area.y + top.height)) {
                        cell.set_symbol("─");
                        cell.set_fg(theme.dim);
                    }
                }
                draw_layout_node(buf, bottom, second, snap, screens, theme);
            }
        }
    }
}

fn draw_leaf(
    buf: &mut Buffer,
    area: Rect,
    pane_id: u32,
    snap: &FrameSnapshot,
    screens: &std::collections::HashMap<u32, Vec<String>>,
    theme: &Theme,
) {
    let is_active = snap.active_pane == pane_id;
    let p = snap.panes.iter().find(|p| p.id == pane_id);
    let (title, cols, rows) = match p {
        Some(p) => (p.title.clone(), p.cols, p.rows),
        None => (String::new(), 0, 0),
    };
    let title = if title.is_empty() {
        format!("@{}  {}x{}", pane_id, cols, rows)
    } else {
        format!("@{} · {}  {}x{}", pane_id, title, cols, rows)
    };

    let border_style = if is_active {
        Style::default().fg(theme.accent)
    } else {
        theme.dim_style()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Line::from(title));
    let inner = block.inner(area);
    block.render(area, buf);

    if inner.width == 0 || inner.height == 0 {
        return;
    }
    // 用终端模拟器的屏幕网格渲染（不再打印累计原始输出）
    //
    // `TerminalState.snapshot()` 返回整个 pane 网格（行数 = tmux pane 行数，可能
    // 比当前可视区高）。当前提示符/最新输出在网格底部，因此取**底部** avail 行
    // 作为可视视口（即“看当前屏幕底部”），而不是顶部（那会是旧内容）。
    let screen = screens.get(&pane_id).cloned().unwrap_or_default();
    let content_style = if is_active {
        Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg)
    };
    let avail = inner.height as usize;
    let start = screen.len().saturating_sub(avail);
    let visible: Vec<String> = screen[start..].to_vec();
    Paragraph::new(visible.join("\n"))
        .style(content_style)
        .render(inner, buf);
}

fn draw_status_bar(buf: &mut Buffer, area: Rect, snap: &FrameSnapshot, theme: &Theme) {
    let n_panes = snap.panes.len();
    let status_key = match snap.status.as_str() {
        "connected" => crate::platform::i18n::Key::StatusConnected,
        "connecting" => crate::platform::i18n::Key::StatusConnecting,
        "disconnected" => crate::platform::i18n::Key::StatusDisconnected,
        "error" => crate::platform::i18n::Key::StatusError,
        "exited" => crate::platform::i18n::Key::StatusExited,
        _ => crate::platform::i18n::Key::StatusUnknown,
    };
    let status = crate::platform::i18n::tr(status_key);
    let panes = crate::platform::i18n::tr(crate::platform::i18n::Key::Panes);
    let palette = crate::platform::i18n::tr(crate::platform::i18n::Key::HintPalette);
    let new_tab = crate::platform::i18n::tr(crate::platform::i18n::Key::HintNewTab);
    let split = crate::platform::i18n::tr(crate::platform::i18n::Key::HintSplit);
    let vertical_split = crate::platform::i18n::tr(crate::platform::i18n::Key::HintVerticalSplit);
    let pane = crate::platform::i18n::tr(crate::platform::i18n::Key::HintPane);
    let quit = crate::platform::i18n::tr(crate::platform::i18n::Key::HintQuit);
    let hint = format!(
        " Alt+P {palette} · Alt+T {new_tab} · Alt+S {split} · Alt+V {vertical_split} · Alt+[ ] {pane} · Ctrl-Q {quit} "
    );
    let status_style = match snap.status.as_str() {
        "connected" => theme.success_style(),
        "error" | "exited" | "disconnected" => theme.danger_style(),
        _ => theme.text(),
    };
    let line = Line::from(vec![
        Span::styled(format!(" {status} "), status_style),
        Span::styled(format!("· {n_panes} {panes} "), theme.dim_style()),
        Span::styled(hint, theme.dim_style()),
    ]);
    Paragraph::new(line)
        .alignment(Alignment::Left)
        .render(area, buf);
}

/// 命令面板（opencode 风格悬浮框）。
fn draw_palette(buf: &mut Buffer, palette: &PaletteState, theme: &Theme) {
    let area = buf.area;
    let w = (area.width * 60 / 100)
        .max(40)
        .min(area.width.saturating_sub(4));
    let h = (area.height * 40 / 100)
        .max(10)
        .min(area.height.saturating_sub(2));
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 3;

    let pal_rect = Rect {
        x,
        y,
        width: w,
        height: h,
    };
    Clear.render(pal_rect, buf);

    let title = format!(" {} · {} ", "Connection", palette.step.title());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme.accent_style())
        .title(title);
    let inner = block.inner(pal_rect);
    block.render(pal_rect, buf);

    // 顶部信息行：来源 + 主机 + 当前目录
    let info_h = 1u16;
    let source_str = match palette.source {
        crate::platform::tui::palette::ConnectSource::Local => "local",
        crate::platform::tui::palette::ConnectSource::Ssh => "ssh",
    };
    let host_str = palette.host.as_deref().unwrap_or("");
    let dir_str = palette.dir.as_deref().unwrap_or("");
    let info = format!(
        " {}{}{}",
        source_str,
        if host_str.is_empty() {
            String::new()
        } else {
            format!("@{host_str}")
        },
        if dir_str.is_empty() {
            String::new()
        } else {
            format!("  cwd:{dir_str}")
        },
    );
    Paragraph::new(info).style(theme.dim_style()).render(
        Rect {
            height: info_h,
            ..inner
        },
        buf,
    );

    // 过滤输入行（opencode 风格）
    let query_h = 1u16;
    Paragraph::new(format!("> {}", palette.query))
        .style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .render(
            Rect {
                y: inner.y + info_h,
                height: query_h,
                ..inner
            },
            buf,
        );

    let list_area = Rect {
        y: inner.y + info_h + query_h,
        height: inner.height.saturating_sub(info_h + query_h),
        ..inner
    };

    let items: Vec<ListItem> = palette
        .items
        .iter()
        .map(|i| {
            let (label, _style) = if i.is_new {
                (
                    Span::styled(format!(" {}", i.label), theme.success_style()),
                    theme.success_style(),
                )
            } else if i.is_dir {
                (
                    Span::styled(format!(" {} {}", "📁", i.label), theme.accent_style()),
                    theme.accent_style(),
                )
            } else {
                (Span::raw(format!(" {}", i.label)), theme.text())
            };
            ListItem::new(Line::from(vec![label]))
        })
        .collect();
    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");
    let mut list_state = palette.list.clone();
    if list_state.selected().is_none() && !palette.items.is_empty() {
        list_state.select(Some(0));
    }
    StatefulWidget::render(list, list_area, buf, &mut list_state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::tui::ffi_bridge::{BridgeLayout, BridgePane, BridgeTab, FrameSnapshot};
    use std::collections::HashMap;

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

    fn render(snap: &FrameSnapshot, palette: Option<&PaletteState>, opts: RenderOpts) -> Buffer {
        // 测试用：从 outputs 构造简单的 screens（按行切分）
        let mut screens = HashMap::new();
        for (pid, data) in &snap.outputs {
            let text = String::from_utf8_lossy(data);
            let lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();
            screens.insert(*pid, lines);
        }
        let mut buf = Buffer::empty(Rect::new(0, 0, opts.cols.max(1), opts.rows.max(1)));
        render_frame(&mut buf, snap, &screens, palette, opts);
        buf
    }

    fn buf_to_string(buf: &Buffer) -> String {
        let mut s = String::new();
        let w = buf.area.width as usize;
        for (i, cell) in buf.content().iter().enumerate() {
            s.push_str(cell.symbol());
            if (i + 1) % w == 0 {
                s.push('\n');
            }
        }
        s
    }

    #[test]
    fn render_has_title_bar() {
        let buf = render(&snap_single_pane(), None, RenderOpts::default());
        let s = buf_to_string(&buf);
        assert!(s.contains("muxterm"));
        assert!(s.contains("connected"));
    }

    #[test]
    fn render_has_tab_bar() {
        let buf = render(&snap_single_pane(), None, RenderOpts::default());
        let s = buf_to_string(&buf);
        assert!(s.contains("1:") && s.contains("t1"));
    }

    #[test]
    fn render_has_status_bar() {
        let buf = render(&snap_single_pane(), None, RenderOpts::default());
        let s = buf_to_string(&buf);
        assert!(s.contains(&crate::platform::i18n::tr(
            crate::platform::i18n::Key::StatusConnected
        )));
        assert!(s.contains("Ctrl-Q"));
        assert!(s.contains("Alt+S"));
    }

    #[test]
    fn render_includes_pane_output() {
        let buf = render(&snap_two_panes(), None, RenderOpts::default());
        let s = buf_to_string(&buf);
        assert!(s.contains("hello") || s.contains("line1"));
    }

    #[test]
    fn render_two_panes_has_vertical_separator() {
        let buf = render(&snap_two_panes(), None, RenderOpts::default());
        let s = buf_to_string(&buf);
        assert!(s.contains("│"));
    }

    #[test]
    fn render_two_panes_shows_both_titles() {
        let buf = render(&snap_two_panes(), None, RenderOpts::default());
        let s = buf_to_string(&buf);
        assert!(s.contains("@1"), "应显示 @1 的 pane 标题: {s:?}");
        assert!(s.contains("@2"), "应显示 @2 的 pane 标题: {s:?}");
    }

    #[test]
    fn render_status_bar_shows_exited_status() {
        let mut snap = snap_single_pane();
        snap.status = "exited".into();
        let buf = render(&snap, None, RenderOpts::default());
        let s = buf_to_string(&buf);
        assert!(s.contains(&crate::platform::i18n::tr(
            crate::platform::i18n::Key::StatusExited
        )));
    }

    #[test]
    fn render_palette_overlay_shows_wizard() {
        let p = PaletteState::new();
        // Source step: local/ssh
        let buf = render(
            &snap_single_pane(),
            Some(&p),
            RenderOpts {
                palette_open: true,
                ..RenderOpts::default()
            },
        );
        let s = buf_to_string(&buf);
        assert!(s.contains("Connection"));
        assert!(s.contains("local") && s.contains("ssh"));
    }

    #[test]
    fn render_palette_action_step_shows_sessions() {
        let mut p = PaletteState::new();
        p.advance(); // source->action (local)
        p.set_items(vec![
            crate::platform::tui::palette::WizardItem::new_item(),
            crate::platform::tui::palette::WizardItem::plain("dev", "dev"),
            crate::platform::tui::palette::WizardItem::plain("prod", "prod"),
        ]);
        let buf = render(
            &snap_single_pane(),
            Some(&p),
            RenderOpts {
                palette_open: true,
                ..RenderOpts::default()
            },
        );
        let s = buf_to_string(&buf);
        assert!(s.contains("new") && s.contains("dev") && s.contains("prod"));
    }

    #[test]
    fn strip_ansi_removes_escape_codes() {
        assert_eq!(strip_ansi("plain"), "plain");
        assert_eq!(strip_ansi("\u{1b}[31mRED\u{1b}[39m"), "RED");
        assert_eq!(strip_ansi("a\u{1b}[38;5;242mb\u{1b}[39m c"), "ab c");
        assert_eq!(strip_ansi("\u{1b}[0mreset\u{1b}[0m"), "reset");
        assert!(strip_ansi("\u{1b}[31mRED\u{1b}[0m").contains("RED"));
        // 不应含任何 ESC 或 '[' 颜色码残留
        let out = strip_ansi("x\u{1b}[32mY\u{1b}[0m z");
        assert!(!out.contains('\u{1b}'));
        assert!(!out.contains("[3"));
    }

    #[test]
    fn render_palette_closed_does_not_overlay() {
        let buf = render(
            &snap_single_pane(),
            None,
            RenderOpts {
                palette_open: false,
                ..RenderOpts::default()
            },
        );
        let s = buf_to_string(&buf);
        assert!(!s.contains("Command Palette"));
    }

    // ── 真实 a.log 字节流保真渲染 ────────────────────────────
    //
    // 这些字节来自用户 SSH session 的 a.log（tmux %output 解码后），
    // 验证 TUI 渲染端不会把空格/UTF-8 弄丢或产生 replacement char。

    /// 去掉 ANSI CSI 颜色序列（\x1b[...m）和光标定位，便于对纯文本断言。
    /// 按 char 迭代，保留多字节 UTF-8（不逐字节拆开）。
    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' {
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for nxt in chars.by_ref() {
                        let b = nxt as u32;
                        if (0x40..=0x7e).contains(&b) {
                            break;
                        }
                    }
                    continue;
                }
                continue;
            }
            out.push(c);
        }
        out
    }

    fn snap_with_output(pane_id: u32, title: &str, out: Vec<u8>) -> FrameSnapshot {
        let mut outputs = HashMap::new();
        outputs.insert(pane_id, out);
        FrameSnapshot {
            tabs: vec![BridgeTab {
                id: 1,
                name: "t1".into(),
                is_active: true,
            }],
            panes: vec![BridgePane {
                id: pane_id,
                cols: 80,
                rows: 24,
                is_active: true,
                title: title.into(),
            }],
            layout: Some(BridgeLayout::Leaf { pane_id }),
            outputs,
            status: "connected".into(),
            active_tab: 1,
            active_pane: pane_id,
        }
    }

    /// 真实 `ls -la` 回显（a.log）：空格必须保留，且不产生 replacement char。
    #[test]
    fn render_real_ls_la_preserves_spaces_and_utf8() {
        let out: Vec<u8> = b"\x08l\x1b[39ms\x1b[39m \x1b[39m \x1b[39m \x1b[39m \x1b[39m \x1b[39m \x1b[39m \x1b[39m \x1b[39m \x1b[39m \x1b[39m \x1b[39m \x1b[39m \x1b[39m \x1b[39m \x1b[39m \x1b[39m \x1b[39m \x1b[39m \x1b[39m \x1b[19D\x0d\x0a".to_vec();
        let snap = snap_with_output(1, "zsh", out);
        let buf = render(&snap, None, RenderOpts::default());
        let joined = buf_to_string(&buf);
        assert!(!joined.contains('\u{fffd}'), "不应出现 replacement char");
        let clean = strip_ansi(&joined);
        assert!(clean.contains("ls "), "ls 后的空格必须保留: {clean:?}");
        assert!(
            clean.matches(' ').count() >= 19,
            "应保留约 19 个空格分隔: {clean:?}"
        );
    }

    /// 真实 codex 提示符（a.log）：UTF-8 的 ❯ 符号、zsh 路径、颜色码。
    #[test]
    fn render_real_codex_prompt_keeps_utf8_and_path() {
        let out: Vec<u8> = b"\x1b[0m\x1b[27m\x1b[24m\x1b[J\x1b[38;5;242mwlz\x1b[39m\x1b[38;5;242m@ryzen\x1b[39m \x1b[34m~/Developer/work/legion\x1b[39m \x1b[38;5;242mfeature/codescan-support-pprof\x1b[38;5;218m*\x1b[39m\x0d\x0a\x0d\x1b[35m\xe2\x9d\xaf\x1b[39m \x1b[K\x1b[?2004h\x0d\x0a".to_vec();
        let snap = snap_with_output(1, "zsh", out);
        let buf = render(
            &snap,
            None,
            RenderOpts {
                cols: 200,
                rows: 24,
                ..RenderOpts::default()
            },
        );
        let joined = buf_to_string(&buf);
        assert!(!joined.contains('\u{fffd}'), "不应出现 replacement char");
        let clean = strip_ansi(&joined);
        assert!(clean.contains("wlz@ryzen"), "应保留 user@host: {clean:?}");
        assert!(
            clean.contains("~/Developer/work/legion"),
            "应保留路径: {clean:?}"
        );
    }

    /// 真实 htop 片段（a.log，含 \x0f SO 字符集切换 + 光标定位）。
    #[test]
    fn render_real_htop_no_replacement_char() {
        let out: Vec<u8> = b"\x1b[2;8H\x1b[32m|\x1b[31m|\x1b[0;1m\x0f\x1b[90m 6.4\x1b[21G    0.0\x1b[34G\x1b[0m\x0f\x1b[31m|\x0f\x1b[90m\xe7\xbc\x96\xe8\xaf\x91\xe6\xb5\x8b\xe8\xaf\x95\x1b[39m\x0d\x0a".to_vec();
        let snap = snap_with_output(1, "htop", out);
        let buf = render(
            &snap,
            None,
            RenderOpts {
                cols: 200,
                rows: 24,
                ..RenderOpts::default()
            },
        );
        let joined = buf_to_string(&buf);
        assert!(
            !joined.contains('\u{fffd}'),
            "htop 不应出现 replacement char: {joined:?}"
        );
        let clean = strip_ansi(&joined);
        assert!(
            clean.contains("编 译 测 试"),
            "htop 中的 UTF-8 中文应保留: {clean:?}"
        );
    }
}
