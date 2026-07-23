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
use crate::core::tmux::client::{
    ConnectMode, TmuxClient, TmuxClientConfig, TmuxClientHandle, TmuxEvent,
};
use crate::core::tmux::command as cmd;
use crate::core::tmux::protocol::{parse_layout_tree, LayoutTree, Message, NotificationKind};
use crate::core::types::{PaneId, SessionId, TabId, WindowId};

/// 后台命令查询标记：记录发出去的命令，收到 %end 时处理响应行。
#[derive(Debug, Clone)]
#[allow(dead_code)]
enum PendingQuery {
    /// list-panes -t <window> -F '...'：解析所有 pane（pane_id, window_id, active, cols, rows）。
    ListPanes { window: WindowId },
    /// list-windows -t <session> -F '...'：解析所有 window（window_id, name, active, layout, panes）。
    ListWindows,
    /// display-message -p -t <pane> '<format>'：取单行响应。
    DisplayMessage { pane: PaneId },
    /// list-sessions：列出 tmux server 上所有 session。
    ListSessions,
}

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

    /// 当前命令响应累积的行（%begin..%end 之间），带 number 标识。
    response_accum: HashMap<i64, Vec<String>>,
    /// 等待响应的命令回调（number → 处理函数）。简化为存命令类型标记。
    pending_queries: VecDeque<PendingQuery>,
    /// 缓存每个 window 的 layout 字符串（从 list-windows 响应获取），用于重建 LayoutNode。
    window_layouts: HashMap<WindowId, String>,
    /// 每个 window 的 pane 数量（从 list-windows 响应获取），用于确认所有 pane 查询完成。
    expected_panes_per_window: HashMap<WindowId, usize>,
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
            response_accum: HashMap::new(),
            pending_queries: VecDeque::new(),
            window_layouts: HashMap::new(),
            expected_panes_per_window: HashMap::new(),
        }
    }

    /// 创建后端并指定 attach 模式（连接已有 tmux session）。
    ///
    /// `target` 是 tmux session 名或 id（如 "demo" 或 "$0"）。
    pub fn new_with_attach(socket: Option<&str>, target: &str) -> Self {
        let mut backend = Self::new(socket);
        backend.config.mode = Some(ConnectMode::Attach {
            target: Some(target.to_string()),
        });
        backend
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
                // layout 变化可能意味着 pane 增减，重新查询 pane 列表
                self.query_list_panes(window);
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
                    // 主动查询该 window 的 pane（%window-add 不带 pane 信息）
                    self.query_list_panes(window);
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
            Message::WindowPaneChanged { window, pane } => {
                // tmux window 对应 muxterm tab（TabId(window.0)）
                let tab_id = TabId(window.0);
                for p in self.panes.iter_mut() {
                    if p.tab == tab_id {
                        p.active = p.id == pane;
                    }
                }
                if let Some(tl) = self.layouts.get_mut(&tab_id) {
                    tl.active = pane;
                }
                self.events
                    .push_back(StateChange::ActivePaneChanged { tab: tab_id, pane });
            }
            Message::SessionWindowChanged { session, window } => {
                // tmux session 的 active window 切换 → muxterm active tab 切换
                let tab_id = TabId(window.0);
                for t in self.tabs.iter_mut() {
                    t.active = t.id == tab_id;
                }
                if let Some(sess) = self.sessions.iter_mut().find(|s| s.id == session) {
                    sess.active_window = Some(window);
                }
                // 也更新 windows active 标记（虚拟 window）
                for w in self.windows.iter_mut() {
                    w.active = w.id == window;
                }
                // 如果目标 tab 的 pane 数据为空，重新查询（兜底）
                let pane_count = self.panes.iter().filter(|p| p.tab == tab_id).count();
                if pane_count == 0 {
                    tracing::debug!(target: "muxterm::tmux", "切 tab 到 @{} 但 pane 为空，重新查询", window.0);
                    self.query_list_panes(window);
                }
                self.events.push_back(StateChange::ActiveTabChanged {
                    window,
                    tab: tab_id,
                });
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
                TmuxEvent::Message(msg) => {
                    // 先处理 ResponseBoundary（begin/end 状态机），再处理其他消息。
                    if let Message::ResponseBoundary(b) = &msg {
                        match b.kind {
                            NotificationKind::Begin => {
                                self.response_accum.insert(b.number, Vec::new());
                            }
                            NotificationKind::End => {
                                let lines =
                                    self.response_accum.remove(&b.number).unwrap_or_default();

                                self.dispatch_response(b.number, lines);
                            }
                            NotificationKind::Error => {
                                let _err_lines =
                                    self.response_accum.remove(&b.number).unwrap_or_default();

                                if let Some(q) = self.pending_queries.pop_front() {
                                    tracing::warn!(
                                        target: "muxterm::tmux",
                                        "tmux 命令 {} 出错（丢弃查询 {:?}）",
                                        b.number,
                                        q
                                    );
                                }
                            }
                        }
                    }
                    // 通知消息（WindowAdd / Output 等）先于对应的 %begin/%end 到达，
                    // 所以先 handle_message 处理通知，再在上面处理响应边界。
                    self.handle_message(msg);
                }
                TmuxEvent::ResponseLine { number, line, .. } => {
                    // 累积到对应 number 的响应缓冲（begin 后、end 前的行）

                    self.response_accum.entry(number).or_default().push(line);
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

    /// 处理一条命令的完整响应（%begin..%end 之间的行）。
    ///
    /// 从 pending_queries 弹出最早的一个查询，按类型解析响应行。
    fn dispatch_response(&mut self, _number: i64, lines: Vec<String>) {
        // 简化：按 FIFO 弹出 pending_queries；tmux 命令是串行的，顺序匹配。
        if let Some(query) = self.pending_queries.pop_front() {
            match query {
                PendingQuery::ListPanes { window } => {
                    self.handle_list_panes_response(window, lines);
                }
                PendingQuery::ListWindows => {
                    self.handle_list_windows_response(lines);
                }
                PendingQuery::DisplayMessage { pane } => {
                    // 单行响应：用作 pane 标题
                    if let Some(line) = lines.first() {
                        let title = line.trim().to_string();
                        if let Some(p) = self.panes.iter_mut().find(|p| p.id == pane) {
                            if p.title != title {
                                p.title = title.clone();
                                self.events
                                    .push_back(StateChange::PaneTitleChanged { pane, title });
                            }
                        }
                    }
                }
                PendingQuery::ListSessions => {
                    // list-sessions 默认格式: "demo: 1 windows (created ...)"
                    for line in &lines {
                        let line = line.trim();
                        if line.is_empty() {
                            continue;
                        }
                        let name = line.split(':').next().unwrap_or("").trim();
                        if name.is_empty() {
                            continue;
                        }
                        let sid = self
                            .sessions
                            .iter()
                            .find(|s| s.name == name)
                            .map(|s| s.id)
                            .unwrap_or(SessionId(self.sessions.len() as u32));
                        if !self.sessions.iter().any(|s| s.name == name) {
                            self.sessions.push(SessionInfo {
                                id: sid,
                                name: name.to_string(),
                                active_window: None,
                            });
                        }
                    }
                    self.events.push_back(StateChange::SessionsChanged);
                }
            }
        }
    }

    /// 解析 `list-panes -a -t <session> -F '...'` 的响应。
    ///
    /// 每行格式：`%N,@M,<active>,<cols>x<rows>,<x>,<y>`（逗号分隔）
    /// 解析 `list-panes -t @N` 的响应。
    ///
    /// 默认格式："1: [70x30] [history ...] %0 (active)"
    /// 参数 window 是这些 pane 所属的 tmux window。
    fn handle_list_panes_response(&mut self, window: WindowId, lines: Vec<String>) {
        tracing::debug!(target: "muxterm::tmux", "list-panes 响应 window=@{}: {} 行", window.0, lines.len());
        let tab_id = TabId(window.0);
        let mut new_panes: Vec<PaneInfo> = Vec::new();
        for line in lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let pane = match extract_pane_id_from_default(line) {
                Some(p) => p,
                None => continue,
            };
            let (cols, rows) = extract_size_from_default(line);
            let active = line.contains("(active)");
            new_panes.push(PaneInfo {
                id: pane,
                tab: tab_id,
                active,
                title: String::new(),
                cols,
                rows,
            });
        }
        let mut changed = false;
        for np in &new_panes {
            if let Some(existing) = self.panes.iter_mut().find(|p| p.id == np.id) {
                if existing.cols != np.cols
                    || existing.rows != np.rows
                    || existing.active != np.active
                {
                    existing.cols = np.cols;
                    existing.rows = np.rows;
                    existing.active = np.active;
                    self.events.push_back(StateChange::PaneResized {
                        pane: np.id,
                        cols: np.cols,
                        rows: np.rows,
                    });
                }
            } else {
                self.panes.push(np.clone());
                self.events.push_back(StateChange::PaneAdded {
                    pane: np.id,
                    tab: tab_id,
                });
                changed = true;
            }
        }
        let valid_ids: Vec<PaneId> = new_panes.iter().map(|p| p.id).collect();
        let to_remove: Vec<PaneId> = self
            .panes
            .iter()
            .filter(|p| p.tab == tab_id && !valid_ids.contains(&p.id))
            .map(|p| p.id)
            .collect();
        for pid in to_remove {
            self.panes.retain(|p| p.id != pid);
            self.events.push_back(StateChange::PaneClosed { pane: pid });
            changed = true;
        }
        if changed || !new_panes.is_empty() {
            self.rebuild_layout(tab_id, window, &new_panes);
        }
    }

    /// 解析 `list-windows -t <session> -F '#{window_id} #{window_name} #{window_active} #{window_layout} #{window_panes}'` 的响应。
    fn handle_list_windows_response(&mut self, lines: Vec<String>) {
        let sess = self.active_session.unwrap_or(SessionId(0));
        for line in lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // 格式：@0,name,1,75ac,140x30,...{...},3（逗号分隔，但 window_layout 含逗号）
            // window_layout 本身含逗号，所以用 splitn(5, ',') 分割前 4 个字段 + 剩余
            let parts: Vec<&str> = line.splitn(5, ',').collect();
            if parts.len() < 5 {
                continue;
            }
            let window = match WindowId::parse(parts[0]) {
                Ok(w) => w,
                Err(_) => continue,
            };
            let name = parts[1].to_string();
            let active = parts[2] == "1";
            let layout_str = parts[3].to_string();
            // 缓存 window_layout 字符串，供 rebuild_layout 使用
            self.window_layouts.insert(window, layout_str);
            let panes_count: usize = parts[4].parse().unwrap_or(0);
            self.expected_panes_per_window.insert(window, panes_count);

            // 更新 / 创建 window（虚拟）
            if let Some(w) = self.windows.iter_mut().find(|w| w.id == window) {
                w.name = name.clone();
                w.active = active;
            } else {
                self.windows.push(WindowInfo {
                    id: window,
                    name: name.clone(),
                    session: sess,
                    active,
                });
            }
            // 更新 / 创建 tab
            let tab_id = TabId(window.0);
            if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                t.name = name.clone();
                t.active = active;
            } else {
                self.tabs.push(TabInfo {
                    id: tab_id,
                    name: name.clone(),
                    window,
                    active,
                });
                self.events.push_back(StateChange::TabAdded {
                    tab: tab_id,
                    window,
                });
            }

            // 主动查询该 window 的 panes
            self.query_list_panes(window);
        }
    }

    /// 发送 list-panes 查询（异步，通过 cmd_tx）。
    fn query_list_panes(&mut self, window: WindowId) {
        // 用 list-panes -t @N 查询单个 window 的 pane（默认格式不含 window_id）。
        let line = format!("list-panes -t @{}\n", window.0);
        if self.dispatch_command(line).is_ok() {
            self.pending_queries
                .push_back(PendingQuery::ListPanes { window });
        }
    }

    /// 发送 list-sessions 查询（列出 tmux server 上所有 session）。
    fn query_list_sessions(&mut self) {
        let line = "list-sessions\n".to_string();
        if self.dispatch_command(line).is_ok() {
            self.pending_queries.push_back(PendingQuery::ListSessions);
        }
    }

    /// 发送 list-windows 查询。
    fn query_list_windows(&mut self) {
        let sess = self.active_session.unwrap_or(SessionId(0));
        let line = format!(
            "list-windows -t {} -F \"#{{window_id}},#{{window_name}},#{{window_active}},#{{window_layout}},#{{window_panes}}\"\n",
            sess
        );

        if self.dispatch_command(line).is_ok() {
            self.pending_queries.push_back(PendingQuery::ListWindows);
        }
    }

    /// 用 parse_layout_tree 重建 LayoutNode 树。
    ///
    /// 需要 list-windows 的 window_layout 字符串。这里通过几何匹配把
    /// LayoutTree 叶子映射到 pane id（位置匹配）。
    fn rebuild_layout(&mut self, tab_id: TabId, window: WindowId, panes: &[PaneInfo]) {
        if panes.is_empty() {
            return;
        }
        let active = panes
            .iter()
            .find(|p| p.active)
            .map(|p| p.id)
            .unwrap_or(panes[0].id);
        if panes.len() == 1 {
            let tree = LayoutNode::leaf(panes[0].id);
            let layout = TabLayout {
                tab: tab_id,
                tree,
                active,
            };
            self.layouts.insert(tab_id, layout.clone());
            self.events.push_back(StateChange::LayoutChanged {
                tab: tab_id,
                layout,
            });
            return;
        }
        let layout_str = match self.window_layouts.get(&window) {
            Some(s) => s.clone(),
            None => {
                self.build_fallback_layout(tab_id, panes, active);
                return;
            }
        };
        let tree = match parse_layout_tree(&layout_str) {
            Ok(lt) => lt,
            Err(e) => {
                tracing::warn!(target: "muxterm::tmux", "layout tree 解析失败: {e}");
                self.build_fallback_layout(tab_id, panes, active);
                return;
            }
        };
        let layout_node = match layout_tree_to_node(&tree, panes) {
            Some(n) => n,
            None => {
                self.build_fallback_layout(tab_id, panes, active);
                return;
            }
        };
        let layout = TabLayout {
            tab: tab_id,
            tree: layout_node,
            active,
        };
        self.layouts.insert(tab_id, layout.clone());
        self.events.push_back(StateChange::LayoutChanged {
            tab: tab_id,
            layout,
        });
    }

    /// 朴素兜底布局：按顺序水平排列 pane。
    fn build_fallback_layout(&mut self, tab_id: TabId, panes: &[PaneInfo], active: PaneId) {
        let mut sorted: Vec<PaneInfo> = panes.to_vec();
        sorted.sort_by_key(|p| (p.cols, p.id.0));
        let mut tree = LayoutNode::leaf(sorted[0].id);
        for p in &sorted[1..] {
            tree.split_at(sorted[0].id, p.id, SplitDir::Horizontal);
        }
        let layout = TabLayout {
            tab: tab_id,
            tree,
            active,
        };
        self.layouts.insert(tab_id, layout.clone());
        self.events.push_back(StateChange::LayoutChanged {
            tab: tab_id,
            layout,
        });
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

    fn all_windows(&self) -> Vec<&WindowInfo> {
        self.windows.iter().collect()
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

        // 等待 tmux 启动事件建立初始 state
        // new-session 模式：等 SessionChanged + WindowAdd
        // attach 模式：等 SessionChanged（window 不通过通知到达，需主动查询）
        let is_attach = matches!(self.config.mode, Some(ConnectMode::Attach { .. }));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            self.pump_events();
            if is_attach {
                // attach 模式只需 session 事件
                if !self.sessions.is_empty() {
                    break;
                }
            } else {
                // new-session 模式需 session + window
                if !self.sessions.is_empty() && !self.windows.is_empty() {
                    break;
                }
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

        // 主动查询所有 window + pane，建立完整初始 state（attach 已有 session 必需）
        self.query_list_windows();
        // 等待 list-windows 响应到达（最多 3 秒），拿到所有 window 列表后再等 pane 查询
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            self.pump_events();
            // 等 list-windows 响应到达：windows 非空且 expected_panes_per_window 非空
            if !self.windows.is_empty() && !self.expected_panes_per_window.is_empty() {
                break;
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
            tokio::task::yield_now().await;
        }
        // 现在所有 window 的 pane 查询已发出（handle_list_windows_response 对每个 window 调了 query_list_panes）
        // 等待所有 window 的 pane 查询响应到达（最多 5 秒）
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            self.pump_events();
            // 检查是否所有 window 的 pane 都已到达
            let all_ready = self
                .expected_panes_per_window
                .iter()
                .all(|(wid, expected)| {
                    let tab_id = TabId(wid.0);
                    self.panes.iter().filter(|p| p.tab == tab_id).count() >= *expected
                });
            if all_ready {
                break;
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
            tokio::task::yield_now().await;
        }

        // 查询所有 session（用于 list-sessions 列出 server 上所有 session）
        self.query_list_sessions();

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

/// 把 LayoutTree（几何拓扑）转成 LayoutNode（pane id 树），按几何位置匹配。
fn layout_tree_to_node(tree: &LayoutTree, panes: &[PaneInfo]) -> Option<LayoutNode> {
    let leaves = collect_layout_leaves(tree);
    if leaves.len() != panes.len() {
        return None;
    }
    let mut mapping = HashMap::new();
    for (leaf, pane) in leaves.iter().zip(panes.iter()) {
        mapping.insert((leaf.x, leaf.y), pane.id);
    }
    layout_tree_to_node_inner(tree, &mapping)
}

fn collect_layout_leaves(tree: &LayoutTree) -> Vec<&LayoutTree> {
    match &tree.children {
        None => vec![tree],
        Some((a, b)) => {
            let mut v = collect_layout_leaves(a);
            v.extend(collect_layout_leaves(b));
            v
        }
    }
}

fn layout_tree_to_node_inner(
    tree: &LayoutTree,
    mapping: &HashMap<(u32, u32), PaneId>,
) -> Option<LayoutNode> {
    match &tree.children {
        None => mapping
            .get(&(tree.x, tree.y))
            .map(|&pid| LayoutNode::leaf(pid)),
        Some((a, b)) => {
            let first = layout_tree_to_node_inner(a, mapping)?;
            let second = layout_tree_to_node_inner(b, mapping)?;
            Some(LayoutNode::Split {
                dir: tree.dir,
                ratio: 500,
                first: Box::new(first),
                second: Box::new(second),
            })
        }
    }
}

/// 从默认格式的 list-panes 行提取 pane id。
///
/// 格式：`0:1.1: [80x24] [history ...] %0 (active)`
/// 提取 `%0` 部分 → PaneId(0)
fn extract_pane_id_from_default(line: &str) -> Option<PaneId> {
    // 找 %N token（pane id）
    for token in line.split_whitespace() {
        if token.starts_with('%') && token.len() > 1 {
            let num = &token[1..];
            // 去除尾部非数字字符（如 "%0(active)" 或 "%0")
            let digits: String = num.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = digits.parse::<u32>() {
                return Some(PaneId(n));
            }
        }
    }
    None
}

/// 从默认格式的 list-panes 行提取尺寸。
///
/// 格式：`... [80x24] ...` → (80, 24)
fn extract_size_from_default(line: &str) -> (u16, u16) {
    // 找 [WxH] 模式
    if let Some(start) = line.find('[') {
        if let Some(end) = line[start..].find(']') {
            let inside = &line[start + 1..start + end];
            if let Some((w, h)) = inside.split_once('x') {
                return (w.parse().unwrap_or(80), h.parse().unwrap_or(24));
            }
        }
    }
    (80, 24)
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
        // 等待 tmux 推送 WindowAdd 事件（轮询 pump_events 而非 sleep）
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(3);
        loop {
            let _ = b.take_events();
            if b.windows.len() > initial_windows {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
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
