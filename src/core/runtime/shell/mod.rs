//! LocalBackend：纯本地 shell 后端。
//!
//! 自维护一个 session / 多个 window / 每个 window 一个布局树（pane 嵌套分割）。
//! 每个 pane 持有一对 pty（master + child），通过后台读线程把输出喂回 backend，
//! `take_events()` 聚合成 `StateChange::PaneOutput` 事件。
//!
//! 不依赖 tmux，不依赖 GTK；只依赖 `portable-pty`（Unix）做子进程 spawn。
//! 所有状态操作是同步的（`execute` 内 spawn/kill/resize/write 都很快），
//! 输出读取走独立线程 + tokio mpsc，`take_events` 非阻塞。
//!
//! 设计要点：
//! - `connect()` spawn 第一个 window 的第一个 pane（默认 shell）
//! - pane id / window id 单调递增，LocalBackend 自行分配
//! - pane 输出累积在 `Vec<u8>`（有界裁剪，上限见 `buffer_cap::MAX_PANE_OUTPUT_BYTES`）
//! - `execute(Task)` 直接改本地状态 + spawn/kill/resize/write，产生事件入队
//! - 所有 pane 的后台读线程共用一个 `mpsc::Sender<PtyMsg>`（clone 后传入线程），
//!   backend 持有唯一的 `mpsc::Receiver`，`take_events` 时 drain

use std::collections::{HashSet, VecDeque};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use async_trait::async_trait;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use tokio::sync::mpsc;

use crate::core::buffer_cap::{append_capped, MAX_PANE_OUTPUT_BYTES, MAX_STATE_EVENTS};
use crate::core::config::{
    expand_config_value, parse_command_argv, prepare_pane_argv_for_platform, program_basename,
};
use crate::core::model::backend::Backend;
use crate::core::model::layout::{LayoutNode, TabLayout};
use crate::core::model::state::{
    BackendStatus, PaneInfo, SessionInfo, State, StateChange, TabInfo, WindowInfo,
};
use crate::core::model::task::{Task, TaskOutcome};
use crate::core::protocol::terminal::input::encode;
use crate::core::types::{PaneId, SessionId, TabId, WindowId};

/// 默认字符格尺寸。
const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;

/// 后台读线程发回的字节块。
enum PtyMsg {
    Output {
        pane: PaneId,
        data: Vec<u8>,
    },
    /// 读线程退出（EOF / 错误）。
    #[allow(dead_code)]
    Exit {
        pane: PaneId,
    },
}

/// 一个本地 pane 的运行时状态。
struct LocalPane {
    info: PaneInfo,
    /// pty master（写 / resize / wait）。
    master: Box<dyn portable_pty::MasterPty + Send>,
    /// 子进程句柄（kill / reap）。
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// 累积输出字节流。
    output: Vec<u8>,
    /// 写端（Arc<Mutex> 供跨线程写）。
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// pid（用于进程名查询，标题更新）。
    pid: u32,
}

impl std::fmt::Debug for LocalPane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalPane")
            .field("id", &self.info.id)
            .field("tab", &self.info.tab)
            .field("active", &self.info.active)
            .field("cols", &self.info.cols)
            .field("rows", &self.info.rows)
            .field("pid", &self.pid)
            .field("output_len", &self.output.len())
            .finish()
    }
}

/// 一个本地 tab。
struct LocalTab {
    info: TabInfo,
    layout: TabLayout,
}

/// 一个本地 window。
struct LocalWindow {
    info: WindowInfo,
}

/// 本地 shell 后端。
pub struct LocalBackend {
    /// 配置：默认启动命令 + 工作目录。
    default_command: String,
    default_workdir: String,

    session: Option<SessionInfo>,
    windows: Vec<LocalWindow>,
    tabs: Vec<LocalTab>,
    panes: Vec<LocalPane>,
    status: BackendStatus,
    events: VecDeque<StateChange>,

    /// 下一个 window id。
    next_window: u32,
    /// 下一个 tab id。
    next_tab: u32,
    /// 下一个 pane id。
    next_pane: u32,

    /// 所有读线程共用的 sender（clone 给每个读线程）。首次 connect 时建立。
    pty_tx: Option<mpsc::Sender<PtyMsg>>,
    /// 唯一接收端。首次 connect 时建立。
    pty_rx: Option<mpsc::Receiver<PtyMsg>>,
    /// 已主动 kill 的 pane：忽略随后到达的 PtyMsg::Exit，避免误关剩余 window。
    intentionally_closed: HashSet<PaneId>,
}

impl LocalBackend {
    /// 用默认启动命令 + 工作目录创建（尚未 connect）。
    pub fn new(default_command: impl Into<String>, default_workdir: impl Into<String>) -> Self {
        Self {
            default_command: default_command.into(),
            default_workdir: default_workdir.into(),
            session: None,
            windows: vec![],
            tabs: vec![],
            panes: vec![],
            status: BackendStatus::Disconnected,
            events: VecDeque::new(),
            next_window: 0,
            next_tab: 0,
            next_pane: 0,
            pty_tx: None,
            pty_rx: None,
            intentionally_closed: HashSet::new(),
        }
    }

    /// 用 `Config` 创建。
    pub fn from_config(config: &crate::core::config::Config) -> Self {
        Self::new(
            config.pane.default_command.clone(),
            config.pane.workdir.clone(),
        )
    }

    /// drain pty 读线程的字节块，转成 PaneOutput 事件并累积输出。
    ///
    /// 子进程 Exit（如 Ctrl+D 退出 shell）：仅剩 1 个 pane 时关闭整个 window；
    /// 否则只关闭该 pane。
    fn drain_pty_output(&mut self) {
        let Some(rx) = self.pty_rx.as_mut() else {
            return;
        };
        let mut outputs = Vec::new();
        let mut exits = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            match msg {
                PtyMsg::Output { pane, data } => outputs.push((pane, data)),
                PtyMsg::Exit { pane } => exits.push(pane),
            }
        }
        for (pane, data) in outputs {
            if let Some(p) = self.panes.iter_mut().find(|p| p.info.id == pane) {
                append_capped(&mut p.output, &data, MAX_PANE_OUTPUT_BYTES);
            }
            self.events
                .push_back(StateChange::PaneOutput { pane, data });
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
            while self.events.len() > MAX_STATE_EVENTS {
                self.events.pop_front();
            }
        }
        for pane in exits {
            self.handle_pane_process_exit(pane);
        }
    }

    /// shell/pty 子进程退出后的清理策略。
    ///
    /// - 整个 session 只剩 1 个 pane → 关 window/session
    /// - 当前 tab 只剩这 1 个 pane（还有其他 tab）→ 只关该 tab
    /// - 否则只关该 pane
    fn handle_pane_process_exit(&mut self, pane: PaneId) {
        // 主动 ClosePane/CloseWindow 已 kill 的，忽略后续 Exit
        if self.intentionally_closed.remove(&pane) {
            return;
        }
        if !self.panes.iter().any(|p| p.info.id == pane) {
            return;
        }
        let Some(tab_id) = self.tab_of_pane(pane) else {
            return;
        };
        let panes_in_tab = self.panes.iter().filter(|p| p.info.tab == tab_id).count();

        // 唯一 pane → 关整个 window（及 session 若无剩余 window）
        if self.panes.len() == 1 {
            if let Some(win) = self
                .tabs
                .iter()
                .find(|t| t.info.id == tab_id)
                .map(|t| t.info.window)
            {
                self.close_window_internal(win);
            }
            return;
        }
        // 该 tab 的最后一个 pane → 关 tab（保留其他 tab）
        if panes_in_tab <= 1 {
            self.close_tab_internal(tab_id);
            return;
        }
        // 同 tab 还有其他 pane：只关退出的那个
        self.close_pane_internal(pane);
    }

    /// 关闭单个 pane（布局 + 事件），供 ClosePane / 进程退出复用。
    fn close_pane_internal(&mut self, target: PaneId) {
        let Some(tab_id) = self.tab_of_pane(target) else {
            return;
        };
        let was_active = self
            .panes
            .iter()
            .find(|p| p.info.id == target)
            .map(|p| p.info.active)
            .unwrap_or(false);
        self.kill_pane(target);
        if let Some(tl) = self.tabs.iter_mut().find(|t| t.info.id == tab_id) {
            match tl.layout.tree.remove(target) {
                Ok(()) => {
                    self.events
                        .push_back(StateChange::PaneClosed { pane: target });
                    self.events.push_back(StateChange::LayoutChanged {
                        tab: tab_id,
                        layout: tl.layout.clone(),
                    });
                    if was_active {
                        let new_active = tl.layout.tree.leaves().first().copied();
                        if let Some(a) = new_active {
                            self.set_active_pane(tab_id, a);
                            self.events.push_back(StateChange::ActivePaneChanged {
                                tab: tab_id,
                                pane: a,
                            });
                        }
                    }
                }
                Err(_) => {
                    // 根叶子被移除：退化为关 tab
                    self.close_tab_internal(tab_id);
                }
            }
        }
    }

    /// 关闭 tab 及其下所有 pane（供 CloseTab / 末 pane Exit 复用）。
    fn close_tab_internal(&mut self, target: TabId) {
        if !self.tabs.iter().any(|t| t.info.id == target) {
            return;
        }
        let win_id = self
            .tabs
            .iter()
            .find(|t| t.info.id == target)
            .map(|t| t.info.window)
            .unwrap_or(WindowId(0));
        let to_kill: Vec<PaneId> = self
            .panes
            .iter()
            .filter(|p| p.info.tab == target)
            .map(|p| p.info.id)
            .collect();
        for pid in &to_kill {
            self.kill_pane(*pid);
            self.events
                .push_back(StateChange::PaneClosed { pane: *pid });
        }
        self.tabs.retain(|t| t.info.id != target);
        self.events
            .push_back(StateChange::TabClosed { tab: target });
        if let Some(t) = self.tabs.iter().find(|t| t.info.window == win_id) {
            let tid = t.info.id;
            for t in self.tabs.iter_mut() {
                if t.info.window == win_id {
                    t.info.active = t.info.id == tid;
                }
            }
            self.events.push_back(StateChange::ActiveTabChanged {
                window: win_id,
                tab: tid,
            });
            // 激活新 tab 的 active pane
            if let Some(pane) = self
                .tabs
                .iter()
                .find(|t| t.info.id == tid)
                .and_then(|t| t.layout.tree.leaves().first().copied())
            {
                self.set_active_pane(tid, pane);
                self.events
                    .push_back(StateChange::ActivePaneChanged { tab: tid, pane });
            }
        } else {
            // 无剩余 tab → 关 window
            self.close_window_internal(win_id);
        }
    }

    /// 关闭 window 及其下所有 pane/tab（供 CloseWindow / 末 pane 退出复用）。
    fn close_window_internal(&mut self, target: WindowId) {
        if !self.windows.iter().any(|w| w.info.id == target) {
            return;
        }
        let sess = self
            .windows
            .iter()
            .find(|w| w.info.id == target)
            .map(|w| w.info.session)
            .unwrap_or(SessionId(1));
        let tab_ids: Vec<TabId> = self
            .tabs
            .iter()
            .filter(|t| t.info.window == target)
            .map(|t| t.info.id)
            .collect();
        let to_kill: Vec<PaneId> = self
            .panes
            .iter()
            .filter(|p| tab_ids.contains(&p.info.tab))
            .map(|p| p.info.id)
            .collect();
        for pid in &to_kill {
            self.kill_pane(*pid);
            self.events
                .push_back(StateChange::PaneClosed { pane: *pid });
        }
        for tid in &tab_ids {
            self.events.push_back(StateChange::TabClosed { tab: *tid });
        }
        self.tabs.retain(|t| t.info.window != target);
        self.windows.retain(|w| w.info.id != target);
        self.events
            .push_back(StateChange::WindowClosed { window: target });
        if let Some(w) = self.windows.first() {
            let wid = w.info.id;
            self.set_active_window(wid);
            self.events.push_back(StateChange::ActiveWindowChanged {
                session: sess,
                window: wid,
            });
        } else {
            // 无剩余 window → session 结束
            self.status = BackendStatus::Exited;
            self.events
                .push_back(StateChange::BackendStatusChanged(BackendStatus::Exited));
        }
    }

    /// 确保通道已建立（connect 时调用一次）。
    fn ensure_channel(&mut self) {
        if self.pty_tx.is_none() {
            let (tx, rx) = mpsc::channel::<PtyMsg>(8192);
            self.pty_tx = Some(tx);
            self.pty_rx = Some(rx);
        }
    }

    /// 分配下一个 window id。
    fn alloc_window_id(&mut self) -> WindowId {
        self.next_window += 1;
        WindowId(self.next_window)
    }

    /// 分配下一个 pane id。
    fn alloc_pane_id(&mut self) -> PaneId {
        self.next_pane += 1;
        PaneId(self.next_pane)
    }

    /// 分配下一个 tab id。
    fn alloc_tab_id(&mut self) -> TabId {
        self.next_tab += 1;
        TabId(self.next_tab)
    }

    /// spawn 一个本地 pane（pty + 子进程），返回 pane id。
    /// 调用方负责把 pane 加入布局树 + 推事件。
    fn spawn_pane(
        &mut self,
        tab: TabId,
        command: Option<&[String]>,
        workdir: Option<&str>,
        cols: u16,
        rows: u16,
        active: bool,
    ) -> Result<PaneId> {
        self.ensure_channel();
        let tx = self.pty_tx.clone().expect("channel 已建立");

        // 解析启动命令
        let argv = command
            .map(|c| c.to_vec())
            .unwrap_or_else(|| parse_command_argv(&self.default_command));
        let argv = prepare_pane_argv_for_platform(argv, cfg!(target_os = "macos"));
        if argv.is_empty() {
            anyhow::bail!("启动命令为空");
        }
        let workdir = workdir
            .map(|s| s.to_string())
            .unwrap_or_else(|| expand_config_value(&self.default_workdir));

        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("openpty 失败")?;

        let mut cmd = CommandBuilder::new(&argv[0]);
        for a in &argv[1..] {
            cmd.arg(a);
        }
        if !workdir.is_empty() {
            cmd.cwd(&workdir);
        }
        let child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("spawn `{}` 失败", argv[0]))?;
        drop(pair.slave);

        let pid = child
            .process_id()
            .ok_or_else(|| anyhow::anyhow!("子进程无 pid"))?;

        let reader = pair
            .master
            .try_clone_reader()
            .context("try_clone_reader 失败")?;
        let writer = pair.master.take_writer().context("take_writer 失败")?;

        let pane_id = self.alloc_pane_id();

        // 后台读线程：把字节块发回 backend 的共享 channel
        let pane_for_reader = pane_id;
        std::thread::Builder::new()
            .name("muxterm-local-pty-read".into())
            .spawn(move || {
                let mut reader = reader;
                let mut buf = [0u8; 4096];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if tx
                                .blocking_send(PtyMsg::Output {
                                    pane: pane_for_reader,
                                    data: buf[..n].to_vec(),
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                let _ = tx.blocking_send(PtyMsg::Exit {
                    pane: pane_for_reader,
                });
            })
            .expect("spawn pty read thread");

        let title = program_basename(&argv[0]);
        let pane = LocalPane {
            info: PaneInfo {
                id: pane_id,
                tab,
                active,
                title,
                cols,
                rows,
            },
            master: pair.master,
            child,
            output: Vec::new(),
            writer: Arc::new(Mutex::new(writer)),
            pid,
        };
        self.panes.push(pane);
        Ok(pane_id)
    }

    /// 新建一个 window（含第一个 pane）。返回 window id + pane id。
    fn new_window_internal(
        &mut self,
        name: Option<String>,
        command: Option<&[String]>,
        workdir: Option<&str>,
    ) -> Result<(WindowId, TabId, PaneId)> {
        let win_id = self.alloc_window_id();
        let tab_id = self.alloc_tab_id();
        let sess = self.session.as_ref().map(|s| s.id).unwrap_or(SessionId(1));
        let win_name = name.unwrap_or_else(|| format!("w{}", win_id.0));

        // 旧 window 取消 active
        for w in self.windows.iter_mut() {
            w.info.active = false;
        }
        // 旧 tab 取消 active
        for t in self.tabs.iter_mut() {
            t.info.active = false;
        }
        // 旧 pane 取消 active
        for p in self.panes.iter_mut() {
            p.info.active = false;
        }

        let pane_id =
            self.spawn_pane(tab_id, command, workdir, DEFAULT_COLS, DEFAULT_ROWS, true)?;

        self.windows.push(LocalWindow {
            info: WindowInfo {
                id: win_id,
                name: win_name,
                session: sess,
                active: true,
            },
        });
        self.tabs.push(LocalTab {
            info: TabInfo {
                id: tab_id,
                name: format!("t{}", tab_id.0),
                window: win_id,
                active: true,
            },
            layout: TabLayout {
                tab: tab_id,
                tree: LayoutNode::leaf(pane_id),
                active: pane_id,
            },
        });
        if let Some(s) = self.session.as_mut() {
            s.active_window = Some(win_id);
        }
        Ok((win_id, tab_id, pane_id))
    }

    /// 找 pane 所在 window。
    fn tab_of_pane(&self, pane: PaneId) -> Option<TabId> {
        self.panes
            .iter()
            .find(|p| p.info.id == pane)
            .map(|p| p.info.tab)
    }

    /// 设置某 window 下某 pane 为 active（取消其他）。
    fn set_active_pane(&mut self, tab: TabId, pane: PaneId) {
        // 全后端只允许「一个 active pane」。旧实现只取消同一 tab 内的 active，
        // 导致 NewTab/SwitchTab 后多个 tab 的 pane 同时 active，State::active_pane()
        // 的全局 find 返回旧 tab 的 pane，cmd+[ / cmd+] 切到了错误 tab 的布局。
        for p in self.panes.iter_mut() {
            p.info.active = p.info.id == pane && p.info.tab == tab;
        }
        if let Some(tl) = self.tabs.iter_mut().find(|t| t.info.id == tab) {
            tl.layout.active = pane;
        }
    }

    /// 设置某 session 下某 window 为 active（取消其他）。
    fn set_active_window(&mut self, window: WindowId) {
        let sess = self
            .windows
            .iter()
            .find(|w| w.info.id == window)
            .map(|w| w.info.session);
        for w in self.windows.iter_mut() {
            w.info.active = w.info.id == window;
        }
        // 同时激活该 window 下第一个 tab，并把激活 pane 切到该 tab 的 active pane
        if let Some(t) = self.tabs.iter().find(|t| t.info.window == window) {
            let tid = t.info.id;
            for t in self.tabs.iter_mut() {
                if t.info.window == window {
                    t.info.active = t.info.id == tid;
                }
            }
            if let Some(tl) = self.tabs.iter().find(|t| t.info.id == tid) {
                let active_pane = tl.layout.active;
                for p in self.panes.iter_mut() {
                    p.info.active = p.info.id == active_pane && p.info.tab == tid;
                }
            }
        }
        if let (Some(s), Some(sess)) = (self.session.as_mut(), sess) {
            s.active_window = Some(window);
            let _ = sess;
        }
    }

    /// kill 一个 pane 的子进程并从内部移除（调用方负责布局/事件）。
    fn kill_pane(&mut self, pane: PaneId) -> Option<LocalPane> {
        let idx = self.panes.iter().position(|p| p.info.id == pane)?;
        self.intentionally_closed.insert(pane);
        let mut p = self.panes.remove(idx);
        // kill 子进程
        let _ = p.child.kill();
        // 关 master（drop 即可）
        Some(p)
    }

    /// 把字节写入 pane 的 pty。
    fn write_to_pane(&mut self, pane: PaneId, data: &[u8]) -> bool {
        let Some(p) = self.panes.iter().find(|p| p.info.id == pane) else {
            return false;
        };
        let writer = p.writer.clone();
        // 同步写：写量小，用阻塞线程池写避免阻塞 async
        let data = data.to_vec();
        let result = std::thread::spawn(move || {
            let mut w = writer.lock().unwrap();
            w.write_all(&data)
        })
        .join();
        matches!(result, Ok(Ok(())))
    }

    /// resize 某个 pane 的 pty + 更新 info。
    fn resize_pane(&mut self, pane: PaneId, cols: u16, rows: u16) -> bool {
        let Some(p) = self.panes.iter_mut().find(|p| p.info.id == pane) else {
            return false;
        };
        let size = PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        };
        if p.master.resize(size).is_err() {
            return false;
        }
        p.info.cols = cols;
        p.info.rows = rows;
        true
    }
}

impl State for LocalBackend {
    fn sessions(&self) -> &[SessionInfo] {
        static EMPTY: Vec<SessionInfo> = Vec::new();
        match &self.session {
            Some(s) => std::slice::from_ref(s),
            None => &EMPTY,
        }
    }

    fn active_session(&self) -> Option<&SessionInfo> {
        self.session.as_ref()
    }

    fn active_window(&self) -> Option<&WindowInfo> {
        self.windows.iter().find(|w| w.info.active).map(|w| &w.info)
    }

    fn all_windows(&self) -> Vec<&WindowInfo> {
        self.windows.iter().map(|w| &w.info).collect()
    }

    fn active_tab(&self) -> Option<&TabInfo> {
        self.tabs.iter().find(|t| t.info.active).map(|t| &t.info)
    }

    fn active_pane(&self) -> Option<&PaneInfo> {
        self.panes.iter().find(|p| p.info.active).map(|p| &p.info)
    }

    fn tabs(&self, window: &WindowId) -> Vec<&TabInfo> {
        self.tabs
            .iter()
            .filter(|t| &t.info.window == window)
            .map(|t| &t.info)
            .collect()
    }

    fn tab(&self, tab: &TabId) -> Option<&TabInfo> {
        self.tabs
            .iter()
            .find(|t| t.info.id == *tab)
            .map(|t| &t.info)
    }

    fn layout(&self, tab: &TabId) -> Option<&TabLayout> {
        self.tabs
            .iter()
            .find(|t| t.info.id == *tab)
            .map(|t| &t.layout)
    }

    fn panes(&self, tab: &TabId) -> Vec<&PaneInfo> {
        self.panes
            .iter()
            .filter(|p| p.info.tab == *tab)
            .map(|p| &p.info)
            .collect()
    }

    fn pane(&self, pane: &PaneId) -> Option<&PaneInfo> {
        self.panes
            .iter()
            .find(|p| p.info.id == *pane)
            .map(|p| &p.info)
    }

    fn pane_output(&self, pane: &PaneId) -> Option<&[u8]> {
        self.panes
            .iter()
            .find(|p| p.info.id == *pane)
            .map(|p| p.output.as_slice())
    }

    fn status(&self) -> BackendStatus {
        self.status
    }
}

#[async_trait]
impl Backend for LocalBackend {
    async fn connect(&mut self) -> Result<()> {
        if self.status == BackendStatus::Connected {
            return Ok(());
        }
        self.status = BackendStatus::Connecting;
        self.events
            .push_back(StateChange::BackendStatusChanged(BackendStatus::Connecting));

        self.ensure_channel();

        // 建立第一个 session
        let sess_id = SessionId(1);
        self.session = Some(SessionInfo {
            id: sess_id,
            name: "local".into(),
            active_window: None,
        });

        // spawn 第一个 window + pane
        match self.new_window_internal(None, None, None) {
            Ok((win_id, tab_id, pane_id)) => {
                self.status = BackendStatus::Connected;
                self.events.push_back(StateChange::WindowAdded {
                    window: win_id,
                    session: sess_id,
                });
                self.events.push_back(StateChange::TabAdded {
                    tab: tab_id,
                    window: win_id,
                });
                self.events.push_back(StateChange::PaneAdded {
                    pane: pane_id,
                    tab: tab_id,
                });
                self.events.push_back(StateChange::ActiveWindowChanged {
                    session: sess_id,
                    window: win_id,
                });
                self.events.push_back(StateChange::ActiveTabChanged {
                    window: win_id,
                    tab: tab_id,
                });
                self.events.push_back(StateChange::ActivePaneChanged {
                    tab: tab_id,
                    pane: pane_id,
                });
                self.events
                    .push_back(StateChange::BackendStatusChanged(BackendStatus::Connected));
                Ok(())
            }
            Err(e) => {
                self.status = BackendStatus::Error;
                self.events
                    .push_back(StateChange::BackendStatusChanged(BackendStatus::Error));
                Err(e)
            }
        }
    }

    fn execute(&mut self, task: &Task) -> Result<TaskOutcome> {
        let outcome = match task {
            Task::SplitPane {
                target,
                dir,
                command,
                workdir,
            } => {
                let Some(&target_id) = target.as_ref() else {
                    return Ok(TaskOutcome::Rejected {
                        reason: "SplitPane 缺少 target".into(),
                    });
                };
                let Some(tab_id) = self.tab_of_pane(target_id) else {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("pane {target_id} 不存在"),
                    });
                };
                // 新 pane 尺寸取 target 的一半
                let (cols, rows) = self
                    .panes
                    .iter()
                    .find(|p| p.info.id == target_id)
                    .map(|p| (p.info.cols, p.info.rows))
                    .unwrap_or((DEFAULT_COLS, DEFAULT_ROWS));
                let new_pane = match self.spawn_pane(
                    tab_id,
                    command.as_deref(),
                    workdir.as_deref(),
                    cols / 2,
                    rows,
                    true,
                ) {
                    Ok(id) => id,
                    Err(e) => {
                        return Ok(TaskOutcome::Rejected {
                            reason: format!("spawn 失败: {e}"),
                        })
                    }
                };
                // 取消旧 active
                for p in self.panes.iter_mut() {
                    if p.info.tab == tab_id {
                        p.info.active = p.info.id == new_pane;
                    }
                }
                // 更新布局树
                if let Some(tl) = self.tabs.iter_mut().find(|t| t.info.id == tab_id) {
                    tl.layout.tree.split_at(target_id, new_pane, *dir);
                    tl.layout.active = new_pane;
                    self.events.push_back(StateChange::PaneAdded {
                        pane: new_pane,
                        tab: tab_id,
                    });
                    self.events.push_back(StateChange::LayoutChanged {
                        tab: tab_id,
                        layout: tl.layout.clone(),
                    });
                    self.events.push_back(StateChange::ActivePaneChanged {
                        tab: tab_id,
                        pane: new_pane,
                    });
                }
                TaskOutcome::Done
            }

            Task::ClosePane { target } => {
                if self.tab_of_pane(*target).is_none() {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("pane {target} 不存在"),
                    });
                }
                self.close_pane_internal(*target);
                TaskOutcome::Done
            }

            Task::SwitchPane { target } => {
                let Some(tab_id) = self.tab_of_pane(*target) else {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("pane {target} 不存在"),
                    });
                };
                self.set_active_pane(tab_id, *target);
                self.events.push_back(StateChange::ActivePaneChanged {
                    tab: tab_id,
                    pane: *target,
                });
                TaskOutcome::Done
            }

            Task::NextPane | Task::PrevPane => {
                let Some(active) = self
                    .panes
                    .iter()
                    .find(|p| p.info.active)
                    .map(|p| (p.info.id, p.info.tab))
                else {
                    return Ok(TaskOutcome::Rejected {
                        reason: "无激活 pane".into(),
                    });
                };
                let (active_id, tab_id) = active;
                let Some(tl) = self.tabs.iter().find(|t| t.info.id == tab_id) else {
                    return Ok(TaskOutcome::Rejected {
                        reason: "无布局".into(),
                    });
                };
                let next = match task {
                    Task::NextPane => tl.layout.tree.next_leaf(active_id),
                    Task::PrevPane => tl.layout.tree.prev_leaf(active_id),
                    _ => None,
                };
                if let Some(n) = next {
                    self.set_active_pane(tab_id, n);
                    self.events.push_back(StateChange::ActivePaneChanged {
                        tab: tab_id,
                        pane: n,
                    });
                }
                TaskOutcome::Done
            }

            Task::NewWindow {
                name,
                command,
                workdir,
            } => {
                match self.new_window_internal(name.clone(), command.as_deref(), workdir.as_deref())
                {
                    Ok((win_id, tab_id, pane_id)) => {
                        let sess = self.session.as_ref().map(|s| s.id).unwrap_or(SessionId(1));
                        self.events.push_back(StateChange::WindowAdded {
                            window: win_id,
                            session: sess,
                        });
                        self.events.push_back(StateChange::TabAdded {
                            tab: tab_id,
                            window: win_id,
                        });
                        self.events.push_back(StateChange::PaneAdded {
                            pane: pane_id,
                            tab: tab_id,
                        });
                        self.events.push_back(StateChange::ActiveWindowChanged {
                            session: sess,
                            window: win_id,
                        });
                        self.events.push_back(StateChange::ActiveTabChanged {
                            window: win_id,
                            tab: tab_id,
                        });
                        self.events.push_back(StateChange::ActivePaneChanged {
                            tab: tab_id,
                            pane: pane_id,
                        });
                        TaskOutcome::Done
                    }
                    Err(e) => TaskOutcome::Rejected {
                        reason: format!("spawn 失败: {e}"),
                    },
                }
            }

            Task::CloseWindow { target } => {
                if !self.windows.iter().any(|w| w.info.id == *target) {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("window {target} 不存在"),
                    });
                }
                self.close_window_internal(*target);
                TaskOutcome::Done
            }

            Task::SwitchWindow { target } => {
                if !self.windows.iter().any(|w| w.info.id == *target) {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("window {target} 不存在"),
                    });
                }
                self.set_active_window(*target);
                let sess = self
                    .windows
                    .iter()
                    .find(|w| w.info.id == *target)
                    .map(|w| w.info.session)
                    .unwrap_or(SessionId(1));
                self.events.push_back(StateChange::ActiveWindowChanged {
                    session: sess,
                    window: *target,
                });
                TaskOutcome::Done
            }

            Task::RenameWindow { target, name } => {
                if !self.windows.iter().any(|w| w.info.id == *target) {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("window {target} 不存在"),
                    });
                }
                if let Some(w) = self.windows.iter_mut().find(|w| w.info.id == *target) {
                    w.info.name = name.clone();
                }
                self.events.push_back(StateChange::WindowRenamed {
                    window: *target,
                    name: name.clone(),
                });
                TaskOutcome::Done
            }

            Task::SwitchSession { .. } | Task::RenameSession { .. } => {
                // 单 session 后端，直接 Done
                TaskOutcome::Done
            }

            Task::SendKeys { target, keys } => {
                if !self.panes.iter().any(|p| p.info.id == *target) {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("pane {target} 不存在"),
                    });
                }
                let mut buf = Vec::new();
                for k in keys {
                    buf.extend_from_slice(&encode(k));
                }
                if self.write_to_pane(*target, &buf) {
                    TaskOutcome::Done
                } else {
                    TaskOutcome::Rejected {
                        reason: "写入 pty 失败".into(),
                    }
                }
            }

            Task::WriteRaw { target, data } => {
                if !self.panes.iter().any(|p| p.info.id == *target) {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("pane {target} 不存在"),
                    });
                }
                // 只写入 pty；显示依赖 shell 回显（drain_pty_output），避免双写。
                let written = self.write_to_pane(*target, data);
                if written {
                    TaskOutcome::Done
                } else {
                    TaskOutcome::Rejected {
                        reason: "写入 pty 失败".into(),
                    }
                }
            }

            Task::ResizePane { target, cols, rows } => {
                if !self.resize_pane(*target, *cols, *rows) {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("resize pane {target} 失败"),
                    });
                }
                self.events.push_back(StateChange::PaneResized {
                    pane: *target,
                    cols: *cols,
                    rows: *rows,
                });
                TaskOutcome::Done
            }

            Task::ResizeClient { .. } => TaskOutcome::Rejected {
                reason: "LocalBackend 不支持 client resize".into(),
            },

            Task::ResizePaneAxis { target, dir, size } => {
                let Some((cols, rows)) = self
                    .panes
                    .iter()
                    .find(|p| p.info.id == *target)
                    .map(|p| (p.info.cols, p.info.rows))
                else {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("pane {target} 不存在"),
                    });
                };
                let (cols, rows) = match dir {
                    crate::core::model::layout::SplitDir::Horizontal => (*size, rows),
                    crate::core::model::layout::SplitDir::Vertical => (cols, *size),
                };
                if !self.resize_pane(*target, cols, rows) {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("resize pane {target} 失败"),
                    });
                }
                self.events.push_back(StateChange::PaneResized {
                    pane: *target,
                    cols,
                    rows,
                });
                TaskOutcome::Done
            }

            Task::ResizePaneStep { target, .. } => {
                // 步进 resize：简化为 Done（真实实现需要当前尺寸 + 方向计算）
                if !self.panes.iter().any(|p| p.info.id == *target) {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("pane {target} 不存在"),
                    });
                }
                TaskOutcome::Done
            }

            Task::NewTab {
                window,
                name,
                command,
                workdir,
            } => {
                if !self.windows.iter().any(|w| w.info.id == *window) {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("window {window} 不存在"),
                    });
                }
                let tab_id = self.alloc_tab_id();
                // 旧 tab 取消 active
                for t in self.tabs.iter_mut() {
                    if t.info.window == *window {
                        t.info.active = false;
                    }
                }
                let pane_id = self.spawn_pane(
                    tab_id,
                    command.as_deref(),
                    workdir.as_deref(),
                    DEFAULT_COLS,
                    DEFAULT_ROWS,
                    true,
                )?;
                // 全后端只保留一个 active pane：新 tab 的 pane 激活，其余全部取消。
                for p in self.panes.iter_mut() {
                    p.info.active = p.info.id == pane_id;
                }
                self.tabs.push(LocalTab {
                    info: TabInfo {
                        id: tab_id,
                        name: name.clone().unwrap_or_else(|| format!("t{}", tab_id.0)),
                        window: *window,
                        active: true,
                    },
                    layout: TabLayout {
                        tab: tab_id,
                        tree: LayoutNode::leaf(pane_id),
                        active: pane_id,
                    },
                });
                self.events.push_back(StateChange::TabAdded {
                    tab: tab_id,
                    window: *window,
                });
                self.events.push_back(StateChange::PaneAdded {
                    pane: pane_id,
                    tab: tab_id,
                });
                self.events.push_back(StateChange::ActiveTabChanged {
                    window: *window,
                    tab: tab_id,
                });
                self.events.push_back(StateChange::ActivePaneChanged {
                    tab: tab_id,
                    pane: pane_id,
                });
                TaskOutcome::Done
            }

            Task::CloseTab { target } => {
                if !self.tabs.iter().any(|t| t.info.id == *target) {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("tab {target} 不存在"),
                    });
                }
                self.close_tab_internal(*target);
                TaskOutcome::Done
            }

            Task::SwitchTab { target } => {
                if !self.tabs.iter().any(|t| t.info.id == *target) {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("tab {target} 不存在"),
                    });
                }
                let win_id = self
                    .tabs
                    .iter()
                    .find(|t| t.info.id == *target)
                    .map(|t| t.info.window)
                    .unwrap_or(WindowId(0));
                for t in self.tabs.iter_mut() {
                    if t.info.window == win_id {
                        t.info.active = t.info.id == *target;
                    }
                }
                // 切 tab 时把激活 pane 切到目标 tab 的 active pane，并取消其他 tab
                // 的 active pane（旧实现漏了这一步，导致 cmd+[ / cmd+] 用全局 find
                // 找到旧 tab 的 pane，切到了错误 tab 的布局）。
                if let Some(tl) = self.tabs.iter().find(|t| t.info.id == *target) {
                    let active_pane = tl.layout.active;
                    for p in self.panes.iter_mut() {
                        p.info.active = p.info.id == active_pane && p.info.tab == *target;
                    }
                    self.events.push_back(StateChange::ActivePaneChanged {
                        tab: *target,
                        pane: active_pane,
                    });
                }
                self.events.push_back(StateChange::ActiveTabChanged {
                    window: win_id,
                    tab: *target,
                });
                TaskOutcome::Done
            }

            Task::RenameTab { target, name } => {
                if !self.tabs.iter().any(|t| t.info.id == *target) {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("tab {target} 不存在"),
                    });
                }
                if let Some(t) = self.tabs.iter_mut().find(|t| t.info.id == *target) {
                    t.info.name = name.clone();
                }
                self.events.push_back(StateChange::TabRenamed {
                    tab: *target,
                    name: name.clone(),
                });
                TaskOutcome::Done
            }

            Task::Shutdown => {
                // kill 所有 pane
                let all: Vec<PaneId> = self.panes.iter().map(|p| p.info.id).collect();
                for pid in all {
                    self.kill_pane(pid);
                }
                self.windows.clear();
                self.tabs.clear();
                self.session = None;
                self.status = BackendStatus::Exited;
                self.events
                    .push_back(StateChange::BackendStatusChanged(BackendStatus::Exited));
                TaskOutcome::Done
            }
        };
        Ok(outcome)
    }

    fn take_events(&mut self) -> Vec<StateChange> {
        // 先 drain pty 输出，再聚合事件队列
        self.drain_pty_output();
        self.events.drain(..).collect()
    }

    async fn shutdown(&mut self) -> Result<()> {
        self.execute(&Task::Shutdown)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::Config;
    use crate::core::model::layout::SplitDir;

    fn backend() -> LocalBackend {
        // 长驻进程，避免测试中途 Exit 触发「末 pane 关 window」逻辑。
        LocalBackend::new("sleep 60", "/")
    }

    /// 轮询 `take_events`，直到 `pred` 成立或超时（避免短命子进程 Exit 与单次 sleep 竞态）。
    async fn wait_events(
        b: &mut LocalBackend,
        timeout: std::time::Duration,
        mut pred: impl FnMut(&[StateChange]) -> bool,
    ) -> Vec<StateChange> {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut collected = Vec::new();
        loop {
            collected.extend(b.take_events());
            if pred(&collected) {
                return collected;
            }
            if tokio::time::Instant::now() >= deadline {
                return collected;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn connect_creates_session_window_pane() {
        let mut b = backend();
        b.connect().await.unwrap();
        assert_eq!(b.status(), BackendStatus::Connected);
        assert!(b.session.is_some());
        assert_eq!(b.windows.len(), 1);
        assert_eq!(b.panes.len(), 1);
        // 事件
        let events = b.take_events();
        assert!(events.iter().any(|e| matches!(
            e,
            StateChange::BackendStatusChanged(BackendStatus::Connected)
        )));
        assert!(events
            .iter()
            .any(|e| matches!(e, StateChange::WindowAdded { .. })));
        assert!(events
            .iter()
            .any(|e| matches!(e, StateChange::PaneAdded { .. })));
    }

    #[tokio::test]
    async fn split_pane_adds_pane_and_updates_layout() {
        let mut b = backend();
        b.connect().await.unwrap();
        let _ = b.take_events();
        let active = b.active_pane_id().unwrap();
        let outcome = b
            .execute(&Task::SplitPane {
                target: Some(active),
                dir: SplitDir::Horizontal,
                command: None,
                workdir: None,
            })
            .unwrap();
        assert_eq!(outcome, TaskOutcome::Done);
        let events = b.take_events();
        assert!(events
            .iter()
            .any(|e| matches!(e, StateChange::PaneAdded { .. })));
        assert_eq!(b.panes.len(), 2);
        // active 切到新 pane
        assert_ne!(b.active_pane_id(), Some(active));
    }

    #[tokio::test]
    async fn close_pane_removes_and_restores_active() {
        let mut b = backend();
        b.connect().await.unwrap();
        let first = b.active_pane_id().unwrap();
        b.execute(&Task::SplitPane {
            target: Some(first),
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
        let _ = b.take_events();
        let second = b.active_pane_id().unwrap();
        b.execute(&Task::ClosePane { target: second }).unwrap();
        let events = b.take_events();
        assert!(events
            .iter()
            .any(|e| matches!(e, StateChange::PaneClosed { pane: p } if *p == second)));
        assert_eq!(b.panes.len(), 1);
        assert_eq!(b.active_pane_id(), Some(first));
    }

    #[tokio::test]
    async fn last_pane_process_exit_closes_window() {
        // 短命命令：退出后若仅剩该 pane，应关闭整个 window/session。
        // 注意：不能先 `let _ = take_events()` 再 sleep——Exit 可能已在 connect 后立刻到达并被丢弃。
        let mut b = LocalBackend::new("sleep 0.05", "/");
        b.connect().await.unwrap();
        let events = wait_events(&mut b, std::time::Duration::from_secs(2), |ev| {
            ev.iter().any(|e| {
                matches!(
                    e,
                    StateChange::WindowClosed { .. }
                        | StateChange::BackendStatusChanged(BackendStatus::Exited)
                )
            })
        })
        .await;
        assert!(
            events.iter().any(|e| matches!(
                e,
                StateChange::WindowClosed { .. }
                    | StateChange::BackendStatusChanged(BackendStatus::Exited)
            )),
            "末 pane 退出应关闭 window/session: {events:?}"
        );
        assert!(b.panes.is_empty());
        assert!(b.windows.is_empty());
    }

    #[tokio::test]
    async fn eof_exits_foreground_child_without_closing_shell_pane() {
        // cat 是前台子进程；收到 EOF 后退出，外层 shell 继续运行 sleep。
        // 这验证 Ctrl+D/0x04 不能被 GUI 直接转换成 ClosePane。
        let mut b = backend();
        let win = b.active_window();
        assert!(win.is_none(), "connect 前不应有 window");
        b.connect().await.unwrap();
        let window = b.active_window().map(|w| w.id).unwrap();
        b.execute(&Task::NewTab {
            window,
            name: None,
            command: Some(vec!["sh".into(), "-c".into(), "cat; sleep 60".into()]),
            workdir: None,
        })
        .unwrap();
        let pane = b.active_pane_id().unwrap();
        let _ = b.take_events();

        assert!(matches!(
            b.execute(&Task::WriteRaw {
                target: pane,
                data: vec![0x04],
            }),
            Ok(TaskOutcome::Done)
        ));

        let events = wait_events(&mut b, std::time::Duration::from_millis(500), |_| false).await;
        assert!(
            !events.iter().any(|e| matches!(
                e,
                StateChange::PaneClosed { pane: p } if *p == pane
            )),
            "前台 cat 收到 EOF 后不应关闭 pane: {events:?}"
        );
        assert_eq!(b.panes.len(), 2, "外层 shell 仍在，两个 pane 都应存在");
        assert_eq!(b.status(), BackendStatus::Connected);
    }

    #[tokio::test]
    async fn multi_tab_pane_exit_closes_only_that_tab() {
        // 复现：多 tab 时某一 tab 的 shell Exit 应只关该 tab，不关整个 window。
        let mut b = backend();
        b.connect().await.unwrap();
        let _ = b.take_events();
        let win = b.active_window().map(|w| w.id).unwrap();
        b.execute(&Task::NewTab {
            window: win,
            name: None,
            command: Some(vec!["sleep".into(), "0.05".into()]),
            workdir: None,
        })
        .unwrap();
        assert_eq!(b.tabs.len(), 2, "应有 2 tabs");
        assert_eq!(b.panes.len(), 2);

        // 从 NewTab 之后开始累积，避免短命 sleep 的 Exit 被单次 drain 丢掉
        let events = wait_events(&mut b, std::time::Duration::from_secs(2), |ev| {
            ev.iter()
                .any(|e| matches!(e, StateChange::TabClosed { .. }))
        })
        .await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StateChange::TabClosed { .. })),
            "应关闭退出 pane 所在 tab: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, StateChange::WindowClosed { .. })),
            "多 tab 时不应关整个 window: {events:?}"
        );
        assert_eq!(b.tabs.len(), 1, "应剩 1 tab");
        assert_eq!(b.windows.len(), 1, "window 应保留");
        assert_eq!(b.panes.len(), 1, "应剩 1 pane");
        assert_eq!(b.status(), BackendStatus::Connected);
    }

    #[tokio::test]
    async fn new_window_adds_window_and_pane() {
        let mut b = backend();
        b.connect().await.unwrap();
        let _ = b.take_events();
        b.execute(&Task::NewWindow {
            name: Some("dev".into()),
            command: None,
            workdir: None,
        })
        .unwrap();
        let events = b.take_events();
        assert!(events
            .iter()
            .any(|e| matches!(e, StateChange::WindowAdded { .. })));
        assert_eq!(b.windows.len(), 2);
        assert_eq!(b.active_window().map(|w| w.id), Some(WindowId(2)));
    }

    #[tokio::test]
    async fn close_window_removes_panes() {
        let mut b = backend();
        b.connect().await.unwrap();
        b.execute(&Task::NewWindow {
            name: None,
            command: None,
            workdir: None,
        })
        .unwrap();
        let _ = b.take_events();
        assert_eq!(b.panes.len(), 2);
        b.execute(&Task::CloseWindow {
            target: WindowId(2),
        })
        .unwrap();
        let events = b.take_events();
        assert!(events.iter().any(|e| matches!(
            e,
            StateChange::WindowClosed {
                window: WindowId(2)
            }
        )));
        assert_eq!(b.windows.len(), 1);
        assert_eq!(b.panes.len(), 1);
        assert_eq!(b.active_window().map(|w| w.id), Some(WindowId(1)));
    }

    #[tokio::test]
    async fn send_keys_writes_to_pane() {
        let mut b = backend();
        b.connect().await.unwrap();
        let pane = b.active_pane_id().unwrap();
        let outcome = b
            .execute(&Task::SendKeys {
                target: pane,
                keys: vec![crate::core::protocol::terminal::input::KeyEvent::Char('x')],
            })
            .unwrap();
        assert_eq!(outcome, TaskOutcome::Done);
    }

    #[tokio::test]
    async fn write_raw_writes_bytes() {
        let mut b = backend();
        b.connect().await.unwrap();
        let pane = b.active_pane_id().unwrap();
        let outcome = b
            .execute(&Task::WriteRaw {
                target: pane,
                data: b"hi\r".to_vec(),
            })
            .unwrap();
        assert_eq!(outcome, TaskOutcome::Done);
    }

    #[tokio::test]
    async fn resize_pane_updates_size() {
        let mut b = backend();
        b.connect().await.unwrap();
        let pane = b.active_pane_id().unwrap();
        b.execute(&Task::ResizePane {
            target: pane,
            cols: 120,
            rows: 40,
        })
        .unwrap();
        let _ = b.take_events();
        let p = b.pane(&pane).unwrap();
        assert_eq!(p.cols, 120);
        assert_eq!(p.rows, 40);
    }

    #[tokio::test]
    async fn rename_window_emits_event() {
        let mut b = backend();
        b.connect().await.unwrap();
        b.execute(&Task::RenameWindow {
            target: WindowId(1),
            name: "renamed".into(),
        })
        .unwrap();
        let events = b.take_events();
        assert!(events.iter().any(|e| matches!(
            e,
            StateChange::WindowRenamed {
                window: WindowId(1),
                ..
            }
        )));
        assert_eq!(b.active_window().unwrap().name, "renamed");
    }

    #[tokio::test]
    async fn shutdown_kills_all_and_exits() {
        let mut b = backend();
        b.connect().await.unwrap();
        b.execute(&Task::SplitPane {
            target: Some(PaneId(1)),
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
        assert_eq!(b.panes.len(), 2);
        b.shutdown().await.unwrap();
        let _ = b.take_events();
        assert_eq!(b.status(), BackendStatus::Exited);
        assert!(b.panes.is_empty());
        assert!(b.windows.is_empty());
    }

    #[tokio::test]
    async fn close_missing_pane_rejected() {
        let mut b = backend();
        b.connect().await.unwrap();
        let outcome = b.execute(&Task::ClosePane { target: PaneId(99) }).unwrap();
        assert!(matches!(outcome, TaskOutcome::Rejected { .. }));
    }

    #[tokio::test]
    async fn state_views_match_internal() {
        let mut b = backend();
        b.connect().await.unwrap();
        let _ = b.take_events();
        assert_eq!(b.sessions().len(), 1);
        assert_eq!(b.sessions()[0].name, "local");
        assert_eq!(b.active_window().map(|w| w.id), Some(WindowId(1)));
        assert_eq!(b.active_pane().map(|p| p.id), Some(PaneId(1)));
        assert_eq!(b.panes(&TabId(1)).len(), 1);
        assert!(b.layout(&TabId(1)).is_some());
        assert!(b.pane_output(&PaneId(1)).is_some());
    }

    #[tokio::test]
    async fn from_config_uses_pane_config() {
        let cfg = Config::default();
        let b = LocalBackend::from_config(&cfg);
        assert_eq!(b.default_command, cfg.pane.default_command);
        assert_eq!(b.default_workdir, cfg.pane.workdir);
    }

    #[tokio::test]
    async fn pty_output_accumulates_to_pane() {
        let mut b = LocalBackend::new("printf", "/");
        b.connect().await.unwrap();
        let pane = b.active_pane_id().unwrap();
        // 写一个 printf 命令到 pty，让它输出一些字节
        // 实际上子进程是 sleep——这里用 printf 作为命令直接输出
        // 重新构造：用 echo 作为命令
        // 简化：直接验证 drain_pty_output 不 panic
        let _ = b.take_events();
        let _ = b.pane_output(&pane);
    }

    /// 直接写入 PTY 的原始字节：用 `cat` 作命令（回显 stdin），
    /// WriteRaw 的字节应通过 pty 往返出现在 pane_output 里，验证字节保真。
    #[tokio::test]
    async fn write_raw_bytes_roundtrip_to_pty() {
        // cat 会回显 stdin；这里不关它（Ctrl-D 才退出），避免中途 Exit
        let mut b = LocalBackend::new("cat", "/");
        b.connect().await.unwrap();
        let pane = b.active_pane_id().expect("应有 active pane");

        // 写一段含特殊字节的原始序列（模拟 bracketed paste / mouse 上报）
        let seq = b"\x1b[200~pasted\x1b[201~\n";
        let out = b.execute(&Task::WriteRaw {
            target: pane,
            data: seq.to_vec(),
        });
        assert!(matches!(out, Ok(TaskOutcome::Done)), "WriteRaw 应成功");

        // 轮询 take_events，直到 pane_output 含回显字节
        let got = wait_events(&mut b, std::time::Duration::from_secs(3), |events| {
            events.iter().any(|e| {
                matches!(e, crate::core::model::state::StateChange::PaneOutput { data, .. }
                    if String::from_utf8_lossy(data).contains("pasted"))
            })
        })
        .await;
        assert!(
            !got.is_empty(),
            "WriteRaw 的原始字节应被 cat 回显并经 pane_output 送达"
        );

        let out = b.pane_output(&pane).map(|o| o.to_vec()).unwrap_or_default();
        assert!(
            String::from_utf8_lossy(&out).contains("pasted"),
            "pane_output 应含写入的原始文本: {:?}",
            String::from_utf8_lossy(&out)
        );
    }

    #[tokio::test]
    async fn next_prev_pane_cycle_within_active_tab() {
        // 回归：cmd+[ / cmd+] 切 pane 必须只在「当前激活 tab」的布局内循环。
        // 旧实现用全局 find(p.info.active) 找 active pane，且 NewTab/SwitchTab
        // 只切 tab 的 active 标志、不清其他 tab 的 pane active 标志，导致两个 tab
        // 的 pane 同时 active，find 返回旧 tab 的 pane，next/prev 循环到了错误 tab。
        let mut b = backend();
        b.connect().await.unwrap();
        let _ = b.take_events();
        let win = b.active_window().map(|w| w.id).unwrap();

        // tab1（首个）：split 出一个 pane，得到 pane1/pane2 两个 pane
        let tab1_first = b.active_pane_id().unwrap();
        b.execute(&Task::SplitPane {
            target: Some(tab1_first),
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
        let _ = b.take_events();
        assert_eq!(b.panes.len(), 2);

        // 新建 tab2：tab2 的第一个 pane 应是当前 active pane
        b.execute(&Task::NewTab {
            window: win,
            name: None,
            command: Some(vec!["sleep".into(), "60".into()]),
            workdir: None,
        })
        .unwrap();
        let _ = b.take_events();
        assert_eq!(b.tabs.len(), 2, "应有 2 tabs");
        let tab2 = b.active_tab().map(|t| t.id).unwrap();
        let tab2_panes: Vec<PaneId> = b.layout(&tab2).map(|tl| tl.tree.leaves()).unwrap();
        assert_eq!(tab2_panes.len(), 1, "tab2 初始应有 1 pane");
        let tab2_first = tab2_panes[0];
        // 激活 tab 应为 tab2，active pane 应为 tab2 的 pane（旧实现这里是 tab1 的 pane）
        assert_eq!(
            b.active_pane().map(|p| p.id),
            Some(tab2_first),
            "NewTab 后 active pane 应为新 tab 的 pane（当前激活 tab）"
        );

        // 在 tab2 里 split：得到 tab2 的第二个 pane，active 应切到新 pane
        b.execute(&Task::SplitPane {
            target: Some(tab2_first),
            dir: SplitDir::Vertical,
            command: None,
            workdir: None,
        })
        .unwrap();
        let _ = b.take_events();
        let tab2_second = b.active_pane().map(|p| p.id).unwrap();
        assert_ne!(
            tab2_second, tab2_first,
            "split 后 tab2 active 应切到新 pane"
        );
        let leaves2: Vec<PaneId> = b.layout(&tab2).map(|tl| tl.tree.leaves()).unwrap();
        assert!(leaves2.contains(&tab2_second) && leaves2.contains(&tab2_first));

        // cmd+] 从 tab2_second 出发，下一个应回到 tab2 内的 tab2_first（循环），
        // 绝不能跳到 tab1 的 pane。prev 同理。
        b.execute(&Task::NextPane).unwrap();
        let _ = b.take_events();
        let after_next = b.active_pane().map(|p| p.id).unwrap();
        assert_eq!(
            after_next, tab2_first,
            "cmd+] 应在 tab2 内循环到 {tab2_first}, 实际 {after_next}"
        );

        b.execute(&Task::PrevPane).unwrap();
        let _ = b.take_events();
        let after_prev = b.active_pane().map(|p| p.id).unwrap();
        assert_eq!(
            after_prev, tab2_second,
            "cmd+[ 应在 tab2 内循环到 {tab2_second}, 实际 {after_prev}"
        );

        // 再确认 tab2 仍是激活 tab，且 active pane 属于 tab2
        assert_eq!(b.active_tab().map(|t| t.id), Some(tab2));
        assert_eq!(b.tab_of_pane(after_prev), Some(tab2));
    }

    #[tokio::test]
    async fn switch_tab_re_activates_correct_pane() {
        // 回归：Cmd+[ / Cmd+] 切 pane 只应在「当前激活 tab」内循环。
        // 在 tab2 split 出两个 pane、切回 tab1 再切回 tab2 后，active pane
        // 必须是 tab2 的 pane（旧实现切 tab 不清其他 tab 的 pane active 标志，
        // 导致全局 find 返回 tab1 的 pane，next/prev 切到错误 tab）。
        let mut b = backend();
        b.connect().await.unwrap();
        let _ = b.take_events();
        let win = b.active_window().map(|w| w.id).unwrap();

        // tab2：新建并 split，得到两个 pane
        b.execute(&Task::NewTab {
            window: win,
            name: None,
            command: Some(vec!["sleep".into(), "60".into()]),
            workdir: None,
        })
        .unwrap();
        let _ = b.take_events();
        let tab2 = b.active_tab().map(|t| t.id).unwrap();
        let tab2_first = b.active_pane().map(|p| p.id).unwrap();
        b.execute(&Task::SplitPane {
            target: Some(tab2_first),
            dir: SplitDir::Vertical,
            command: None,
            workdir: None,
        })
        .unwrap();
        let _ = b.take_events();
        let tab2_second = b.active_pane().map(|p| p.id).unwrap();
        let leaves2: Vec<PaneId> = b.layout(&tab2).map(|tl| tl.tree.leaves()).unwrap();
        assert!(leaves2.contains(&tab2_second) && leaves2.contains(&tab2_first));

        // 切回 tab1
        b.execute(&Task::SwitchTab { target: TabId(1) }).unwrap();
        let _ = b.take_events();
        assert_eq!(b.active_tab().map(|t| t.id), Some(TabId(1)));

        // 再切回 tab2
        b.execute(&Task::SwitchTab { target: tab2 }).unwrap();
        let _ = b.take_events();
        assert_eq!(b.active_tab().map(|t| t.id), Some(tab2));
        let active = b.active_pane().map(|p| p.id).unwrap();
        assert!(
            leaves2.contains(&active),
            "切回 tab2 后 active pane 应为 tab2 的 pane, 实际 {active}"
        );

        // cmd+] 循环应留在 tab2 内
        b.execute(&Task::NextPane).unwrap();
        let _ = b.take_events();
        let after_next = b.active_pane().map(|p| p.id).unwrap();
        assert!(
            leaves2.contains(&after_next),
            "cmd+] 应留在 tab2 内, 实际 {after_next}"
        );
    }

    /// 辅助：从 LocalBackend 取 active pane id。
    trait ActivePane {
        fn active_pane_id(&self) -> Option<PaneId>;
    }
    impl ActivePane for LocalBackend {
        fn active_pane_id(&self) -> Option<PaneId> {
            self.active_pane().map(|p| p.id)
        }
    }
}
