//! TUI 事件循环入口（经 FFI + ratatui）。
//!
//! `run()` 进入 crossterm raw mode + alternate screen，经共享 `FfiClient` 调用
//! `muxterm_*` C ABI（不直接持有 TerminalModel / Runtime）。
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

use crate::frontend::command_queue::{ClientCommand, CommandQueue};
use crate::frontend::event_pump::EventPump;
use crate::frontend::ffi_client::{ClientTask, FfiClient};
use crate::frontend::mirror::should_forward_parser_response;
use crate::frontend::tui::emulate::Cell;
use crate::frontend::tui::input::{encode, ArrowDir, KeyEvent as MuxKeyEvent};
use crate::frontend::tui::model::{status_label, FrameSnapshot};
use crate::frontend::tui::palette::{
    ConnectAction, ConnectSource, PaletteState, WizardItem, WizardStep,
};
use crate::frontend::tui::render::{render_frame, RenderOpts};
use crate::frontend::tui::scene::{drain_store_into_scenes, SceneStack};
use crate::frontend::tui::terminal::TerminalManager;
use crate::frontend::view_store::ViewStore;

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

    let (mut runtime_type, socket, session) = resolve_runtime(&opts);
    let mut bridge = EventPump::new(
        FfiClient::new(runtime_type, socket.as_deref(), session.as_deref())
            .context("FfiClient::new")?,
    );
    let mut commands = CommandQueue::default();
    let mut store = ViewStore::default();
    let mut scenes = SceneStack::new(is_direct_pty_terminal(runtime_type));

    // 连接后给查询响应一点时间，再做一次 poll 让初始状态到达
    std::thread::sleep(Duration::from_millis(300));
    let _ = bridge.sync_view_store(&mut store);
    let events = bridge.poll_into_with_events(&mut store);
    drain_store_into_scenes(&mut store, &mut scenes);
    for event in &events {
        scenes.apply_workspace_event(event);
    }
    if let Some(id) = store.active_workspace_id() {
        scenes.show(id);
    }

    let terminal_backend = CrosstermBackend::new(&mut *out);
    let mut terminal = Terminal::new(terminal_backend).context("ratatui Terminal::new")?;

    let mut palette = PaletteState::new();
    palette.socket = opts.socket.clone();
    let mut palette_open = false;

    // 首帧：从常驻 Scene + ViewStore 渲染，不向 Core 拉 pane 输出。
    let snap = snapshot_from_store(&store, &scenes, bridge.client().status_code());
    if let Some(scene) = scenes.visible_mut() {
        let replies = sync_terminals(&mut scene.terminals, &snap);
        maybe_send_replies(&mut commands, runtime_type, replies);
    }
    let _ = commands.flush(bridge.client());
    draw_visible(&mut terminal, &snap, &scenes, &palette, palette_open)?;

    // 每 50ms 事件轮询；仅当有实际状态变更时（事件非空 / 按键 / resize）才重绘，
    // 避免空轮询也做昂贵 snapshot+draw（拉取全部 pane 输出 + 全屏渲染）。
    loop {
        let events = bridge.poll_into_with_events(&mut store);
        drain_store_into_scenes(&mut store, &mut scenes);
        for event in &events {
            scenes.apply_workspace_event(event);
        }
        if let Some(id) = store.active_workspace_id() {
            scenes.show(id);
        }
        let mut needs_redraw = !events.is_empty();

        if poll(Duration::from_millis(50)).context("poll event")? {
            let ev = read().context("read event")?;
            match ev {
                Event::Key(key) => {
                    if palette_open {
                        if handle_palette_key(
                            &mut palette,
                            &key,
                            &mut bridge,
                            &mut runtime_type,
                            &mut store,
                            &mut scenes,
                            &mut palette_open,
                        )? {
                            needs_redraw = true;
                        }
                    } else if is_quit(&key) {
                        break;
                    } else {
                        let snap =
                            snapshot_from_store(&store, &scenes, bridge.client().status_code());
                        if handle_key(
                            &mut commands,
                            &key,
                            &snap,
                            &mut scenes,
                            &mut palette_open,
                            &mut palette,
                        ) {
                            needs_redraw = true;
                        }
                    }
                }
                Event::Resize(c, r) => {
                    // 渲染器在内容区外占用左右边框 2 列、顶部/状态/边框等 8 行，
                    // tmux client 尺寸必须等于内容区，否则 htop/codex 按全屏
                    // 重绘后被裁切/错位。
                    let cols = c.saturating_sub(2).max(20);
                    let rows = r.saturating_sub(8).max(8);
                    if runtime_type != "local" {
                        commands.push(ClientCommand::Resize {
                            workspace_id: None,
                            pane_id: None,
                            cols,
                            rows,
                        });
                    } else {
                        let snap =
                            snapshot_from_store(&store, &scenes, bridge.client().status_code());
                        if let Some(pane) = active_pane_id(&snap) {
                            commands.push(ClientCommand::Resize {
                                workspace_id: None,
                                pane_id: Some(pane),
                                cols,
                                rows,
                            });
                        }
                    }
                    needs_redraw = true;
                }
                _ => {}
            }
        }

        let _ = commands.flush(bridge.client());

        // 仅在确有变化时重绘
        if needs_redraw {
            let snap = snapshot_from_store(&store, &scenes, bridge.client().status_code());
            if let Some(scene) = scenes.visible_mut() {
                let replies = sync_terminals(&mut scene.terminals, &snap);
                maybe_send_replies(&mut commands, runtime_type, replies);
            }
            let _ = commands.flush(bridge.client());
            draw_visible(&mut terminal, &snap, &scenes, &palette, palette_open)?;
        }
    }

    // tmux/daemon 用显式 detach；local shell 没有 tmux client，直接由 Drop
    // 做普通 shutdown。FFI detach 失败时仍让 Drop 兜底，不能 panic。
    if matches!(runtime_type, "tmux" | "daemon") {
        let _ = bridge.client().detach();
    }
    drop(bridge);
    Ok(())
}

fn snapshot_from_store(store: &ViewStore, scenes: &SceneStack, status_code: u32) -> FrameSnapshot {
    let Some(id) = scenes.visible_id().or_else(|| store.active_workspace_id()) else {
        return FrameSnapshot {
            status: status_label(status_code).to_string(),
            ..FrameSnapshot::default()
        };
    };
    let Some(view) = store.workspace(id) else {
        return FrameSnapshot {
            status: status_label(status_code).to_string(),
            ..FrameSnapshot::default()
        };
    };
    let visible_tab = scenes.visible().and_then(|scene| scene.visible_tab(view));
    FrameSnapshot::from_workspace_view(view, status_label(status_code), visible_tab)
}

/// 根据快照同步各 pane 终端：
/// - 已有状态的 pane 只调整尺寸（增量一律走 render 事件）；
/// - 首次见到的 pane 建空 VT，等 mailbox 播种；
/// - **不**按激活 tab 清理状态（切 tab 不丢屏幕），也不重放截断的历史。
fn sync_terminals(term_mgr: &mut TerminalManager, snap: &FrameSnapshot) -> Vec<(u32, Vec<u8>)> {
    for p in &snap.panes {
        let cols = p.cols.max(1);
        let rows = p.rows.max(1);
        if term_mgr.has(p.id) {
            term_mgr.resize_pane(p.id, cols, rows);
        } else if let Some(full) = snap.outputs.get(&p.id) {
            term_mgr.seed(p.id, cols, rows, full);
        } else {
            term_mgr.ensure(p.id, cols, rows);
        }
    }
    term_mgr.drain_replies()
}

/// 把终端生成的查询应答（OSC 10/11、CSI DA 等）原样写回 shell/pty。
///
/// 仅本地 / daemon 后端需要（前端是该 PTY 的终端模拟器，写回 pty 是正确行为）。
/// tmux 控制模式下应答经 `send-keys -l` 回写会被 pane 回显并执行，造成
/// `git lg` 的 `10;rgb:...` / `65;...c` 泄漏，因此必须丢弃。
fn maybe_send_replies(
    commands: &mut CommandQueue,
    runtime_type: &str,
    replies: Vec<(u32, Vec<u8>)>,
) {
    let is_tmux_mirror = !is_direct_pty_terminal(runtime_type);
    if should_forward_parser_response(true, is_tmux_mirror) {
        send_replies(commands, replies);
    }
}

fn send_replies(commands: &mut CommandQueue, replies: Vec<(u32, Vec<u8>)>) {
    for (pane_id, data) in replies {
        commands.push(ClientCommand::Input {
            workspace_id: None,
            pane_id,
            data,
            quiet: false,
        });
    }
}

/// 前端是否为 pane PTY 的直接终端模拟器。
///
/// 仅 `local` 模式是：查询应答写回 pty 是正确行为。tmux 控制模式
/// （`tmux` / `tmux-ssh`）以及 daemon 代理（daemon 可能代理 tmux，client
/// 侧无法分辨）都不是，解析器应答必须丢弃，否则经 send-keys 注入会泄漏成
/// shell 字面命令。
fn is_direct_pty_terminal(runtime: &str) -> bool {
    runtime == "local"
}

/// 当前激活 pane；快照里没有标记时退回第一个。
fn active_pane_id(snap: &FrameSnapshot) -> Option<u32> {
    if snap.active_pane != 0 {
        Some(snap.active_pane)
    } else {
        snap.panes.first().map(|p| p.id)
    }
}

fn draw_visible<W: std::io::Write>(
    terminal: &mut Terminal<CrosstermBackend<&mut W>>,
    snap: &FrameSnapshot,
    scenes: &SceneStack,
    palette: &PaletteState,
    palette_open: bool,
) -> Result<()> {
    let empty = TerminalManager::new();
    let term_mgr = scenes
        .visible()
        .map(|scene| &scene.terminals)
        .unwrap_or(&empty);
    draw(terminal, snap, term_mgr, palette, palette_open)
}

fn draw<W: std::io::Write>(
    terminal: &mut Terminal<CrosstermBackend<&mut W>>,
    snap: &FrameSnapshot,
    term_mgr: &TerminalManager,
    palette: &PaletteState,
    palette_open: bool,
) -> Result<()> {
    // 收集每个 pane 的带样式屏幕网格（含光标行，用于视口定位）
    let mut screens: std::collections::HashMap<u32, Vec<Vec<Cell>>> =
        std::collections::HashMap::new();
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
fn resolve_runtime(opts: &TuiOpts) -> (&'static str, Option<String>, Option<String>) {
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

/// 用 core discovery 查找已有的工作区候选（产品名，不是 tmux session）。
fn find_existing_tmux_session(socket: Option<&str>) -> Option<String> {
    FfiClient::discover_existing("local", None, socket)
        .ok()?
        .first()
        .map(|workspace| workspace.name.clone())
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
/// 完成后触发重连（替换共享 `FfiClient`）。
fn handle_palette_key(
    palette: &mut PaletteState,
    key: &KeyEvent,
    bridge: &mut EventPump,
    runtime_type: &mut &'static str,
    store: &mut ViewStore,
    scenes: &mut SceneStack,
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
                    reconnect(bridge, runtime_type, &action, sock.as_deref())?;
                    // 重连后旧 Scene 全部失效：清空并按新后端重设应答策略。
                    *store = ViewStore::default();
                    scenes.clear();
                    scenes.set_forward_replies(is_direct_pty_terminal(runtime_type));
                    let _ = bridge.sync_view_store(store);
                    let events = bridge.poll_into_with_events(store);
                    drain_store_into_scenes(store, scenes);
                    for event in &events {
                        scenes.apply_workspace_event(event);
                    }
                    if let Some(id) = store.active_workspace_id() {
                        scenes.show(id);
                    }
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
            let hosts = FfiClient::discover_ssh_hosts()
                .unwrap_or_default()
                .into_iter()
                .map(|host| host.alias)
                .collect::<Vec<_>>();
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
                    FfiClient::discover_existing("local", None, palette.socket.as_deref())
                        .unwrap_or_default()
                }
                ConnectSource::Ssh => {
                    let host = palette.host.clone().unwrap_or_default();
                    FfiClient::discover_existing("ssh", Some(host.as_str()), None)
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
            let entries = FfiClient::list_dir("local", None, &dir).unwrap_or_default();
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

/// 连接动作 → 替换共享 FFI client。
fn reconnect(
    bridge: &mut EventPump,
    runtime_kind: &mut &'static str,
    action: &ConnectAction,
    socket: Option<&str>,
) -> Result<()> {
    let sock = socket.map(|s| s.to_string());
    let (runtime_type, socket, session, ssh_alias, start_dir) = match action {
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

    let new_bridge = FfiClient::new_connect(
        runtime_type,
        socket.as_deref(),
        session.as_deref(),
        ssh_alias.as_deref(),
        start_dir.as_deref(),
    )?;
    bridge.replace_client(new_bridge);
    *runtime_kind = runtime_type;
    // 等初始状态
    std::thread::sleep(Duration::from_millis(300));
    let _ = bridge.poll();
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
    commands: &mut CommandQueue,
    key: &KeyEvent,
    snap: &FrameSnapshot,
    scenes: &mut SceneStack,
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
                    commands.push(ClientCommand::Task {
                        workspace_id: None,
                        task: ClientTask::NewTab,
                    });
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
                        let tab_id = snap.tabs[n - 1].id;
                        if let Some(scene) = scenes.visible_mut() {
                            scene.set_visible_tab(tab_id);
                        }
                        commands.push(ClientCommand::Task {
                            workspace_id: None,
                            task: ClientTask::SwitchTab { tab_id },
                        });
                        return true;
                    }
                    return false;
                }
                'w' => {
                    let tab = snap.tabs.iter().find(|t| t.is_active).or(snap.tabs.first());
                    if let Some(t) = tab {
                        commands.push(ClientCommand::Task {
                            workspace_id: None,
                            task: ClientTask::CloseTab { tab_id: t.id },
                        });
                        return true;
                    }
                    return false;
                }
                's' => {
                    commands.push(ClientCommand::Task {
                        workspace_id: None,
                        task: ClientTask::SplitPane {
                            pane_id: target.unwrap_or(0),
                            horizontal: true,
                        },
                    });
                    return true;
                }
                'v' => {
                    commands.push(ClientCommand::Task {
                        workspace_id: None,
                        task: ClientTask::SplitPane {
                            pane_id: target.unwrap_or(0),
                            horizontal: false,
                        },
                    });
                    return true;
                }
                '[' => {
                    commands.push(ClientCommand::Task {
                        workspace_id: None,
                        task: ClientTask::PreviousPane,
                    });
                    return true;
                }
                ']' => {
                    commands.push(ClientCommand::Task {
                        workspace_id: None,
                        task: ClientTask::NextPane,
                    });
                    return true;
                }
                _ => {
                    let Some(pane) = target else { return false };
                    let bytes = encode(&MuxKeyEvent::Alt(c));
                    commands.push(ClientCommand::Input {
                        workspace_id: None,
                        pane_id: pane,
                        data: bytes,
                        quiet: false,
                    });
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
            commands.push(ClientCommand::Input {
                workspace_id: None,
                pane_id: pane,
                data: bytes,
                quiet: false,
            });
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
    commands.push(ClientCommand::Input {
        workspace_id: None,
        pane_id: pane,
        data: bytes,
        quiet: false,
    });
    true
}
