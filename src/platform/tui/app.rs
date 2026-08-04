//! TUI 事件循环入口（经 FFI + ratatui）。
//!
//! `run()` 进入 crossterm raw mode + alternate screen，经 `CoreBridge` 调用
//! `muxterm_*` C ABI（不直接持有 TerminalModel / Backend）。
//! 轮询键盘 → execute / send_input；轮询 poll_events → 重绘。
//! Ctrl-Q 退出，Alt+T 新建 tab，Alt+S / Alt+V 分割 pane，Alt+P 命令面板。
//!
//! 用 ratatui 渲染，跨平台（Windows/Linux/macOS 均有 crossterm 后端）。

use std::io::stdout;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{poll, read, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::core::protocol::terminal::input::{encode, ArrowDir, KeyEvent as MuxKeyEvent};
use crate::platform::tui::ffi_bridge::{tasks, CoreBridge, FrameSnapshot};
use crate::platform::tui::palette::PaletteState;
use crate::platform::tui::render::{render_frame, RenderOpts};

/// TUI 启动参数。
pub struct TuiOpts {
    pub socket: Option<String>,
    pub session: Option<String>,
}

/// 启动 TUI 前端。
pub fn run(opts: TuiOpts) -> Result<()> {
    enable_raw_mode().context("enable raw mode")?;
    let mut out = stdout();
    let result = run_inner(&mut out, opts);
    // 恢复终端（无论如何都执行）
    let _ = disable_raw_mode();
    let _ = execute!(out, LeaveAlternateScreen);
    result
}

fn run_inner<W: std::io::Write>(out: &mut W, opts: TuiOpts) -> Result<()> {
    execute!(out, EnterAlternateScreen).context("enter alternate screen")?;

    let (backend_type, socket, session) = resolve_backend(&opts);
    let mut bridge = CoreBridge::new(backend_type, socket.as_deref(), session.as_deref())
        .context("CoreBridge::new")?;

    // 连接后给查询响应一点时间，再做一次 poll 让初始状态到达
    std::thread::sleep(Duration::from_millis(300));
    let _ = bridge.poll_events();

    let backend = CrosstermBackend::new(&mut *out);
    let mut terminal = Terminal::new(backend).context("ratatui Terminal::new")?;

    let mut palette = PaletteState::new();
    palette.refresh();
    let mut palette_open = false;

    loop {
        // drain 状态变更
        let events = bridge.poll_events();
        if !events.is_empty() {
            draw(&mut terminal, &bridge.snapshot(), &palette, palette_open)?;
        }

        if poll(Duration::from_millis(100)).context("poll event")? {
            let ev = read().context("read event")?;
            match ev {
                Event::Key(key) => {
                    if palette_open {
                        if handle_palette_key(&mut palette, &key, &mut bridge, &mut palette_open) {
                            draw(&mut terminal, &bridge.snapshot(), &palette, palette_open)?;
                        }
                    } else if is_quit(&key) {
                        break;
                    } else {
                        let snap = bridge.snapshot();
                        if handle_key(&mut bridge, &key, &snap, &mut palette_open, &mut palette) {
                            draw(&mut terminal, &bridge.snapshot(), &palette, palette_open)?;
                        }
                    }
                }
                Event::Resize(_, _) => {
                    draw(&mut terminal, &bridge.snapshot(), &palette, palette_open)?;
                }
                _ => {}
            }
        }
    }

    drop(bridge);
    Ok(())
}

fn draw<W: std::io::Write>(
    terminal: &mut Terminal<CrosstermBackend<&mut W>>,
    snap: &FrameSnapshot,
    palette: &PaletteState,
    palette_open: bool,
) -> Result<()> {
    terminal.draw(|f| {
        let area = f.area();
        let opts = RenderOpts {
            cols: area.width,
            rows: area.height,
            max_output_lines: 20,
            palette_open,
        };
        let buf = f.buffer_mut();
        render_frame(buf, snap, Some(palette), opts);
    })?;
    Ok(())
}

/// 解析 backend 类型与参数。
fn resolve_backend(opts: &TuiOpts) -> (&'static str, Option<String>, Option<String>) {
    match (opts.socket.as_deref(), opts.session.as_deref()) {
        (Some(sock), session) => {
            let sock = sock.trim();
            let sock_opt = if sock.is_empty() {
                None
            } else {
                Some(sock.to_string())
            };
            let session = match session {
                Some(name) => Some(name.to_string()),
                None => find_existing_tmux_session(sock_opt.as_deref()),
            };
            ("tmux", sock_opt, session)
        }
        (None, Some(name)) => ("daemon", None, Some(name.to_string())),
        (None, None) => ("local", None, None),
    }
}

/// 用 `tmux -L <socket> list-sessions` 查找已有的 session 名。
fn find_existing_tmux_session(socket: Option<&str>) -> Option<String> {
    let mut cmd = std::process::Command::new("tmux");
    if let Some(s) = socket {
        cmd.args(["-L", s]);
    }
    cmd.args(["list-sessions", "-F", "#{session_name}"]);
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let output = cmd.output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().next().map(|s| s.trim().to_string())
}

/// Ctrl-Q / Ctrl-D / Ctrl-C 退出。
fn is_quit(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(
            key.code,
            KeyCode::Char('q') | KeyCode::Char('d') | KeyCode::Char('c')
        )
}

/// 命令面板按键：返回是否触发 UI 动作（需要重绘）。
fn handle_palette_key(
    palette: &mut PaletteState,
    key: &KeyEvent,
    bridge: &mut CoreBridge,
    palette_open: &mut bool,
) -> bool {
    match key.code {
        KeyCode::Esc => {
            *palette_open = false;
            true
        }
        KeyCode::Enter => {
            if let Some(cmd) = palette.selected() {
                let _ = run_palette_command(bridge, cmd.id);
                *palette_open = false;
                return true;
            }
            false
        }
        KeyCode::Up => {
            palette.list.select_previous();
            true
        }
        KeyCode::Down => {
            palette.list.select_next();
            true
        }
        KeyCode::Char(c) => {
            palette.input.push(c);
            palette.refresh();
            true
        }
        KeyCode::Backspace => {
            palette.input.pop();
            palette.refresh();
            true
        }
        _ => false,
    }
}

/// 执行面板命令。返回是否成功执行。
fn run_palette_command(bridge: &CoreBridge, id: &str) -> i32 {
    match id {
        "new_tab" => bridge.execute(tasks::new_tab()),
        "new_pane_h" => bridge.execute(tasks::split_h(0)),
        "new_pane_v" => bridge.execute(tasks::split_v(0)),
        "close_pane" => bridge.execute(tasks::close_pane(0)),
        "close_tab" => bridge.execute(tasks::close_tab(0)),
        "switch_pane_next" => bridge.execute(tasks::next_pane()),
        "switch_pane_prev" => bridge.execute(tasks::prev_pane()),
        _ => {
            // 其余命令（session/ssh/settings/cli）在本阶段是占位，直接返回成功（不崩溃）。
            0
        }
    }
}

/// 处理按键：结构命令走 execute，字符输入走 encode + send_input。
fn handle_key(
    bridge: &mut CoreBridge,
    key: &KeyEvent,
    snap: &FrameSnapshot,
    palette_open: &mut bool,
    palette: &mut PaletteState,
) -> bool {
    let target = if snap.active_pane != 0 {
        Some(snap.active_pane)
    } else {
        snap.panes.first().map(|p| p.id)
    };

    // Alt 组合
    if key.modifiers.contains(KeyModifiers::ALT) {
        if let KeyCode::Char(c) = key.code {
            let lower = c.to_ascii_lowercase();
            match lower {
                't' => {
                    let _ = bridge.execute(tasks::new_tab());
                    return true;
                }
                'p' => {
                    palette.refresh();
                    *palette_open = true;
                    return true;
                }
                '1' | '2' | '3' | '4' | '5' | '6' | '7' | '8' | '9' => {
                    let n = lower.to_digit(10).unwrap() as usize;
                    if n <= snap.tabs.len() {
                        let _ = bridge.execute(tasks::switch_tab(snap.tabs[n - 1].id));
                        return true;
                    }
                    return false;
                }
                'w' => {
                    let tab = snap.tabs.iter().find(|t| t.is_active).or(snap.tabs.first());
                    if let Some(t) = tab {
                        let _ = bridge.execute(tasks::close_tab(t.id));
                        return true;
                    }
                    return false;
                }
                's' => {
                    let _ = bridge.execute(tasks::split_h(target.unwrap_or(0)));
                    return true;
                }
                'v' => {
                    let _ = bridge.execute(tasks::split_v(target.unwrap_or(0)));
                    return true;
                }
                '[' => {
                    let _ = bridge.execute(tasks::prev_pane());
                    return true;
                }
                ']' => {
                    let _ = bridge.execute(tasks::next_pane());
                    return true;
                }
                _ => {
                    let Some(pane) = target else { return false };
                    let bytes = encode(&MuxKeyEvent::Alt(c));
                    let _ = bridge.send_input(pane, &bytes);
                    return true;
                }
            }
        }
    }

    // Ctrl 组合（Ctrl-Q/D/C 已在 is_quit 处理）
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        if let KeyCode::Char(c) = key.code {
            let Some(pane) = target else { return false };
            let bytes = encode(&MuxKeyEvent::Ctrl(c));
            let _ = bridge.send_input(pane, &bytes);
            return true;
        }
    }

    let Some(pane) = target else { return false };
    let mux_key = match key.code {
        KeyCode::Char(c) => MuxKeyEvent::Char(c),
        KeyCode::Enter => MuxKeyEvent::Enter,
        KeyCode::Tab => MuxKeyEvent::Tab,
        KeyCode::BackTab => MuxKeyEvent::Tab,
        KeyCode::Backspace => MuxKeyEvent::Backspace,
        KeyCode::Esc => MuxKeyEvent::Escape,
        KeyCode::Up => MuxKeyEvent::Arrow(ArrowDir::Up),
        KeyCode::Down => MuxKeyEvent::Arrow(ArrowDir::Down),
        KeyCode::Left => MuxKeyEvent::Arrow(ArrowDir::Left),
        KeyCode::Right => MuxKeyEvent::Arrow(ArrowDir::Right),
        KeyCode::F(n) => MuxKeyEvent::Function(n),
        _ => return false,
    };
    let bytes = encode(&mux_key);
    let _ = bridge.send_input(pane, &bytes);
    true
}
