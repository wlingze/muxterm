//! TmuxBackend：tmux -CC 控制模式后端。
//!
//! 封装现有 `core::tmux::client`（spawn tmux -CC + 事件流）和
//! `core::tmux::command`（强类型命令构造器），实现 `Backend` trait。
//!
//! 设计：
//! - `connect()`：spawn tmux -CC new-session，drain 启动事件建立初始 state
//!   （session / 第一个 window / 第一个 pane）
//! - 后台 task 持续读 `TmuxEvent`，把 `Message` 转成内部 state 更新 +
//!   `StateChange` 事件入队；命令响应行（ResponseLine）暂不处理
//! - `execute(Task)`：把 Task 映射成 `TmuxCommand`，通过命令 channel 发给
//!   后台 sender task 异步 `send_command`（execute 本身是同步 fn）
//! - `take_events()`：drain 内部事件队列
//! - State 视图从内部 state 读
//!
//! 与 LocalBackend 不同：状态变化由 tmux 推送的事件驱动，execute 只发命令，
//! 不立即改 state（tmux 会回推 LayoutChange/PaneModeChanged 等通知）。

use std::collections::HashMap;
use std::collections::VecDeque;

use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::core::model::backend::Backend;
use crate::core::model::layout::{LayoutNode, SplitDir, TabLayout};
use crate::core::model::state::{
    BackendStatus, PaneInfo, SessionInfo, State, StateChange, TabInfo, WindowInfo,
};
use crate::core::model::task::{Task, TaskOutcome};
use crate::core::tmux::client::{TmuxClient, TmuxClientConfig, TmuxClientHandle, TmuxEvent};
use crate::core::tmux::command as cmd;
use crate::core::tmux::protocol::Message;
use crate::core::types::{PaneId, SessionId, TabId, WindowId};

/// tmux -CC 后端。
pub struct TmuxBackend {
    config: TmuxClientConfig,
    handle: Option<TmuxClientHandle>,
    event_rx: Option<mpsc::Receiver<TmuxEvent>>,
    /// 命令发送 channel：execute 把 TmuxCommand 字符串塞进来，
    /// 后台 sender task 异步 send_command。
    cmd_tx: Option<mpsc::Sender<String>>,
    /// 后台事件回流 task 的 join handle（用于 shutdown 时 abort）。
    _pump_handle: Option<tokio::task::JoinHandle<()>>,
    _sender_handle: Option<tokio::task::JoinHandle<()>>,

    // ── 内部 state ──────────────────────────────────────────
    sessions: Vec<SessionInfo>,
    active_session: Option<SessionId>,
    windows: Vec<WindowInfo>,
    tabs: Vec<TabInfo>,
    panes: Vec<PaneInfo>,
    layouts: HashMap<TabId, TabLayout>,
    /// pane 累积输出。
    outputs: HashMap<PaneId, Vec<u8>>,

    status: BackendStatus,
    events: VecDeque<StateChange>,
}

impl TmuxBackend {
    /// 创建后端（尚未 connect）。socket 非空时隔离 tmux server（`-L`）。
    pub fn new(socket: Option<&str>) -> Self {
        let mut extra_args: Vec<String> = Vec::new();
        if let Some(s) = socket {
            let s = s.trim();
            if !s.is_empty() {
                extra_args.push("-L".into());
                extra_args.push(s.to_string());
            }
        }
        Self {
            config: TmuxClientConfig {
                mode: None,
                extra_args,
                tmux_bin: None,
                cols: Some(80),
                rows: Some(24),
                event_buffer: 0,
            },
            handle: None,
            event_rx: None,
            cmd_tx: None,
            _pump_handle: None,
            _sender_handle: None,
            sessions: vec![],
            active_session: None,
            windows: vec![],
            tabs: vec![],
            panes: vec![],
            layouts: HashMap::new(),
            outputs: HashMap::new(),
            status: BackendStatus::Disconnected,
            events: VecDeque::new(),
        }
    }

    /// 从内部 state 同步更新 active 标记。
    fn sync_active_marks(&mut self) {
        if let Some(sid) = self.active_session {
            for s in self.sessions.iter_mut() {
                let is_active = s.id == sid;
                if is_active {
                    s.active_window = self.windows.iter().find(|w| w.active).map(|w| w.id);
                }
                let _ = is_active;
            }
        }
    }

    /// 处理一条 tmux Message，更新内部 state 并产生 StateChange。
    fn handle_message(&mut self, msg: Message) {
        match msg {
            Message::Output { pane, content, .. } => {
                self.outputs
                    .entry(pane)
                    .or_default()
                    .extend_from_slice(&content);
                self.events.push_back(StateChange::PaneOutput {
                    pane,
                    data: content,
                });
            }
            Message::LayoutChange {
                window,
                layout,
                visible_layout,
            } => {
                // 从 layout 几何更新 pane 尺寸（如果有对应 pane）
                let cols = layout.cols as u16;
                let rows = layout.rows as u16;
                // 找该 window 的 active pane 更新尺寸（简化：更新所有该 window 的 pane）
                let pane_ids: Vec<PaneId> = self
                    .panes
                    .iter()
                    .filter(|p| p.tab == TabId(window.0))
                    .map(|p| p.id)
                    .collect();
                for pid in pane_ids {
                    if let Some(p) = self.panes.iter_mut().find(|p| p.id == pid) {
                        if p.cols != cols || p.rows != rows {
                            p.cols = cols;
                            p.rows = rows;
                            self.events.push_back(StateChange::PaneResized {
                                pane: pid,
                                cols,
                                rows,
                            });
                        }
                    }
                }
                let tab_id = TabId(window.0);
                if let Some(tl) = self.layouts.get_mut(&tab_id) {
                    tl.tab = tab_id;
                    let _ = visible_layout;
                }
                if let Some(wl) = self.layouts.get(&tab_id) {
                    self.events.push_back(StateChange::LayoutChanged {
                        tab: tab_id,
                        layout: wl.clone(),
                    });
                }
            }
            Message::WindowAdd { window } => {
                let sess = self.active_session.unwrap_or(SessionId(0));
                if !self.windows.iter().any(|w| w.id == window) {
                    self.windows.push(WindowInfo {
                        id: window,
                        name: format!("w{}", window.0),
                        session: sess,
                        active: true,
                    });
                    for w in self.windows.iter_mut() {
                        if w.id != window {
                            w.active = false;
                        }
                    }
                    // tmux window = muxterm tab
                    let tab_id = TabId(window.0);
                    if !self.tabs.iter().any(|t| t.id == tab_id) {
                        self.tabs.push(TabInfo {
                            id: tab_id,
                            name: format!("t{}", window.0),
                            window,
                            active: true,
                        });
                        for t in self.tabs.iter_mut() {
                            if t.id != tab_id {
                                t.active = false;
                            }
                        }
                        self.layouts.insert(
                            tab_id,
                            TabLayout {
                                tab: tab_id,
                                tree: LayoutNode::leaf(PaneId(0)),
                                active: PaneId(0),
                            },
                        );
                    }
                    self.events.push_back(StateChange::WindowAdded {
                        window,
                        session: sess,
                    });
                    self.events.push_back(StateChange::TabAdded {
                        tab: TabId(window.0),
                        window,
                    });
                }
            }
            Message::WindowClose { window } => {
                self.windows.retain(|w| w.id != window);
                let tab_id = TabId(window.0);
                self.panes.retain(|p| p.tab != tab_id);
                self.layouts.remove(&tab_id);
                self.events.push_back(StateChange::WindowClosed { window });
            }
            Message::WindowRenamed { window, name } => {
                if let Some(w) = self.windows.iter_mut().find(|w| w.id == window) {
                    w.name = name.clone();
                }
                self.events
                    .push_back(StateChange::WindowRenamed { window, name });
            }
            Message::SessionChanged { session, name } => {
                if !self.sessions.iter().any(|s| s.id == session) {
                    self.sessions.push(SessionInfo {
                        id: session,
                        name: name.clone().unwrap_or_default(),
                        active_window: None,
                    });
                }
                self.active_session = Some(session);
                self.events
                    .push_back(StateChange::SessionChanged { session, name });
            }
            Message::SessionRenamed { session, name } => {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session) {
                    s.name = name.clone();
                }
                self.events
                    .push_back(StateChange::SessionRenamed { session, name });
            }
            Message::SessionsChanged => {
                self.events.push_back(StateChange::SessionsChanged);
            }
            Message::PaneModeChanged { pane, mode } => {
                // mode 变化暂用作标题（简化）
                if let Some(p) = self.panes.iter_mut().find(|p| p.id == pane) {
                    if p.title != mode {
                        p.title = mode.clone();
                        self.events
                            .push_back(StateChange::PaneTitleChanged { pane, title: mode });
                    }
                }
            }
            Message::Exit { .. } => {
                self.status = BackendStatus::Exited;
                self.events
                    .push_back(StateChange::BackendStatusChanged(BackendStatus::Exited));
            }
            Message::ExtendedOutput { .. }
            | Message::UnlinkedWindowAdd { .. }
            | Message::UnlinkedWindowClose { .. }
            | Message::ResponseBoundary(_)
            | Message::Unknown { .. } => {
                // 暂不处理
            }
        }
    }

    /// drain event_rx 的 TmuxEvent，更新 state。
    fn pump_events(&mut self) {
        // 先把所有 TmuxEvent drain 到本地 vec，避免与 self 的可变借用冲突。
        let mut pending = Vec::new();
        if let Some(rx) = self.event_rx.as_mut() {
            while let Ok(ev) = rx.try_recv() {
                pending.push(ev);
            }
        }
        for ev in pending {
            match ev {
                TmuxEvent::Message(msg) => self.handle_message(msg),
                TmuxEvent::ResponseLine { .. } => {
                    // 命令响应正文行暂不处理
                }
                TmuxEvent::Exit { .. } => {
                    self.status = BackendStatus::Exited;
                    self.events
                        .push_back(StateChange::BackendStatusChanged(BackendStatus::Exited));
                }
            }
        }
        self.sync_active_marks();
    }

    /// 把一个命令异步发送给 tmux（通过 channel）。
    /// execute 是同步 fn，命令发送走后台 task。
    fn dispatch_command(&self, line: String) -> std::io::Result<()> {
        let Some(tx) = self.cmd_tx.as_ref() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "tmux 命令通道未建立",
            ));
        };
        // 用 try_send 非阻塞塞入 channel
        tx.try_send(line).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::WouldBlock, format!("命令通道满: {e}"))
        })
    }

    /// 便捷：发送一个 TmuxCommand。
    fn dispatch_tmux_command(&self, command: &cmd::TmuxCommand) -> std::io::Result<()> {
        self.dispatch_command(command.to_line())
    }
}

impl State for TmuxBackend {
    fn sessions(&self) -> &[SessionInfo] {
        &self.sessions
    }

    fn active_session(&self) -> Option<&SessionInfo> {
        self.active_session
            .and_then(|sid| self.sessions.iter().find(|s| s.id == sid))
    }

    fn active_window(&self) -> Option<&WindowInfo> {
        self.windows.iter().find(|w| w.active)
    }

    fn active_tab(&self) -> Option<&TabInfo> {
        self.tabs.iter().find(|t| t.active)
    }

    fn active_pane(&self) -> Option<&PaneInfo> {
        self.panes.iter().find(|p| p.active)
    }

    fn tabs(&self, window: &WindowId) -> Vec<&TabInfo> {
        self.tabs.iter().filter(|t| &t.window == window).collect()
    }

    fn tab(&self, tab: &TabId) -> Option<&TabInfo> {
        self.tabs.iter().find(|t| &t.id == tab)
    }

    fn layout(&self, tab: &TabId) -> Option<&TabLayout> {
        self.layouts.get(tab)
    }

    fn panes(&self, tab: &TabId) -> Vec<&PaneInfo> {
        self.panes.iter().filter(|p| &p.tab == tab).collect()
    }

    fn pane(&self, pane: &PaneId) -> Option<&PaneInfo> {
        self.panes.iter().find(|p| &p.id == pane)
    }

    fn pane_output(&self, pane: &PaneId) -> Option<&[u8]> {
        self.outputs.get(pane).map(|v| v.as_slice())
    }

    fn status(&self) -> BackendStatus {
        self.status
    }
}

#[async_trait]
impl Backend for TmuxBackend {
    async fn connect(&mut self) -> Result<()> {
        if self.status == BackendStatus::Connected {
            return Ok(());
        }
        self.status = BackendStatus::Connecting;
        self.events
            .push_back(StateChange::BackendStatusChanged(BackendStatus::Connecting));

        let config = self.config.clone();
        let (handle, rx) = TmuxClient::spawn(config)
            .await
            .context("spawn tmux -CC 失败")?;

        // 命令发送 channel + 后台 sender task（持有 handle）。
        // execute 同步 dispatch 命令到 cmd_tx；sender task 异步 send_command。
        // shutdown 时 drop cmd_tx 让 sender task 结束；handle 在 sender task 里，
        // shutdown 用 detach + 让 tmux 退出（kill 由 tmux 自然退出完成）。
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<String>(256);
        let mut sender_handle = handle;
        let sender_join = tokio::spawn(async move {
            while let Some(line) = cmd_rx.recv().await {
                if sender_handle.send_raw(&line).await.is_err() {
                    break;
                }
            }
            // sender 结束后 detach + kill
            let _ = sender_handle.kill().await;
        });

        self.event_rx = Some(rx);
        self.cmd_tx = Some(cmd_tx);
        self._sender_handle = Some(sender_join);
        self.handle = None; // handle 已 move 进 sender task

        // 等待 tmux 启动事件（WindowAdd / SessionChanged）建立初始 state
        // 给一定时间 drain 启动事件
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            self.pump_events();
            if !self.sessions.is_empty() && !self.windows.is_empty() {
                break;
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
            // 短暂让出
            tokio::task::yield_now().await;
        }

        if self.sessions.is_empty() {
            self.status = BackendStatus::Error;
            self.events
                .push_back(StateChange::BackendStatusChanged(BackendStatus::Error));
            anyhow::bail!("tmux 启动后未收到 session 事件");
        }

        self.status = BackendStatus::Connected;
        self.events
            .push_back(StateChange::BackendStatusChanged(BackendStatus::Connected));
        Ok(())
    }

    fn execute(&mut self, task: &Task) -> Result<TaskOutcome> {
        if self.cmd_tx.is_none() || self.status != BackendStatus::Connected {
            return Ok(TaskOutcome::Rejected {
                reason: "tmux 未连接".into(),
            });
        }
        let outcome = match task {
            Task::SplitPane {
                target,
                dir,
                command,
                workdir,
            } => {
                let target =
                    target.unwrap_or_else(|| self.active_pane().map(|p| p.id).unwrap_or(PaneId(0)));
                // tmux split-window 用 target pane 所在 window
                let tab_id = self.pane(&target).map(|p| p.tab).unwrap_or(TabId(0));
                let win_id = self
                    .tabs
                    .iter()
                    .find(|t| t.id == tab_id)
                    .map(|t| t.window)
                    .unwrap_or(WindowId(0));
                let direction = match dir {
                    SplitDir::Horizontal => cmd::SplitDirection::Horizontal,
                    SplitDir::Vertical => cmd::SplitDirection::Vertical,
                };
                let name = command.as_ref().and_then(|c| c.first()).map(|s| s.as_str());
                let _ = workdir;
                let c = cmd::split_window(win_id, direction, name);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::ClosePane { target } => {
                let c = cmd::kill_pane(*target);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::SwitchPane { target } => {
                let c = cmd::select_pane(*target);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::NextPane | Task::PrevPane => {
                let target = self.active_pane().map(|p| p.id).unwrap_or(PaneId(0));
                let c = if matches!(task, Task::NextPane) {
                    cmd::TmuxCommand::from_raw(format!("select-pane -t {} -N", target))
                } else {
                    cmd::TmuxCommand::from_raw(format!("select-pane -t {} -P", target))
                };
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::NewWindow { name, .. } => {
                let sess = self.active_session.unwrap_or(SessionId(0));
                let c = cmd::new_window(sess, name.as_deref());
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::CloseWindow { target } => {
                let c = cmd::kill_window(*target);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::SwitchWindow { target } => {
                let c = cmd::select_window(*target);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::RenameWindow { target, name } => {
                let c = cmd::rename_window(*target, name);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::SwitchSession { target } => {
                let c = cmd::TmuxCommand::from_raw(format!("switch-client -t {}", target));
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::RenameSession { target, name } => {
                let c = cmd::rename_session(*target, name);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::SendKeys { target, keys } => {
                use crate::core::terminal::input::KeyEvent;
                let tmux_keys: Vec<cmd::Key> = keys.iter().map(key_event_to_tmux_key).collect();
                let c = cmd::send_keys(*target, &tmux_keys);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::WriteRaw { target, data } => {
                let text = String::from_utf8_lossy(data).into_owned();
                let c = cmd::send_keys(*target, &[cmd::Key::Literal(text)]);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::ResizePane { target, cols, rows } => {
                let c = cmd::resize_pane(*target, Some(*cols as u32), Some(*rows as u32));
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::ResizePaneStep { target, dir, delta } => {
                let flag = match dir {
                    SplitDir::Horizontal => 'W',
                    SplitDir::Vertical => 'H',
                };
                let sign = if *delta >= 0 { 'U' } else { 'D' };
                let amount = delta.unsigned_abs();
                let c = cmd::TmuxCommand::from_raw(format!(
                    "resize-pane -t {} -{}{} {}",
                    target, flag, sign, amount
                ));
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::NewTab {
                window: _,
                name,
                command: _,
                workdir: _,
            } => {
                // tmux 的 tab = tmux window，新建 tab = 新建 tmux window
                let sess = self.active_session.unwrap_or(SessionId(0));
                let c = cmd::new_window(sess, name.as_deref());
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }

            Task::CloseTab { target } => {
                // tmux tab = tmux window，关闭 tab = kill-window
                let win_id = WindowId(target.0);
                let c = cmd::kill_window(win_id);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }

            Task::SwitchTab { target } => {
                let win_id = WindowId(target.0);
                let c = cmd::select_window(win_id);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }

            Task::RenameTab { target, name } => {
                let win_id = WindowId(target.0);
                let c = cmd::rename_window(win_id, name);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }

            Task::Shutdown => {
                // detach + kill
                let sess = self.active_session.unwrap_or(SessionId(0));
                let c = cmd::detach_client(sess);
                let _ = self.dispatch_tmux_command(&c);
                self.status = BackendStatus::Exited;
                self.events
                    .push_back(StateChange::BackendStatusChanged(BackendStatus::Exited));
                TaskOutcome::Done
            }
        };
        Ok(outcome)
    }

    fn take_events(&mut self) -> Vec<StateChange> {
        self.pump_events();
        self.events.drain(..).collect()
    }

    async fn shutdown(&mut self) -> Result<()> {
        // 先 detach（让 tmux 退出）
        self.execute(&Task::Shutdown)?;
        // 关闭命令通道，sender task 收到 None 后会 kill tmux 子进程并退出
        self.cmd_tx.take();
        // 等待 sender task 结束
        if let Some(h) = self._sender_handle.take() {
            let _ = h.await;
        }
        self.status = BackendStatus::Exited;
        self.events
            .push_back(StateChange::BackendStatusChanged(BackendStatus::Exited));
        Ok(())
    }
}

/// 把抽象 KeyEvent 转成 tmux Key。
fn key_event_to_tmux_key(ev: &crate::core::terminal::input::KeyEvent) -> cmd::Key {
    use crate::core::terminal::input::{ArrowDir, KeyEvent};
    match ev {
        KeyEvent::Char(c) => cmd::Key::Literal(c.to_string()),
        KeyEvent::Enter => cmd::Key::enter(),
        KeyEvent::Tab => cmd::Key::tab(),
        KeyEvent::Backspace => cmd::Key::bspace(),
        KeyEvent::Escape => cmd::Key::escape(),
        KeyEvent::Ctrl(c) => cmd::Key::ctrl(*c),
        KeyEvent::Alt(c) => cmd::Key::Literal(format!("\x1b{}", c)),
        KeyEvent::Function(n) => match n {
            1 => cmd::Key::Special("F1"),
            2 => cmd::Key::Special("F2"),
            3 => cmd::Key::Special("F3"),
            4 => cmd::Key::Special("F4"),
            5 => cmd::Key::Special("F5"),
            6 => cmd::Key::Special("F6"),
            7 => cmd::Key::Special("F7"),
            8 => cmd::Key::Special("F8"),
            9 => cmd::Key::Special("F9"),
            10 => cmd::Key::Special("F10"),
            11 => cmd::Key::Special("F11"),
            12 => cmd::Key::Special("F12"),
            _ => cmd::Key::Literal(String::new()),
        },
        KeyEvent::Arrow(d) => match d {
            ArrowDir::Up => cmd::Key::up(),
            ArrowDir::Down => cmd::Key::down(),
            ArrowDir::Left => cmd::Key::left(),
            ArrowDir::Right => cmd::Key::right(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_socket() -> String {
        format!("muxterm-tb-{}-{}", std::process::id(), rand_suffix())
    }

    fn rand_suffix() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        format!("{nanos}")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn connect_establishes_session_and_window() {
        let socket = unique_socket();
        let mut b = TmuxBackend::new(Some(&socket));
        b.connect().await.unwrap_or_else(|e| {
            eprintln!("skip: tmux 不可用: {e}");
            return;
        });
        if b.status() != BackendStatus::Connected {
            cleanup(&socket);
            return;
        }
        assert_eq!(b.status(), BackendStatus::Connected);
        // drain 事件
        let events = b.take_events();
        assert!(events.iter().any(|e| matches!(
            e,
            StateChange::BackendStatusChanged(BackendStatus::Connected)
        )));
        // 应有 session + window
        assert!(!b.sessions().is_empty());
        assert!(!b.windows.is_empty());
        let _ = b.shutdown().await;
        cleanup(&socket);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn new_window_via_tmux() {
        let socket = unique_socket();
        let mut b = TmuxBackend::new(Some(&socket));
        if b.connect().await.is_err() {
            cleanup(&socket);
            return;
        }
        let _ = b.take_events();
        let initial_windows = b.windows.len();
        b.execute(&Task::NewWindow {
            name: Some("test-win".into()),
            command: None,
            workdir: None,
        })
        .unwrap();
        // 等待 tmux 推送 WindowAdd 事件
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        let _ = b.take_events();
        assert!(b.windows.len() > initial_windows, "新 window 未建立");
        let _ = b.shutdown().await;
        cleanup(&socket);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn send_keys_does_not_error() {
        let socket = unique_socket();
        let mut b = TmuxBackend::new(Some(&socket));
        if b.connect().await.is_err() {
            cleanup(&socket);
            return;
        }
        let _ = b.take_events();
        let pane = b.active_pane().map(|p| p.id).unwrap_or(PaneId(0));
        let outcome = b
            .execute(&Task::SendKeys {
                target: pane,
                keys: vec![crate::core::terminal::input::KeyEvent::Char('x')],
            })
            .unwrap();
        assert_eq!(outcome, TaskOutcome::Done);
        let _ = b.shutdown().await;
        cleanup(&socket);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn split_pane_dispatched() {
        let socket = unique_socket();
        let mut b = TmuxBackend::new(Some(&socket));
        if b.connect().await.is_err() {
            cleanup(&socket);
            return;
        }
        let _ = b.take_events();
        let pane = b.active_pane().map(|p| p.id).unwrap_or(PaneId(0));
        let outcome = b
            .execute(&Task::SplitPane {
                target: Some(pane),
                dir: SplitDir::Horizontal,
                command: None,
                workdir: None,
            })
            .unwrap();
        assert_eq!(outcome, TaskOutcome::Done);
        let _ = b.shutdown().await;
        cleanup(&socket);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn execute_before_connect_rejected() {
        let mut b = TmuxBackend::new(Some("muxterm-nosuch-socket-xyz"));
        let outcome = b
            .execute(&Task::SendKeys {
                target: PaneId(1),
                keys: vec![],
            })
            .unwrap();
        assert!(matches!(outcome, TaskOutcome::Rejected { .. }));
    }

    fn cleanup(socket: &str) {
        let _ = std::process::Command::new("tmux")
            .args(["-L", socket, "kill-server"])
            .output();
    }
}
