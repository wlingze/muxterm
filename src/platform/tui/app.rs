//! TUI 事件循环入口（经 FFI + ratatui）。
//!
//! `run()` 进入 crossterm raw mode + alternate screen，经 `CoreBridge` 调用
//! `muxterm_*` C ABI（不直接持有 TerminalModel / Backend）。
//! 轮询键盘 → execute / send_input；轮询 poll_events → 重绘。
//! Ctrl-Q 退出，Alt+T 新建 tab，Alt+S / Alt+V 分割 pane，Alt+P 连接向导。
//!
//! 用 ratatui 渲染，跨平台（Windows/Linux/macOS 均有 crossterm 后端）。

use std::io::stdout;
use std::path::PathBuf;
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
use crate::platform::tui::palette::{
    ConnectAction, ConnectSource, PaletteState, WizardItem, WizardStep,
};
use crate::platform::tui::render::{render_frame, RenderOpts};
use crate::platform::tui::terminal::TerminalManager;

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
    palette.socket = opts.socket.clone();
    let mut palette_open = false;
    // 终端渲染状态：每个 pane 一个 TerminalState
    let mut term_mgr = TerminalManager::new();

    // 首帧：立即渲染一次（不依赖事件）
    let snap = bridge.snapshot();
    sync_term_sizes(&mut term_mgr, &snap);
    draw(&mut terminal, &snap, &term_mgr, &palette, palette_open)?;

    // 每 50ms 事件轮询；仅当有实际状态变更时（事件非空 / 按键 / resize）才重绘，
    // 避免空轮询也做昂贵 snapshot+draw（拉取全部 pane 输出 + 全屏渲染）。
    loop {
        let events = bridge.poll_events();
        let mut needs_redraw = !events.is_empty();

        if poll(Duration::from_millis(50)).context("poll event")? {
            let ev = read().context("read event")?;
            match ev {
                Event::Key(key) => {
                    if palette_open {
                        if handle_palette_key(&mut palette, &key, &mut bridge, &mut palette_open)? {
                            needs_redraw = true;
                        }
                    } else if is_quit(&key) {
                        break;
                    } else {
                        let snap = bridge.snapshot();
                        if handle_key(&mut bridge, &key, &snap, &mut palette_open, &mut palette) {
                            needs_redraw = true;
                        }
                    }
                }
                Event::Resize(_, _) => needs_redraw = true,
                _ => {}
            }
        }

        // 仅在确有变化时重绘
        if needs_redraw {
            let snap = bridge.snapshot();
            sync_term_sizes(&mut term_mgr, &snap);
            draw(&mut terminal, &snap, &term_mgr, &palette, palette_open)?;
        }
    }

    // tmux/daemon 用显式 detach；local shell 没有 tmux client，直接由 Drop
    // 做普通 shutdown。FFI detach 失败时仍让 Drop 兜底，不能 panic。
    if matches!(backend_type, "tmux" | "daemon") {
        let _ = bridge.detach();
    }
    drop(bridge);
    Ok(())
}

/// 根据快照里的 pane 尺寸同步/补齐各 pane 终端，并清理已删除 pane。
///
/// 用每个 pane 的**累计输出**做增量同步（GTK 同款 delta 方案）：每次只 feed
/// 比上次已 feed 长度更新的部分，覆盖首屏播种 + 持续增量 + 输出截断重置。
fn sync_term_sizes(term_mgr: &mut TerminalManager, snap: &FrameSnapshot) {
    let mut ids: Vec<u32> = Vec::new();
    for p in &snap.panes {
        ids.push(p.id);
        let cols = p.cols.max(1);
        let rows = p.rows.max(1);
        // 每次用累计输出做增量同步（GTK 同款 delta 方案），
        // 覆盖首屏播种 + 持续增量 + 输出截断重置。
        let full = snap.outputs.get(&p.id).cloned().unwrap_or_default();
        term_mgr.sync_output(p.id, cols, rows, &full);
    }
    term_mgr.retain(&ids);
}

fn draw<W: std::io::Write>(
    terminal: &mut Terminal<CrosstermBackend<&mut W>>,
    snap: &FrameSnapshot,
    term_mgr: &TerminalManager,
    palette: &PaletteState,
    palette_open: bool,
) -> Result<()> {
    // 收集每个 pane 的带样式屏幕网格（含光标行，用于视口定位）
    let mut screens: std::collections::HashMap<
        u32,
        Vec<Vec<crate::core::protocol::terminal::emulate::Cell>>,
    > = std::collections::HashMap::new();
    let mut cursors = std::collections::HashMap::new();
    for p in &snap.panes {
        if let Some((sc, cur)) = term_mgr.styled_screen_with_cursor(p.id) {
            cursors.insert(p.id, cur);
            screens.insert(p.id, sc);
        }
    }
    terminal.draw(|f| {
        let area = f.area();
        let opts = RenderOpts {
            cols: area.width,
            rows: area.height,
            max_output_lines: 20,
            palette_open,
        };
        let buf = f.buffer_mut();
        render_frame(buf, snap, &screens, &cursors, Some(palette), opts);
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

/// 向导按键。返回 `Ok(true)` 表示需要重绘。
///
/// 在进入某一步时自动加载对应的数据（hosts / sessions / 目录），
/// 完成后触发重连（重建 `CoreBridge`）。
fn handle_palette_key(
    palette: &mut PaletteState,
    key: &KeyEvent,
    bridge: &mut CoreBridge,
    palette_open: &mut bool,
) -> Result<bool> {
    match key.code {
        KeyCode::Esc => {
            *palette_open = false;
            Ok(true)
        }
        KeyCode::Enter => {
            match palette.advance() {
                Some(action) => {
                    // 向导完成 → 重连（用当前 socket）
                    let sock = palette.socket.clone();
                    reconnect(bridge, &action, sock.as_deref())?;
                    *palette_open = false;
                    Ok(true)
                }
                None => {
                    // 进入下一步：加载对应数据
                    load_step_data(palette);
                    Ok(true)
                }
            }
        }
        KeyCode::Up => {
            palette.list.select_previous();
            Ok(true)
        }
        KeyCode::Down => {
            palette.list.select_next();
            Ok(true)
        }
        KeyCode::Backspace => {
            if !palette.query.is_empty() {
                // 有过滤输入时，先删过滤字符
                palette.pop_query();
            } else {
                // 返回上一步
                palette.back();
                load_step_data(palette);
            }
            Ok(true)
        }
        KeyCode::Char(' ') => {
            // 空格：目录 step 进入子目录
            if palette.step == WizardStep::Directory {
                if let Some(item) = palette.selected() {
                    if item.is_dir {
                        let new_dir = join_dir(&item.value, &palette.dir);
                        palette.dir = Some(new_dir);
                        load_step_data(palette);
                    }
                }
            } else {
                // 其它 step：空格作为过滤字符输入
                palette.push_query(' ');
            }
            Ok(true)
        }
        KeyCode::Char(c) => {
            // 普通字符：过滤输入（opencode 风格）
            if !c.is_control() {
                palette.push_query(c);
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// 按当前 step 加载数据到 palette。
fn load_step_data(palette: &mut PaletteState) {
    // 进入新 step 时重置过滤输入，避免旧查询污染
    palette.query.clear();
    match palette.step {
        WizardStep::Source => {
            palette.set_items(vec![
                WizardItem::plain("local（本机 tmux）", "local"),
                WizardItem::plain("ssh（远程机器）", "ssh"),
            ]);
        }
        WizardStep::Host => {
            let hosts = crate::core::discovery::list_local_ssh_hosts(None);
            let items: Vec<WizardItem> = if hosts.is_empty() {
                vec![WizardItem::plain("（未配置 ~/.ssh/config Host）", "")]
            } else {
                hosts
                    .into_iter()
                    .map(|h| WizardItem::plain(&h, &h))
                    .collect()
            };
            palette.set_items(items);
        }
        WizardStep::Action => {
            // 顶部默认 new + 已存在 session 列表
            let sessions = match palette.source {
                ConnectSource::Local => {
                    crate::core::discovery::list_local_tmux_sessions(palette.socket.as_deref())
                }
                ConnectSource::Ssh => {
                    let host = palette.host.clone().unwrap_or_default();
                    let timeout = Duration::from_secs(5);
                    let ssh_config = std::env::var("MUXTERM_SSH_CONFIG_PATH").ok();
                    crate::core::discovery::list_ssh_tmux_sessions(
                        &host,
                        ssh_config.as_deref(),
                        None,
                        timeout,
                    )
                    .unwrap_or_default()
                }
            };
            let mut items = vec![WizardItem::new_item()];
            for s in &sessions {
                let label = if s.attached {
                    format!("{} (attached, {} win)", s.name, s.windows)
                } else {
                    format!("{} ({} win)", s.name, s.windows)
                };
                items.push(WizardItem::plain(label, &s.name));
            }
            palette.set_items(items);
        }
        WizardStep::Directory => {
            let dir = palette.dir.clone().unwrap_or_else(|| {
                std::env::current_dir()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| ".".into())
            });
            palette.dir = Some(dir.clone());
            let entries = crate::core::discovery::list_local_dir(&PathBuf::from(&dir));
            let mut items = vec![WizardItem::dir("..（返回上级）", "..")];
            for e in entries {
                if e.is_dir {
                    let full = join_dir(&e.name, &palette.dir);
                    items.push(WizardItem::dir(&e.name, &full));
                }
            }
            palette.set_items(items);
        }
    }
}

/// 连接动作 → 重建 CoreBridge。
fn reconnect(bridge: &mut CoreBridge, action: &ConnectAction, socket: Option<&str>) -> Result<()> {
    let sock = socket.map(|s| s.to_string());
    let (backend_type, socket, session, ssh_alias, start_dir) = match action {
        ConnectAction::Attach {
            source,
            host,
            session,
        } => match source {
            ConnectSource::Local => ("tmux", sock.clone(), Some(session.clone()), None, None),
            ConnectSource::Ssh => (
                "tmux-ssh",
                sock.clone(),
                Some(session.clone()),
                host.clone(),
                None,
            ),
        },
        ConnectAction::New {
            source,
            host,
            directory,
        } => match source {
            ConnectSource::Local => ("tmux", sock.clone(), None, None, directory.clone()),
            ConnectSource::Ssh => (
                "tmux-ssh",
                sock.clone(),
                None,
                host.clone(),
                directory.clone(),
            ),
        },
    };

    let new_bridge = CoreBridge::new_connect(
        backend_type,
        socket.as_deref(),
        session.as_deref(),
        ssh_alias.as_deref(),
        start_dir.as_deref(),
    )?;
    *bridge = new_bridge;
    // 等初始状态
    std::thread::sleep(Duration::from_millis(300));
    let _ = bridge.poll_events();
    Ok(())
}

/// 目录 step 拼接路径（支持 .. / 相对）。
fn join_dir(name: &str, base: &Option<String>) -> String {
    let base = base.clone().unwrap_or_else(|| ".".to_string());
    if name == ".." {
        // 返回上级
        let p = PathBuf::from(&base);
        return p
            .parent()
            .map(|x| x.to_string_lossy().into_owned())
            .unwrap_or_else(|| base.clone());
    }
    let p = PathBuf::from(&base).join(name);
    p.to_string_lossy().into_owned()
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
                    load_step_data(palette);
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
