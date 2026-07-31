//! TUI 事件循环入口（经 FFI）。
//!
//! `run()` 进入 crossterm raw mode + alternate screen，经 `CoreBridge` 调用
//! `muxterm_*` C ABI（不直接持有 TerminalModel / Backend）。
//! 轮询键盘 → execute / send_input；轮询 poll_events → 重绘。
//! Ctrl-Q 退出，Alt+T 新建 tab，Alt+数字切 tab。
//!
//! 不做单元测试（需要真实终端 I/O）；渲染逻辑在 `render.rs` 单测，
//! 集成测试在 `tests/` 目录（spawn TUI 进程 + tmux capture）。

use std::io::{stdout, Write};
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{poll, read, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, size, Clear, ClearType, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use crossterm::{execute, queue};

use crate::core::protocol::terminal::input::{encode, ArrowDir, KeyEvent as MuxKeyEvent};
use crate::platform::tui::ffi_bridge::{tasks, CoreBridge, FrameSnapshot};
use crate::platform::tui::render::{render_frame, RenderOpts};

/// TUI 启动参数。
///
/// - `socket`：CLI `-L/--socket`；有值 → tmux 模式
/// - `session`：CLI `-s/--session`；local 模式连 daemon，tmux 模式指定 attach 目标
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
    let _ = execute!(out, Show, LeaveAlternateScreen);
    result
}

fn run_inner(out: &mut impl Write, opts: TuiOpts) -> Result<()> {
    execute!(out, EnterAlternateScreen, Hide, Clear(ClearType::All))
        .context("enter alternate screen")?;

    let (backend_type, socket, session) = resolve_backend(&opts);
    let mut bridge = CoreBridge::new(backend_type, socket.as_deref(), session.as_deref())
        .context("CoreBridge::new")?;

    // 连接后给查询响应一点时间，再做一次 poll 让初始状态到达
    std::thread::sleep(Duration::from_millis(300));
    let _ = bridge.poll_events();

    let (cols, rows) = size().unwrap_or((80, 24));
    let mut opts_render = RenderOpts {
        cols: cols.max(20),
        rows: rows.max(8),
        max_output_lines: 20,
    };

    redraw(out, &bridge.snapshot(), opts_render)?;

    loop {
        // drain 状态变更（poll_events 内部走 model.refresh）
        let events = bridge.poll_events();
        if !events.is_empty() {
            if let Ok((c, r)) = size() {
                opts_render.cols = c.max(20);
                opts_render.rows = r.max(8);
            }
            redraw(out, &bridge.snapshot(), opts_render)?;
        }

        if poll(Duration::from_millis(100)).context("poll event")? {
            let ev = read().context("read event")?;
            match ev {
                Event::Key(key) => {
                    if is_quit(&key) {
                        break;
                    }
                    let snap = bridge.snapshot();
                    if handle_key(&mut bridge, &key, &snap) {
                        redraw(out, &bridge.snapshot(), opts_render)?;
                    }
                }
                Event::Resize(c, r) => {
                    opts_render.cols = c.max(20);
                    opts_render.rows = r.max(8);
                    redraw(out, &bridge.snapshot(), opts_render)?;
                }
                _ => {}
            }
        }
    }

    // Drop 会 shutdown + free
    drop(bridge);
    Ok(())
}

/// 解析 backend 类型与参数。
///
/// | 参数 | FFI backend |
/// |------|-------------|
/// | `-L sock`（可选 `-s name`） | `"tmux"` |
/// | `-s name`（无 `-L`） | `"daemon"` |
/// | 都无 | `"local"` |
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

/// 重绘一帧。
fn redraw(out: &mut impl Write, snap: &FrameSnapshot, opts: RenderOpts) -> Result<()> {
    queue!(out, crossterm::cursor::MoveTo(0, 0), Clear(ClearType::All)).context("clear")?;
    let lines = render_frame(snap, opts);
    for (i, line) in lines.iter().enumerate() {
        queue!(out, crossterm::cursor::MoveTo(0, i as u16)).context("move cursor")?;
        out.write_all(line.as_bytes()).context("write line")?;
    }
    out.flush().context("flush")?;
    Ok(())
}

/// Ctrl-Q / Ctrl-D / Ctrl-C 退出。
fn is_quit(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(
            key.code,
            KeyCode::Char('q') | KeyCode::Char('d') | KeyCode::Char('c')
        )
}

/// 处理按键：结构命令走 execute，字符输入走 encode + send_input。
/// 返回是否触发了 UI 动作（需要重绘）。
fn handle_key(bridge: &mut CoreBridge, key: &KeyEvent, snap: &FrameSnapshot) -> bool {
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
                _ => {
                    let Some(pane) = target else {
                        return false;
                    };
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
            let Some(pane) = target else {
                return false;
            };
            let bytes = encode(&MuxKeyEvent::Ctrl(c));
            let _ = bridge.send_input(pane, &bytes);
            return true;
        }
    }

    let Some(pane) = target else {
        return false;
    };
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
