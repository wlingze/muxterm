//! TmuxBackend：tmux -CC 控制模式后端。
//!
//! 封装现有 `runtime::tmux::client`（spawn tmux -CC + 事件流）和
//! `runtime::tmux::command`（强类型命令构造器），实现 `Backend` trait。
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

use std::collections::{HashMap, HashSet, VecDeque};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::core::buffer_cap::{append_capped, MAX_PANE_OUTPUT_BYTES, MAX_STATE_EVENTS};
use crate::core::model::backend::Backend;
use crate::core::model::layout::{LayoutNode, SplitDir, TabLayout};
use crate::core::model::state::{
    BackendStatus, PaneInfo, SessionInfo, State, StateChange, TabInfo, WindowInfo,
};
use crate::core::model::task::{Task, TaskOutcome};
use crate::core::runtime::tmux::client::{
    ConnectMode, TmuxClient, TmuxClientConfig, TmuxClientHandle, TmuxEvent,
};
use crate::core::runtime::tmux::command as cmd;
use crate::core::runtime::tmux::protocol::{
    parse_layout_tree, LayoutTree, Message, NotificationKind,
};
use crate::core::types::{PaneId, SessionId, TabId, WindowId};

/// 后台命令查询标记：记录发出去的命令，收到 %end 时处理响应行。
#[derive(Debug, Clone)]
#[allow(dead_code)]
enum PendingQuery {
    /// 非查询命令的响应占位；避免 split/send-keys 的 `%end` 消耗后续查询。
    Ignore,
    /// list-panes -t <window> -F '...'：解析所有 pane（pane_id, window_id, active, cols, rows）。
    ListPanes { window: WindowId },
    /// list-windows -t <session> -F '...'：解析所有 window（window_id, name, active, layout, panes）。
    ListWindows,
    /// display-message -p -t <pane> '<format>'：取单行响应。
    DisplayMessage { pane: PaneId },
    /// capture-pane -e -p -t <pane>：恢复 attach 时 tmux 已存在的可见屏幕。
    CapturePane { pane: PaneId },
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
    cmd_tx: Option<mpsc::UnboundedSender<String>>,
    /// 后台事件回流 task 的 join handle（用于 shutdown 时 abort）。
    _pump_handle: Option<tokio::task::JoinHandle<()>>,
    _sender_handle: Option<tokio::task::JoinHandle<()>>,
    /// sender task 的异步写错误；由前端轮询成可见状态事件。
    command_error_rx: Option<mpsc::UnboundedReceiver<String>>,

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
    /// `%begin <number>` 到达时从 pending_queries 队首取出的查询，按 number 登记。
    ///
    /// tmux 控制模式是串行的，但高输出下 `%begin/%end` 仍可能与多个在途查询
    /// 交叠。按 number 匹配能避免用简单的 FIFO `pop_front` 错配查询。
    pending_by_number: HashMap<i64, PendingQuery>,
    /// 缓存每个 window 的 layout 字符串（从 list-windows 响应获取），用于重建 LayoutNode。
    window_layouts: HashMap<WindowId, String>,
    /// 每个 window 的 pane 数量（从 list-windows 响应获取），用于确认所有 pane 查询完成。
    expected_panes_per_window: HashMap<WindowId, usize>,
    /// attach 初始快照查询中的 pane。初始 `%output` 不能先喂给前端，
    /// 否则随后 capture-pane 只能追加，已有屏幕内容会重复或缺失。
    initial_capture_pending: HashSet<PaneId>,
    /// 已完成 attach 初始快照的 pane；之后的 `%output` 才是实时增量。
    initial_capture_done: HashSet<PaneId>,
    /// attach 初始快照查询进行期间到达的实时 `%output` 缓冲。
    ///
    /// capture-pane 返回的是查询瞬间的完整屏幕；在「发出 capture-pane」到「收到
    /// 响应」之间的窗口里 shell 若产生输出，tmux 会继续发 `%output`，这些增量如果
    /// 直接丢弃会丢数据。这里暂存它们，快照返回后拼接到快照尾部，从而既保留完整
    /// 屏幕又不错过查询期间的实时增量。
    initial_capture_buf: HashMap<PaneId, Vec<u8>>,
}

impl TmuxBackend {
    // ── 层级映射（docs/LAYER-MAPPING.md 权威定义）──────────
    //
    // muxterm: Session → Window → Tab → Pane  (4 层)
    // tmux:    session → window → pane          (3 层)
    //
    // 映射规则：
    //   tmux session  → muxterm Session  (1:1)
    //   tmux window   → muxterm Tab      (1:1)  ← tmux window = muxterm Tab，不是 Window！
    //   tmux pane     → muxterm Pane     (1:1)
    //   (虚拟)        → muxterm Window   (固定 1 个，绑定 Session)
    //
    // 因此：
    //   self.windows 永远只有 1 个 WindowInfo（虚拟 Window，id=WindowId(1)）
    //   self.tabs 的每个 tab.window = WindowId(1)（指向虚拟 Window，不是 tmux @N）
    //   self.tabs 的每个 tab.id = TabId(tmux_window_index)
    //   self.panes 的每个 pane.tab = TabId(tmux_window_index)
    //   list-windows 返回 1 个 Window
    //   list-tabs 返回 N 个 Tab（对应 tmux 的 N 个 window）
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
                ssh_alias: None,
            },
            handle: None,
            event_rx: None,
            cmd_tx: None,
            _pump_handle: None,
            _sender_handle: None,
            command_error_rx: None,
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
            pending_by_number: HashMap::new(),
            window_layouts: HashMap::new(),
            expected_panes_per_window: HashMap::new(),
            initial_capture_pending: HashSet::new(),
            initial_capture_done: HashSet::new(),
            initial_capture_buf: HashMap::new(),
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

    /// 创建远程 SSH tmux 后端并 attach 到已有 session。
    ///
    /// SSH 的读写、pty 和 tmux -CC 参数仍由 `TmuxClient::spawn_ssh` 统一处理，
    /// 这里仅把 alias 写入客户端配置，避免平台前端自行解析控制协议。
    pub fn new_with_ssh_attach(alias: &str, target: &str) -> Self {
        let mut backend = Self::new(None);
        backend.config.ssh_alias = Some(alias.to_string());
        backend.config.mode = Some(ConnectMode::Attach {
            target: Some(target.to_string()),
        });
        backend
    }

    /// 创建后端并指定 new-session 模式 + session 名。
    pub fn new_with_session_name(socket: Option<&str>, name: &str) -> Self {
        let mut backend = Self::new(socket);
        backend.config.mode = Some(ConnectMode::NewSession {
            name: Some(name.to_string()),
            start_directory: None,
        });
        backend
    }

    /// 通过 SSH alias 在远端启动 tmux -CC（new-session 模式）。
    ///
    /// `ssh_alias` 是 `~/.ssh/config` 里的 Host 名；`socket` 是远端 tmux 的 `-L` socket 名（可选）。
    pub fn new_ssh(ssh_alias: &str, socket: Option<&str>) -> Self {
        let mut backend = Self::new(socket);
        backend.config.ssh_alias = Some(ssh_alias.to_string());
        backend
    }

    /// 通过 SSH alias 在远端 attach 已有 session。
    pub fn new_ssh_attach(ssh_alias: &str, socket: Option<&str>, target: &str) -> Self {
        let mut backend = Self::new_ssh(ssh_alias, socket);
        backend.config.mode = Some(ConnectMode::Attach {
            target: Some(target.to_string()),
        });
        backend
    }

    /// 创建新 session，并指定起始工作目录（session 名由 tmux 自动生成）。
    pub fn new_with_cwd(socket: Option<&str>, start_directory: Option<&str>) -> Self {
        let mut backend = Self::new(socket);
        backend.config.mode = Some(ConnectMode::NewSession {
            name: None,
            start_directory: start_directory.map(|s| s.to_string()),
        });
        backend
    }

    /// 虚拟 Window 的固定 id。一个 session 永远只有 1 个 Window。
    const VIRTUAL_WINDOW_ID: WindowId = WindowId(1);

    /// 确保虚拟 Window 存在（connect / SessionChanged 时调用）。
    fn ensure_virtual_window(&mut self) {
        let sess = self.active_session.unwrap_or(SessionId(0));
        if !self.windows.iter().any(|w| w.id == Self::VIRTUAL_WINDOW_ID) {
            self.windows.push(WindowInfo {
                id: Self::VIRTUAL_WINDOW_ID,
                name: format!("w{}", Self::VIRTUAL_WINDOW_ID.0),
                session: sess,
                active: true,
            });
        }
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == sess) {
            s.active_window = Some(Self::VIRTUAL_WINDOW_ID);
        }
    }

    /// 事件队列过长时丢弃最旧的 PaneOutput，避免挂起轮询时涨到数 GB。
    fn trim_event_queue(&mut self) {
        while self.events.len() > MAX_STATE_EVENTS {
            let Some(idx) = self
                .events
                .iter()
                .position(|e| matches!(e, StateChange::PaneOutput { .. }))
            else {
                break;
            };
            self.events.remove(idx);
        }
        // 若仍超限（几乎全是结构事件），硬裁最旧
        while self.events.len() > MAX_STATE_EVENTS {
            self.events.pop_front();
        }
    }

    /// 合并同一 tab 尚未交给前端的布局事件。
    ///
    /// 窗口 resize 会让 tmux 连续发送 layout-change；前端只需要最新完整
    /// layout。保留中间快照会让 GUI 反复重建 pane 树，表现为闪烁和比例跳动。
    fn push_layout_changed(&mut self, layout: TabLayout) {
        let tab = layout.tab;
        self.events.retain(
            |event| !matches!(event, StateChange::LayoutChanged { tab: old, .. } if *old == tab),
        );
        self.events
            .push_back(StateChange::LayoutChanged { tab, layout });
    }

    /// abort/卡死路径：按 `-L socket` 强制 kill-server，回收残留 tmux。
    fn force_cleanup_tmux_server(&self) {
        let mut socket: Option<&str> = None;
        let mut it = self.config.extra_args.iter();
        while let Some(a) = it.next() {
            if a == "-L" {
                socket = it.next().map(String::as_str);
                break;
            }
        }
        if let Some(s) = socket {
            let Ok(mut child) = Command::new(self.config.tmux_bin.as_deref().unwrap_or("tmux"))
                .args(["-L", s, "kill-server"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            else {
                return;
            };
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) | Err(_) => break,
                    Ok(None) if Instant::now() >= deadline => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                    Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                }
            }
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

    fn is_attach_mode(&self) -> bool {
        matches!(self.config.mode.as_ref(), Some(ConnectMode::Attach { .. }))
    }

    /// 处理一条 tmux Message，更新内部 state 并产生 StateChange。
    fn handle_message(&mut self, msg: Message) {
        match msg {
            Message::Output { pane, content, .. } => {
                // attach 的初始控制流可能先发一个 prompt，再由 list-panes
                // 查询完整屏幕。先暂存这段不完整输出（而不是直接丢弃），
                // capture-pane 返回后以完整快照初始化，并把暂存的实时增量
                // 拼到快照尾部；这样既保留完整屏幕又不丢查询期间的输出。
                if self.is_attach_mode() && !self.initial_capture_done.contains(&pane) {
                    // 若尚未发起 capture 查询（pending 未建立），说明此时
                    // 只是启动期提示；等 query_capture_pane 真正发出查询后再
                    // 开始缓冲，避免把启动 prompt 与屏幕内容混在一起。
                    if self.initial_capture_pending.contains(&pane) {
                        let buf = self.initial_capture_buf.entry(pane).or_default();
                        append_capped(buf, &content, MAX_PANE_OUTPUT_BYTES);
                        tracing::debug!(
                            target: "muxterm::tmux",
                            pane = pane.0,
                            len = content.len(),
                            "attach 快照查询期间暂存实时 %output"
                        );
                    } else {
                        tracing::debug!(
                            target: "muxterm::tmux",
                            pane = pane.0,
                            "attach 启动 prompt 已忽略（等待 capture 快照）"
                        );
                    }
                    return;
                }
                tracing::debug!(
                    target: "muxterm::tmux",
                    pane = pane.0,
                    len = content.len(),
                    "实时 %output 交付"
                );
                append_capped(
                    self.outputs.entry(pane).or_default(),
                    &content,
                    MAX_PANE_OUTPUT_BYTES,
                );
                self.events.push_back(StateChange::PaneOutput {
                    pane,
                    data: content,
                });
                self.trim_event_queue();
            }
            Message::LayoutChange {
                window,
                layout,
                visible_layout: _,
            } => {
                // `%layout-change` 携带的是最新完整布局。先保存它，再查询 pane
                // 几何；list-panes 返回后 rebuild_layout 会用这棵最新树建模。
                // 旧实现只发查询、不更新 window_layouts，导致随后仍用旧树或
                // fallback 平铺树渲染，尤其在 attach 后再次 split 时会暴露。
                tracing::debug!(
                    target: "muxterm::tmux",
                    window = window.0,
                    layout = %layout.raw,
                    "%layout-change 已保存并重新查询 pane"
                );
                self.window_layouts.insert(window, layout.raw.clone());
                if let Ok(tree) = parse_layout_tree(&layout.raw) {
                    self.expected_panes_per_window
                        .insert(window, collect_layout_leaves(&tree).len());
                }
                self.query_list_panes(window);
            }
            Message::WindowAdd { window } => {
                // tmux window → muxterm Tab（不是 Window！）
                let _sess = self.active_session.unwrap_or(SessionId(0));
                self.ensure_virtual_window();
                let tab_id = TabId(window.0);
                if !self.tabs.iter().any(|t| t.id == tab_id) {
                    self.tabs.push(TabInfo {
                        id: tab_id,
                        name: format!("t{}", window.0),
                        window: Self::VIRTUAL_WINDOW_ID, // 指向虚拟 Window
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
                    self.events.push_back(StateChange::TabAdded {
                        tab: tab_id,
                        window: Self::VIRTUAL_WINDOW_ID,
                    });
                }
                // 主动查询该 tmux window 的 pane
                self.query_list_panes(window);
            }
            Message::WindowClose { window } => {
                // tmux window 关闭 → muxterm Tab 关闭（虚拟 Window 不动）
                let tab_id = TabId(window.0);
                self.panes.retain(|p| p.tab != tab_id);
                self.layouts.remove(&tab_id);
                self.tabs.retain(|t| t.id != tab_id);
                self.events
                    .push_back(StateChange::TabClosed { tab: tab_id });
            }
            Message::WindowRenamed { window, name } => {
                // tmux window 重命名 → muxterm Tab 重命名
                let tab_id = TabId(window.0);
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                    t.name = name.clone();
                }
                self.events
                    .push_back(StateChange::TabRenamed { tab: tab_id, name });
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
                self.ensure_virtual_window();
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
                // 虚拟 Window 不动（永远 1 个）
                let tab_id = TabId(window.0);
                for t in self.tabs.iter_mut() {
                    t.active = t.id == tab_id;
                }
                if let Some(sess) = self.sessions.iter_mut().find(|s| s.id == session) {
                    sess.active_window = Some(Self::VIRTUAL_WINDOW_ID);
                }
                // 如果目标 tab 的 pane 数据为空，重新查询（兜底）
                let pane_count = self.panes.iter().filter(|p| p.tab == tab_id).count();
                if pane_count == 0 {
                    tracing::debug!(target: "muxterm::tmux", "切 tab 到 @{} 但 pane 为空，重新查询", window.0);
                    self.query_list_panes(window);
                }
                self.events.push_back(StateChange::ActiveTabChanged {
                    window: Self::VIRTUAL_WINDOW_ID,
                    tab: tab_id,
                });
            }
            Message::ExtendedOutput { .. }
            | Message::Pause { .. }
            | Message::Continue { .. }
            | Message::UnlinkedWindowAdd { .. }
            | Message::UnlinkedWindowClose { .. }
            | Message::ResponseBoundary(_)
            | Message::Unknown { .. } => {
                // 暂不处理（第一版只识别 %pause/%continue，安全忽略，不阻塞状态机）
            }
        }
    }

    /// drain event_rx 的 TmuxEvent，更新 state。
    fn pump_events(&mut self) {
        let mut command_errors = Vec::new();
        if let Some(rx) = self.command_error_rx.as_mut() {
            while let Ok(message) = rx.try_recv() {
                command_errors.push(message);
            }
        }
        for message in command_errors {
            tracing::error!(target: "muxterm::tmux", "发送 tmux 命令失败: {message}");
            self.status = BackendStatus::Error;
            self.events
                .push_back(StateChange::BackendStatusChanged(BackendStatus::Error));
        }

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
                                // tmux 串行执行命令：`%begin <n>` 到达时，队首查询即
                                // 该命令的响应槽。按 number 登记，end/error 时精确匹配，
                                // 避免高输出下 FIFO pop 错配。
                                if let Some(q) = self.pending_queries.pop_front() {
                                    self.pending_by_number.insert(b.number, q);
                                }
                            }
                            NotificationKind::End => {
                                let lines =
                                    self.response_accum.remove(&b.number).unwrap_or_default();
                                self.dispatch_response(b.number, lines);
                            }
                            NotificationKind::Error => {
                                self.handle_response_error(b.number);
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
    fn dispatch_response(&mut self, number: i64, lines: Vec<String>) {
        if let Some(query) = self.pending_by_number.remove(&number) {
            match query {
                PendingQuery::Ignore => {}
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
                PendingQuery::CapturePane { pane } => {
                    // capture-pane -p 按行返回当前可见屏幕；拼回 CRLF 后喂给
                    // terminal emulator。attach 初始阶段必须以快照替换此前
                    // 被抑制的 `%output`，不能因为已有 prompt 就跳过恢复。
                    let mut data = lines.join("\r\n").into_bytes();
                    if !data.is_empty() {
                        data.extend_from_slice(b"\r\n");
                    }
                    if self.is_attach_mode() {
                        self.initial_capture_pending.remove(&pane);
                        self.initial_capture_done.insert(pane);
                        // 把查询期间暂存的实时增量拼到快照尾部：快照是查询瞬间
                        // 的完整屏幕，实时增量是其后到达的追加输出，二者按序拼接
                        // 才不会丢数据、也不会把屏幕内容错位。
                        if let Some(live) = self.initial_capture_buf.remove(&pane) {
                            if !live.is_empty() {
                                data.extend_from_slice(&live);
                            }
                        }
                        let snapshot = if data.len() > MAX_PANE_OUTPUT_BYTES {
                            data[data.len() - MAX_PANE_OUTPUT_BYTES..].to_vec()
                        } else {
                            data
                        };
                        self.outputs.insert(pane, snapshot.clone());
                        if !snapshot.is_empty() {
                            self.events.push_back(StateChange::PaneOutput {
                                pane,
                                data: snapshot,
                            });
                            self.trim_event_queue();
                        }
                    } else if !data.is_empty()
                        && self
                            .outputs
                            .get(&pane)
                            .is_none_or(|output| output.is_empty())
                    {
                        append_capped(
                            self.outputs.entry(pane).or_default(),
                            &data,
                            MAX_PANE_OUTPUT_BYTES,
                        );
                        self.events
                            .push_back(StateChange::PaneOutput { pane, data });
                        self.trim_event_queue();
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

    /// 处理一条命令响应的 `%error` 边界。
    ///
    /// 出错时移除按 number 登记的查询，并确保 attach 的 capture 失败不会永久
    /// 抑制该 pane 的实时输出（否则会黑屏）。实时输出缓冲也随之清空。
    fn handle_response_error(&mut self, number: i64) {
        let _err_lines = self.response_accum.remove(&number).unwrap_or_default();
        if let Some(q) = self.pending_by_number.remove(&number) {
            match q {
                PendingQuery::CapturePane { pane } => {
                    // capture 失败时不能永久抑制该 pane 的后续输出；让实时流
                    // 继续恢复渲染。已暂存的实时增量在 `%output` 缓冲里，这里
                    // 直接丢弃（避免与后续 live 输出重复拼接）。
                    self.initial_capture_pending.remove(&pane);
                    self.initial_capture_done.insert(pane);
                    self.initial_capture_buf.remove(&pane);
                    tracing::warn!(
                        target: "muxterm::tmux",
                        "tmux 命令 {number} 的 pane @{} 屏幕恢复失败",
                        pane.0
                    );
                }
                other => {
                    tracing::warn!(
                        target: "muxterm::tmux",
                        "tmux 命令 {number} 出错（丢弃查询 {other:?}）",
                    );
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
        if let Some(expected) = self
            .expected_panes_per_window
            .get(&window)
            .copied()
            .filter(|count| *count > 0)
        {
            if new_panes.len() != expected {
                tracing::debug!(
                    target: "muxterm::tmux",
                    "忽略 window=@{} 的不完整 pane 快照: got={}, expected={}",
                    window.0,
                    new_panes.len(),
                    expected
                );
                self.query_list_panes(window);
                return;
            }
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
        // attach 的控制模式不一定会把当前屏幕历史作为 %output 推送；对尚无
        // 累计输出的 pane 查询一次可见屏幕。查询结果通过同一个事件队列回流，
        // 不阻塞 pane/layout 状态机。
        for pane in new_panes {
            self.query_capture_pane(pane.id);
        }
    }

    /// 解析 `list-windows -t <session> -F '#{window_id},#{window_name},#{window_active},#{window_layout},#{window_panes}'` 的响应。
    fn handle_list_windows_response(&mut self, lines: Vec<String>) {
        // tmux list-windows 返回所有 tmux window → 每个创建/更新一个 muxterm Tab
        // 虚拟 Window 不动（永远 1 个）
        self.ensure_virtual_window();
        for line in lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // window_layout 本身含逗号（如 `d67e,80x24,0,0{...}`），不能用 splitn(5)
            let Some((tmux_window, name, active, layout_str, panes_count)) =
                parse_list_windows_line(line)
            else {
                tracing::warn!(target: "muxterm::tmux", "list-windows 行解析失败: {line}");
                continue;
            };
            self.window_layouts.insert(tmux_window, layout_str);
            self.expected_panes_per_window
                .insert(tmux_window, panes_count);

            // tmux window → muxterm Tab
            let tab_id = TabId(tmux_window.0);
            if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                t.name = name.clone();
                t.active = active;
            } else {
                self.tabs.push(TabInfo {
                    id: tab_id,
                    name: name.clone(),
                    window: Self::VIRTUAL_WINDOW_ID, // 指向虚拟 Window
                    active,
                });
                self.events.push_back(StateChange::TabAdded {
                    tab: tab_id,
                    window: Self::VIRTUAL_WINDOW_ID,
                });
            }

            // 主动查询该 tmux window 的 panes
            self.query_list_panes(tmux_window);
        }
    }

    /// 发送 list-panes 查询（异步，通过 cmd_tx）。
    fn query_list_panes(&mut self, window: WindowId) {
        // 用 list-panes -t @N 查询单个 window 的 pane（默认格式不含 window_id）。
        if self.pending_queries.iter().any(|query| {
            matches!(query, PendingQuery::ListPanes { window: pending } if *pending == window)
        }) {
            return;
        }
        let line = format!("list-panes -t @{}\n", window.0);
        if self.dispatch_command(line).is_ok() {
            self.replace_last_pending(PendingQuery::ListPanes { window });
        }
    }

    /// 查询 pane 当前可见屏幕，用于 attach 初始渲染恢复。
    fn query_capture_pane(&mut self, pane: PaneId) {
        if !self.is_attach_mode() {
            return;
        }
        if self.pending_queries.iter().any(
            |query| matches!(query, PendingQuery::CapturePane { pane: pending } if *pending == pane),
        ) || self.initial_capture_done.contains(&pane)
        {
            return;
        }
        let line = format!("capture-pane -e -p -t %{}\n", pane.0);
        if self.dispatch_command(line).is_ok() {
            self.initial_capture_pending.insert(pane);
            self.replace_last_pending(PendingQuery::CapturePane { pane });
        }
    }

    /// 发送 list-sessions 查询（列出 tmux server 上所有 session）。
    fn query_list_sessions(&mut self) {
        let line = "list-sessions\n".to_string();
        if self.dispatch_command(line).is_ok() {
            self.replace_last_pending(PendingQuery::ListSessions);
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
            self.replace_last_pending(PendingQuery::ListWindows);
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
            self.push_layout_changed(layout);
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
        self.push_layout_changed(layout);
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
        self.push_layout_changed(layout);
    }

    /// 把一个命令异步发送给 tmux（通过 channel）。
    /// execute 是同步 fn，命令发送走后台 task。
    fn replace_last_pending(&mut self, query: PendingQuery) {
        if let Some(last) = self.pending_queries.back_mut() {
            *last = query;
        }
    }

    fn dispatch_command(&mut self, line: String) -> std::io::Result<()> {
        let Some(tx) = self.cmd_tx.as_ref() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "tmux 命令通道未建立",
            ));
        };
        // UnboundedSender 只会在 sender task 已退出时失败，不会在快速键入/粘贴
        // 时返回 WouldBlock 丢掉 shell 输入；实际写入仍由后台 task 串行化。
        tx.send(line).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                format!("命令通道已关闭: {e}"),
            )
        })?;
        // 所有 control-mode 命令都按 FIFO 占一个响应槽；查询调用方会
        // 立即把最后一个占位替换成具体 PendingQuery。
        self.pending_queries.push_back(PendingQuery::Ignore);
        Ok(())
    }

    /// 便捷：发送一个 TmuxCommand。
    fn dispatch_tmux_command(&mut self, command: &cmd::TmuxCommand) -> std::io::Result<()> {
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
        // 所有 tab 都属于虚拟 Window（window 字段 = VIRTUAL_WINDOW_ID）
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
        // 命令（尤其是逐字输入）必须按 FIFO 无损排队。bounded + try_send
        // 会在快速键入/粘贴时返回 WouldBlock，直接丢掉 shell 输入。
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<String>();
        let (command_error_tx, command_error_rx) = mpsc::unbounded_channel::<String>();
        let mut sender_handle = handle;
        let sender_join = tokio::spawn(async move {
            while let Some(line) = cmd_rx.recv().await {
                if let Err(error) = sender_handle.send_raw(&line).await {
                    let _ = command_error_tx.send(error.to_string());
                    break;
                }
            }
            // sender 结束后 detach + kill
            let _ = sender_handle.kill().await;
        });

        self.event_rx = Some(rx);
        self.cmd_tx = Some(cmd_tx);
        self.command_error_rx = Some(command_error_rx);
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
            // 短暂睡眠：仅 yield_now 忙等会饿死读循环，且拉长真实等待
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
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
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
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
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
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
                // tmux split-window 用 target pane 所在 tmux window
                // tab_id.0 = tmux window index → WindowId(tab_id.0) = tmux @N
                let tab_id = self.pane(&target).map(|p| p.tab).unwrap_or(TabId(0));
                let tmux_win = WindowId(tab_id.0); // tmux window id = @N
                let direction = match dir {
                    SplitDir::Horizontal => cmd::SplitDirection::Horizontal,
                    SplitDir::Vertical => cmd::SplitDirection::Vertical,
                };
                let name = command.as_ref().and_then(|c| c.first()).map(|s| s.as_str());
                let _ = workdir;
                let c = cmd::split_window(tmux_win, direction, name);
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
                // muxterm 只有 1 个虚拟 Window，CloseWindow 在 TmuxBackend 中
                // 实际是关闭 Tab（target.0 = tmux window index）
                let c = cmd::kill_window(*target);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::SwitchWindow { target } => {
                // muxterm 只有 1 个虚拟 Window，SwitchWindow 在 TmuxBackend 中
                // 实际是切换 Tab（target.0 = tmux window index）
                let c = cmd::select_window(*target);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::RenameWindow { target, name } => {
                // muxterm 只有 1 个虚拟 Window，RenameWindow 在 TmuxBackend 中
                // 实际是重命名 Tab（target.0 = tmux window index）
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
                let c = cmd::send_keys_bytes(*target, data);
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
            Task::ResizeClient { cols, rows } => {
                let c = cmd::refresh_client_size(*cols as u32, *rows as u32);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送 client resize 命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::ResizePaneAxis { target, dir, size } => {
                let c = match dir {
                    SplitDir::Horizontal => cmd::resize_pane(*target, Some(*size as u32), None),
                    SplitDir::Vertical => cmd::resize_pane(*target, None, Some(*size as u32)),
                };
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送 pane 轴向 resize 命令失败".into(),
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

            Task::Detach => {
                // 显式 detach 只关闭当前 control client，不杀 tmux server/session。
                let sess = self.active_session.unwrap_or(SessionId(0));
                let c = cmd::detach_client(sess);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送 detach-client 失败".into(),
                    });
                }
                // 关闭发送 channel：sender 会先写完已排队的 detach-client，
                // 然后只回收 `tmux -CC` control client，不触碰 session。
                self.cmd_tx.take();
                self.status = BackendStatus::Disconnected;
                self.events.push_back(StateChange::BackendStatusChanged(
                    BackendStatus::Disconnected,
                ));
                TaskOutcome::Done
            }

            Task::Shutdown => {
                // 生命周期清理仍使用独立的 shutdown 状态；正常的 tmux
                // shutdown 也先 detach control client，再回收本地进程句柄。
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
        // 已经由显式 Task::Detach 关闭 channel 时，不再重复发送命令。
        if self.cmd_tx.is_some() {
            self.execute(&Task::Shutdown)?;
        }
        // 关闭命令通道，sender task 收到 None 后会 kill tmux 子进程并退出
        self.cmd_tx.take();
        // 等待 sender task 结束；pty 写卡死时 abort，避免测试/CI 无限挂起
        let mut aborted = false;
        if let Some(mut h) = self._sender_handle.take() {
            tokio::select! {
                r = &mut h => {
                    let _ = r;
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => {
                    tracing::warn!(target = "muxterm::tmux_backend", "shutdown: sender task 超时，abort");
                    h.abort();
                    aborted = true;
                }
            }
        }
        if aborted {
            // abort 可能跳过 sender 末尾的 kill()；强制回收 server / 子进程
            self.force_cleanup_tmux_server();
        }
        // 丢掉事件接收端，让读线程/读 task 在 send 失败后退出，停止无界积压
        self.event_rx.take();
        self.outputs.clear();
        self.events.clear();
        self.status = BackendStatus::Exited;
        self.events
            .push_back(StateChange::BackendStatusChanged(BackendStatus::Exited));
        Ok(())
    }
}

/// 解析 list-windows -F 单行。
///
/// 格式：`@N,name,active,LAYOUT,panes`
/// LAYOUT 含逗号，因此前三个字段用 `split_once`，最后一个用 `rsplit_once`。
fn parse_list_windows_line(line: &str) -> Option<(WindowId, String, bool, String, usize)> {
    let (id_str, rest) = line.split_once(',')?;
    let (name, rest) = rest.split_once(',')?;
    let (active_str, rest) = rest.split_once(',')?;
    let (layout_str, panes_str) = rest.rsplit_once(',')?;
    let tmux_window = WindowId::parse(id_str).ok()?;
    let active = active_str == "1";
    let panes_count = panes_str.parse().ok()?;
    Some((
        tmux_window,
        name.to_string(),
        active,
        layout_str.to_string(),
        panes_count,
    ))
}

/// 把 LayoutTree（几何拓扑）转成 LayoutNode（pane id 树），按几何位置匹配。
fn layout_tree_to_node(tree: &LayoutTree, panes: &[PaneInfo]) -> Option<LayoutNode> {
    let leaves = collect_layout_leaves(tree);
    if leaves.len() != panes.len() {
        return None;
    }
    // 优先用 layout 叶子的 flags（tmux pane index）映射 PaneId
    let pane_by_idx: HashMap<u32, PaneId> = panes.iter().map(|p| (p.id.0, p.id)).collect();
    let mut mapping = HashMap::new();
    let mapped_by_flags = leaves.iter().all(|leaf| {
        if let Some(&pid) = pane_by_idx.get(&leaf.flags) {
            mapping.insert((leaf.x, leaf.y), pid);
            true
        } else {
            false
        }
    });
    if !mapped_by_flags {
        mapping.clear();
        for (leaf, pane) in leaves.iter().zip(panes.iter()) {
            mapping.insert((leaf.x, leaf.y), pane.id);
        }
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
                ratio: layout_split_ratio(tree, a, b),
                first: Box::new(first),
                second: Box::new(second),
            })
        }
    }
}

/// 从 tmux 子节点几何计算 first 的布局比例（0..=1000）。
///
/// tmux 的 layout 几何包含分隔线两侧的 pane 尺寸，因此用两个子节点在
/// 当前分割轴上的尺寸计算比例即可得到稳定的近似值；不能固定写成 500，
/// 否则 attach 后的非对称布局会被 GUI 重新均分。
fn layout_split_ratio(tree: &LayoutTree, first: &LayoutTree, second: &LayoutTree) -> u16 {
    let (first_size, second_size) = match tree.dir {
        SplitDir::Horizontal => (first.cols, second.cols),
        SplitDir::Vertical => (first.rows, second.rows),
    };
    let total = first_size.saturating_add(second_size);
    if total == 0 {
        return 500;
    }
    ((first_size.saturating_mul(1000) / total).clamp(50, 950)) as u16
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
fn key_event_to_tmux_key(ev: &crate::core::protocol::terminal::input::KeyEvent) -> cmd::Key {
    use crate::core::protocol::terminal::input::{ArrowDir, KeyEvent};
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

    #[test]
    fn parse_list_windows_line_keeps_full_layout_with_commas() {
        let line = "@1,zsh,1,d67e,80x24,0,0{40x24,0,0,0,39x24,41,0[39x12,41,0,1,39x11,41,13,2]},3";
        let (wid, name, active, layout, panes) = parse_list_windows_line(line).unwrap();
        assert_eq!(wid, WindowId(1));
        assert_eq!(name, "zsh");
        assert!(active);
        assert_eq!(
            layout,
            "d67e,80x24,0,0{40x24,0,0,0,39x24,41,0[39x12,41,0,1,39x11,41,13,2]}"
        );
        assert_eq!(panes, 3);
        // 完整 layout 应能解析出嵌套 vertical
        let tree = parse_layout_tree(&layout).unwrap();
        assert_eq!(tree.dir, crate::core::model::layout::SplitDir::Horizontal);
        let right = tree.children.as_ref().unwrap().1.as_ref();
        assert_eq!(right.dir, crate::core::model::layout::SplitDir::Vertical);
    }

    #[test]
    fn parse_list_windows_line_rejects_short() {
        assert!(parse_list_windows_line("@1,name").is_none());
        assert!(parse_list_windows_line("").is_none());
    }

    #[test]
    fn command_queue_accepts_high_frequency_input_without_would_block() {
        let mut backend = TmuxBackend::new(None);
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        backend.cmd_tx = Some(tx);
        backend.status = BackendStatus::Connected;

        let burst = 4_096;
        for _ in 0..burst {
            let outcome = backend
                .execute(&Task::WriteRaw {
                    target: PaneId(1),
                    data: b"x".to_vec(),
                })
                .unwrap();
            assert_eq!(outcome, TaskOutcome::Done);
        }

        let mut received = 0;
        while rx.try_recv().is_ok() {
            received += 1;
        }
        assert_eq!(received, burst, "高频输入不能因队列满而丢失");
    }

    #[test]
    fn asynchronous_command_error_becomes_backend_status_instead_of_panic() {
        let mut backend = TmuxBackend::new(None);
        let (tx, rx) = mpsc::unbounded_channel::<String>();
        backend.command_error_rx = Some(rx);
        tx.send("pty 已关闭".into()).unwrap();

        backend.pump_events();

        assert_eq!(backend.status, BackendStatus::Error);
        assert!(backend.events.iter().any(|event| matches!(
            event,
            StateChange::BackendStatusChanged(BackendStatus::Error)
        )));
    }

    #[test]
    fn layout_change_rebuilds_from_latest_nested_tmux_tree() {
        let mut b = TmuxBackend::new(None);
        let window = WindowId(0);
        let latest = "1268,140x30,0,0{70x30,0,0,0,69x30,71,0[69x15,71,0,1,69x14,71,16,2]}";

        b.handle_message(Message::LayoutChange {
            window,
            layout: crate::core::runtime::tmux::protocol::LayoutChange::parse(latest).unwrap(),
            visible_layout: None,
        });

        // 没有命令通道的单元测试里 query_list_panes 会被跳过，但最新 raw
        // 仍必须保留下来，随后 list-panes 响应才能按新树建模。
        assert_eq!(
            b.window_layouts.get(&window).map(String::as_str),
            Some(latest)
        );

        b.handle_list_panes_response(
            window,
            vec![
                "0: [70x30] %0 (active)".into(),
                "1: [69x15] %1".into(),
                "2: [69x14] %2".into(),
            ],
        );

        let tree = &b.layouts[&TabId(0)].tree;
        let LayoutNode::Split {
            dir: SplitDir::Horizontal,
            ratio: root_ratio,
            first,
            second,
        } = tree
        else {
            panic!("根节点应为左右 split: {tree:?}");
        };
        assert!((500..=510).contains(root_ratio));
        assert!(matches!(first.as_ref(), LayoutNode::Leaf(PaneId(0))));
        let LayoutNode::Split {
            dir: SplitDir::Vertical,
            ratio: nested_ratio,
            first: nested_first,
            second: nested_second,
        } = second.as_ref()
        else {
            panic!("右子树应为上下 split: {second:?}");
        };
        assert!((510..=525).contains(nested_ratio));
        assert!(matches!(nested_first.as_ref(), LayoutNode::Leaf(PaneId(1))));
        assert!(matches!(
            nested_second.as_ref(),
            LayoutNode::Leaf(PaneId(2))
        ));
    }

    #[test]
    fn incomplete_pane_snapshot_does_not_collapse_layout() {
        let mut b = TmuxBackend::new(None);
        let window = WindowId(0);
        let layout = "1268,140x30,0,0{70x30,0,0,0,69x30,71,0[69x15,71,0,1,69x14,71,16,2]}";
        b.handle_message(Message::LayoutChange {
            window,
            layout: crate::core::runtime::tmux::protocol::LayoutChange::parse(layout).unwrap(),
            visible_layout: None,
        });

        b.handle_list_panes_response(
            window,
            vec!["0: [70x30] %0 (active)".into(), "1: [69x15] %1".into()],
        );
        assert!(!b.layouts.contains_key(&TabId(0)));
        assert!(b.panes.is_empty());
    }

    #[test]
    fn pending_layout_events_are_coalesced_per_tab() {
        let mut b = TmuxBackend::new(None);
        let window = WindowId(0);
        let layout = "1268,140x30,0,0{70x30,0,0,0,69x30,71,0[69x15,71,0,1,69x14,71,16,2]}";
        let message = || Message::LayoutChange {
            window,
            layout: crate::core::runtime::tmux::protocol::LayoutChange::parse(layout).unwrap(),
            visible_layout: None,
        };
        b.handle_message(message());
        b.handle_list_panes_response(
            window,
            vec![
                "0: [70x30] %0 (active)".into(),
                "1: [69x15] %1".into(),
                "2: [69x14] %2".into(),
            ],
        );
        b.handle_message(message());
        b.handle_list_panes_response(
            window,
            vec![
                "0: [70x30] %0 (active)".into(),
                "1: [69x15] %1".into(),
                "2: [69x14] %2".into(),
            ],
        );
        let count = b
            .events
            .iter()
            .filter(
                |event| matches!(event, StateChange::LayoutChanged { tab, .. } if *tab == TabId(0)),
            )
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn capture_pane_response_restores_existing_screen_without_double_feed() {
        let mut b = TmuxBackend::new(None);
        let pane = PaneId(7);
        b.pending_by_number
            .insert(1, PendingQuery::CapturePane { pane });

        b.dispatch_response(1, vec!["\u{1b}[32mrestored shell".into(), "prompt$".into()]);
        assert_eq!(
            b.outputs.get(&pane).unwrap(),
            b"\x1b[32mrestored shell\r\nprompt$\r\n"
        );
        assert!(b.events.iter().any(|event| matches!(
            event,
            StateChange::PaneOutput { pane: event_pane, data }
                if *event_pane == pane && data.starts_with(b"\x1b[32mrestored")
        )));

        // 若 tmux 已经主动推送了 %output，capture 快照不能再次追加。
        b.pending_by_number
            .insert(2, PendingQuery::CapturePane { pane });
        b.dispatch_response(2, vec!["duplicate".into()]);
        assert_eq!(
            b.outputs.get(&pane).unwrap(),
            b"\x1b[32mrestored shell\r\nprompt$\r\n"
        );
    }

    #[test]
    fn attach_initial_output_waits_for_full_capture_snapshot() {
        let mut b = TmuxBackend::new_with_attach(None, "existing");
        let pane = PaneId(3);

        // attach 初始流里的 prompt 不是完整屏幕，不能先暴露给 GUI。
        b.handle_message(Message::Output {
            pane,
            content: b"prompt$ ".to_vec(),
            raw_content: "prompt$ ".into(),
        });
        assert!(!b.outputs.contains_key(&pane));
        assert!(b.events.is_empty());

        b.pending_by_number
            .insert(1, PendingQuery::CapturePane { pane });
        b.dispatch_response(1, vec!["old command".into(), "prompt$ ".into()]);
        assert_eq!(
            b.outputs.get(&pane).unwrap(),
            b"old command\r\nprompt$ \r\n"
        );

        // 快照完成后，后续输出恢复为普通增量。
        b.handle_message(Message::Output {
            pane,
            content: b"live\r\n".to_vec(),
            raw_content: "live\\r\\n".into(),
        });
        assert!(b.outputs.get(&pane).unwrap().ends_with(b"live\r\n"));
    }

    #[test]
    fn command_response_placeholder_does_not_consume_capture_query() {
        let mut b = TmuxBackend::new_with_attach(None, "existing");
        let pane = PaneId(4);
        b.pending_by_number.insert(1, PendingQuery::Ignore);
        b.pending_by_number
            .insert(2, PendingQuery::CapturePane { pane });

        // split/send-keys 等普通命令的响应先到，不能把 capture 查询错配掉。
        b.dispatch_response(1, vec!["ignored".into()]);
        b.dispatch_response(2, vec!["restored".into()]);

        assert_eq!(b.outputs.get(&pane).unwrap(), b"restored\r\n");
    }

    #[test]
    fn attach_live_output_during_capture_is_appended_after_snapshot() {
        let mut b = TmuxBackend::new_with_attach(None, "existing");
        let pane = PaneId(5);

        // 发起 capture 查询后，查询期间到达的实时输出先暂存，不直接暴露。
        b.initial_capture_pending.insert(pane);
        b.handle_message(Message::Output {
            pane,
            content: b"live-during-capture\r\n".to_vec(),
            raw_content: "live-during-capture\\r\\n".into(),
        });
        assert!(!b.outputs.contains_key(&pane));
        assert!(b.events.is_empty());

        // capture 快照返回：完整屏幕 + 查询期间的实时增量拼接。
        b.pending_by_number
            .insert(1, PendingQuery::CapturePane { pane });
        b.dispatch_response(1, vec!["screen line".into()]);
        assert_eq!(
            b.outputs.get(&pane).unwrap(),
            b"screen line\r\nlive-during-capture\r\n"
        );

        // 之后 %output 恢复为普通增量。
        b.handle_message(Message::Output {
            pane,
            content: b"after-capture\r\n".to_vec(),
            raw_content: "after-capture\\r\\n".into(),
        });
        assert!(b
            .outputs
            .get(&pane)
            .unwrap()
            .ends_with(b"after-capture\r\n"));
    }

    #[test]
    fn attach_capture_failure_recovers_live_output_without_black_screen() {
        let mut b = TmuxBackend::new_with_attach(None, "existing");
        let pane = PaneId(6);

        b.initial_capture_pending.insert(pane);
        // %error 而不是 %end：capture 失败。
        b.pending_by_number
            .insert(1, PendingQuery::CapturePane { pane });
        b.dispatch_response(1, vec!["error".into()]);
        b.handle_response_error(1);

        // 失败后不能永久抑制 pane 输出：后续实时输出必须照常渲染。
        assert!(b.initial_capture_done.contains(&pane));
        b.handle_message(Message::Output {
            pane,
            content: b"live-after-error\r\n".to_vec(),
            raw_content: "live-after-error\\r\\n".into(),
        });
        assert!(b
            .outputs
            .get(&pane)
            .unwrap()
            .ends_with(b"live-after-error\r\n"));
        assert!(b.events.iter().any(|event| matches!(
            event,
            StateChange::PaneOutput { pane: ep, data }
                if *ep == pane && data.ends_with(b"live-after-error\r\n")
        )));
    }

    #[test]
    fn response_number_matching_does_not_misassign_interleaved_queries() {
        // 高输出下多个 %begin/%end 交叠时，必须按 number 精确匹配，而不是 FIFO。
        let mut b = TmuxBackend::new(None);
        let p1 = PaneId(10);
        let p2 = PaneId(11);

        // begin 1（CapturePane p1）、begin 2（CapturePane p2）
        b.pending_by_number
            .insert(1, PendingQuery::CapturePane { pane: p1 });
        b.pending_by_number
            .insert(2, PendingQuery::CapturePane { pane: p2 });

        // 响应乱序返回：end 2 先到，end 1 后到。
        b.dispatch_response(2, vec!["second screen".into()]);
        b.dispatch_response(1, vec!["first screen".into()]);

        assert_eq!(b.outputs.get(&p2).unwrap(), b"second screen\r\n");
        assert_eq!(b.outputs.get(&p1).unwrap(), b"first screen\r\n");
    }

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

    fn cleanup(socket: &str) {
        let _ = std::process::Command::new("tmux")
            .args(["-L", socket, "kill-server"])
            .output();
    }

    /// 整个用例上限，防止 connect/shutdown/pty 写卡住拖死 CI（曾 15min 挂起）。
    const TMUX_TEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn connect_establishes_session_and_window() {
        let socket = unique_socket();
        let run = async {
            let mut b = TmuxBackend::new(Some(&socket));
            b.connect().await.unwrap_or_else(|e| {
                eprintln!("skip: tmux 不可用: {e}");
            });
            if b.status() != BackendStatus::Connected {
                return;
            }
            assert_eq!(b.status(), BackendStatus::Connected);
            let events = b.take_events();
            assert!(events.iter().any(|e| matches!(
                e,
                StateChange::BackendStatusChanged(BackendStatus::Connected)
            )));
            assert!(!b.sessions().is_empty());
            assert!(!b.windows.is_empty());
            let _ = b.shutdown().await;
        };
        let timed = tokio::time::timeout(TMUX_TEST_TIMEOUT, run).await;
        cleanup(&socket);
        if timed.is_err() {
            panic!("connect_establishes_session_and_window 超时");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn new_window_via_tmux() {
        let socket = unique_socket();
        let run = async {
            let mut b = TmuxBackend::new(Some(&socket));
            if b.connect().await.is_err() {
                return;
            }
            let _ = b.take_events();
            let initial_tabs = b.tabs.len();
            b.execute(&Task::NewWindow {
                name: Some("test-win".into()),
                command: None,
                workdir: None,
            })
            .unwrap();
            let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(3);
            loop {
                let _ = b.take_events();
                if b.tabs.len() > initial_tabs {
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    break;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
            assert!(
                b.tabs.len() > initial_tabs,
                "新 tab（tmux window）未建立: tabs={}",
                b.tabs.len()
            );
            assert_eq!(b.windows.len(), 1, "虚拟 Window 应始终只有 1 个");
            let _ = b.shutdown().await;
        };
        let timed = tokio::time::timeout(TMUX_TEST_TIMEOUT, run).await;
        cleanup(&socket);
        if timed.is_err() {
            panic!("new_window_via_tmux 超时");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn send_keys_does_not_error() {
        let socket = unique_socket();
        let run = async {
            let mut b = TmuxBackend::new(Some(&socket));
            if b.connect().await.is_err() {
                return;
            }
            let _ = b.take_events();
            let pane = b.active_pane().map(|p| p.id).unwrap_or(PaneId(0));
            let outcome = b
                .execute(&Task::SendKeys {
                    target: pane,
                    keys: vec![crate::core::protocol::terminal::input::KeyEvent::Char('x')],
                })
                .unwrap();
            assert_eq!(outcome, TaskOutcome::Done);
            let _ = b.shutdown().await;
        };
        let timed = tokio::time::timeout(TMUX_TEST_TIMEOUT, run).await;
        cleanup(&socket);
        if timed.is_err() {
            panic!("send_keys_does_not_error 超时（tmux socket/shutdown 挂起）");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn split_pane_dispatched() {
        let socket = unique_socket();
        let run = async {
            let mut b = TmuxBackend::new(Some(&socket));
            if b.connect().await.is_err() {
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
        };
        let timed = tokio::time::timeout(TMUX_TEST_TIMEOUT, run).await;
        cleanup(&socket);
        if timed.is_err() {
            panic!("split_pane_dispatched 超时");
        }
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

    /// 回归：大量 %output 不得把 outputs/events 撑到数 GB（曾观测挂起时 ~20GB）。
    #[test]
    fn pane_output_accumulation_is_capped() {
        use crate::core::buffer_cap::MAX_PANE_OUTPUT_BYTES;
        use crate::core::runtime::tmux::protocol::Message;

        let mut b = TmuxBackend::new(None);
        let pane = PaneId(42);
        let chunk = vec![b'x'; 64 * 1024];
        // 灌入远超上限的数据
        for _ in 0..80 {
            b.handle_message(Message::Output {
                pane,
                content: chunk.clone(),
                raw_content: String::new(),
            });
        }
        let stored = b.outputs.get(&pane).map(|v| v.len()).unwrap_or(0);
        assert!(
            stored <= MAX_PANE_OUTPUT_BYTES,
            "outputs 应有界，实际 {stored} > {MAX_PANE_OUTPUT_BYTES}"
        );
        assert!(
            b.events.len() <= crate::core::buffer_cap::MAX_STATE_EVENTS,
            "events 应有界，实际 {}",
            b.events.len()
        );
    }

    /// 流控：%pause / %continue 被安全忽略，不阻塞后续 %output 累积与状态机。
    #[test]
    fn flow_control_pause_continue_safely_ignored() {
        use crate::core::runtime::tmux::protocol::Message;

        let mut b = TmuxBackend::new(None);
        let pane = PaneId(7);

        // 在 %output 之间穿插 %pause / %continue，验证不破坏输出累积
        b.handle_message(Message::Output {
            pane,
            content: b"a".to_vec(),
            raw_content: String::new(),
        });
        b.handle_message(Message::Pause { args: "100".into() });
        b.handle_message(Message::Output {
            pane,
            content: b"b".to_vec(),
            raw_content: String::new(),
        });
        b.handle_message(Message::Continue { args: "100".into() });
        b.handle_message(Message::Output {
            pane,
            content: b"c".to_vec(),
            raw_content: String::new(),
        });

        // 三条 output 都应累积，未被 pause/continue 截断或丢弃
        let out = b.outputs.get(&pane).cloned().unwrap_or_default();
        assert_eq!(out, b"abc", "pause/continue 不应破坏 %output 累积");
        // 事件队列里应有对应数量的 PaneOutput
        let out_events = b
            .events
            .iter()
            .filter(|e| matches!(e, crate::core::model::state::StateChange::PaneOutput { .. }))
            .count();
        assert_eq!(out_events, 3, "应有 3 个 PaneOutput 事件");
    }

    /// %window-pane-changed：切换某 window 的 active pane，应触发 ActivePaneChanged。
    #[test]
    fn window_pane_changed_updates_active_pane() {
        use crate::core::model::state::StateChange;
        use crate::core::runtime::tmux::protocol::Message;

        let mut b = TmuxBackend::new(None);
        // 预置一个 window + 两个 pane 在同一 tab
        let win = crate::core::types::WindowId(0);
        let tab = crate::core::types::TabId(0);
        b.tabs.push(crate::core::model::state::TabInfo {
            id: tab,
            name: "t0".into(),
            window: crate::core::runtime::tmux::backend::TmuxBackend::VIRTUAL_WINDOW_ID,
            active: true,
        });
        b.panes.push(crate::core::model::state::PaneInfo {
            id: crate::core::types::PaneId(1),
            tab,
            cols: 40,
            rows: 24,
            active: true,
            title: "p1".into(),
        });
        b.panes.push(crate::core::model::state::PaneInfo {
            id: crate::core::types::PaneId(2),
            tab,
            cols: 40,
            rows: 24,
            active: false,
            title: "p2".into(),
        });

        b.handle_message(Message::WindowPaneChanged {
            window: win,
            pane: crate::core::types::PaneId(2),
        });

        // pane 2 应变为 active
        let p2 = b
            .panes
            .iter()
            .find(|p| p.id == crate::core::types::PaneId(2))
            .unwrap();
        assert!(p2.active, "window-pane-changed 后 pane2 应 active");
        let p1 = b
            .panes
            .iter()
            .find(|p| p.id == crate::core::types::PaneId(1))
            .unwrap();
        assert!(!p1.active, "pane1 应不再 active");
        // 应有 ActivePaneChanged 事件
        assert!(
            b.events.iter().any(|e| matches!(e, StateChange::ActivePaneChanged { pane, .. } if *pane == crate::core::types::PaneId(2))),
            "应有 ActivePaneChanged(pane2)"
        );
    }

    /// %session-window-changed：切换 session 的 active window → active tab 切换。
    #[test]
    fn session_window_changed_updates_active_tab() {
        use crate::core::model::state::StateChange;
        use crate::core::runtime::tmux::protocol::Message;

        let mut b = TmuxBackend::new(None);
        let session = crate::core::types::SessionId(0);
        b.sessions.push(crate::core::model::state::SessionInfo {
            id: session,
            name: "s0".into(),
            active_window: None,
        });
        // 预置两个 tab（对应两个 tmux window @0 @1）
        for (id, active) in [(0u32, true), (1, false)] {
            b.tabs.push(crate::core::model::state::TabInfo {
                id: crate::core::types::TabId(id),
                name: format!("t{id}"),
                window: crate::core::runtime::tmux::backend::TmuxBackend::VIRTUAL_WINDOW_ID,
                active,
            });
        }

        b.handle_message(Message::SessionWindowChanged {
            session,
            window: crate::core::types::WindowId(1),
        });

        // tab1 应变为 active，tab0 不再 active
        let t1 = b
            .tabs
            .iter()
            .find(|t| t.id == crate::core::types::TabId(1))
            .unwrap();
        assert!(t1.active, "session-window-changed 后 tab1 应 active");
        let t0 = b
            .tabs
            .iter()
            .find(|t| t.id == crate::core::types::TabId(0))
            .unwrap();
        assert!(!t0.active, "tab0 应不再 active");
        // 应有 ActiveTabChanged 事件
        assert!(
            b.events.iter().any(|e| matches!(e, StateChange::ActiveTabChanged { tab, .. } if *tab == crate::core::types::TabId(1))),
            "应有 ActiveTabChanged(tab1)"
        );
    }

    /// %extended-output（hyperlink 等）被安全忽略，不破坏 %output 累积或状态机。
    #[test]
    fn extended_output_safely_ignored() {
        use crate::core::runtime::tmux::protocol::Message;

        let mut b = TmuxBackend::new(None);
        let pane = PaneId(9);

        b.handle_message(Message::Output {
            pane,
            content: b"x".to_vec(),
            raw_content: String::new(),
        });
        // 穿插一个 hyperlink 类的 %extended-output
        b.handle_message(Message::ExtendedOutput {
            pane,
            output_type: "hyperlink".into(),
            args: "file:///tmp".into(),
        });
        b.handle_message(Message::Output {
            pane,
            content: b"y".to_vec(),
            raw_content: String::new(),
        });

        // output 仍完整累积（xy），extended-output 不打断
        let out = b.outputs.get(&pane).cloned().unwrap_or_default();
        assert_eq!(out, b"xy", "%extended-output 不应破坏 %output 累积");
    }

    /// 布局变化与 %output 交织：%layout-change 不应重置已累积的 pane 输出。
    #[test]
    fn layout_change_does_not_reset_pane_output() {
        use crate::core::runtime::tmux::protocol::Message;

        let mut b = TmuxBackend::new(None);
        let pane = PaneId(3);

        // 先灌入一些输出
        b.handle_message(Message::Output {
            pane,
            content: b"before-layout".to_vec(),
            raw_content: String::new(),
        });

        // 插入 %layout-change（带合法 layout 字符串）
        b.handle_message(Message::LayoutChange {
            window: crate::core::types::WindowId(0),
            layout: crate::core::runtime::tmux::protocol::LayoutChange::parse("80x24,0,0,0")
                .unwrap(),
            visible_layout: None,
        });

        // 布局变化后再来输出
        b.handle_message(Message::Output {
            pane,
            content: b"-after-layout".to_vec(),
            raw_content: String::new(),
        });

        // 输出累积应完整（前 + 后），布局变化不重置
        let out = b.outputs.get(&pane).cloned().unwrap_or_default();
        assert_eq!(
            String::from_utf8_lossy(&out),
            "before-layout-after-layout",
            "layout-change 不应重置 pane 输出累积"
        );
    }

    /// 回归：shutdown 必须在有限时间内返回（含清理 outputs）。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn shutdown_completes_and_clears_buffers() {
        let socket = unique_socket();
        let run = async {
            let mut b = TmuxBackend::new(Some(&socket));
            if b.connect().await.is_err() {
                return;
            }
            // 人为塞一点输出缓冲
            b.handle_message(crate::core::runtime::tmux::protocol::Message::Output {
                pane: PaneId(1),
                content: vec![b'z'; 1024],
                raw_content: String::new(),
            });
            assert!(!b.outputs.is_empty());
            let _ = b.shutdown().await;
            assert!(b.outputs.is_empty(), "shutdown 后应清空 outputs");
            assert_eq!(b.status(), BackendStatus::Exited);
        };
        let timed = tokio::time::timeout(TMUX_TEST_TIMEOUT, run).await;
        cleanup(&socket);
        assert!(timed.is_ok(), "shutdown_completes_and_clears_buffers 超时");
    }
}
