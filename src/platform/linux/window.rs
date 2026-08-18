//! 主窗口：FFI 驱动的 GTK4 前端。
//!
//! - 启动 `CoreBridge`（connect）
//! - 16ms 轮询 `poll_events`，分发到 tab / pane
//! - 快捷键 → `execute(CTask)`
//! - 退出 → `shutdown()` 或 Drop（`muxterm_free`）

use std::cell::RefCell;
use std::collections::VecDeque;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{ApplicationWindow, Box, CssProvider, EventControllerKey, Orientation, Window};
use vte4::prelude::*;

use anyhow::anyhow;

use crate::core::attention::clock::RealClock;
use crate::core::attention::engine::{AttentionEngine, PaneAttention};
use crate::core::attention::signal::{AttentionSignal, AttentionSource};
use crate::core::catalog::ResolveIntent;
use crate::core::config::{Action, Config, OnLastPaneExit, Theme};
use crate::core::config_edit::set_dotted_key;
use crate::core::discovery::existing::ExistingEntry;
use crate::core::model::backend::{Runtime, RuntimeCapability};
use crate::core::model::layout::{LayoutNode, SplitDir};
use crate::core::model::state::{BackendStatus, StateChange};
use crate::core::model::task::{Task, TaskOutcome};
use crate::core::quickconnect::model::QuickConnect;
use crate::core::runtime::HerdrRuntime;
use crate::core::transport::ssh::probe::SshReach;
use crate::core::types::{PaneId, TabId};
use crate::core::workspace::id::WorkspaceId;
use crate::core::workspace::pool::{WorkspacePool, WorkspacePoolPolicy};
use crate::core::workspace::spec::WorkspaceSpec;
use crate::core::workspace::workspace::Workspace;
use crate::platform::i18n::{self, Key};
use crate::platform::linux::attention_ui::{window_title, GioSink, NotificationSink};
use crate::platform::linux::command_palette::{parse_palette_action, PaletteAction};
use crate::platform::linux::ffi_bridge::CoreBridge;
use crate::platform::linux::keymap::KeyMap;
use crate::platform::linux::layout_host::LayoutHost;
use crate::platform::linux::lifecycle::{cycle_pane_id, should_close_window};
use crate::platform::linux::pane_view::PaneView;
use crate::platform::linux::panel_model::PanelTab;
use crate::platform::linux::quickconnect::event_policy::ClientSizePolicy;
use crate::platform::linux::quickconnect::font::{FontSettings, Preferences};
use crate::platform::linux::quickconnect::model::{TargetConfig, TargetRuntime, TargetTransport};
use crate::platform::linux::quickconnect::project_flow::{
    ProjectConnectFlow, ProjectConnectIntent, ProjectConnectState,
};
use crate::platform::linux::quickconnect::status_style::{StatusBarMode, StatusBarSnapshot};
use crate::platform::linux::quickconnect::store::QuickConnectStore;
use crate::platform::linux::quickconnect::tab_gate::TabSwitchGate;
use crate::platform::linux::quickconnect_panel::{
    build_root_items, ExistingNav, ExistingPanelState, PanelItem,
};
use crate::platform::linux::status_bar::{ConnectionSummary, StatusBar};
use crate::platform::linux::tmux_dialog::{self, TmuxAction};

/// 主窗口。
pub struct AppWindow {
    pub window: Window,
    /// 保持 UI 状态与 CoreBridge 存活（轮询闭包只用 Weak，避免循环引用）。
    _state: Rc<RefCell<UiState>>,
}

struct UiState {
    /// core 连接池：Runtime 生命周期只在 core（W5）。
    pool: WorkspacePool,
    /// 每个工作区一个像素缓存（VTE 不随切走销毁；Runtime 不在 GUI）。
    pixel_cache: std::collections::HashMap<WorkspaceId, LayoutHost>,
    /// 当前挂载到窗口的 LayoutHost 对应的工作区。
    mounted_ws: Option<WorkspaceId>,
    /// 本轮结构事件触发 refresh_ui 后，已经从 core snapshot seed 的 pane。
    /// 对应的 PaneSnapshot 事件只需作为通知消费一次，不能再次 reset/feed。
    snapshot_seeded_this_batch: HashSet<u32>,
    /// 供 `WorkspacePool::open` 同步 block_on；后台任务存活到应用退出。
    rt: tokio::runtime::Runtime,
    qc_store: QuickConnectStore,
    poll_source: Option<glib::SourceId>,
    /// 当前终端字体（config + 运行期偏好）。
    font: FontSettings,
    /// config.toml 的字号，Reset 回到这里。
    config_font_size: f32,
    theme: Theme,
    theme_name: String,
    status: StatusBar,
    status_mode: StatusBarMode,
    last_status_at: Instant,
    status_interval: Duration,
    keymap: KeyMap,
    active_tab: u32,
    active_pane: u32,
    /// 最近一次同步给后端/PTY 的尺寸。
    /// - SharedClientResize（tmux）：`(None, cols, rows)` 整窗 client size
    /// - 其它 Runtime（shell / Herdr）：`(Some(pane), cols, rows)` 按 pane 跳过，
    ///   避免切 tab 后同像素尺寸被全局缓存吞掉 ResizePane（htop 0826）。
    last_client_size: Option<(Option<u32>, u16, u16)>,
    /// tmux SharedClientResize：同一尺寸连续命中才 dispatch（约 10×16ms），
    /// 避免 map 时 106→284→142 连发 -C（dogfood 2152）。
    pending_client_size: Option<(u16, u16)>,
    pending_client_hits: u8,
    tab_gate: TabSwitchGate,
    preferences: Preferences,
    on_last_pane_exit: OnLastPaneExit,
    /// 事件分发里不能同步 `window.close()`（可能正握着 RefCell）。
    pending_close: bool,
    /// 注意力引擎（信号 → 状态机 → blocked 工作区聚合）。
    attention: AttentionEngine<RealClock>,
    /// 本轮进入 blocked 的 workspace 通知日志（测试钩子读取）。
    notification_log: Vec<String>,
    /// 通知出口（生产 GioSink fail-soft；测试可替换）。
    notification_sink: std::boxed::Box<dyn NotificationSink>,
    /// 面板是否打开及当前 tab（测试钩子 + badge 点击入口）。
    panel_open: Option<PanelTab>,
    /// 用户显式 Quit（Ctrl+Q / 命令面板）：close_request 放行真正关闭。
    quit_requested: bool,
    /// 最近一次 STATE_BACKEND_STATUS 的 pane_id 编码（连接状态）。
    runtime_status: u32,
    /// tmux status-left/right 订阅推送值（覆盖默认状态栏文案）。
    status_left: Option<String>,
    status_right: Option<String>,
    /// 每个工作区的 tmux `-L` socket 名（仅 tmux 工作区有）。
    workspace_sockets: std::collections::HashMap<WorkspaceId, Option<String>>,
    /// 上一次流量快照（down, up）与墙钟（W15a 速率差）。
    last_traffic: Option<(u64, u64)>,
    last_traffic_at: Option<Instant>,
    /// 后台连接结果队列（W15c：open_spec 离开 GTK 线程，16ms poll 收编）。
    pending_connects: std::collections::VecDeque<std::sync::mpsc::Receiver<PendingConnect>>,
    /// 后台 worktree 创建结果队列（H4：建 checkout + 新格 connect 离开 GTK 线程）。
    pending_worktree_creates:
        std::collections::VecDeque<std::sync::mpsc::Receiver<anyhow::Result<Workspace>>>,
    /// SSH 可达性探测结果队列（W15d：面板打开时后台探测，TTL 缓存）。
    pending_ssh_probes: std::collections::VecDeque<std::sync::mpsc::Receiver<(String, SshReach)>>,
    /// SSH 别名 → (可达性, 探测时间)；TTL 内复用，不在 16ms tick 扫。
    ssh_reach_cache: std::collections::HashMap<String, (SshReach, Instant)>,
    /// W20：已有的连接面板共享状态（nav + 本地/SSH 数据）。
    existing: Rc<RefCell<ExistingPanelState>>,
    /// W20：SSH 已有连接探测是否在跑（防并发）。
    existing_ssh_probing: bool,
    /// W20：SSH 已有连接探测结果队列。
    pending_existing_ssh:
        std::collections::VecDeque<std::sync::mpsc::Receiver<ExistingSshProbeResult>>,
    /// C7：本地已有连接探测结果队列（open_panel 不阻塞 GTK）。
    pending_local_probe: std::collections::VecDeque<std::sync::mpsc::Receiver<ExistingProbeMsg>>,
    /// W21 测试钩子：最近一次经 PaneView input_cb 的原始输入。
    last_raw_input: Vec<u8>,
    /// VTE 输入回调只把 owner identity 和原始字节放入 FIFO；实际的
    /// Runtime 写入统一在 GTK poll 中完成，避免回调重入 UiState。
    surface_input_queue: Rc<RefCell<VecDeque<SurfaceInput>>>,
    /// W17a 自动重连：是否已有重连线程在跑（防并发重连）。
    reconnecting: bool,
    /// 重连失败退避：下一次允许发起重连的时刻。
    reconnect_retry_at: Option<Instant>,
    /// 连续失败次数（指数退避基数）。
    reconnect_attempts: u32,
    /// 重连结果队列（新 Runtime 回主线程后 swap 进同一个 Workspace）。
    pending_reconnects: std::collections::VecDeque<std::sync::mpsc::Receiver<ReconnectResult>>,
    /// 窗口根容器（挂载当前工作区的 LayoutHost.root_box）。
    root_box: gtk4::Box,
    /// 终端区 Overlay：LayoutHost.root_box 是主 child，回底按钮浮在上面。
    layout_overlay: gtk4::Overlay,
    /// 回底按钮（W16a：滚离底部后显示，点击回到尾部）。
    jump_latest: gtk4::Button,
    /// 离开底部期间累计的新行数（W18e：按钮显示 +N）。
    jump_unseen: u32,
    /// 断线水印（W16b：tmux server 死后保留最后一帧 + 覆盖提示）。
    disconnect_overlay: gtk4::Label,
    /// 搜索命中高亮（W17c：客户端覆盖层，不改 pane 字节）。
    search_highlight: gtk4::Label,
    /// 当前 pane 内查找条（W18f：Ctrl+F / test_open_pane_find 同一条生产路径）。
    pane_find: gtk4::Box,
    pane_find_entry: gtk4::Entry,
    /// 上次看到这里（W18g）：(workspace, pane) → 离开时的最后一行文本。
    last_seen: std::collections::HashMap<(String, u32), String>,
    /// 上次看到这里标记（客户端覆盖层，不改 pane 字节）。
    last_seen_mark: gtk4::Button,
    /// 命令刻度（W18h）：最近成功/失败命令的滚动条旁标记。
    cmd_mark_ok: gtk4::Button,
    cmd_mark_fail: gtk4::Button,
    /// 刻度点击要滚到的命令文本（由 update_command_marks 更新）。
    cmd_mark_ok_text: std::rc::Rc<std::cell::RefCell<Option<String>>>,
    cmd_mark_fail_text: std::rc::Rc<std::cell::RefCell<Option<String>>>,
    /// VTE scrollback 行数（新建 LayoutHost 时用）。
    scrollback_lines: u32,
    /// 启动配置的 tmux `-L` socket（本地 tmux 连接默认用它）。
    default_socket: Option<String>,
    /// 自身弱引用（滚动 provider 用，避免循环引用）。
    self_weak: std::rc::Weak<RefCell<UiState>>,
}

/// 一个 Surface 输入事件的稳定 owner。
///
/// PaneView 可以在 layout 重建、tab 切换或 workspace 切换之后才触发
/// callback，因此不能在 callback 时读取「当前 active workspace」。
#[derive(Debug, Clone, PartialEq, Eq)]
struct SurfaceInput {
    workspace: WorkspaceId,
    pane: PaneId,
    data: Vec<u8>,
}

impl UiState {
    fn active_workspace(&self) -> &Workspace {
        self.pool.active().expect("必须有前台连接")
    }

    fn active_workspace_mut(&mut self) -> &mut Workspace {
        self.pool.active_mut().expect("必须有前台连接")
    }

    fn active_ws_id(&self) -> &WorkspaceId {
        self.pool.active_id().expect("必须有前台连接")
    }

    fn active_layout(&self) -> &LayoutHost {
        let id = self.active_ws_id();
        self.pixel_cache
            .get(id)
            .expect("active workspace 必须有 layout")
    }

    fn active_layout_mut(&mut self) -> &mut LayoutHost {
        let id = self.active_ws_id().clone();
        self.pixel_cache
            .get_mut(&id)
            .expect("active workspace 必须有 layout")
    }

    /// 当前前台是否 tmux/SSH 控制 client（local shell 不支持 detach）。
    fn uses_tmux(&self) -> bool {
        matches!(
            self.active_workspace().state().workspace_runtime(),
            "tmux" | "ssh" | "tmux-ssh"
        )
    }
}

impl AppWindow {
    /// 有序关闭：停轮询 → 摘掉子树 → destroy 窗口，避免与 PaneView 持有的 VTE 交叉销毁。
    pub fn shutdown(self) {
        crate::platform::linux::quickconnect_panel::clear_panel_hooks();
        {
            let mut s = self._state.borrow_mut();
            if let Some(id) = s.poll_source.take() {
                id.remove();
            }
            s.pool.shutdown_all();
            for layout in s.pixel_cache.values_mut() {
                layout.reset(false);
                while let Some(child) = layout.root_box.first_child() {
                    layout.root_box.remove(&child);
                }
            }
            // 显式释放全部 LayoutHost/PaneView/VTE：GTK 对象必须在本窗口
            // destroy 前解构，否则 VTE 的 GL 资源残留到下一个测试窗口
            // realize 时才 finalize，与新的 GL 初始化交叉 = 堆损坏
            // （linux_herdr_agent_e2e 连续多测试时可见 double free）。
            s.pixel_cache.clear();
            // Popover 挂在状态点按钮上：先解除父子关系，避免 dot 销毁时
            // popover 仍引用它（finalize-with-children 堆损坏）。
            s.status.popover_widget().unparent();
        }
        self.window.set_child(None::<&gtk4::Widget>);
        self.window.destroy();
        // 让 GTK 在窗口销毁后继续跑完 pending finalize，避免跨测试残留。
        while glib::MainContext::default().iteration(false) {}
    }

    pub fn new(cfg: Config, theme: Theme) -> Self {
        let window = ApplicationWindow::builder()
            .title("muxterm")
            .default_width(960)
            .default_height(640)
            .build();
        let window: Window = window.upcast();

        let socket = {
            let s = cfg.tmux.socket.trim();
            if s.is_empty() {
                None
            } else {
                Some(s.to_string())
            }
        };
        let session = {
            let s = cfg.tmux.default_session.trim();
            if s.is_empty() {
                None
            } else {
                Some(s.to_string())
            }
        };

        let requested_tmux = socket.is_some();
        // core 池：Runtime 生命周期只在 core；GUI 只 bind 当前 Workspace。
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .build()
            .expect("tokio runtime");
        let mut pool =
            WorkspacePool::new(WorkspacePoolPolicy::new(cfg.pool.max_slots.max(1) as usize));
        let startup_id = if requested_tmux {
            let spec = WorkspaceSpec::local_tmux(session.clone(), socket.clone())
                .with_scrollback_lines(cfg.scrollback.lines);
            let id = spec.id();
            let opened = rt.block_on(pool.open_spec(&spec));
            match opened {
                Ok(_) => Some(id),
                Err(e) => {
                    tracing::error!(target = "muxterm::linux", "启动核心失败: {e}");
                    None
                }
            }
        } else {
            None
        };
        let startup_id = startup_id.unwrap_or_else(|| {
            let spec = WorkspaceSpec::local_shell("").with_scrollback_lines(cfg.scrollback.lines);
            let id = spec.id();
            rt.block_on(pool.open_spec(&spec))
                .expect("local runtime 必须可用");
            id
        });
        let mut startup_sockets = std::collections::HashMap::new();
        if requested_tmux {
            // 启动 attach 的工作区也要登记 socket（W17a 重连要用）。
            startup_sockets.insert(startup_id.clone(), socket.clone());
        }

        let root = Box::builder()
            .orientation(Orientation::Vertical)
            .spacing(0)
            .build();
        root.add_css_class("muxterm-root");

        let preferences = Preferences::load();
        let theme_name = preferences
            .theme
            .clone()
            .unwrap_or_else(|| cfg.theme.name.clone())
            .to_ascii_lowercase();
        let theme = Theme::load(&theme_name).unwrap_or(theme);
        apply_chrome_css(&theme);
        let config_font_size = cfg.font.size;
        let mut font = FontSettings {
            family: cfg.font.family.clone(),
            size: cfg.font.size,
        };
        if let Some(size) = preferences.font_size {
            font.size = FontSettings::clamp_size(size);
        }
        let status_mode = preferences
            .statusbar_mode
            .as_deref()
            .map(|m| StatusBarMode::from_toml(Some(m)))
            .unwrap_or_else(|| StatusBarMode::from_toml(Some(&cfg.statusbar.mode)));

        let uses_tmux = matches!(
            pool.active().map(|w| w.state().workspace_runtime()),
            Some("tmux" | "ssh" | "tmux-ssh")
        );
        let mut pixel_cache = std::collections::HashMap::new();
        let layout = LayoutHost::new(theme.clone(), font.clone(), uses_tmux, cfg.scrollback.lines);
        pixel_cache.insert(startup_id.clone(), layout);
        let status = StatusBar::new(status_mode, theme.clone());
        status.container.add_css_class("status-bar");

        // 唯一 chrome：一条 status bar（LINUX-PLAN §3），没有第二条 TabBar。
        // 终端区包一层 Overlay：回底按钮浮在 VTE 右下角（W16a）。
        let layout_overlay = gtk4::Overlay::new();
        layout_overlay.set_hexpand(true);
        layout_overlay.set_vexpand(true);
        layout_overlay.set_child(Some(
            &pixel_cache
                .get(&startup_id)
                .expect("startup layout")
                .root_box,
        ));
        let jump_latest = gtk4::Button::with_label("↓");
        jump_latest.set_widget_name("muxterm-jump-latest");
        jump_latest.set_halign(gtk4::Align::End);
        jump_latest.set_valign(gtk4::Align::End);
        jump_latest.set_margin_end(12);
        jump_latest.set_margin_bottom(12);
        jump_latest.set_visible(false);
        let disconnect_overlay = gtk4::Label::new(Some("已断开"));
        disconnect_overlay.set_widget_name("muxterm-disconnect-overlay");
        disconnect_overlay.set_halign(gtk4::Align::Center);
        disconnect_overlay.set_valign(gtk4::Align::Center);
        disconnect_overlay.add_css_class("muxterm-disconnect-overlay");
        disconnect_overlay.set_visible(false);
        let search_highlight = gtk4::Label::new(Some("▮"));
        search_highlight.set_widget_name("muxterm-search-highlight");
        search_highlight.set_halign(gtk4::Align::Start);
        search_highlight.set_valign(gtk4::Align::Center);
        search_highlight.set_margin_start(4);
        search_highlight.add_css_class("muxterm-search-highlight");
        search_highlight.set_visible(false);
        let pane_find = gtk4::Box::builder()
            .orientation(Orientation::Horizontal)
            .spacing(6)
            .margin_top(8)
            .margin_start(8)
            .margin_end(8)
            .build();
        pane_find.set_widget_name("muxterm-pane-find");
        pane_find.set_halign(gtk4::Align::Start);
        pane_find.set_valign(gtk4::Align::Start);
        pane_find.add_css_class("muxterm-pane-find");
        let pane_find_entry = gtk4::Entry::new();
        pane_find_entry.set_widget_name("muxterm-pane-find-entry");
        pane_find_entry.set_placeholder_text(Some("find in pane…"));
        pane_find.append(&pane_find_entry);
        pane_find.set_visible(false);
        let last_seen_mark = gtk4::Button::with_label("上次看到这里");
        last_seen_mark.set_widget_name("muxterm-last-seen");
        last_seen_mark.set_halign(gtk4::Align::Start);
        last_seen_mark.set_valign(gtk4::Align::Center);
        last_seen_mark.set_margin_start(4);
        last_seen_mark.add_css_class("muxterm-last-seen");
        last_seen_mark.set_visible(false);
        let cmd_mark_ok_text = Rc::new(RefCell::new(None::<String>));
        let cmd_mark_fail_text = Rc::new(RefCell::new(None::<String>));
        let cmd_mark_ok = gtk4::Button::with_label("✓");
        cmd_mark_ok.set_widget_name("muxterm-cmd-mark-ok");
        cmd_mark_ok.set_halign(gtk4::Align::End);
        cmd_mark_ok.set_valign(gtk4::Align::Center);
        cmd_mark_ok.set_margin_end(2);
        cmd_mark_ok.add_css_class("muxterm-cmd-mark-ok");
        cmd_mark_ok.set_visible(false);
        let cmd_mark_fail = gtk4::Button::with_label("✗");
        cmd_mark_fail.set_widget_name("muxterm-cmd-mark-fail");
        cmd_mark_fail.set_halign(gtk4::Align::End);
        cmd_mark_fail.set_valign(gtk4::Align::Center);
        cmd_mark_fail.set_margin_end(2);
        cmd_mark_fail.add_css_class("muxterm-cmd-mark-fail");
        cmd_mark_fail.set_visible(false);
        layout_overlay.add_overlay(&pane_find);
        layout_overlay.add_overlay(&search_highlight);
        layout_overlay.add_overlay(&disconnect_overlay);
        layout_overlay.add_overlay(&last_seen_mark);
        layout_overlay.add_overlay(&cmd_mark_ok);
        layout_overlay.add_overlay(&cmd_mark_fail);
        layout_overlay.add_overlay(&jump_latest);
        root.append(&layout_overlay);
        root.append(&status.container);
        window.set_child(Some(&root));

        let keymap = KeyMap::from_bindings(&cfg.keybindings);
        let qc_store =
            QuickConnectStore::new_unified(crate::core::config::Config::user_config_path());
        let state = Rc::new(RefCell::new(UiState {
            pool,
            pixel_cache,
            mounted_ws: Some(startup_id),
            snapshot_seeded_this_batch: HashSet::new(),
            rt,
            qc_store,
            poll_source: None,
            font,
            config_font_size,
            theme,
            theme_name,
            status,
            status_mode,
            last_status_at: Instant::now()
                .checked_sub(Duration::from_secs(10))
                .unwrap_or_else(Instant::now),
            status_interval: Duration::from_secs(1),
            keymap,
            active_tab: 0,
            active_pane: 0,
            last_client_size: None,
            pending_client_size: None,
            pending_client_hits: 0,
            tab_gate: TabSwitchGate::new(Duration::from_millis(1500)),
            preferences,
            on_last_pane_exit: cfg.behavior.on_last_pane_exit,
            pending_close: false,
            attention: AttentionEngine::new(cfg.attention.clone(), RealClock),
            notification_log: Vec::new(),
            notification_sink: std::boxed::Box::new(GioSink::new(None)),
            panel_open: None,
            quit_requested: false,
            runtime_status: crate::core::protocol::ffi::types::BACKEND_STATUS_CONNECTED,
            status_left: None,
            status_right: None,
            workspace_sockets: startup_sockets,
            last_traffic: None,
            last_traffic_at: None,
            pending_connects: std::collections::VecDeque::new(),
            pending_worktree_creates: std::collections::VecDeque::new(),
            pending_ssh_probes: std::collections::VecDeque::new(),
            ssh_reach_cache: std::collections::HashMap::new(),
            existing: Rc::new(RefCell::new(ExistingPanelState::default())),
            existing_ssh_probing: false,
            pending_existing_ssh: std::collections::VecDeque::new(),
            pending_local_probe: std::collections::VecDeque::new(),
            last_raw_input: Vec::new(),
            surface_input_queue: Rc::new(RefCell::new(VecDeque::new())),
            reconnecting: false,
            reconnect_retry_at: None,
            reconnect_attempts: 0,
            pending_reconnects: std::collections::VecDeque::new(),
            root_box: root.clone(),
            layout_overlay,
            jump_latest,
            jump_unseen: 0,
            disconnect_overlay,
            search_highlight,
            pane_find,
            pane_find_entry,
            last_seen: std::collections::HashMap::new(),
            last_seen_mark,
            cmd_mark_ok,
            cmd_mark_fail,
            cmd_mark_ok_text: cmd_mark_ok_text.clone(),
            cmd_mark_fail_text: cmd_mark_fail_text.clone(),
            scrollback_lines: cfg.scrollback.lines,
            default_socket: socket.clone(),
            self_weak: std::rc::Weak::new(),
        }));
        state.borrow_mut().self_weak = Rc::downgrade(&state);

        // status bar 中区 tab 按钮 → SwitchTab(id)
        {
            let st = state.clone();
            state
                .borrow()
                .status
                .connect_window_activate(move |tab_id| {
                    let mut s = st.borrow_mut();
                    request_switch_tab(&mut s, tab_id);
                });
        }

        // 命令刻度点击：滚到对应命令文本所在行（W18h）。
        {
            let st = state.clone();
            let text = cmd_mark_ok_text.clone();
            state.borrow().cmd_mark_ok.connect_clicked(move |_| {
                scroll_to_command_text(&st, &text);
            });
        }
        {
            let st = state.clone();
            let text = cmd_mark_fail_text.clone();
            state.borrow().cmd_mark_fail.connect_clicked(move |_| {
                scroll_to_command_text(&st, &text);
            });
        }

        // 上次看到这里：点击滚回离开时的那一行（W18g）。
        {
            let st = state.clone();
            state.borrow().last_seen_mark.connect_clicked(move |_| {
                let s = st.borrow();
                let ws = active_workspace_id(&s);
                let pane = s.active_pane;
                if let Some(text) = s.last_seen.get(&(ws.clone(), pane)).cloned() {
                    let lines = s.active_workspace().pane_last_n_lines(PaneId(pane), 10_000);
                    if let Some(row) = lines.iter().position(|l| l.contains(&text)) {
                        if let Some(view) = s.active_layout().pane(pane).cloned() {
                            if let Some(adj) = view.terminal().vadjustment() {
                                adj.set_value(adj.lower() + row as f64);
                            }
                        }
                    }
                }
                s.last_seen_mark.set_visible(false);
            });
        }

        // 当前 pane 内查找：输入即滚到第一个命中（W18f）。
        {
            let st = state.clone();
            state.borrow().pane_find_entry.connect_changed(move |e| {
                let q = e.text().to_string();
                if q.is_empty() {
                    return;
                }
                let s = st.borrow();
                let pane = s.active_pane;
                let hits = s.active_workspace().search_pane(PaneId(pane), &q);
                if let Some(hit) = hits.first() {
                    if let Some(row) = s
                        .active_workspace()
                        .pane_line_index_by_seq(PaneId(pane), hit.seq)
                    {
                        if let Some(view) = s.active_layout().pane(pane).cloned() {
                            if let Some(adj) = view.terminal().vadjustment() {
                                adj.set_value(adj.lower() + row as f64);
                            }
                        }
                    }
                }
            });
        }

        // 回底按钮：把当前激活 pane 的 VTE 滚回尾部（W16a）。
        {
            let st = state.clone();
            state.borrow().jump_latest.connect_clicked(move |_| {
                let mut s = st.borrow_mut();
                s.jump_unseen = 0;
                if let Some(view) = s.active_layout().pane(s.active_pane).cloned() {
                    if let Some(adj) = view.terminal().vadjustment() {
                        adj.set_value(adj.upper());
                    }
                }
            });
        }

        // 状态点 → popover：由 StatusBar 的 connect_clicked 处理（C8.4）。

        // 通知/面板按钮：n=0 → Workspaces，n>0 → Attention
        {
            let st = state.clone();
            let win = window.clone();
            state.borrow().status.connect_attention_activate(move || {
                let n = st.borrow().attention.blocked_workspace_count();
                let tab = if n > 0 {
                    PanelTab::Attention
                } else {
                    PanelTab::Workspaces
                };
                open_panel(&st, &win, tab);
            });
        }

        // 新建 tab 按钮 → Action::NewTab
        {
            let st = state.clone();
            state.borrow().status.connect_new_tab(move || {
                let mut s = st.borrow_mut();
                let _ = s.active_workspace_mut().execute(Task::NewTab {
                    name: None,
                    command: None,
                    workdir: None,
                });
                // Accepted 不得手工 refresh：等 LayoutChanged/MutationSettled。
            });
        }

        // worktree 创建按钮 → 对话框（仅 support() 含 WorktreeList 时可见）。
        {
            let st = state.clone();
            let win = window.clone();
            state.borrow().status.connect_worktree_create(move || {
                show_worktree_create_dialog(&st, &win);
            });
        }

        // worktree 创建按钮 → 对话框（仅 support() 含 WorktreeList 时可见）。
        {
            let st = state.clone();
            let win = window.clone();
            state.borrow().status.connect_worktree_create(move || {
                show_worktree_create_dialog(&st, &win);
            });
        }

        // 快捷键
        {
            let st = state.clone();
            let controller = EventControllerKey::new();
            controller.set_propagation_phase(gtk4::PropagationPhase::Capture);
            let window_for_palette = window.clone();
            controller.connect_key_pressed(move |c, keyval, _keycode, mods| {
                // GTK4 回调里的 mods 可能不含已被 keyval 消费的 Shift；
                // 再并上 current_event_state，Ctrl+Shift+C 才进 Copy 而不是 \\003。
                let mods = mods
                    | (c.current_event_state()
                        & (gdk::ModifierType::CONTROL_MASK
                            | gdk::ModifierType::SHIFT_MASK
                            | gdk::ModifierType::ALT_MASK
                            | gdk::ModifierType::SUPER_MASK));
                let action = {
                    let s = st.borrow();
                    s.keymap.lookup(keyval, mods)
                };
                let Some(action) = action else {
                    return glib::Propagation::Proceed;
                };
                // Ctrl+Q 必须在放下 RefCell 之后再 close：close-request 会再借同一把锁。
                if action == Action::Quit {
                    st.borrow_mut().quit_requested = true;
                    window_for_palette.close();
                    return glib::Propagation::Stop;
                }
                // QuickConnect 面板的 rebuild 会同步 borrow state：先放锁再打开。
                if action == Action::QuickConnect {
                    open_panel(&st, &window_for_palette, PanelTab::Workspaces);
                    return glib::Propagation::Stop;
                }
                if action == Action::Search {
                    open_panel(&st, &window_for_palette, PanelTab::Search);
                    return glib::Propagation::Stop;
                }
                // W18f：Ctrl+F = 当前 pane 内查找（生产路径，与 test_open_pane_find 同）。
                if keyval == gdk::Key::f
                    && mods.contains(gdk::ModifierType::CONTROL_MASK)
                    && !mods.contains(gdk::ModifierType::SHIFT_MASK)
                {
                    open_pane_find(&st, &window_for_palette);
                    return glib::Propagation::Stop;
                }
                let mut s = st.borrow_mut();
                handle_action(&mut s, action, &window_for_palette, &st);
                glib::Propagation::Stop
            });
            window.add_controller(controller);
        }

        // 关闭窗口：非 Quit 动作隐藏并保持 16ms 轮询；Quit 才真正关闭。
        // try_borrow：命令面板 Detach 可能仍握着 RefMut 时同步 close（dogfood 0826）。
        {
            let st = state.clone();
            let win = window.clone();
            window.connect_close_request(move |_| {
                let quit = st.try_borrow().map(|s| s.quit_requested).unwrap_or(false);
                match close_intent(quit) {
                    CloseIntent::Quit => glib::Propagation::Proceed,
                    CloseIntent::HideKeepPolling => {
                        win.set_visible(false);
                        glib::Propagation::Stop
                    }
                }
            });
        }

        // 首次刷新 + 窗口级 16ms 轮询（切连接后仍打到当前 active slot）
        {
            let mut s = state.borrow_mut();
            let events = s.active_workspace_mut().refresh();
            let wid = s.active_ws_id().clone();
            let ws = workspace_replica_id(&wid);
            for event in &events {
                apply_attention_event_from_workspace(&mut s, &wid, &ws, event);
            }
            refresh_ui(&mut s);
            report_all_pane_colours(&mut s);
            maybe_refresh_status(&mut s, true);
        }

        {
            let st_weak = Rc::downgrade(&state);
            let win_weak = window.downgrade();
            let id = glib::timeout_add_local(Duration::from_millis(16), move || {
                // W19e：glib trampoline 不能 unwind；panic 先在这里接住，
                // 报告 + 弹窗后继续轮询（Break 会让轮询停掉 = 假死）。
                let outcome = crate::platform::linux::fault_gtk::run("linux.poll", || {
                    if win_weak.upgrade().is_none() {
                        return glib::ControlFlow::Break;
                    }
                    let Some(st) = st_weak.upgrade() else {
                        return glib::ControlFlow::Break;
                    };
                    let pending_close = {
                        drain_pending_connects(&st);
                        drain_ssh_probes(&st);
                        drain_local_existing(&st);
                        drain_pending_reconnects(&st);
                        maybe_schedule_reconnect(&st);
                        let mut s = st.borrow_mut();
                        // 后台工作区由 core 池 poll：PaneBuf 已在 Workspace::refresh 里
                        // 喂好，这里把注意力信号应用到引擎，并把 Surface 事件
                        // 按 (WorkspaceId, PaneId) 送进对应 background pixel cache。
                        for (wid, events) in s.pool.poll_background() {
                            dispatch_event_batch_for(&mut s, &wid, events);
                        }
                        s.pool.evict_expired();
                        for wid in s.pool.take_evicted() {
                            s.pixel_cache.remove(&wid);
                        }
                        let events = s.active_workspace_mut().refresh();
                        let mut structural = false;
                        for ev in &events {
                            if matches!(
                                ev,
                                StateChange::TabAdded { .. }
                                    | StateChange::TabClosed { .. }
                                    | StateChange::LayoutChanged { .. }
                                    | StateChange::PaneAdded { .. }
                                    | StateChange::PaneClosed { .. }
                            ) {
                                structural = true;
                            }
                        }
                        dispatch_event_batch(&mut s, events);
                        // blocked 与 done 通知都要在 16ms poll 里收编（W17d）：
                        // test_poll_once 的 drain 可能在 16ms poll 应用信号之前运行，
                        // 只 drain blocked 会让后台 Done 的通知永远等不到下一次 poll。
                        drain_attention_notifications(&mut s);
                        sync_pane_outputs(&mut s);
                        sync_window_size(&mut s);
                        // 输入必须在本轮 topology/snapshot/geometry 收编之后
                        // 写入，避免 attach 新 pane 尚未完成首帧时丢掉 send-keys。
                        drain_surface_input(&mut s);
                        maybe_refresh_status(&mut s, structural);
                        refresh_connection_summary(&mut s);
                        update_command_marks(&s);
                        update_jump_latest(&s);
                        if let Some(w) = win_weak.upgrade() {
                            refresh_attention_chrome(&s, &w);
                        }
                        let close = s.pending_close;
                        if close {
                            s.pending_close = false;
                        }
                        close
                    };
                    if pending_close {
                        st.borrow_mut().quit_requested = true;
                        if let Some(w) = win_weak.upgrade() {
                            w.close();
                        }
                    }
                    glib::ControlFlow::Continue
                });
                // 接住 panic 后进程必须继续：fault_gtk::run 已弹窗，
                // 这里统一 Continue（不 Break，避免轮询停掉）。
                outcome.unwrap_or(glib::ControlFlow::Continue)
            });
            state.borrow_mut().poll_source = Some(id);
        }

        Self {
            window,
            _state: state,
        }
    }

    /// W21 测试钩子：向指定 pane 的生产滚轮路径发一次滚动。
    pub fn test_emit_scroll(&self, pane: u32, delta_y: f64) {
        let s = self._state.borrow();
        if let Some(view) = s.active_layout().pane(pane).cloned() {
            view.test_emit_scroll(delta_y);
        }
    }

    /// 测试用：向当前激活 pane 发送原始输入（如 `echo hi\n` / `\x04` Ctrl+D）。
    pub fn test_send_input(&self, data: &[u8]) {
        let mut s = self._state.borrow_mut();
        let ws = active_workspace_id(&s);
        let pane = s.active_pane;
        let _ = s.active_workspace_mut().execute(Task::WriteRaw {
            target: PaneId(pane),
            data: data.to_vec(),
        });
        s.attention.on_user_input(&ws, pane);
    }

    /// 测试用：向当前 VTE 发出生产 `commit` 信号。与 `test_send_input` 不同，
    /// 这里必须经过 PaneView.connect_input → 当前 Runtime 的完整输入路径。
    pub fn test_emit_active_pane_commit(&self, text: &str) -> bool {
        let view = {
            let s = self._state.borrow();
            s.active_layout().pane(s.active_pane).cloned()
        };
        let Some(view) = view else {
            return false;
        };
        view.test_emit_commit(text);
        true
    }

    /// 测试用：调用生产快捷键动作分发，禁止集成测试绕过 `handle_action`
    /// 直接构造 `Task`。
    pub fn test_handle_action(&self, action: Action) {
        let mut s = self._state.borrow_mut();
        handle_action(&mut s, action, &self.window, &self._state);
    }

    /// 测试用：当前工作区的稳定 replica id，供 `WorkspacePool` 切换断言。
    pub fn test_active_workspace_replica_id(&self) -> String {
        active_workspace_id(&self._state.borrow())
    }

    /// 测试用：当前工作区 Runtime id。
    pub fn test_active_workspace_runtime(&self) -> String {
        self._state
            .borrow()
            .active_workspace()
            .runtime()
            .workspace_runtime()
            .to_string()
    }

    /// 测试用：能力判断必须走 Runtime 契约，不能按 runtime 名字分支。
    pub fn test_active_runtime_supports(&self, capability: RuntimeCapability) -> bool {
        self._state
            .borrow()
            .active_workspace()
            .runtime()
            .support()
            .contains(&capability)
    }

    /// 测试用：对当前 Runtime 执行真实 detach，并保留精确 outcome。
    pub fn test_detach_active_workspace_outcome(&self) -> anyhow::Result<TaskOutcome> {
        self._state
            .borrow_mut()
            .active_workspace_mut()
            .execute(Task::Detach)
    }

    /// 测试用：走生产 `adjust_font(+1)`（Ctrl+= 热路径）。
    pub fn test_increase_font(&self) {
        let mut s = self._state.borrow_mut();
        adjust_font(&mut s, 1);
    }

    /// 测试用：走生产 `adjust_font(-1)`（Ctrl+- 热路径）。
    pub fn test_decrease_font(&self) {
        let mut s = self._state.borrow_mut();
        adjust_font(&mut s, -1);
    }

    /// 测试用：当前 UiState 字号（缩放热路径断言）。
    pub fn test_font_size(&self) -> f32 {
        self._state.borrow().font.size
    }

    /// 测试用：当前激活 pane 的核心输出快照。
    pub fn test_active_pane_output(&self) -> Vec<u8> {
        let s = self._state.borrow();
        s.active_workspace()
            .state()
            .pane_output(&PaneId(s.active_pane))
            .map(|o| o.to_vec())
            .unwrap_or_default()
    }

    /// 测试用：当前激活 pane 的 VTE 可见文本（比核心缓冲更能发现黑屏）。
    pub fn test_active_pane_vte_text(&self) -> String {
        let s = self._state.borrow();
        s.active_layout()
            .pane(s.active_pane)
            .map(|v| v.visible_text())
            .unwrap_or_default()
    }

    /// 测试用：把 pane 的 VTE 滚动到顶部（mock-codex 末帧头在 row 1，
    /// 视口默认在底部时 text_format 只返回可见区，看不到 TOKEN_HEADER）。
    pub fn test_scroll_pane_to_top(&self, pane_id: u32) {
        let s = self._state.borrow();
        if let Some(view) = s.active_layout().pane(pane_id) {
            if let Some(adj) = view.terminal().vadjustment() {
                adj.set_value(adj.lower());
            }
        }
    }

    /// 测试用：指定 pane 的 VTE 文本。
    pub fn test_pane_vte_text(&self, pane_id: u32) -> String {
        let s = self._state.borrow();
        let t = s
            .active_layout()
            .pane(pane_id)
            .map(|v| v.visible_text())
            .unwrap_or_default();
        t
    }

    /// 测试用：指定 pane 的 VTE scrollback + 当前屏完整文本。
    pub fn test_pane_vte_buffer_text(&self, pane_id: u32) -> String {
        let s = self._state.borrow();
        s.active_layout()
            .pane(pane_id)
            .map(|view| view.buffer_text())
            .unwrap_or_default()
    }

    /// 测试用：指定 pane 的 VTE **当前屏幕**文本（不含 scrollback）。
    ///
    /// Ctrl-L 后旧内容可能留在 VTE scrollback；“当前屏不可见 BEFORE”
    /// 断言必须只看屏幕，不能把 scrollback 算进去。
    pub fn test_pane_screen_text(&self, pane_id: u32) -> String {
        let s = self._state.borrow();
        s.active_layout()
            .pane(pane_id)
            .map(|v| v.screen_text())
            .unwrap_or_default()
    }

    /// 测试用：指定 pane 的 VTE 光标行（0 起；最后一行 = rows-1）。
    pub fn test_pane_cursor_row(&self, pane_id: u32) -> i64 {
        let s = self._state.borrow();
        s.active_layout()
            .pane(pane_id)
            .map(|v| v.cursor_row())
            .unwrap_or(-1)
    }

    /// 测试用：指定 pane 的 VTE 屏幕行数。
    pub fn test_pane_screen_rows(&self, pane_id: u32) -> i64 {
        let s = self._state.borrow();
        s.active_layout()
            .pane(pane_id)
            .map(|v| v.screen_rows())
            .unwrap_or(0)
    }

    /// 测试用：当前 tab 布局 leaf pane id。
    pub fn test_layout_leaf_ids(&self) -> Vec<u32> {
        let s = self._state.borrow();
        let ids = s
            .active_workspace()
            .state()
            .layout(&TabId(s.active_tab))
            .map(|l| l.tree.leaves().into_iter().map(|p| p.0).collect())
            .unwrap_or_default();
        ids
    }

    /// 测试用：按先序返回当前 GTK 布局里每个 GtkPaned 的真实方向。
    ///
    /// 不能只断言 core LayoutNode；这里要保证 Herdr 的上下分割最终确实
    /// 变成 GTK Vertical，而不是在 platform 边界再次被翻成左右分割。
    pub fn test_gtk_paned_orientations(&self) -> Vec<gtk4::Orientation> {
        fn collect(widget: &gtk4::Widget, out: &mut Vec<gtk4::Orientation>) {
            let Ok(paned) = widget.clone().downcast::<gtk4::Paned>() else {
                return;
            };
            out.push(paned.orientation());
            if let Some(child) = paned.start_child() {
                collect(&child, out);
            }
            if let Some(child) = paned.end_child() {
                collect(&child, out);
            }
        }

        let s = self._state.borrow();
        let mut orientations = Vec::new();
        if let Some(root) = s.active_layout().active_root_widget() {
            collect(&root, &mut orientations);
        }
        orientations
    }

    /// 测试用：真实 GTK 子树签名。`H(L,V(L,L))` 表示左侧单 pane，右侧
    /// 再上下分割；可抓住把 Herdr Vertical 错画成 Horizontal 的回归。
    pub fn test_gtk_layout_signature(&self) -> String {
        fn signature(widget: &gtk4::Widget) -> String {
            let Ok(paned) = widget.clone().downcast::<gtk4::Paned>() else {
                return "L".to_string();
            };
            let direction = match paned.orientation() {
                gtk4::Orientation::Horizontal => "H",
                gtk4::Orientation::Vertical => "V",
                _ => "?",
            };
            let start = paned
                .start_child()
                .map(|child| signature(&child))
                .unwrap_or_else(|| "_".to_string());
            let end = paned
                .end_child()
                .map(|child| signature(&child))
                .unwrap_or_else(|| "_".to_string());
            format!("{direction}({start},{end})")
        }

        let s = self._state.borrow();
        s.active_layout()
            .active_root_widget()
            .map(|root| signature(&root))
            .unwrap_or_default()
    }

    /// 测试用：pane 控件分配尺寸（0×0 = 白屏）。
    pub fn test_pane_allocation(&self, pane_id: u32) -> (i32, i32) {
        let s = self._state.borrow();
        s.active_layout()
            .pane(pane_id)
            .map(|v| {
                let w = v.widget();
                (w.width(), w.height())
            })
            .unwrap_or((0, 0))
    }

    /// 测试用：flush 全部 VTE 合并缓冲后再读文本。
    pub fn test_flush_feeds(&self) {
        self._state.borrow().active_layout().flush_all_feeds();
    }

    /// 测试用：轮询一次并返回本批 `PaneOutput` 条数（1820 CPU）。
    pub fn test_poll_output_event_count(&self) -> usize {
        drain_pending_connects(&self._state);
        drain_pending_worktree_creates(&self._state);
        drain_ssh_probes(&self._state);
        drain_existing_ssh(&self._state);
        drain_local_existing(&self._state);
        drain_pending_reconnects(&self._state);
        maybe_schedule_reconnect(&self._state);
        let (n, pending_close) = {
            let mut s = self._state.borrow_mut();
            let events = s.active_workspace_mut().refresh();
            let n = events
                .iter()
                .filter(|e| {
                    matches!(
                        e,
                        StateChange::PaneOutput { .. } | StateChange::PaneFrame { .. }
                    )
                })
                .count();
            dispatch_event_batch(&mut s, events);
            drain_attention_notifications(&mut s);
            sync_pane_outputs(&mut s);
            maybe_refresh_status(&mut s, true);
            refresh_connection_summary(&mut s);
            update_command_marks(&s);
            update_jump_latest(&s);
            refresh_attention_chrome(&s, &self.window);
            let close = s.pending_close;
            if close {
                s.pending_close = false;
            }
            (n, close)
        };
        if pending_close {
            self._state.borrow_mut().quit_requested = true;
            self.window.close();
        }
        n
    }

    /// 测试用：当前激活 pane 的 VTE reset 次数（F1 Surface 契约）。
    pub fn test_active_pane_resets(&self) -> u32 {
        let s = self._state.borrow();
        s.active_layout()
            .pane(s.active_pane)
            .map(|v| v.render_trace().resets)
            .unwrap_or(0)
    }

    /// 测试用：当前激活 pane 是否已经完成首屏 Surface seed。
    ///
    /// 新 tab/远端 SSH 的 pane topology 可能先收敛，随后才在 GTK
    /// allocation 后播种快照；输入契约必须从 seed 完成后开始计数，不能
    /// 把正常的首屏 reset 误报成命令期间的 reset。
    pub fn test_active_pane_seeded(&self) -> bool {
        let s = self._state.borrow();
        s.active_layout()
            .pane(s.active_pane)
            .is_some_and(|view| view.is_seeded())
    }

    /// 测试用：指定 pane 的渲染痕迹（seeds/feeds/bytes）。
    pub fn test_pane_render_trace(&self, pane_id: u32) -> (u32, u32, usize) {
        let s = self._state.borrow();
        s.active_layout()
            .pane(pane_id)
            .map(|v| {
                let t = v.render_trace();
                (t.seeds, t.feeds, t.bytes_fed)
            })
            .unwrap_or((0, 0, 0))
    }

    /// 测试用：清空当前激活 pane 的渲染痕迹（切 tab 前归零）。
    pub fn test_clear_active_pane_render_trace(&self) {
        let s = self._state.borrow();
        if let Some(v) = s.active_layout().pane(s.active_pane) {
            v.clear_render_trace();
        }
    }

    /// 测试用：tab / 当前 tab 的 pane 数量。
    pub fn test_tab_and_pane_counts(&self) -> (usize, usize) {
        let s = self._state.borrow();
        let state = s.active_workspace().state();
        let n_tabs = state.tabs().len();
        let n_panes = state.panes(&TabId(s.active_tab)).len();
        (n_tabs, n_panes)
    }

    /// 测试用：状态栏文案。
    pub fn test_status_text(&self) -> String {
        self._state.borrow().status.plain_text()
    }

    /// 测试用：手动轮询一次核心事件并刷新输出（不等待 16ms 定时器）。
    pub fn test_poll_once(&self) {
        drain_pending_connects(&self._state);
        drain_pending_worktree_creates(&self._state);
        drain_ssh_probes(&self._state);
        drain_existing_ssh(&self._state);
        drain_local_existing(&self._state);
        drain_pending_reconnects(&self._state);
        maybe_schedule_reconnect(&self._state);
        let pending_close = {
            let mut s = self._state.borrow_mut();
            let events = s.active_workspace_mut().refresh();
            dispatch_event_batch(&mut s, events);
            drain_attention_notifications(&mut s);
            sync_pane_outputs(&mut s);
            sync_window_size(&mut s);
            drain_surface_input(&mut s);
            maybe_refresh_status(&mut s, true);
            refresh_connection_summary(&mut s);
            update_command_marks(&s);
            update_jump_latest(&s);
            refresh_attention_chrome(&s, &self.window);
            let close = s.pending_close;
            if close {
                s.pending_close = false;
            }
            close
        };
        if pending_close {
            self._state.borrow_mut().quit_requested = true;
            self.window.close();
        }
    }

    /// W21 测试钩子：最近一次经 PaneView input_cb 的原始输入。
    pub fn test_last_raw_input(&self) -> Vec<u8> {
        self._state.borrow().last_raw_input.clone()
    }

    /// W21 测试钩子：指定 pane 的 reply_state 是否在 alt-screen。
    pub fn test_pane_alternate_screen(&self, pane: u32) -> bool {
        self._state
            .borrow()
            .active_layout()
            .pane(pane)
            .map(|v| v.test_alternate_screen())
            .unwrap_or(false)
    }

    /// W19e 测试钩子：注入一次 fault（report + 弹窗），进程必须继续。
    pub fn test_inject_fault(&self, token: &str) {
        crate::platform::linux::fault_gtk::inject_fault(token);
    }

    /// 测试用：主窗口本身（供 widget 树断言）。
    pub fn test_window(&self) -> gtk4::Window {
        self.window.clone()
    }

    /// 测试用：QuickConnect 面板是否打开。
    pub fn test_panel_open(&self) -> bool {
        self._state.borrow().panel_open.is_some()
    }

    /// 测试用：当前面板 tab（0=workspaces / 1=attention / 2=search）。
    pub fn test_active_panel_tab(&self) -> u32 {
        self._state
            .borrow()
            .panel_open
            .map(|t| t as u32)
            .unwrap_or(0)
    }

    /// 测试用：全部 tab id（core 顺序）。
    pub fn test_tab_ids(&self) -> Vec<u32> {
        self._state
            .borrow()
            .active_workspace()
            .state()
            .tabs()
            .iter()
            .map(|t| t.id.0)
            .collect()
    }

    /// 测试用：全部 tab 名（core 顺序；W7 new_tab_shortcut 断言非空/raw label）。
    pub fn test_tab_names(&self) -> Vec<String> {
        self._state
            .borrow()
            .active_workspace()
            .state()
            .tabs()
            .iter()
            .map(|t| t.name.clone())
            .collect()
    }

    /// 测试用：指定 replica 的 herdr 运行时 stream 探针（takeover_watchdog 用）。
    /// 返回 (stream_starts, control_takeover_starts, takeover_suppressed, actual_mode)。
    pub fn test_herdr_probe(&self, replica: &str, pane: u32) -> Option<(u64, u64, bool, String)> {
        let s = self._state.borrow();
        let ws = s
            .pool
            .list()
            .into_iter()
            .find(|w| w.id().replica_id() == replica)?;
        let rt = ws.runtime().as_any().downcast_ref::<HerdrRuntime>()?;
        let pane = PaneId(pane);
        Some((
            rt.test_stream_starts(pane),
            rt.test_control_takeover_starts(pane),
            rt.test_takeover_suppressed(pane),
            format!("{:?}", rt.test_actual_mode(pane)),
        ))
    }

    /// 测试用：当前激活 tab id。
    pub fn test_active_tab_id(&self) -> u32 {
        self._state.borrow().active_tab
    }

    /// 测试用：已有的连接探测线程是否已收完（local-first 流式结束后 idle）。
    pub fn test_existing_probe_idle(&self) -> bool {
        let s = self._state.borrow();
        s.pending_local_probe.is_empty() && !s.existing.borrow().probe_inflight
    }

    /// 测试用：以指定 tab 打开面板（0=workspaces / 1=attention / 2=search）。
    pub fn test_open_panel(&self, tab: u32) {
        let state = self._state.clone();
        let tab = match tab {
            0 => PanelTab::Workspaces,
            1 => PanelTab::Attention,
            _ => PanelTab::Search,
        };
        open_panel(&state, &self.window, tab);
    }

    /// 测试用：当前 blocked 工作区数（红点 N）。
    pub fn test_attention_blocked_workspaces(&self) -> usize {
        self._state.borrow().attention.blocked_workspace_count()
    }

    /// 测试用：窗口标题（M3.4 接红点前缀，当前返回原始标题）。
    pub fn test_window_title(&self) -> String {
        self.window
            .title()
            .map(|t| t.to_string())
            .unwrap_or_default()
    }

    /// 测试用：工作区 PaneBuf 中某 pane 的最近 n 行。
    pub fn test_replica_last_n(&self, pane_id: u32, n: usize) -> Vec<String> {
        let s = self._state.borrow();
        s.active_workspace().pane_last_n_lines(PaneId(pane_id), n)
    }

    /// 测试用：绕过 tmux 直接向工作区 PaneBuf/AttentionEngine 注入字节。
    pub fn test_feed_replica(&self, pane_id: u32, bytes: &[u8]) {
        let mut s = self._state.borrow_mut();
        let ws = active_workspace_id(&s);
        let wid = s.active_ws_id().clone();
        s.active_workspace_mut()
            .feed_pane_bytes(PaneId(pane_id), bytes, 80, 24);
        apply_attention_from_workspace(&mut s, &wid, &ws, pane_id);
    }

    /// 测试用：本轮进入 blocked 的 workspace 通知记录。
    pub fn test_notifications_recorded(&self) -> Vec<String> {
        self._state.borrow().notification_log.clone()
    }

    /// 测试用：所有工作区 Done pane 数之和（任务完成，不是 blocked）。
    pub fn test_attention_done_count(&self) -> usize {
        self._state
            .borrow()
            .attention
            .snapshot()
            .iter()
            .map(|w| w.done)
            .sum()
    }

    /// 测试用：当前激活 pane id。
    pub fn test_active_pane_id(&self) -> u32 {
        self._state.borrow().active_pane
    }

    /// 测试用：SwitchPane（后台完成通知必须打在非前台 pane）。
    pub fn test_switch_pane(&self, pane_id: u32) {
        let _ = self
            ._state
            .borrow_mut()
            .active_workspace_mut()
            .execute(Task::SwitchPane {
                target: PaneId(pane_id),
            });
    }

    /// 测试用：生产搜索路径 `WorkspacePool::search_all`（不是 Mock PaneBuf）。
    pub fn test_search_all(&self, query: &str) -> Vec<(String, u32, String)> {
        self._state
            .borrow()
            .pool
            .search_all(query)
            .into_iter()
            .map(|h| (h.workspace_id, h.pane_id.0, h.line))
            .collect()
    }

    /// 测试用：连接一个 QuickConnect 目标（走生产 connect_target 路径）。
    pub fn test_connect_target(&self, config: TargetConfig) {
        connect_target(&self._state.clone(), config);
    }

    /// 测试用：后台打开任意 `WorkspaceSpec`（SSH loopback 必须带远端 `-L`）。
    ///
    /// 等连接完成并激活后再返回：测试随后 `wait_ready` / 取 leaf 时看到的是
    /// 新工作区，而不是启动时的本地 shell（W18b 的 pane id 才不会串）。
    pub fn test_open_spec(&self, spec: WorkspaceSpec) {
        let id = spec.id();
        let socket = spec.socket.clone();
        let config = if spec.transport == "ssh" {
            TargetConfig::tmux_session(
                spec.session.clone(),
                TargetTransport::Ssh {
                    name: spec.alias.clone().unwrap_or_default(),
                },
            )
        } else {
            TargetConfig::tmux_session(spec.session.clone(), TargetTransport::Local)
        };
        spawn_background_connect(
            &self._state.clone(),
            spec,
            id.clone(),
            socket,
            ProjectConnectFlow::new_with_intent(&config, ProjectConnectIntent::AttachOnly),
            config,
            true,
        );
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            self.test_poll_once();
            while glib::MainContext::default().iteration(false) {}
            let (active, connecting) = {
                let s = self._state.borrow();
                (s.pool.active_id().cloned(), !s.pending_connects.is_empty())
            };
            if active.as_ref() == Some(&id) && !connecting {
                return;
            }
        }
    }

    /// 测试用：当前池里各工作区 replica id（`name@transport`）。
    pub fn test_workspace_replica_ids(&self) -> Vec<String> {
        self._state
            .borrow()
            .pool
            .list()
            .into_iter()
            .map(|w| w.id().replica_id())
            .collect()
    }

    /// 测试用：池里各工作区的 runtime 种类（断言没误开成本地 tmux）。
    pub fn test_workspace_runtimes(&self) -> Vec<String> {
        self._state
            .borrow()
            .pool
            .list()
            .into_iter()
            .map(|w| w.id().runtime.clone())
            .collect()
    }

    /// 测试用：按 replica id 激活工作区（上次看到这里 / 跨工作区搜索）。
    pub fn test_activate_workspace(&self, replica: &str) {
        let mut s = self._state.borrow_mut();
        activate_attention_workspace(&mut s, replica);
    }

    /// 测试用：只搜当前工作区当前 pane。
    pub fn test_search_pane(&self, pane: u32, query: &str) -> Vec<(String, u32, String)> {
        self._state
            .borrow()
            .active_workspace()
            .search_pane(PaneId(pane), query)
            .into_iter()
            .map(|h| (h.workspace_id, h.pane_id.0, h.line))
            .collect()
    }

    /// 测试用：只搜当前工作区全部 pane。
    pub fn test_search_workspace(&self, query: &str) -> Vec<(String, u32, String)> {
        self._state
            .borrow()
            .active_workspace()
            .search_workspace(query)
            .into_iter()
            .map(|h| (h.workspace_id, h.pane_id.0, h.line))
            .collect()
    }

    /// 测试用：打开当前 pane 内查找条（与 Ctrl+F 同一条生产路径）。
    pub fn test_open_pane_find(&self) {
        open_pane_find(&self._state.clone(), &self.window);
    }

    /// 测试用：Attention 小 VTE 按键（必须走 peek `connect_input`，不要直接 WriteRaw）。
    pub fn test_peek_emit_input(&self, data: &[u8]) {
        crate::platform::linux::quickconnect_panel::test_emit_peek_input(data);
    }
}

fn handle_action(s: &mut UiState, action: Action, window: &Window, state: &Rc<RefCell<UiState>>) {
    match action {
        Action::NewTab | Action::NewWindow => {
            let _ = s.active_workspace_mut().execute(Task::NewTab {
                name: None,
                command: None,
                workdir: None,
            });
            // Accepted 不得手工 refresh：等 16ms 批里 LayoutChanged/MutationSettled。
            return;
        }
        Action::NewPane => {
            let pane = s.active_pane;
            let _ = s.active_workspace_mut().execute(Task::SplitPane {
                target: Some(PaneId(pane)),
                dir: SplitDir::Horizontal,
                command: None,
                workdir: None,
            });
            return;
        }
        Action::NewPaneVertical => {
            let pane = s.active_pane;
            let _ = s.active_workspace_mut().execute(Task::SplitPane {
                target: Some(PaneId(pane)),
                dir: SplitDir::Vertical,
                command: None,
                workdir: None,
            });
            return;
        }
        Action::SwitchTab1 => switch_tab_n(s, 1),
        Action::SwitchTab2 => switch_tab_n(s, 2),
        Action::SwitchTab3 => switch_tab_n(s, 3),
        Action::SwitchTab4 => switch_tab_n(s, 4),
        Action::SwitchTab5 => switch_tab_n(s, 5),
        Action::SwitchTab6 => switch_tab_n(s, 6),
        Action::SwitchTab7 => switch_tab_n(s, 7),
        Action::SwitchTab8 => switch_tab_n(s, 8),
        Action::SwitchTab9 => switch_tab_n(s, 9),
        Action::SwitchTabLast => {
            let tabs = s.active_workspace().state().tabs();
            if let Some(t) = tabs.last() {
                request_switch_tab(s, t.id.0);
            }
        }
        Action::SwitchPaneNext | Action::SwitchPanePrev => {
            switch_pane_offset(s, matches!(action, Action::SwitchPaneNext));
        }
        Action::CommandPalette => {
            open_command_palette(s, window, state);
            return;
        }
        Action::Search | Action::Unknown => {}
        Action::QuickConnect => {
            // 调用方（快捷键/命令面板）必须先释放 RefMut 再打开面板；
            // 这里只做标记，由 handle_action 的调用方处理。
            let _ = (s, window, state);
            return;
        }
        Action::Quit => {
            window.close();
            return;
        }
        Action::Copy => {
            copy_active_pane(s);
            return;
        }
        Action::Paste => {
            paste_active_pane(s, state);
            return;
        }
        Action::IncreaseFontSize => adjust_font(s, 1),
        Action::DecreaseFontSize => adjust_font(s, -1),
        Action::ResetFontSize => reset_font(s),
        Action::TogglePaneFullscreen => toggle_fullscreen(s),
    }
    refresh_ui(s);
}

fn open_command_palette(s: &UiState, window: &Window, state: &Rc<RefCell<UiState>>) {
    let parent = window.clone();
    let callback_parent = parent.clone();
    let callback_window = parent.clone();
    let callback_state = state.clone();
    let uses_tmux = s.uses_tmux();
    let next_theme = if s.theme_name.eq_ignore_ascii_case("dark") {
        "Light"
    } else {
        "Dark"
    };
    let next_status_mode = match s.status_mode {
        StatusBarMode::Tmux => StatusBarMode::Theme.as_str(),
        StatusBarMode::Theme => StatusBarMode::Tmux.as_str(),
    };
    crate::platform::linux::command_palette::show_for_runtime(
        &parent,
        uses_tmux,
        next_theme,
        next_status_mode,
        move |id| {
            run_palette_command(&callback_state, &callback_window, &callback_parent, id);
        },
    );
}

fn run_palette_command(state: &Rc<RefCell<UiState>>, window: &Window, parent: &Window, id: &str) {
    let Some(action) = parse_palette_action(id) else {
        tracing::warn!(target = "muxterm::linux", "未知命令面板动作: {id}");
        return;
    };
    match action {
        PaletteAction::Language => {
            let language_parent = parent.clone();
            let callback_state = state.clone();
            crate::platform::linux::command_palette::show_language(&language_parent, move |_| {
                let mut s = callback_state.borrow_mut();
                maybe_refresh_status(&mut s, true);
            });
        }
        PaletteAction::TmuxDetach => {
            // 必须先放下 RefMut 再 close：close-request 会再借同一把 UiState。
            let should_quit = {
                let mut s = state.borrow_mut();
                matches!(
                    s.active_workspace_mut().execute(Task::Detach),
                    Ok(crate::core::model::task::TaskOutcome::Done)
                )
            };
            if should_quit {
                request_quit_close(state, window);
            }
        }
        PaletteAction::SshDisconnect => {
            let mut s = state.borrow_mut();
            if s.uses_tmux() {
                let _ = s.active_workspace_mut().execute(Task::Detach);
            }
        }
        PaletteAction::QuickConnect => {
            open_quick_connect(state, window);
        }
        PaletteAction::ToggleTheme => {
            let mut s = state.borrow_mut();
            toggle_theme(&mut s);
        }
        PaletteAction::ToggleStatusBarMode => {
            let mut s = state.borrow_mut();
            toggle_status_mode(&mut s);
        }
        PaletteAction::TogglePaneFullscreen => {
            let mut s = state.borrow_mut();
            toggle_fullscreen(&mut s);
            refresh_ui(&mut s);
        }
        PaletteAction::IncreaseFontSize => {
            let mut s = state.borrow_mut();
            adjust_font(&mut s, 1);
        }
        PaletteAction::DecreaseFontSize => {
            let mut s = state.borrow_mut();
            adjust_font(&mut s, -1);
        }
        PaletteAction::ResetFontSize => {
            let mut s = state.borrow_mut();
            reset_font(&mut s);
        }
        PaletteAction::Quit => {
            request_quit_close(state, window);
        }
        PaletteAction::NewTab => {
            let mut s = state.borrow_mut();
            let _ = s.active_workspace_mut().execute(Task::NewTab {
                name: None,
                command: None,
                workdir: None,
            });
            // Accepted 不得手工 refresh：等 LayoutChanged/MutationSettled。
        }
        PaletteAction::NewPane => {
            let mut s = state.borrow_mut();
            let pane = s.active_pane;
            let _ = s.active_workspace_mut().execute(Task::SplitPane {
                target: Some(PaneId(pane)),
                dir: SplitDir::Horizontal,
                command: None,
                workdir: None,
            });
        }
        PaletteAction::NewPaneVertical => {
            let mut s = state.borrow_mut();
            let pane = s.active_pane;
            let _ = s.active_workspace_mut().execute(Task::SplitPane {
                target: Some(PaneId(pane)),
                dir: SplitDir::Vertical,
                command: None,
                workdir: None,
            });
        }
        PaletteAction::ClosePane => {
            let mut s = state.borrow_mut();
            let pane = s.active_pane;
            let _ = s.active_workspace_mut().execute(Task::ClosePane {
                target: PaneId(pane),
            });
            refresh_ui(&mut s);
        }
        PaletteAction::CloseTab => {
            let mut s = state.borrow_mut();
            let tab = s.active_tab;
            let _ = s
                .active_workspace_mut()
                .execute(Task::CloseTab { target: TabId(tab) });
            refresh_ui(&mut s);
        }
        PaletteAction::CloseWindow => window.close(),
        PaletteAction::SwitchPaneNext => {
            let mut s = state.borrow_mut();
            switch_pane_offset(&mut s, true);
            refresh_ui(&mut s);
        }
        PaletteAction::SwitchPanePrev => {
            let mut s = state.borrow_mut();
            switch_pane_offset(&mut s, false);
            refresh_ui(&mut s);
        }
        PaletteAction::SwitchTab(n) => {
            let mut s = state.borrow_mut();
            switch_tab_n(&mut s, n);
        }
        PaletteAction::TmuxAttach => open_tmux_attach(state, parent, false),
        PaletteAction::TmuxNew => open_tmux_attach(state, parent, true),
        PaletteAction::SshConnect => open_ssh_connect(state, parent),
        PaletteAction::SearchPanes => {
            open_panel(state, parent, PanelTab::Search);
        }
        PaletteAction::RenamePane => {
            tracing::info!(target = "muxterm::linux", "命令 {id} 尚未接到 GTK 对话框");
        }
        PaletteAction::Preferences => {
            open_preferences(state, parent);
        }
        PaletteAction::ReloadConfig | PaletteAction::OpenConfig => {
            tracing::info!(target = "muxterm::linux", "命令 {id} 尚未接到 GTK 对话框");
        }
    }
}

fn toggle_fullscreen(s: &mut UiState) {
    let pane = s.active_pane;
    if s.uses_tmux() {
        let _ = s
            .active_workspace_mut()
            .execute(Task::TogglePaneFullscreen {
                target: PaneId(pane),
            });
    } else {
        let next = match s.active_layout().fullscreen_pane() {
            Some(id) if id == pane => None,
            _ => Some(pane),
        };
        s.active_layout_mut().set_fullscreen_pane(next);
    }
}

/// 把 dotted key 写回 config.toml（唯一事实源；不再写 preferences.toml）。
pub fn persist_config(dotted: &str, value: toml_edit::Item) {
    let Some(path) = Config::user_config_path() else {
        return;
    };
    let raw = std::fs::read_to_string(&path).unwrap_or_default();
    if let Ok(out) = set_dotted_key(&raw, dotted, value) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&path, out);
    }
}

/// C8：字号写盘防抖（300ms），避免 Ctrl+= 热路径同步写 config.toml。
/// 用 generation 作废旧回调，不 remove 已触发的 SourceId（glib 会 panic）。
fn schedule_font_persist(size: f32) {
    use std::cell::Cell;
    thread_local! {
        static FONT_PERSIST_GEN: Cell<u64> = const { Cell::new(0) };
    }
    FONT_PERSIST_GEN.with(|gen| {
        let my_gen = gen.get().wrapping_add(1);
        gen.set(my_gen);
        glib::timeout_add_local(std::time::Duration::from_millis(300), move || {
            let current = FONT_PERSIST_GEN.with(|g| g.get());
            if current == my_gen {
                persist_config("font.size", toml_edit::value(f64::from(size)));
            }
            glib::ControlFlow::Break
        });
    });
}

fn adjust_font(s: &mut UiState, direction: i32) {
    let next = FontSettings::zoomed(s.font.size, direction);
    if (next - s.font.size).abs() < f32::EPSILON {
        return;
    }
    s.font.size = next;
    // C8：热路径只改当前前台 LayoutHost，立刻返回；后台 cache 在 activate
    // 时按尺寸差补。写盘防抖 300ms，不阻塞按键。
    s.active_layout_mut().set_font_size(next);
    schedule_font_persist(next);
}

fn reset_font(s: &mut UiState) {
    s.font.size = s.config_font_size;
    let font = s.font.clone();
    for layout in s.pixel_cache.values_mut() {
        layout.set_font(&font);
    }
    persist_config("font.size", toml_edit::value(f64::from(s.config_font_size)));
}

fn toggle_theme(s: &mut UiState) {
    let next_name = Theme::toggle_target(&s.theme_name);
    let Ok(theme) = Theme::load(next_name) else {
        tracing::error!(
            target = "muxterm::linux",
            "加载主题 {next_name} 失败，保持当前主题"
        );
        return;
    };
    s.theme_name = next_name.to_string();
    s.theme = theme.clone();
    for layout in s.pixel_cache.values_mut() {
        layout.apply_theme(&theme);
    }
    s.status.apply_theme(&theme);
    apply_chrome_css(&theme);
    persist_config("theme.name", toml_edit::value(next_name.to_string()));
    report_all_pane_colours(s);
}

fn toggle_status_mode(s: &mut UiState) {
    let next = match s.status_mode {
        StatusBarMode::Tmux => StatusBarMode::Theme,
        StatusBarMode::Theme => StatusBarMode::Tmux,
    };
    s.status_mode = next;
    s.status.set_mode(next);
    persist_config(
        "statusbar.mode",
        toml_edit::value(next.as_str().to_string()),
    );
    maybe_refresh_status(s, true);
}

fn report_all_pane_colours(s: &mut UiState) {
    if !s.uses_tmux() {
        return;
    }
    let fg = s.theme.foreground;
    let bg = s.theme.background;
    let panes: Vec<PaneId> = s
        .active_workspace()
        .state()
        .tabs()
        .iter()
        .flat_map(|t| s.active_workspace().state().panes(&t.id))
        .map(|p| p.id)
        .collect();
    for pane in panes {
        let _ = s.active_workspace_mut().execute(Task::ReportPaneColours {
            target: pane,
            fg,
            bg,
        });
    }
}

fn copy_active_pane(s: &UiState) {
    if let Some(view) = s.active_layout().pane(s.active_pane) {
        view.copy_clipboard();
    }
}

fn paste_active_pane(s: &UiState, state: &Rc<RefCell<UiState>>) {
    let Some(view) = s.active_layout().pane(s.active_pane).cloned() else {
        return;
    };
    let pane_id = view.pane_id();
    let bracketed = view.bracketed_paste();
    let st = Rc::downgrade(state);
    let clipboard = view.widget().clipboard();
    clipboard.read_text_async(gtk4::gio::Cancellable::NONE, move |result| {
        let Ok(Some(text)) = result else {
            return;
        };
        let text =
            crate::core::protocol::terminal::mirror::sanitize_paste(text.as_str(), bracketed);
        let data =
            crate::core::protocol::terminal::mirror::encode_clipboard_paste(&text, bracketed);
        if data.is_empty() {
            return;
        }
        let Some(st) = st.upgrade() else {
            return;
        };
        let mut s = st.borrow_mut();
        let _ = s.active_workspace_mut().execute(Task::WriteRaw {
            target: PaneId(pane_id),
            data,
        });
    });
}

fn switch_tab_n(s: &mut UiState, n: usize) {
    let tabs = s.active_workspace().state().tabs();
    if let Some(t) = tabs.get(n.saturating_sub(1)) {
        request_switch_tab(s, t.id.0);
    }
}

fn request_switch_tab(s: &mut UiState, tab_id: u32) {
    if tab_id == s.active_tab {
        return;
    }
    s.tab_gate.request(tab_id);
    let _ = s.active_workspace_mut().execute(Task::SwitchTab {
        target: TabId(tab_id),
    });
}

/// 与 macOS `movePane` 对齐：用当前 tab 快照算目标，发 SwitchPane。
/// 不要发 NextPane——tmux 布局树若没解析完会落到无效的
/// `select-pane -t @N -N/-P`（2219.log 14:41:29）。
fn switch_pane_offset(s: &mut UiState, forward: bool) {
    let panes = s.active_workspace().state().panes(&TabId(s.active_tab));
    let ids: Vec<u32> = panes.iter().map(|p| p.id.0).collect();
    let active = panes
        .iter()
        .find(|p| p.active)
        .map(|p| p.id.0)
        .unwrap_or(s.active_pane);
    if let Some(target) = cycle_pane_id(&ids, active, forward) {
        let _ = s.active_workspace_mut().execute(Task::SwitchPane {
            target: PaneId(target),
        });
    }
}

/// 把工作区 PaneBuf 的注意力信号应用到引擎（前台/后台共用）。
///
/// PaneBuf 已在 `Workspace::refresh` 里喂好；这里只取信号，不再维护
/// GUI 侧副本（W6：PaneBuf 收进 core Workspace）。
fn apply_attention_from_workspace(s: &mut UiState, wid: &WorkspaceId, ws: &str, pane: u32) {
    let Some(workspace) = s.pool.get_mut(wid) else {
        return;
    };
    let signals = workspace.take_attention_signals(PaneId(pane));
    let (last_line, seq) = workspace.pane_last_line_seq(PaneId(pane));
    s.attention.apply(ws, pane, &signals, &last_line, seq);
    // 前台 pane 的输出视为已看见：CommandDone 清成 Idle，前台 `ls` 不进 attention。
    if pane == s.active_pane && s.pool.active_id() == Some(wid) {
        s.attention.on_became_visible(ws, pane);
    }
}

/// 刷新状态栏红点与窗口标题（blocked 工作区数）。
fn refresh_attention_chrome(s: &UiState, window: &Window) {
    let n = s.attention.blocked_workspace_count();
    s.status.set_attention(n);
    let workspace = s
        .pool
        .active()
        .map(|w| w.name().to_string())
        .unwrap_or_else(|| "muxterm".into());
    window.set_title(Some(&window_title(n, &workspace)));
}

/// 回底按钮可见性：VTE 滚离底部时显示，回到尾部隐藏（W16a）。
/// 把当前 pane 滚到包含指定文本的行（命令刻度 / 上次看到这里共用）。
fn scroll_to_command_text(state: &Rc<RefCell<UiState>>, text: &Rc<RefCell<Option<String>>>) {
    let s = state.borrow();
    let Some(text) = text.borrow().clone() else {
        return;
    };
    let pane = s.active_pane;
    let lines = s.active_workspace().pane_last_n_lines(PaneId(pane), 10_000);
    if let Some(row) = lines.iter().position(|l| l.contains(&text)) {
        if let Some(view) = s.active_layout().pane(pane).cloned() {
            if let Some(adj) = view.terminal().vadjustment() {
                adj.set_value(adj.lower() + row as f64);
            }
        }
    }
}

/// 从当前 pane 的 OSC 133 刻度刷新红/绿标记（W18h）。
fn update_command_marks(s: &UiState) {
    let marks = s
        .active_workspace()
        .pane_command_marks(PaneId(s.active_pane));
    let ok = marks.iter().rev().find(|m| m.exit_code == Some(0));
    let fail = marks
        .iter()
        .rev()
        .find(|m| m.exit_code.is_some_and(|c| c != 0));
    if let Some(m) = ok {
        s.cmd_mark_ok.set_visible(true);
        s.cmd_mark_ok.set_tooltip_text(Some(&m.command));
        *s.cmd_mark_ok_text.borrow_mut() = Some(m.command.clone());
    } else {
        s.cmd_mark_ok.set_visible(false);
        *s.cmd_mark_ok_text.borrow_mut() = None;
    }
    if let Some(m) = fail {
        s.cmd_mark_fail.set_visible(true);
        s.cmd_mark_fail.set_tooltip_text(Some(&m.command));
        *s.cmd_mark_fail_text.borrow_mut() = Some(m.command.clone());
    } else {
        s.cmd_mark_fail.set_visible(false);
        *s.cmd_mark_fail_text.borrow_mut() = None;
    }
}

/// 当前激活 pane 的 VTE 是否在底部（scroll lock / 回底按钮共用）。
fn view_at_bottom(view: &std::rc::Rc<PaneView>) -> bool {
    view.terminal()
        .vadjustment()
        .map(|adj| {
            let page = adj.page_size();
            let upper = adj.upper();
            // VTE 内容不足一屏时 upper 可能等于 page_size，视为已在底部。
            upper - page <= adj.value() + 1.0
        })
        .unwrap_or(true)
}

fn update_jump_latest(s: &UiState) {
    let at_bottom = s
        .active_layout()
        .pane(s.active_pane)
        .map(view_at_bottom)
        .unwrap_or(true);
    s.jump_latest.set_visible(!at_bottom);
    if at_bottom {
        // 回到尾部：搜索高亮不再有意义（W17c）。
        s.search_highlight.set_visible(false);
    } else if s.jump_unseen > 0 {
        s.jump_latest.set_label(&format!("↓ +{}", s.jump_unseen));
    } else {
        s.jump_latest.set_label("↓");
    }
}

/// 把当前连接摘要刷到状态点 popover（C7.7）。
///
/// 速率由连续两次 `traffic_bytes()` 快照 + 墙钟差出来（W15a），
/// 禁止把累计字节标成 `B/s`。
fn refresh_connection_summary(s: &mut UiState) {
    let Some(ws) = s.pool.active() else {
        return;
    };
    let id = ws.id();
    let kind = match id.runtime.as_str() {
        "tmux-ssh" | "ssh" => "ssh",
        "tmux" => "tmux",
        _ => "local",
    };
    let host = id
        .alias
        .clone()
        .or_else(|| (!id.session.is_empty()).then(|| id.session.clone()));
    let status = match s.runtime_status {
        crate::core::protocol::ffi::types::BACKEND_STATUS_CONNECTED => "connected",
        crate::core::protocol::ffi::types::BACKEND_STATUS_CONNECTING => "connecting",
        _ => "disconnected",
    };
    let (down, up) = ws.runtime().traffic_bytes();
    let now = Instant::now();
    let (down_rate, up_rate) = match (s.last_traffic, s.last_traffic_at) {
        (Some((pdown, pup)), Some(at)) => {
            let dt = now.duration_since(at);
            (
                crate::core::format::rate_bps(pdown, down, dt),
                crate::core::format::rate_bps(pup, up, dt),
            )
        }
        _ => (0, 0),
    };
    s.last_traffic = Some((down, up));
    s.last_traffic_at = Some(now);
    s.status.set_connection_summary(&ConnectionSummary {
        kind: kind.into(),
        host,
        status: status.into(),
        down,
        up,
        down_rate,
        up_rate,
    });
}

/// 当前前台连接的 workspace id（ReplicaStore 键）。
fn active_workspace_id(s: &UiState) -> String {
    s.pool
        .active_id()
        .map(workspace_replica_id)
        .unwrap_or_default()
}

/// WorkspaceId → ReplicaStore 键（`name@transport`，与 QuickConnect 一致）。
fn workspace_replica_id(id: &WorkspaceId) -> String {
    id.replica_id()
}

/// 批处理顺序计划：结构 →（frame/snapshot/history）→ output。
///
/// 纯函数（L0 可测）：tmux/Herdr 可在同一轮把 resize、snapshot 和 live
/// output 一起送到 UI；按输入顺序直接喂会让 CUP/DECSTBM 仍按旧网格解释。
/// 返回三个阶段的索引序列（各阶段内部保持原始顺序）。
fn batch_order_plan(events: &[StateChange]) -> (Vec<usize>, Vec<usize>, Vec<usize>) {
    let has_structural = events.iter().any(|ev| {
        matches!(
            ev,
            StateChange::TabAdded { .. }
                | StateChange::TabClosed { .. }
                | StateChange::LayoutChanged { .. }
                | StateChange::PaneAdded { .. }
                | StateChange::PaneClosed { .. }
                | StateChange::PaneResized { .. }
        )
    });
    if !has_structural {
        // 无结构事件：保持原始顺序直接分发。
        return ((0..events.len()).collect(), Vec::new(), Vec::new());
    }
    let mut structure = Vec::new();
    let mut baseline = Vec::new();
    let mut output = Vec::new();
    for (i, ev) in events.iter().enumerate() {
        match ev {
            StateChange::PaneSnapshot { .. }
            | StateChange::PaneFrame { .. }
            | StateChange::PaneHistory { .. } => baseline.push(i),
            StateChange::PaneOutput { .. } => output.push(i),
            _ => structure.push(i),
        }
    }
    (structure, baseline, output)
}

/// Effects collected while applying one Core event batch.
///
/// Structural events update Core immediately, but GTK topology is committed
/// only after the final structural state is known.  This keeps tmux and Herdr
/// on the same event contract and prevents a frame/output from observing an
/// intermediate tree.
#[derive(Debug, Default)]
struct UiBatchEffects {
    topology_changed: bool,
}

impl UiBatchEffects {
    fn note_topology(&mut self) {
        self.topology_changed = true;
    }

    fn commit(self, s: &mut UiState, wid: &WorkspaceId) {
        if self.topology_changed {
            refresh_workspace_layout(s, wid);
        }
    }
}

fn dispatch_event_batch(s: &mut UiState, events: Vec<StateChange>) {
    s.snapshot_seeded_this_batch.clear();
    let wid = s.active_ws_id().clone();
    let mut effects = UiBatchEffects::default();
    let (structure, baseline, output) = batch_order_plan(&events);
    if !baseline.is_empty() || !output.is_empty() {
        for i in &structure {
            dispatch_event(s, &events[*i], &mut effects);
        }
        effects.commit(s, &wid);
        for i in &baseline {
            dispatch_event(s, &events[*i], &mut UiBatchEffects::default());
        }
        for i in &output {
            dispatch_event(s, &events[*i], &mut UiBatchEffects::default());
        }
    } else {
        for ev in &events {
            dispatch_event(s, ev, &mut effects);
        }
        effects.commit(s, &wid);
    }
    s.snapshot_seeded_this_batch.clear();
}

/// 按 `(WorkspaceId, PaneId)` 找常驻 PaneView（hidden tab / background
/// workspace 也必须有）；找不到说明 topology 阶段没建好，属于 lifecycle
/// failure，不能静默丢帧。
fn resident_pane_view(
    s: &UiState,
    wid: &WorkspaceId,
    pane: u32,
) -> Option<std::rc::Rc<crate::platform::linux::pane_view::PaneView>> {
    s.pixel_cache
        .get(wid)
        .and_then(|layout| layout.pane(pane).cloned())
}

/// workspace-aware 事件分发：`wid` 的目标 LayoutHost 在 pixel_cache 里。
/// 与 active-only 的 [`dispatch_event`] 共享结构/注意力逻辑，但 Surface
/// 字节永远按 `(WorkspaceId, PaneId)` 进对应 pixel cache，绝不进错窗口。
fn dispatch_event_for(
    s: &mut UiState,
    wid: &WorkspaceId,
    ev: &StateChange,
    effects: &mut UiBatchEffects,
) {
    let ws = wid.replica_id();
    apply_attention_event_from_workspace(s, wid, &ws, ev);
    let is_active = s.pool.active_id() == Some(wid);
    match ev {
        StateChange::PaneSnapshot { pane, data } => {
            let ws = wid.replica_id();
            apply_attention_from_workspace(s, wid, &ws, pane.0);
            if let Some(view) = resident_pane_view(s, wid, pane.0) {
                sync_pane_grid_size_for(s, wid, pane.0);
                let seeded_from_core = s.snapshot_seeded_this_batch.contains(&pane.0);
                if !seeded_from_core
                    && surface_allocation_is_seedable(
                        view.widget().is_realized(),
                        view.widget().width(),
                        view.widget().height(),
                    )
                {
                    let (cols, rows) = s
                        .pool
                        .get(wid)
                        .and_then(|w| w.state().pane(pane))
                        .map(|p| (p.cols, p.rows))
                        .unwrap_or((80, 24));
                    view.seed_snapshot(data, cols, rows);
                    forward_parser_replies_for(s, wid, pane.0);
                }
            }
        }
        StateChange::PaneHistory { pane, data } => {
            if let Some(view) = resident_pane_view(s, wid, pane.0) {
                sync_pane_grid_size_for(s, wid, pane.0);
                view.prepend_history(data);
            }
        }
        StateChange::PaneOutput { pane, data } | StateChange::PaneFrame { pane, data } => {
            if let Some(view) = resident_pane_view(s, wid, pane.0) {
                sync_pane_grid_size_for(s, wid, pane.0);
                // 未分配像素时仍入队（feed_* 不 flush），可 paint 后再补放。
                // 直接丢弃会让 Cursor 等候框等 live 重绘永远缺帧。
                match ev {
                    StateChange::PaneFrame { .. } => view.feed_full(data),
                    _ => view.feed_output(data),
                }
                view.flush_deferred_feed();
                view.flush_deferred_history();
                if is_active {
                    // W18e：离开底部期间的新行累计到回底按钮 +N（只在前台）。
                    if !view_at_bottom(&view) {
                        s.jump_unseen = s
                            .jump_unseen
                            .saturating_add(data.iter().filter(|&&b| b == b'\n').count() as u32);
                    }
                    forward_parser_replies_for(s, wid, pane.0);
                }
            }
        }
        // Index 专属快照：永不进入 Surface。
        StateChange::PaneIndexSnapshot { .. } => {}
        StateChange::MutationSettled { .. } => {
            // 异步 mutation 最终结果：只在前台 workspace 转成可见通知。
            if is_active {
                notify_mutation_settled(s, ev);
            }
        }
        StateChange::ActiveTabChanged { tab } => {
            effects.note_topology();
            if is_active {
                s.tab_gate.on_tab_changed(tab.0);
                s.active_tab = tab.0;
            }
        }
        StateChange::ActivePaneChanged { pane, .. } => {
            if is_active {
                // W18g：离开当前 pane 前记下副本 seq（上次看到这里）。
                let ws = active_workspace_id(s);
                let old = s.active_pane;
                if old != pane.0 {
                    let (last_line, _) = s.active_workspace().pane_last_line_seq(PaneId(old));
                    s.last_seen.insert((ws.clone(), old), last_line);
                }
                s.active_pane = pane.0;
                s.attention.on_became_visible(&ws, pane.0);
                let has_unseen = s.last_seen.get(&(ws.clone(), pane.0)).is_some_and(|seen| {
                    s.active_workspace().pane_last_line_seq(PaneId(pane.0)).0 != *seen
                });
                s.last_seen_mark.set_visible(has_unseen);
            }
        }
        StateChange::TabClosed { tab } => {
            effects.note_topology();
            if is_active {
                s.tab_gate.on_tab_closed(tab.0);
                mark_pending_close_if_session_ended(s);
            }
        }
        StateChange::TabAdded { .. } => {
            // 新 tab 可能已是快照里的 active tab；必须重建 UI 让 active_tab 跟上。
            effects.note_topology();
        }
        StateChange::TabOrderChanged => {
            effects.note_topology();
        }
        StateChange::LayoutChanged { .. } | StateChange::PaneAdded { .. } => {
            effects.note_topology();
        }
        StateChange::PaneClosed { .. } => {
            effects.note_topology();
            if is_active {
                mark_pending_close_if_session_ended(s);
            }
        }
        StateChange::StatusBarSubscription { name, value, pane } => {
            if is_active && name.starts_with("muxterm.pane-cmd") {
                let ws = active_workspace_id(s);
                s.attention.set_process_name(
                    &ws,
                    pane.map(|p| p.0).unwrap_or(0),
                    Some(value.clone()),
                );
            } else if is_active && name == "muxterm.status-left" {
                s.status_left = Some(value.clone());
                maybe_refresh_status(s, true);
            } else if is_active && name == "muxterm.status-right" {
                s.status_right = Some(value.clone());
                maybe_refresh_status(s, true);
            }
        }
        StateChange::BackendStatusChanged(status) => {
            if matches!(status, BackendStatus::Connecting) {
                // 新一轮 attach 会重新 capture 历史；清掉旧 generation 的
                // 保留批次，避免 reattach 后 seed_snapshot 重放旧历史。
                if let Some(layout) = s.pixel_cache.get(wid) {
                    for pane in layout.pane_ids() {
                        if let Some(view) = layout.pane(pane) {
                            view.begin_attach_generation();
                        }
                    }
                }
            }
            if is_active {
                s.runtime_status = match status {
                    BackendStatus::Connected => {
                        crate::core::protocol::ffi::types::BACKEND_STATUS_CONNECTED
                    }
                    BackendStatus::Connecting => {
                        crate::core::protocol::ffi::types::BACKEND_STATUS_CONNECTING
                    }
                    BackendStatus::Disconnected => {
                        crate::core::protocol::ffi::types::BACKEND_STATUS_DISCONNECTED
                    }
                    BackendStatus::Error => crate::core::protocol::ffi::types::BACKEND_STATUS_ERROR,
                    BackendStatus::Exited => {
                        crate::core::protocol::ffi::types::BACKEND_STATUS_EXITED
                    }
                };
                // W16b：tmux server 死后保留最后一帧 + 水印。
                let is_tmux = s.uses_tmux();
                match status {
                    BackendStatus::Connected => {
                        s.disconnect_overlay.set_visible(false);
                    }
                    BackendStatus::Disconnected if is_tmux => {
                        s.disconnect_overlay.set_visible(true);
                    }
                    BackendStatus::Exited if is_tmux => {
                        tracing::info!(
                            target = "muxterm::linux",
                            "tmux runtime exited; keep last frame"
                        );
                        s.disconnect_overlay.set_visible(true);
                    }
                    BackendStatus::Exited => {
                        tracing::info!(target = "muxterm::linux", "runtime exited");
                        if should_close_window(true, 0, s.on_last_pane_exit) {
                            s.pending_close = true;
                        }
                    }
                    _ => {}
                }
                maybe_refresh_status(s, true);
            }
        }
        StateChange::PaneResized { pane, cols, rows } => {
            effects.note_topology();
            if let Some(view) = resident_pane_view(s, wid, pane.0) {
                view.ensure_grid_size(*cols, *rows);
            }
        }
        _ => {}
    }
}

/// workspace-aware 四阶段批处理：结构 →（前台一次 refresh）→ frame → output。
/// background workspace 只更新自己的 pixel cache，不得切窗口当前页。
fn dispatch_event_batch_for(s: &mut UiState, wid: &WorkspaceId, events: Vec<StateChange>) {
    s.snapshot_seeded_this_batch.clear();
    let mut effects = UiBatchEffects::default();
    let (structure, baseline, output) = batch_order_plan(&events);
    if !baseline.is_empty() || !output.is_empty() {
        for i in &structure {
            dispatch_event_for(s, wid, &events[*i], &mut effects);
        }
        effects.commit(s, wid);
        for i in &baseline {
            dispatch_event_for(s, wid, &events[*i], &mut UiBatchEffects::default());
        }
        for i in &output {
            dispatch_event_for(s, wid, &events[*i], &mut UiBatchEffects::default());
        }
    } else {
        for ev in &events {
            dispatch_event_for(s, wid, ev, &mut effects);
        }
        effects.commit(s, wid);
    }
    s.snapshot_seeded_this_batch.clear();
}

/// 异步 mutation 最终结果的可见通知：失败显示 toast，成功只记日志。
///
/// `MutationSettled` 是唯一最终事件；GTK 不得因 `Accepted` 手工刷新或
/// 显示“创建完成”，只有这里的 Failed 才弹用户可见通知。
fn notify_mutation_settled(s: &mut UiState, ev: &StateChange) {
    let StateChange::MutationSettled {
        operation_id,
        kind,
        result,
    } = ev
    else {
        return;
    };
    let kind_name = match kind {
        crate::core::model::state::MutationKind::NewTab => "新 tab",
        crate::core::model::state::MutationKind::SplitPane => "分屏",
    };
    match result {
        crate::core::model::state::MutationResult::Completed => {
            tracing::info!(
                target: "muxterm::linux",
                operation_id = operation_id,
                kind = ?kind,
                "异步 mutation 完成"
            );
        }
        crate::core::model::state::MutationResult::Failed { stage, reason } => {
            tracing::warn!(
                target: "muxterm::linux",
                operation_id = operation_id,
                kind = ?kind,
                stage = ?stage,
                error = %reason,
                "异步 mutation 失败"
            );
            let stage_name = match stage {
                crate::core::model::state::MutationStage::Queue => "排队",
                crate::core::model::state::MutationStage::Dispatch => "派发",
                crate::core::model::state::MutationStage::AuthorityConvergence => "权威收敛",
                crate::core::model::state::MutationStage::StreamBootstrap => "流启动",
            };
            let body = format!("{kind_name}失败（{stage_name}）：{reason}");
            s.notification_sink
                .notify_done(&active_workspace_id(s), &body);
        }
    }
}

/// 会让 Workspace 产生 attention 信号的通用 Runtime 事件。
///
/// 这里故意只识别产品 `StateChange`，不识别 Herdr event 名或 Runtime id。
fn attention_event_pane(event: &StateChange) -> Option<u32> {
    match event {
        StateChange::PaneOutput { pane, .. }
        | StateChange::PaneFrame { pane, .. }
        | StateChange::PaneAgentChanged { pane, .. } => Some(pane.0),
        _ => None,
    }
}

fn apply_attention_event_from_workspace(
    s: &mut UiState,
    wid: &WorkspaceId,
    ws: &str,
    event: &StateChange,
) {
    if let StateChange::PaneClosed { pane } = event {
        s.attention.remove_pane(ws, pane.0);
    } else if let Some(pane) = attention_event_pane(event) {
        apply_attention_from_workspace(s, wid, ws, pane);
    }
}

fn dispatch_event(s: &mut UiState, ev: &StateChange, effects: &mut UiBatchEffects) {
    let ws = active_workspace_id(s);
    let wid = s.active_ws_id().clone();
    apply_attention_event_from_workspace(s, &wid, &ws, ev);
    match ev {
        StateChange::PaneSnapshot { pane, data } => {
            let ws = active_workspace_id(s);
            let wid = s.active_ws_id().clone();
            apply_attention_from_workspace(s, &wid, &ws, pane.0);
            if let Some(view) = s.active_layout().pane(pane.0).cloned() {
                sync_pane_grid_size(s, pane.0);
                // Snapshot 是替换而不是增量。只有在 GTK widget 已经有
                // 有效分配时直接 reset/feed；未 realize 的 pane 留给下一轮
                // seed_unseeded_pane 从 core Surface 补种，避免白屏。
                let seeded_from_core = s.snapshot_seeded_this_batch.contains(&pane.0);
                if !seeded_from_core
                    && surface_allocation_is_seedable(
                        view.widget().is_realized(),
                        view.widget().width(),
                        view.widget().height(),
                    )
                {
                    let (cols, rows) = s
                        .active_workspace()
                        .state()
                        .pane(pane)
                        .map(|p| (p.cols, p.rows))
                        .unwrap_or((80, 24));
                    view.seed_snapshot(data, cols, rows);
                    forward_parser_replies(s, pane.0);
                }
            }
        }
        StateChange::PaneHistory { pane, data } => {
            if let Some(view) = s.active_layout().pane(pane.0).cloned() {
                sync_pane_grid_size(s, pane.0);
                view.prepend_history(data);
            }
        }
        StateChange::PaneOutput { pane, data } | StateChange::PaneFrame { pane, data } => {
            if let Some(view) = s.active_layout().pane(pane.0).cloned() {
                // Codex 的 CUP/EL 按 tmux pane 列数生成；VTE 网格必须先对齐，
                // 否则输入框只剩「最近一个词」（2219.log tab2 %2）。
                sync_pane_grid_size(s, pane.0);
                // 未分配像素时仍入队（feed_* 不 flush），可 paint 后再补放。
                match ev {
                    StateChange::PaneFrame { .. } => view.feed_full(data),
                    _ => view.feed_output(data),
                }
                view.flush_deferred_feed();
                view.flush_deferred_history();
                // W18e：离开底部期间的新行累计到回底按钮 +N。
                if !view_at_bottom(&view) {
                    s.jump_unseen = s
                        .jump_unseen
                        .saturating_add(data.iter().filter(|&&b| b == b'\n').count() as u32);
                }
                forward_parser_replies(s, pane.0);
            }
        }
        // Index 专属快照（pane.read 等无头来源）：永不进入 Surface。
        // Workspace 已把它喂进 Index（搜索/attention），这里明确 no-op。
        StateChange::PaneIndexSnapshot { .. } => {}
        StateChange::MutationSettled { .. } => {
            // 异步 mutation 最终结果：转成用户可见通知（W5 接线）。
            notify_mutation_settled(s, ev);
        }
        StateChange::ActiveTabChanged { tab } => {
            s.tab_gate.on_tab_changed(tab.0);
            s.active_tab = tab.0;
            effects.note_topology();
        }
        StateChange::ActivePaneChanged { pane, .. } => {
            // W18g：离开当前 pane 前记下副本 seq（上次看到这里）。
            let ws = active_workspace_id(s);
            let old = s.active_pane;
            if old != pane.0 {
                let (last_line, _) = s.active_workspace().pane_last_line_seq(PaneId(old));
                s.last_seen.insert((ws.clone(), old), last_line);
            }
            s.active_pane = pane.0;
            s.attention.on_became_visible(&ws, pane.0);
            // 回到有未读输出的 pane：显示标记。
            let has_unseen = s.last_seen.get(&(ws.clone(), pane.0)).is_some_and(|seen| {
                s.active_workspace().pane_last_line_seq(PaneId(pane.0)).0 != *seen
            });
            s.last_seen_mark.set_visible(has_unseen);
        }
        StateChange::TabClosed { tab } => {
            s.tab_gate.on_tab_closed(tab.0);
            effects.note_topology();
            mark_pending_close_if_session_ended(s);
        }
        StateChange::TabAdded { .. } => {
            // 新 tab 可能已是快照里的 active tab（tmux %window-add 后
            // add_window_tab 会标记它 active）；必须重建 UI 让 active_tab 跟上。
            effects.note_topology();
        }
        StateChange::TabOrderChanged => {
            effects.note_topology();
        }
        StateChange::LayoutChanged { .. } | StateChange::PaneAdded { .. } => {
            effects.note_topology();
        }
        StateChange::PaneClosed { .. } => {
            effects.note_topology();
            mark_pending_close_if_session_ended(s);
        }
        StateChange::StatusBarSubscription { name, value, pane } => {
            if name.starts_with("muxterm.pane-cmd") {
                let ws = active_workspace_id(s);
                s.attention.set_process_name(
                    &ws,
                    pane.map(|p| p.0).unwrap_or(0),
                    Some(value.clone()),
                );
            } else if name == "muxterm.status-left" {
                s.status_left = Some(value.clone());
                maybe_refresh_status(s, true);
            } else if name == "muxterm.status-right" {
                s.status_right = Some(value.clone());
                maybe_refresh_status(s, true);
            } else {
                // 其它订阅：值已变化，强制按快照刷新一次。
                maybe_refresh_status(s, true);
            }
        }
        StateChange::BackendStatusChanged(status) => {
            if matches!(status, BackendStatus::Connecting) {
                // 新一轮 attach 会重新 capture 历史；清掉旧 generation 的
                // 保留批次，避免 reattach 后 seed_snapshot 重放旧历史。
                if let Some(layout) = s.pixel_cache.get(&wid) {
                    for pane in layout.pane_ids() {
                        if let Some(view) = layout.pane(pane) {
                            view.begin_attach_generation();
                        }
                    }
                }
            }
            s.runtime_status = match status {
                BackendStatus::Connected => {
                    crate::core::protocol::ffi::types::BACKEND_STATUS_CONNECTED
                }
                BackendStatus::Connecting => {
                    crate::core::protocol::ffi::types::BACKEND_STATUS_CONNECTING
                }
                BackendStatus::Disconnected => {
                    crate::core::protocol::ffi::types::BACKEND_STATUS_DISCONNECTED
                }
                BackendStatus::Error => crate::core::protocol::ffi::types::BACKEND_STATUS_ERROR,
                BackendStatus::Exited => crate::core::protocol::ffi::types::BACKEND_STATUS_EXITED,
            };
            // W16b：tmux server 死后保留最后一帧 + 水印，不 pending_close 整窗。
            // shell runtime 仍按 on_last_pane_exit 策略处理。
            let is_tmux = s.uses_tmux();
            match status {
                BackendStatus::Connected => {
                    s.disconnect_overlay.set_visible(false);
                }
                BackendStatus::Disconnected if is_tmux => {
                    s.disconnect_overlay.set_visible(true);
                }
                BackendStatus::Exited if is_tmux => {
                    tracing::info!(
                        target = "muxterm::linux",
                        "tmux runtime exited; keep last frame"
                    );
                    s.disconnect_overlay.set_visible(true);
                }
                BackendStatus::Exited => {
                    tracing::info!(target = "muxterm::linux", "runtime exited");
                    if should_close_window(true, 0, s.on_last_pane_exit) {
                        s.pending_close = true;
                    }
                }
                _ => {}
            }
            maybe_refresh_status(s, true);
        }
        StateChange::PaneResized { pane, cols, rows } => {
            effects.note_topology();
            if let Some(view) = s.active_layout().pane(pane.0) {
                view.ensure_grid_size(*cols, *rows);
            }
        }
        _ => {}
    }
}

fn mark_pending_close_if_session_ended(s: &mut UiState) {
    let n_tabs = s.active_workspace().state().tabs().len();
    if should_close_window(false, n_tabs, s.on_last_pane_exit) {
        s.pending_close = true;
    }
}

/// 取走本轮 blocked / done 通知并交给 sink（测试日志也记录）。
fn drain_attention_notifications(s: &mut UiState) {
    let blocked = s.attention.take_new_blocked_notifications();
    for ws in &blocked {
        tracing::info!(
            target: "muxterm::notify",
            workspace = %ws,
            "blocked workspace notification"
        );
        s.notification_sink.notify_blocked(ws, "needs attention");
        s.notification_log.push(format!("{ws}: needs attention"));
    }
    let done = s.attention.take_new_done_notifications();
    for ws in &done {
        tracing::info!(
            target: "muxterm::notify",
            workspace = %ws,
            "background task done"
        );
        s.notification_sink.notify_done(ws, "task complete");
        s.notification_log.push(format!("{ws}: task complete"));
    }
}

fn refresh_ui(s: &mut UiState) {
    let wid = s.active_ws_id().clone();
    refresh_workspace_layout(s, &wid);
    maybe_refresh_status(s, true);
    sync_chrome_visibility(s);
}

/// Commit one workspace's final topology to its persistent LayoutHost.
///
/// This function intentionally does not switch the active window for a
/// background workspace.  It only creates/reparents resident PaneViews and
/// feeds an already-realized Surface from that workspace's Core state.
fn refresh_workspace_layout(s: &mut UiState, wid: &WorkspaceId) {
    let is_active = s.pool.active_id() == Some(wid);
    let (tab_ids, active_tab, layouts, panes) = {
        let Some(workspace) = s.pool.get(wid) else {
            return;
        };
        let state = workspace.state();
        let tabs = state.tabs();
        let tab_ids: Vec<u32> = tabs.iter().map(|t| t.id.0).collect();
        let active_tab = tabs
            .iter()
            .find(|t| t.active)
            .map(|t| t.id.0)
            .or_else(|| tabs.first().map(|t| t.id.0));
        // W4：topology sync 必须为**所有** tab 的 leaves 建立常驻 PaneView，
        // 不能只建 active tab；hidden tab 的 frame/output 隐藏期间继续 feed。
        let layouts: Vec<(u32, LayoutNode)> = tabs
            .iter()
            .filter_map(|t| state.layout(&t.id).map(|l| (t.id.0, l.tree.clone())))
            .collect();
        let panes: Vec<(u32, u16, u16, bool)> = active_tab
            .map(|tid| {
                state
                    .panes(&TabId(tid))
                    .iter()
                    .map(|p| (p.id.0, p.cols, p.rows, p.active))
                    .collect()
            })
            .unwrap_or_default();
        (tab_ids, active_tab, layouts, panes)
    };

    if is_active {
        // tab 列表由 status bar 中区渲染（apply 时按签名重建），这里只维护门禁。
        s.tab_gate.on_snapshot(&tab_ids);
        if let Some(active) = active_tab {
            s.active_tab = active;
        }
        if !s.tab_gate.is_released() {
            sync_chrome_visibility(s);
            return;
        }
    }

    let owner = wid.clone();
    let input_queue = s.surface_input_queue.clone();
    let input_cb = move |pane_id: u32, data: &[u8]| {
        input_queue.borrow_mut().push_back(SurfaceInput {
            workspace: owner.clone(),
            pane: PaneId(pane_id),
            data: data.to_vec(),
        });
    };

    // 重建布局（pane 控件跨 tab 保留：像素缓存，不因换 tab 销毁）。
    if !layouts.is_empty() {
        if let Some(layout) = s.pixel_cache.get_mut(wid) {
            for (tab, tree) in &layouts {
                layout.apply_layout(*tab, tree, &input_cb);
            }
            // 全部 tab 常驻后，把 active tab 放回可见页（apply_layout 会
            // 依次 set_visible_child，最后一次调用决定显示页）。
            if let Some(active) = active_tab {
                layout.show_tab(active);
            }
        }

        for (pane_id, cols, rows, pane_active) in panes {
            if let Some(view) = resident_pane_view(s, wid, pane_id) {
                // Surface：已有 pane 只 show/hide，不 reset、不 dump；
                // 滚动走 VTE 自身 scrollback（F5）。
                view.ensure_grid_size(cols, rows);
                // attach 保真（1820.log 白屏）：布局建好后，把 core 里
                // capture-pane 快照播种进 VTE。快照事件可能在视图创建前
                // 已消费，不能只依赖 PaneOutput 增量。
                seed_unseeded_pane_for(s, wid, &view, pane_id, cols, rows);
                if is_active && pane_active {
                    s.active_pane = pane_id;
                    view.grab_focus();
                }
            }
        }
    }
}

fn local_status_snapshot(npanes: usize, tabs: &[(u32, String, bool)]) -> StatusBarSnapshot {
    let connected = i18n::tr(Key::StatusConnected);
    let panes = i18n::tr(Key::Panes);
    let close_hint = i18n::tr(Key::WindowCloseHint);
    let mut snap = crate::platform::linux::quickconnect::status_style::snapshot_from_tabs(
        "local", npanes, tabs,
    );
    snap.left = format!("{connected} | {npanes} {panes}");
    snap.right = close_hint;
    snap.interval = 1;
    snap
}

fn maybe_refresh_status(s: &mut UiState, force: bool) {
    let state = s.active_workspace().state();
    let tabs = state.tabs();
    let npanes = state.panes(&TabId(s.active_tab)).len();
    let session = state.workspace_name().to_string();
    let rows: Vec<(u32, String, bool)> = tabs
        .iter()
        .map(|t| (t.id.0, t.name.clone(), t.active))
        .collect();
    let mut snap = if s.uses_tmux() {
        crate::platform::linux::quickconnect::status_style::snapshot_from_tabs(
            &session, npanes, &rows,
        )
    } else {
        local_status_snapshot(npanes, &rows)
    };
    // tmux ≥3.2 订阅推送的 status-left/right 覆盖默认文案（零轮询）。
    if let Some(left) = s.status_left.as_deref() {
        snap.left = left.to_string();
    }
    if let Some(right) = s.status_right.as_deref() {
        snap.right = right.to_string();
    }
    let _ = force;
    s.status.apply(&snap);
    sync_chrome_visibility(s);
}

/// 窗口关闭意图：非 Quit 动作 → 隐藏并保持轮询；Quit → 真正关闭。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseIntent {
    HideKeepPolling,
    Quit,
}

/// 根据是否 Quit 动作决定关闭意图（M3.5 纯函数）。
pub fn close_intent(is_quit_action: bool) -> CloseIntent {
    if is_quit_action {
        CloseIntent::Quit
    } else {
        CloseIntent::HideKeepPolling
    }
}

/// 标记 Quit 并关闭窗口。调用方不得在持有 `RefMut<UiState>` 时进入。
fn request_quit_close(state: &Rc<RefCell<UiState>>, window: &Window) {
    state.borrow_mut().quit_requested = true;
    window.close();
}

/// 是否仍需要按时间轮询状态栏。
///
/// tmux 订阅生效时值变化走推送，轮询只在订阅未生效或强制刷新时发生。
pub fn should_poll_status(
    sub_active: bool,
    last: Instant,
    now: Instant,
    interval: Duration,
) -> bool {
    !sub_active && now.duration_since(last) >= interval
}

fn sync_chrome_visibility(s: &UiState) {
    // 唯一 chrome：status bar 永远可见，没有第二条 tab 带。
    // worktree 创建入口只按 support() 露出（禁止 if runtime == "herdr"）。
    let worktree = s
        .pool
        .active()
        .map(|w| {
            w.runtime()
                .support()
                .contains(&RuntimeCapability::WorktreeList)
        })
        .unwrap_or(false);
    s.status.set_worktree_visible(worktree);
}

/// Drain VTE input callbacks on the production GTK poll.
///
/// The queue is deliberately independent from `UiState`: PaneView callbacks
/// can outlive the layout pass that installed them, so they must never borrow
/// or mutate the state directly.  An input whose workspace/pane disappeared is
/// dropped with a diagnostic instead of being redirected to the active pane.
fn drain_surface_input(s: &mut UiState) {
    let pending = take_surface_input(&s.surface_input_queue);
    let mut pending = pending.into_iter().peekable();
    while let Some(first) = pending.next() {
        // GTK/VTE emits one commit for each typed character.  Keep adjacent
        // commits for the same owner together so a newly-created tmux pane
        // receives one ordered write instead of a burst of independent
        // control-mode commands that can race pane startup.
        let workspace_id = first.workspace.clone();
        let pane_id = first.pane;
        let mut data = first.data;
        while let Some(next) = pending.peek() {
            if next.workspace != workspace_id || next.pane != pane_id {
                break;
            }
            let next = pending.next().expect("peeked SurfaceInput");
            data.extend_from_slice(&next.data);
        }
        s.last_raw_input = data.clone();
        let Some(workspace) = s.pool.get_mut(&workspace_id) else {
            tracing::debug!(
                target = "muxterm::surface",
                workspace = %workspace_id,
                pane = %pane_id,
                "drop input for evicted workspace"
            );
            continue;
        };
        if workspace.state().pane(&pane_id).is_none() {
            tracing::debug!(
                target = "muxterm::surface",
                workspace = %workspace_id,
                pane = %pane_id,
                "drop input for closed pane"
            );
            continue;
        }
        let result = workspace.execute(Task::WriteRaw {
            target: pane_id,
            data,
        });
        if let Err(error) = result {
            tracing::warn!(
                target = "muxterm::surface",
                workspace = %workspace_id,
                pane = %pane_id,
                error = %error,
                "surface input write failed"
            );
        }
    }
}

fn take_surface_input(queue: &Rc<RefCell<VecDeque<SurfaceInput>>>) -> Vec<SurfaceInput> {
    queue.borrow_mut().drain(..).collect()
}

fn sync_pane_outputs(s: &mut UiState) {
    // Surface：已挂载 pane 的增量走 StateChange::PaneOutput；这里不再 dump replica。
    // F3 的 capture 门接管首屏 seed；未 realized 的 pane 保持 unseeded，
    // 等窗口 present 后由下一次轮询补种（present 前 feed 会被 VTE 丢弃）。
    let panes: Vec<(u32, u16, u16)> = s
        .active_workspace()
        .state()
        .panes(&TabId(s.active_tab))
        .iter()
        .map(|p| (p.id.0, p.cols, p.rows))
        .collect();
    for (pane_id, cols, rows) in panes {
        if let Some(view) = s.active_layout().pane(pane_id).cloned() {
            view.ensure_grid_size(cols, rows);
            seed_unseeded_pane(s, &view, pane_id, cols, rows);
        }
    }
}

/// 把 core 里已就绪的 attach 快照播种进尚未播种的 VTE。
///
/// 窗口 present/realize 前 feed 会被 VTE 丢弃（白屏），所以只在 widget
/// 已 realized 时播种；未 realized 的 pane 保持 unseeded，等布局挂载后
/// 由下一次 refresh_ui / sync_pane_outputs 补种。
fn seed_unseeded_pane(
    s: &mut UiState,
    view: &std::rc::Rc<PaneView>,
    pane_id: u32,
    cols: u16,
    rows: u16,
) {
    let wid = s.active_ws_id().clone();
    seed_unseeded_pane_for(s, &wid, view, pane_id, cols, rows);
}

/// VTE 只有在 realize 且二维分配都有效时才能可靠接收首帧。
fn surface_allocation_is_seedable(realized: bool, width: i32, height: i32) -> bool {
    realized && width > 0 && height > 0
}

fn seed_unseeded_pane_for(
    s: &mut UiState,
    wid: &WorkspaceId,
    view: &std::rc::Rc<PaneView>,
    pane_id: u32,
    cols: u16,
    rows: u16,
) {
    if view.is_seeded() || !view.can_paint_surface() {
        if view.is_seeded() && view.can_paint_surface() {
            view.flush_deferred_history();
            view.flush_deferred_feed();
        }
        return;
    }
    if let Some(bytes) = s
        .pool
        .get(wid)
        .and_then(|workspace| workspace.state().pane_output(&PaneId(pane_id)))
        .map(|b| b.to_vec())
    {
        tracing::info!(
            target: "muxterm::surface",
            pane = pane_id,
            bytes = bytes.len(),
            "surface baseline seed from current-generation frame"
        );
        view.seed_raw(&bytes, cols, rows);
        s.snapshot_seeded_this_batch.insert(pane_id);
    } else {
        tracing::info!(
            target: "muxterm::surface",
            pane = pane_id,
            "pane view unseeded and core pane_output empty"
        );
    }
}

fn sync_pane_grid_size(s: &UiState, pane_id: u32) {
    let Some(view) = s.active_layout().pane(pane_id) else {
        return;
    };
    if let Some(pane) = s
        .active_workspace()
        .state()
        .panes(&TabId(s.active_tab))
        .iter()
        .find(|p| p.id.0 == pane_id)
    {
        view.ensure_grid_size(pane.cols, pane.rows);
    }
}

/// 按 `(WorkspaceId, PaneId)` 对齐字符格（hidden tab / background 也适用）。
fn sync_pane_grid_size_for(s: &UiState, wid: &WorkspaceId, pane_id: u32) {
    let Some(view) = resident_pane_view(s, wid, pane_id) else {
        return;
    };
    let (cols, rows) = s
        .pool
        .get(wid)
        .and_then(|w| w.state().pane(&PaneId(pane_id)))
        .map(|p| (p.cols, p.rows))
        .unwrap_or((80, 24));
    view.ensure_grid_size(cols, rows);
}

fn forward_parser_replies(s: &mut UiState, pane_id: u32) {
    // 查询应答以工作区 PaneBuf 的 TerminalState 为事实源（LINUX-PLAN §2.5）。
    // tmux/SSH 镜像模式由 refresh-client -r 代答 OSC/DA，不能写回 PTY。
    let replies = s.active_workspace_mut().take_reply(PaneId(pane_id));
    if s.uses_tmux() {
        return;
    }
    if !replies.is_empty() {
        let _ = s.active_workspace_mut().execute(Task::WriteRaw {
            target: PaneId(pane_id),
            data: replies,
        });
    }
}

/// 按 WorkspaceId 转发 parser replies（background workspace 也 flush）。
fn forward_parser_replies_for(s: &mut UiState, wid: &WorkspaceId, pane_id: u32) {
    let Some(ws) = s.pool.get_mut(wid) else {
        return;
    };
    let replies = ws.take_reply(PaneId(pane_id));
    let is_tmux = ws
        .runtime()
        .support()
        .contains(&RuntimeCapability::SharedClientResize);
    if is_tmux {
        return;
    }
    if !replies.is_empty() {
        let _ = ws.execute(Task::WriteRaw {
            target: PaneId(pane_id),
            data: replies,
        });
    }
}

/// 把窗口内容区的新字符格尺寸同步给 Runtime。
///
/// 共享 client viewport 的 Runtime（tmux）收到整个 Workspace 的尺寸；
/// 其它 Runtime（shell / Herdr）收到当前 Surface 的实际字符格尺寸。
/// platform 只问 capability，不按实现名字分支。
///
/// tmux SharedClientResize：同一尺寸连续 ~10 次 poll（约 160ms）才 -C，
/// 过滤窗口 map / VTE preferred 抖动（dogfood 2152：106→284→142）。
const CLIENT_SIZE_STABLE_HITS: u8 = 10;

fn sync_window_size(s: &mut UiState) {
    let Some(view) = s.active_layout().pane(s.active_pane) else {
        return;
    };
    let term = view.terminal();
    let cw = term.char_width();
    let ch = term.char_height();
    if cw <= 0 || ch <= 0 {
        return;
    }
    let root_w = s.active_layout().root_box.width().max(0) as u64;
    let root_h = s.active_layout().root_box.height().max(0) as u64;
    if root_w == 0 || root_h == 0 {
        return;
    }
    let allocated = term.width() > 0 && term.height() > 0;
    let multi_pane = s
        .active_workspace()
        .state()
        .panes(&TabId(s.active_tab))
        .len()
        > 1;
    let shared_client_resize = s
        .active_workspace()
        .runtime()
        .support()
        .contains(&RuntimeCapability::SharedClientResize);
    let (cols, rows) = if shared_client_resize {
        let cols =
            match ClientSizePolicy::cols(term.column_count(), allocated, root_w, cw, multi_pane) {
                Some(cols) => cols,
                None => return,
            };
        let rows = match ClientSizePolicy::rows(root_h, ch) {
            Some(rows) => rows,
            None => return,
        };
        (cols, rows)
    } else {
        if !allocated {
            return;
        }
        let cols = (i64::from(term.width()) / cw).clamp(2, i64::from(u16::MAX)) as u16;
        let rows = (i64::from(term.height()) / ch).clamp(1, i64::from(u16::MAX)) as u16;
        (cols, rows)
    };
    if shared_client_resize {
        if s.last_client_size == Some((None, cols, rows)) {
            s.pending_client_size = None;
            s.pending_client_hits = 0;
            return;
        }
        if s.pending_client_size == Some((cols, rows)) {
            s.pending_client_hits = s.pending_client_hits.saturating_add(1);
        } else {
            s.pending_client_size = Some((cols, rows));
            s.pending_client_hits = 1;
        }
        if s.pending_client_hits < CLIENT_SIZE_STABLE_HITS {
            return;
        }
        s.last_client_size = Some((None, cols, rows));
        s.pending_client_size = None;
        s.pending_client_hits = 0;
        let _ = s
            .active_workspace_mut()
            .execute(Task::ResizeClient { cols, rows });
    } else {
        let pane = s.active_pane;
        if s.last_client_size == Some((Some(pane), cols, rows)) {
            return;
        }
        s.last_client_size = Some((Some(pane), cols, rows));
        let _ = s.active_workspace_mut().execute(Task::ResizePane {
            target: PaneId(pane),
            cols,
            rows,
        });
    }
}

/// SSH 可达性缓存 TTL（W15d：面板打开时后台探测，TTL 内复用）。
const SSH_PROBE_TTL: Duration = Duration::from_secs(30);

/// 后台探测一个 SSH 别名（`ssh -o BatchMode=yes -o ConnectTimeout=2 <alias> true`）。
fn spawn_ssh_probe(s: &mut UiState, alias: String) {
    let (tx, rx) = std::sync::mpsc::channel::<(String, SshReach)>();
    s.pending_ssh_probes.push_back(rx);
    std::thread::spawn(move || {
        let args = crate::core::transport::ssh::probe::ssh_probe_args(&alias, 2);
        let status = std::process::Command::new("ssh")
            .args(&args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let reach = match status {
            Ok(st) => crate::core::transport::ssh::probe::classify_ssh_probe(st.code()),
            Err(_) => SshReach::Err,
        };
        let _ = tx.send((alias, reach));
    });
}

/// 面板打开时收集 SSH 灯：TTL 内用缓存，否则 Unknown 并后台探测。
fn collect_ssh_reach(s: &mut UiState, workspaces: &[PanelItem]) -> HashMap<String, SshReach> {
    let now = Instant::now();
    let mut out = HashMap::new();
    let mut seen = Vec::new();
    for item in workspaces {
        if let PanelItem::Target(entry, _) = item {
            if let TargetTransport::Ssh { name } = &entry.config.transport {
                if seen.contains(name) {
                    continue;
                }
                seen.push(name.clone());
                let fresh = s
                    .ssh_reach_cache
                    .get(name.as_str())
                    .is_some_and(|(_, at)| now.duration_since(*at) < SSH_PROBE_TTL);
                if fresh {
                    out.insert(name.clone(), s.ssh_reach_cache[name.as_str()].0);
                } else {
                    out.insert(name.clone(), SshReach::Unknown);
                    spawn_ssh_probe(s, name.clone());
                }
            }
        }
    }
    out
}

/// 收编后台 SSH 探测结果（16ms poll 与 test_poll_once 共用）。
fn drain_ssh_probes(state: &Rc<RefCell<UiState>>) {
    let mut done = false;
    while !done {
        let result = {
            let mut s = state.borrow_mut();
            let Some(rx) = s.pending_ssh_probes.front() else {
                break;
            };
            match rx.try_recv() {
                Ok(r) => {
                    s.pending_ssh_probes.pop_front();
                    Some(r)
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    done = true;
                    None
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    s.pending_ssh_probes.pop_front();
                    None
                }
            }
        };
        if let Some((alias, reach)) = result {
            state
                .borrow_mut()
                .ssh_reach_cache
                .insert(alias, (reach, Instant::now()));
        }
    }
}

/// C6：Catalog SessionCandidate → 面板 ExistingEntry（socket 按 namespace 推导）。
fn candidate_to_existing(c: &crate::core::catalog::driver::SessionCandidate) -> ExistingEntry {
    let runtime = match c.runtime_id.as_str() {
        "herdr" => TargetRuntime::Herdr,
        "shell" => TargetRuntime::Shell,
        _ => TargetRuntime::Tmux,
    };
    let transport = if c.transport_id == "ssh" {
        TargetTransport::Ssh {
            name: c.target.clone(),
        }
    } else {
        TargetTransport::Local
    };
    let herdr_socket = if runtime == TargetRuntime::Herdr {
        if c.socket.as_deref().is_some_and(|socket| !socket.is_empty()) {
            c.socket.clone()
        } else {
            let home = std::env::var("HOME").unwrap_or_default();
            match c.namespace.as_deref() {
                Some(ns) if !ns.is_empty() && ns != "default" => {
                    Some(format!("{home}/.config/herdr/sessions/{ns}/herdr.sock"))
                }
                _ => Some(format!("{home}/.config/herdr/herdr.sock")),
            }
        }
    } else {
        None
    };
    let tmux_socket = (runtime == TargetRuntime::Tmux)
        .then(|| c.socket.clone())
        .flatten();
    ExistingEntry {
        title: c.name.clone(),
        runtime,
        transport,
        tmux_session: (runtime == TargetRuntime::Tmux)
            .then(|| c.session.clone().unwrap_or_else(|| c.name.clone())),
        tmux_socket,
        herdr_session: (runtime == TargetRuntime::Herdr).then(|| {
            c.namespace
                .clone()
                .filter(|ns| !ns.is_empty())
                .unwrap_or_else(|| "default".to_string())
        }),
        herdr_workspace_id: (runtime == TargetRuntime::Herdr)
            .then(|| c.extra.clone())
            .filter(|s| !s.is_empty()),
        herdr_socket,
    }
}

/// 已有的连接探测增量：先推 local 行，SSH 完成后再推；Done 才清 inflight。
enum ExistingProbeMsg {
    Rows(Vec<ExistingEntry>),
    Done,
}

fn merge_existing_entries(ex: &mut ExistingPanelState, entries: Vec<ExistingEntry>) {
    for e in entries {
        match &e.transport {
            TargetTransport::Ssh { name } => {
                if !ex.hosts.contains(name) {
                    ex.hosts.push(name.clone());
                }
                let bucket = ex.remote.entry(name.clone()).or_default();
                if !bucket.contains(&e) {
                    bucket.push(e);
                }
            }
            TargetTransport::Local => {
                if !ex.locals.contains(&e) {
                    ex.locals.push(e);
                }
            }
        }
    }
}

/// C7/C9：已有的连接探测。先 `discover_sessions("local")` 立刻推表，
/// 再按 SSH host 最多 4 路并发。禁止等 `all` 串完才刷新（archmini 上 cd/mac 会冻 Loading）。
fn spawn_local_existing_probe(s: &mut UiState) {
    s.existing.borrow_mut().probe_inflight = true;
    let (tx, rx) = std::sync::mpsc::channel::<ExistingProbeMsg>();
    s.pending_local_probe.push_back(rx);
    std::thread::spawn(move || {
        let mut catalog = crate::core::catalog::Catalog::with_builtins();
        tracing::debug!(target = "muxterm::linux", "existing probe: local start");
        let local: Vec<ExistingEntry> = catalog
            .discover_sessions("local", "")
            .unwrap_or_default()
            .iter()
            .map(candidate_to_existing)
            .collect();
        tracing::debug!(
            target = "muxterm::linux",
            n = local.len(),
            "existing probe: local done"
        );
        let _ = tx.send(ExistingProbeMsg::Rows(local));

        let ssh_config = std::env::var_os("MUXTERM_SSH_CONFIG_PATH").map(std::path::PathBuf::from);
        let aliases: Vec<String> = crate::core::discovery::list_ssh_hosts(ssh_config.as_deref())
            .unwrap_or_default()
            .into_iter()
            .map(|h| h.alias)
            .collect();
        tracing::debug!(
            target = "muxterm::linux",
            hosts = ?aliases,
            "existing probe: ssh hosts"
        );
        for chunk in aliases.chunks(4) {
            std::thread::scope(|scope| {
                let handles: Vec<_> = chunk
                    .iter()
                    .map(|alias| {
                        let alias = alias.clone();
                        scope.spawn(move || {
                            tracing::debug!(
                                target = "muxterm::linux",
                                alias = %alias,
                                "existing probe: ssh start"
                            );
                            let mut catalog = crate::core::catalog::Catalog::with_builtins();
                            let entries: Vec<ExistingEntry> = catalog
                                .discover_sessions("ssh", &alias)
                                .unwrap_or_default()
                                .iter()
                                .map(candidate_to_existing)
                                .collect();
                            tracing::debug!(
                                target = "muxterm::linux",
                                alias = %alias,
                                n = entries.len(),
                                "existing probe: ssh done"
                            );
                            entries
                        })
                    })
                    .collect();
                for handle in handles {
                    if let Ok(entries) = handle.join() {
                        if !entries.is_empty() {
                            let _ = tx.send(ExistingProbeMsg::Rows(entries));
                        }
                    }
                }
            });
        }
        let _ = tx.send(ExistingProbeMsg::Done);
    });
}

/// 收编已有连接探测结果（16ms poll 与 test_poll_once 共用）。
fn drain_local_existing(state: &Rc<RefCell<UiState>>) {
    let mut wait = false;
    while !wait {
        let msg = {
            let mut s = state.borrow_mut();
            let Some(rx) = s.pending_local_probe.front() else {
                break;
            };
            match rx.try_recv() {
                Ok(ExistingProbeMsg::Rows(rows)) => Some(Ok(rows)),
                Ok(ExistingProbeMsg::Done) => {
                    s.pending_local_probe.pop_front();
                    Some(Err(()))
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    wait = true;
                    None
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    s.pending_local_probe.pop_front();
                    Some(Err(()))
                }
            }
        };
        match msg {
            Some(Ok(entries)) => {
                let n = entries.len();
                let s = state.borrow();
                let mut ex = s.existing.borrow_mut();
                merge_existing_entries(&mut ex, entries);
                drop(ex);
                tracing::debug!(
                    target = "muxterm::linux",
                    n,
                    "existing probe: ui rows applied"
                );
                crate::platform::linux::quickconnect_panel::refresh_current();
            }
            Some(Err(())) => {
                state.borrow().existing.borrow_mut().probe_inflight = false;
                tracing::debug!(target = "muxterm::linux", "existing probe: ui done");
                crate::platform::linux::quickconnect_panel::refresh_current();
            }
            None => {}
        }
    }
}

/// W20：SSH 已有的连接探测（tmux + Herdr），后台线程，最多 4 路并发。
fn spawn_existing_ssh_probe(state: &Rc<RefCell<UiState>>) {
    {
        let mut s = state.borrow_mut();
        if s.existing_ssh_probing {
            return;
        }
        s.existing_ssh_probing = true;
        s.existing.borrow_mut().probe_inflight = true;
    }
    let ssh_config = std::env::var_os("MUXTERM_SSH_CONFIG_PATH").map(std::path::PathBuf::from);
    let aliases: Vec<String> = crate::core::discovery::list_ssh_hosts(ssh_config.as_deref())
        .unwrap_or_default()
        .into_iter()
        .map(|h| h.alias)
        .collect();
    let (tx, rx) = std::sync::mpsc::channel::<Vec<(String, Vec<ExistingEntry>)>>();
    state.borrow_mut().pending_existing_ssh.push_back(rx);
    std::thread::spawn(move || {
        // 最多 4 路并发：慢 host 不能把整表拖到串行 10s 级。
        let results: Vec<(String, Vec<ExistingEntry>)> = aliases
            .chunks(4)
            .flat_map(|chunk| {
                std::thread::scope(|scope| {
                    let handles: Vec<_> = chunk
                        .iter()
                        .map(|alias| {
                            scope.spawn(move || {
                                let mut catalog = crate::core::catalog::Catalog::with_builtins();
                                let entries = catalog
                                    .discover_sessions("ssh", alias)
                                    .unwrap_or_default()
                                    .iter()
                                    .map(candidate_to_existing)
                                    .collect();
                                (alias.clone(), entries)
                            })
                        })
                        .collect();
                    handles
                        .into_iter()
                        .map(|h| h.join().unwrap_or_default())
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let _ = tx.send(results);
    });
}

/// 收编 SSH 已有连接探测结果（16ms poll 与 test_poll_once 共用）。
fn drain_existing_ssh(state: &Rc<RefCell<UiState>>) {
    let mut done = false;
    while !done {
        let result = {
            let mut s = state.borrow_mut();
            let Some(rx) = s.pending_existing_ssh.front() else {
                break;
            };
            match rx.try_recv() {
                Ok(r) => {
                    s.pending_existing_ssh.pop_front();
                    s.existing_ssh_probing = false;
                    s.existing.borrow_mut().probe_inflight = false;
                    Some(r)
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    done = true;
                    None
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    s.pending_existing_ssh.pop_front();
                    s.existing_ssh_probing = false;
                    s.existing.borrow_mut().probe_inflight = false;
                    None
                }
            }
        };
        if let Some(results) = result {
            let mut hosts = Vec::new();
            let mut remote = std::collections::HashMap::new();
            for (alias, entries) in results {
                if !entries.is_empty() {
                    hosts.push(alias.clone());
                    remote.insert(alias, entries);
                }
            }
            {
                let s = state.borrow();
                let mut ex = s.existing.borrow_mut();
                ex.hosts = hosts;
                ex.remote = remote;
            }
            crate::platform::linux::quickconnect_panel::refresh_current();
        }
    }
}

/// W17a：tmux 控制 client 掉线后自动重连。
///
/// 只重连 tmux 类 runtime；shell runtime 掉线仍按原策略。重连线程构造**新**
/// Runtime（同一 socket/session），成功后 swap 进同一个 Workspace——PaneBuf
/// 在 Workspace 侧，不会因换 client 丢索引。
fn maybe_schedule_reconnect(state: &Rc<RefCell<UiState>>) {
    let mut s = state.borrow_mut();
    if s.reconnecting {
        return;
    }
    let Some(ws) = s.pool.active() else {
        return;
    };
    let is_tmux = matches!(ws.state().workspace_runtime(), "tmux" | "ssh" | "tmux-ssh");
    if !is_tmux || ws.runtime().runtime_status() == BackendStatus::Connected {
        return;
    }
    let now = Instant::now();
    if s.reconnect_retry_at.is_some_and(|at| now < at) {
        return;
    }
    let id = ws.id().clone();
    let socket = s.workspace_sockets.get(&id).cloned().flatten();
    let scrollback = s.scrollback_lines;
    let spec = reconnect_spec(&id, socket, scrollback);
    let handle = s.rt.handle().clone();
    let (tx, rx) = std::sync::mpsc::channel::<ReconnectResult>();
    s.reconnecting = true;
    s.pending_reconnects.push_back(rx);
    std::thread::spawn(move || {
        // 新 client attach 会清掉 window_bell_flag，必须在 attach 之前查
        //（断线期间的 BEL 不会以 %output 重放）。
        let bell = query_window_bell_flag(&spec);
        let result = connect_runtime_blocking(&spec, &handle);
        let _ = tx.send((id, result, bell));
    });
}

/// 从 WorkspaceId + socket 重建连接规格（本地/SSH tmux attach）。
fn reconnect_spec(id: &WorkspaceId, socket: Option<String>, scrollback: u32) -> WorkspaceSpec {
    if id.transport == "ssh" {
        WorkspaceSpec::ssh_tmux(
            id.alias.clone().unwrap_or_default(),
            Some(id.session.clone()),
            socket,
        )
        .with_scrollback_lines(scrollback)
    } else {
        WorkspaceSpec::local_tmux(Some(id.session.clone()), socket)
            .with_scrollback_lines(scrollback)
    }
}

/// 后台线程：构造新 TmuxRuntime 并 connect（复用 tokio handle 保持任务存活）。
fn connect_runtime_blocking(
    spec: &WorkspaceSpec,
    handle: &tokio::runtime::Handle,
) -> anyhow::Result<std::boxed::Box<dyn Runtime>> {
    let mut runtime = spec.build_runtime();
    handle.block_on(async {
        tokio::time::timeout(Duration::from_secs(10), runtime.connect())
            .await
            .map_err(|_| anyhow::anyhow!("reconnect timed out after 10s"))?
    })?;
    Ok(runtime)
}

/// 收编重连结果（16ms poll 与 test_poll_once 共用）。
fn drain_pending_reconnects(state: &Rc<RefCell<UiState>>) {
    let result = {
        let mut s = state.borrow_mut();
        let Some(rx) = s.pending_reconnects.front() else {
            return;
        };
        match rx.try_recv() {
            Ok(r) => {
                s.pending_reconnects.pop_front();
                s.reconnecting = false;
                Some(r)
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                s.pending_reconnects.pop_front();
                s.reconnecting = false;
                None
            }
        }
    };
    if let Some((id, Ok(runtime), bell)) = result {
        handle_reconnect_success(state, id, runtime, bell);
    } else if let Some((_, Err(e), _)) = result {
        let mut s = state.borrow_mut();
        s.reconnect_attempts = s.reconnect_attempts.saturating_add(1);
        let delay = Duration::from_secs(1u64 << s.reconnect_attempts.min(3));
        s.reconnect_retry_at = Some(Instant::now() + delay);
        tracing::warn!(
            target = "muxterm::linux",
            "reconnect failed (attempt {}): {e}; retry in {delay:?}",
            s.reconnect_attempts
        );
    }
}

/// 断线期间查 `#{window_bell_flag}`（必须在新 client attach 之前查，attach 会清 flag）。
///
/// SSH 工作区必须走 `ssh <alias> tmux -L <远端 socket> ...`，禁止对本机
/// `tmux -L <远端名>`（那会打到错的 server 或什么都没有）。
fn query_window_bell_flag(spec: &WorkspaceSpec) -> bool {
    if spec.transport == "ssh" {
        let alias = spec.alias.as_deref().unwrap_or("");
        let socket = spec.socket.as_deref().unwrap_or("");
        let session = &spec.session;
        let mut cmd = std::process::Command::new("ssh");
        if let Ok(cfg) = std::env::var("MUXTERM_SSH_CONFIG_PATH") {
            cmd.args(["-F", &cfg]);
        }
        cmd.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5", alias, "--"])
            .arg(format!(
                "tmux -L {socket} display-message -p -t {session} '#{{window_bell_flag}}'"
            ));
        cmd.output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "1")
            .unwrap_or(false)
    } else {
        let socket = spec.socket.as_deref().unwrap_or("");
        std::process::Command::new("tmux")
            .args([
                "-L",
                socket,
                "display-message",
                "-p",
                "-t",
                &spec.session,
                "#{window_bell_flag}",
            ])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "1")
            .unwrap_or(false)
    }
}

/// 重连成功：换 Runtime、隐藏水印；断线期间的 BEL 重新推导成 Blocked。
///
/// 只对「仍处于断线状态」的工作区生效：若断线期间用户已重新 attach
/// （新 Runtime 已插入且 Connected），旧重连结果必须丢弃——否则会换掉
/// 更新的 Runtime，并丢失其尚未消费的 capture 事件（PaneBuf 空、搜索
/// 不到断线前 token）。
fn handle_reconnect_success(
    state: &Rc<RefCell<UiState>>,
    id: WorkspaceId,
    runtime: std::boxed::Box<dyn Runtime>,
    bell: bool,
) {
    let mut s = state.borrow_mut();
    if let Some(ws) = s.pool.get_mut(&id) {
        if ws.runtime().runtime_status() != BackendStatus::Connected {
            ws.swap_runtime(runtime);
        }
    }
    s.reconnect_attempts = 0;
    s.reconnect_retry_at = None;
    s.disconnect_overlay.set_visible(false);
    if bell {
        let ws = active_workspace_id(&s);
        let pane = s.active_pane;
        let (last_line, seq) = s.active_workspace().pane_last_line_seq(PaneId(pane));
        s.attention.apply(
            &ws,
            pane,
            &[AttentionSignal::AttentionRequest {
                source: AttentionSource::Bel,
            }],
            &last_line,
            seq,
        );
    }
}

/// 打开当前 pane 内查找条（W18f：Ctrl+F 与 test_open_pane_find 共用）。
fn open_pane_find(state: &Rc<RefCell<UiState>>, _window: &Window) {
    let s = state.borrow();
    s.pane_find.set_visible(true);
    s.pane_find_entry.grab_focus();
}

fn open_quick_connect(state: &Rc<RefCell<UiState>>, window: &Window) {
    open_panel(state, window, PanelTab::Workspaces);
}

/// 打开三 tab 面板（initial_tab 由入口决定：Alt+Q → Workspaces，红点 → Attention）。
///
/// 内部自行 borrow：show() 的 rebuild 会同步触发 peek_text（st.borrow()），
/// 调用方不能同时持有 RefMut。
fn open_panel(state: &Rc<RefCell<UiState>>, window: &Window, initial_tab: PanelTab) {
    let (workspaces, attention, theme, font, win, st, ssh_reach) = {
        let mut s = state.borrow_mut();
        let recents = recent_target_configs(&s.pool, 5);
        s.qc_store.replace_recents(&recents);
        let current = s.pool.active().map(workspace_to_target_config);
        let store = s.qc_store.clone();
        let theme = s.theme.clone();
        let font = s.font.clone();
        let win = window.clone();
        let st = state.clone();
        let workspaces = build_root_items(&store, current.as_ref());
        let ssh_reach = collect_ssh_reach(&mut s, &workspaces);
        // C7：本地列出搬后台线程（GTK 线程禁止 ssh / 扫 herdr socket），
        // 结果经 16ms poll 收编，和 SSH probe 同一模式。
        spawn_local_existing_probe(&mut s);
        let ws = active_workspace_id(&s);
        let active_pane = s.active_pane;
        let attention: Vec<PaneAttention> = s
            .attention
            .snapshot()
            .into_iter()
            .flat_map(|w| w.panes)
            .filter(|p| !(p.workspace_id == ws && p.pane_id == active_pane))
            .collect();
        s.panel_open = Some(initial_tab);
        (workspaces, attention, theme, font, win, st, ssh_reach)
    };
    if !window.is_visible() {
        window.present();
    }
    crate::platform::linux::quickconnect_panel::show(
        &win,
        crate::platform::linux::quickconnect_panel::PanelShowArgs {
            initial_tab,
            workspaces,
            attention,
            theme,
            font,
            on_connect: {
                let st = st.clone();
                std::boxed::Box::new(move |cfg| {
                    connect_target(&st, cfg);
                })
            },
            on_existing_connect: {
                let st = st.clone();
                std::boxed::Box::new(move |cfg| {
                    connect_target_with_intent(&st, cfg, ProjectConnectIntent::AttachOnly);
                })
            },
            on_edit: {
                let st = st.clone();
                let win = win.clone();
                std::boxed::Box::new(move |cfg| {
                    open_target_config(&st, &win, Some(cfg));
                })
            },
            on_new_project: {
                let st = st.clone();
                let win = win.clone();
                std::boxed::Box::new(move || {
                    open_target_config(&st, &win, None);
                })
            },
            on_jump_pane: {
                let st = st.clone();
                std::boxed::Box::new(move |ws, pane, seq| {
                    jump_to_attention_pane(&st, &ws, pane, seq);
                })
            },
            on_send_input: {
                let st = st.clone();
                std::boxed::Box::new(move |ws, pane, data| {
                    let mut s = st.borrow_mut();
                    activate_attention_workspace(&mut s, &ws);
                    let _ = s.active_workspace_mut().execute(Task::WriteRaw {
                        target: PaneId(pane),
                        data: data.to_vec(),
                    });
                    s.attention.on_user_input(&ws, pane);
                })
            },
            on_mute: {
                let st = st.clone();
                std::boxed::Box::new(move |ws, pane, duration| {
                    let mut s = st.borrow_mut();
                    s.attention.mute_for(&ws, pane, duration);
                })
            },
            peek_bytes: {
                let st = st.clone();
                std::boxed::Box::new(move |ws, pane| {
                    let s = st.borrow();
                    let wid = s
                        .pool
                        .list()
                        .into_iter()
                        .find(|w| workspace_replica_id(w.id()) == ws);
                    let Some(w) = wid else {
                        return (80, 24, Vec::new());
                    };
                    let (cols, rows) = w
                        .state()
                        .pane(&PaneId(pane))
                        .map(|p| (p.cols, p.rows))
                        .unwrap_or((80, 24));
                    (cols, rows, w.pane_raw_bytes(PaneId(pane)))
                })
            },
            search: {
                let st = st.clone();
                std::boxed::Box::new(move |query, scope| {
                    // C8：空 query 不扫 replica（emulate 已返回空）。
                    if query.trim().is_empty() {
                        return Vec::new();
                    }
                    let s = st.borrow();
                    let hits = match scope {
                        crate::platform::linux::panel_model::SearchScope::Pane => s
                            .active_workspace()
                            .search_pane(PaneId(s.active_pane), query),
                        crate::platform::linux::panel_model::SearchScope::Workspace => {
                            s.active_workspace().search_workspace(query)
                        }
                        crate::platform::linux::panel_model::SearchScope::All => {
                            s.pool.search_all(query)
                        }
                    };
                    hits.into_iter().map(Into::into).collect()
                })
            },
            on_close: {
                let st = st.clone();
                std::boxed::Box::new(move || {
                    st.borrow_mut().panel_open = None;
                })
            },
            ssh_reach,
            existing: state.borrow().existing.clone(),
            on_existing_nav: {
                let st = st.clone();
                std::boxed::Box::new(move |nav| {
                    if nav == ExistingNav::SshHosts {
                        spawn_existing_ssh_probe(&st);
                    }
                })
            },
        },
    );
}

/// 跳到注意力 pane：若目标工作区不是当前前台连接，先切连接；
/// 命中在别的 tab 时先 `SwitchTab` 再 `SwitchPane`（W15b）。
/// `seq` 是搜索命中的 PaneBuf 行号（W17c）：切完后把 VTE 滚到该行并显示高亮。
fn jump_to_attention_pane(state: &Rc<RefCell<UiState>>, ws: &str, pane: u32, seq: u64) {
    let mut s = state.borrow_mut();
    activate_attention_workspace(&mut s, ws);
    // 按 pane 查所在 tab（SearchRow 已带 tab_id，但回调只传 ws/pane；
    // 这里从 core 状态反查，结果必须切 tab）。
    let tab_id = {
        let state = s.active_workspace().state();
        state
            .tabs()
            .iter()
            .find(|t| state.panes(&t.id).iter().any(|p| p.id.0 == pane))
            .map(|t| t.id.0)
    };
    if let Some(tid) = tab_id {
        if tid != s.active_tab {
            request_switch_tab(&mut s, tid);
        }
    }
    // 激活 pane（若已在前台连接中）。
    let _ = s.active_workspace_mut().execute(Task::SwitchPane {
        target: PaneId(pane),
    });
    // 搜索命中：滚到该行并显示客户端高亮（W17c）。
    if seq > 0 {
        let row = s
            .active_workspace()
            .pane_line_index_by_seq(PaneId(pane), seq);
        if let Some(row) = row {
            if let Some(view) = s.active_layout().pane(pane).cloned() {
                if let Some(adj) = view.terminal().vadjustment() {
                    adj.set_value(adj.lower() + row as f64);
                }
                s.search_highlight.set_visible(true);
            }
        }
    }
    // 跳转完成后面板关闭（W15b；独立面板测试不经过这里，面板保持打开）。
    drop(s);
    crate::platform::linux::quickconnect_panel::close_current();
}

/// 目标工作区不是当前前台时切连接；相同则不动（避免无谓的 layout 重建）。
fn activate_attention_workspace(s: &mut UiState, ws: &str) {
    if active_workspace_id(s) == ws {
        return;
    }
    let id = attention_workspace_id(s, ws);
    if let Some(id) = id {
        if s.pool.get(&id).is_some() {
            activate_existing(s, id);
        }
    }
}

/// 按 workspace_id（name@transport）找 WorkspaceId。
fn attention_workspace_id(s: &UiState, ws: &str) -> Option<WorkspaceId> {
    s.pool
        .list()
        .into_iter()
        .map(|w| w.id().clone())
        .find(|id| workspace_replica_id(id) == ws)
}

/// 打开配置页：保存/热加载后重读 config.toml 并应用主题/字体/attention。
fn open_preferences(state: &Rc<RefCell<UiState>>, window: &Window) {
    let Some(path) = Config::user_config_path() else {
        tracing::warn!(target = "muxterm::linux", "无用户配置目录，无法打开配置页");
        return;
    };
    let st = state.clone();
    crate::platform::linux::preferences_window::show(
        window,
        path,
        std::boxed::Box::new(move || {
            let mut s = st.borrow_mut();
            if let Ok(cfg) = Config::load() {
                s.attention.set_config(cfg.attention.clone());
                s.config_font_size = cfg.font.size;
                s.font.size = FontSettings::clamp_size(cfg.font.size);
                s.font.family = cfg.font.family.clone();
                s.theme_name = cfg.theme.name.to_ascii_lowercase();
                if let Ok(t) = Theme::load(&s.theme_name) {
                    s.theme = t.clone();
                    apply_chrome_css(&t);
                    for layout in s.pixel_cache.values_mut() {
                        layout.apply_theme(&t);
                    }
                    s.status.apply_theme(&t);
                }
                s.status_mode = StatusBarMode::from_toml(Some(&cfg.statusbar.mode));
                s.status.set_mode(s.status_mode);
                maybe_refresh_status(&mut s, true);
            }
        }),
    );
}

fn open_target_config(
    state: &Rc<RefCell<UiState>>,
    window: &Window,
    editing: Option<TargetConfig>,
) {
    let store = state.borrow().qc_store.clone();
    let hosts = CoreBridge::discover_ssh_hosts().unwrap_or_default();
    let runtimes = crate::core::catalog::Catalog::with_builtins().runtime_list();
    let st = state.clone();
    let win = window.clone();
    crate::platform::linux::target_config_window::show(
        window,
        editing,
        store,
        hosts,
        runtimes,
        {
            let st = st.clone();
            let win = win.clone();
            move |saved| {
                let mut s = st.borrow_mut();
                s.qc_store.upsert_project(&saved);
                drop(s);
                open_quick_connect(&st, &win);
            }
        },
        {
            let st = st.clone();
            let win = win.clone();
            move || {
                open_quick_connect(&st, &win);
            }
        },
    );
}

/// 重连结果：新 Runtime + 断线期间是否响过 bell（W17a）。
type ReconnectResult = (
    WorkspaceId,
    anyhow::Result<std::boxed::Box<dyn Runtime>>,
    bool,
);

/// worktree 创建对话框：分支 + 路径，Create 后后台建 checkout 并开新格。
fn show_worktree_create_dialog(state: &Rc<RefCell<UiState>>, parent: &gtk4::Window) {
    let dialog = gtk4::Window::builder()
        .title("新建 worktree")
        .modal(true)
        .transient_for(parent)
        .default_width(460)
        .build();
    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    vbox.set_margin_top(12);
    vbox.set_margin_bottom(12);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);
    let branch = gtk4::Entry::builder()
        .placeholder_text("分支名（如 feat/xxx）")
        .build();
    branch.set_widget_name("muxterm-worktree-create-branch");
    let path = gtk4::Entry::builder()
        .placeholder_text("checkout 路径（如 /tmp/muxterm-test-herdr-wt-1）")
        .build();
    path.set_widget_name("muxterm-worktree-create-path");
    let create = gtk4::Button::with_label("创建");
    create.set_widget_name("muxterm-worktree-create-confirm");
    let cancel = gtk4::Button::with_label("取消");
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    row.append(&cancel);
    row.append(&create);
    vbox.append(&branch);
    vbox.append(&path);
    vbox.append(&row);
    dialog.set_child(Some(&vbox));

    let dlg = dialog.clone();
    cancel.connect_clicked(move |_| dlg.close());
    let st = state.clone();
    let dlg = dialog.clone();
    create.connect_clicked(move |_| {
        let branch_text = branch.text().to_string();
        let path_text = path.text().to_string();
        if branch_text.trim().is_empty() || path_text.trim().is_empty() {
            return;
        }
        spawn_worktree_create(
            &st,
            branch_text.trim().to_string(),
            path_text.trim().to_string(),
        );
        dlg.close();
    });
    dialog.present();
}

/// 后台线程：Herdr worktree.create + 新格 connect，结果走队列收编。
fn spawn_worktree_create(state: &Rc<RefCell<UiState>>, branch: String, path: String) {
    let (session, source_ws, session_name, socket) = {
        let s = state.borrow();
        let Some(ws) = s.pool.active() else {
            return;
        };
        let Some(rt) = ws.runtime().as_any().downcast_ref::<HerdrRuntime>() else {
            return;
        };
        (
            rt.session_arc().clone(),
            rt.workspace_id().to_string(),
            rt.session().name().to_string(),
            rt.session().socket_path().to_string_lossy().to_string(),
        )
    };
    let handle = state.borrow().rt.handle().clone();
    let (tx, rx) = std::sync::mpsc::channel::<anyhow::Result<Workspace>>();
    state.borrow_mut().pending_worktree_creates.push_back(rx);
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<Workspace> {
            let record = session.worktree_create(&source_ws, &branch, &path, None, None)?;
            let new_ws = record
                .open_workspace_id
                .ok_or_else(|| anyhow!("worktree.create 未返回 workspace_id"))?;
            let spec = WorkspaceSpec::herdr(session_name, new_ws, socket);
            let id = spec.id();
            let name = spec.name();
            let mut runtime = spec.build_runtime();
            handle.block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(10), runtime.connect())
                    .await
                    .map_err(|_| anyhow!("worktree connect 超时"))?
            })?;
            Ok(Workspace::new(id, name, runtime))
        })();
        let _ = tx.send(result);
    });
}

/// 收编后台 worktree 创建结果：成功 insert_connected，失败进 notification_log。
fn drain_pending_worktree_creates(state: &Rc<RefCell<UiState>>) {
    let mut done = false;
    while !done {
        let pending = {
            let mut s = state.borrow_mut();
            let Some(rx) = s.pending_worktree_creates.front() else {
                break;
            };
            match rx.try_recv() {
                Ok(r) => {
                    s.pending_worktree_creates.pop_front();
                    Some(r)
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    done = true;
                    None
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    s.pending_worktree_creates.pop_front();
                    None
                }
            }
        };
        if let Some(result) = pending {
            match result {
                Ok(workspace) => {
                    let mut s = state.borrow_mut();
                    s.pool.insert_connected(workspace);
                    after_activate(&mut s);
                }
                Err(e) => {
                    let detail = e.to_string();
                    tracing::error!(
                        target = "muxterm::linux",
                        "worktree create failed: {detail}"
                    );
                    state
                        .borrow_mut()
                        .notification_log
                        .push(format!("worktree create failed: {detail}"));
                }
            }
        }
    }
}

/// W20：SSH 已有连接探测结果（alias → 该 host 的 tmux/Herdr 行）。
type ExistingSshProbeResult = Vec<(String, Vec<ExistingEntry>)>;

/// worktree 创建对话框：分支 + 路径，Create 后后台建 checkout 并开新格。
fn show_worktree_create_dialog(state: &Rc<RefCell<UiState>>, parent: &gtk4::Window) {
    let dialog = gtk4::Window::builder()
        .title("新建 worktree")
        .modal(true)
        .transient_for(parent)
        .default_width(460)
        .build();
    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    vbox.set_margin_top(12);
    vbox.set_margin_bottom(12);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);
    let branch = gtk4::Entry::builder()
        .placeholder_text("分支名（如 feat/xxx）")
        .build();
    branch.set_widget_name("muxterm-worktree-create-branch");
    let path = gtk4::Entry::builder()
        .placeholder_text("checkout 路径（如 /tmp/muxterm-test-herdr-wt-1）")
        .build();
    path.set_widget_name("muxterm-worktree-create-path");
    let create = gtk4::Button::with_label("创建");
    create.set_widget_name("muxterm-worktree-create-confirm");
    let cancel = gtk4::Button::with_label("取消");
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    row.append(&cancel);
    row.append(&create);
    vbox.append(&branch);
    vbox.append(&path);
    vbox.append(&row);
    dialog.set_child(Some(&vbox));

    let dlg = dialog.clone();
    cancel.connect_clicked(move |_| dlg.close());
    let st = state.clone();
    let dlg = dialog.clone();
    create.connect_clicked(move |_| {
        let branch_text = branch.text().to_string();
        let path_text = path.text().to_string();
        if branch_text.trim().is_empty() || path_text.trim().is_empty() {
            return;
        }
        spawn_worktree_create(
            &st,
            branch_text.trim().to_string(),
            path_text.trim().to_string(),
        );
        dlg.close();
    });
    dialog.present();
}

/// 后台线程：Herdr worktree.create + 新格 connect，结果走队列收编。
fn spawn_worktree_create(state: &Rc<RefCell<UiState>>, branch: String, path: String) {
    let (session, source_ws, session_name, socket) = {
        let s = state.borrow();
        let Some(ws) = s.pool.active() else {
            return;
        };
        let Some(rt) = ws.runtime().as_any().downcast_ref::<HerdrRuntime>() else {
            return;
        };
        (
            rt.session_arc().clone(),
            rt.workspace_id().to_string(),
            rt.session().name().to_string(),
            rt.session().socket_path().to_string_lossy().to_string(),
        )
    };
    let handle = state.borrow().rt.handle().clone();
    let (tx, rx) = std::sync::mpsc::channel::<anyhow::Result<Workspace>>();
    state.borrow_mut().pending_worktree_creates.push_back(rx);
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<Workspace> {
            let record = session.worktree_create(&source_ws, &branch, &path, None, None)?;
            let new_ws = record
                .open_workspace_id
                .ok_or_else(|| anyhow!("worktree.create 未返回 workspace_id"))?;
            let spec = WorkspaceSpec::herdr(session_name, new_ws, socket);
            let id = spec.id();
            let name = spec.name();
            let mut runtime = spec.build_runtime();
            handle.block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(10), runtime.connect())
                    .await
                    .map_err(|_| anyhow!("worktree connect 超时"))?
            })?;
            Ok(Workspace::new(id, name, runtime))
        })();
        let _ = tx.send(result);
    });
}

/// 收编后台 worktree 创建结果：成功 insert_connected，失败进 notification_log。
fn drain_pending_worktree_creates(state: &Rc<RefCell<UiState>>) {
    let mut done = false;
    while !done {
        let pending = {
            let mut s = state.borrow_mut();
            let Some(rx) = s.pending_worktree_creates.front() else {
                break;
            };
            match rx.try_recv() {
                Ok(r) => {
                    s.pending_worktree_creates.pop_front();
                    Some(r)
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    done = true;
                    None
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    s.pending_worktree_creates.pop_front();
                    None
                }
            }
        };
        if let Some(result) = pending {
            match result {
                Ok(workspace) => {
                    let mut s = state.borrow_mut();
                    s.pool.insert_connected(workspace);
                    after_activate(&mut s);
                }
                Err(e) => {
                    let detail = e.to_string();
                    tracing::error!(
                        target = "muxterm::linux",
                        "worktree create failed: {detail}"
                    );
                    state
                        .borrow_mut()
                        .notification_log
                        .push(format!("worktree create failed: {detail}"));
                }
            }
        }
    }
}

/// 后台连接结果（W15c：open_spec 离开 GTK 线程）。
struct PendingConnect {
    id: WorkspaceId,
    socket: Option<String>,
    flow: ProjectConnectFlow,
    config: TargetConfig,
    existing: bool,
    result: anyhow::Result<Workspace>,
}

/// 在后台线程完成 `open_spec` 的阻塞部分（build runtime + connect），
/// 结果经 channel 回主线程，由 16ms poll / test_poll_once 收编。
fn spawn_background_connect(
    state: &Rc<RefCell<UiState>>,
    spec: WorkspaceSpec,
    id: WorkspaceId,
    socket: Option<String>,
    flow: ProjectConnectFlow,
    config: TargetConfig,
    existing: bool,
) {
    let handle = state.borrow().rt.handle().clone();
    let (tx, rx) = std::sync::mpsc::channel::<PendingConnect>();
    state.borrow_mut().pending_connects.push_back(rx);
    std::thread::spawn(move || {
        let result = connect_workspace_blocking(&spec, &handle);
        let _ = tx.send(PendingConnect {
            id,
            socket,
            flow,
            config,
            existing,
            result,
        });
    });
}

/// 后台线程：构造 Runtime 并 connect（SSH 卡住时只阻塞这个线程）。
fn connect_workspace_blocking(
    spec: &WorkspaceSpec,
    handle: &tokio::runtime::Handle,
) -> anyhow::Result<Workspace> {
    let id = spec.id();
    let name = spec.name();
    let mut runtime = spec.build_runtime();
    // transport 已带 ConnectTimeout=10；这里再兜底硬超时，防止个别路径卡死。
    handle.block_on(async {
        tokio::time::timeout(Duration::from_secs(10), runtime.connect())
            .await
            .map_err(|_| anyhow::anyhow!("connect timed out after 10s"))?
    })?;
    Ok(Workspace::new_with_scrollback(
        id,
        name,
        runtime,
        spec.scrollback_lines as usize,
    ))
}

/// 收编后台连接结果（16ms poll 与 test_poll_once 共用）。
fn drain_pending_connects(state: &Rc<RefCell<UiState>>) {
    let mut done = false;
    while !done {
        let pending = {
            let mut s = state.borrow_mut();
            let Some(rx) = s.pending_connects.front() else {
                break;
            };
            match rx.try_recv() {
                Ok(p) => {
                    s.pending_connects.pop_front();
                    Some(p)
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    done = true;
                    None
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    s.pending_connects.pop_front();
                    None
                }
            }
        };
        if let Some(pending) = pending {
            handle_connect_outcome(state, pending);
        }
    }
}

/// 后台连接完成：成功收编进池并激活；失败写 notification_log + 继续流程。
fn handle_connect_outcome(state: &Rc<RefCell<UiState>>, pending: PendingConnect) {
    let PendingConnect {
        id,
        socket,
        mut flow,
        config,
        existing,
        result,
    } = pending;
    match result {
        Ok(workspace) => {
            let mut s = state.borrow_mut();
            s.pool.insert_connected(workspace);
            s.workspace_sockets.insert(id, socket);
            after_activate(&mut s);
            if existing {
                flow.attach_existing_succeeded();
            } else {
                flow.attach_created_succeeded();
            }
        }
        Err(e) => {
            let detail = e.to_string();
            let name = id.replica_id();
            tracing::error!(target = "muxterm::linux", "connect failed: {detail}");
            // 失败必须进 notification_log（W15c），不能只 tracing::error。
            state
                .borrow_mut()
                .notification_log
                .push(format!("{name}: connect failed: {detail}"));
            if existing {
                flow.attach_existing_failed(&detail);
                step_project_flow(state, config, flow);
            } else {
                flow.attach_created_failed(&detail);
                tracing::error!(
                    target = "muxterm::linux",
                    "attach created session failed: {detail}"
                );
            }
        }
    }
}

fn start_local_shell(state: &Rc<RefCell<UiState>>, config: TargetConfig) {
    let session =
        crate::platform::linux::quickconnect::model::QuickConnect::default_name(&config.path);
    let id = workspace_id_for_config(&config, &session);
    {
        let mut s = state.borrow_mut();
        if s.pool.get(&id).is_some() {
            activate_existing(&mut s, id);
            return;
        }
    }
    let spec = WorkspaceSpec::local_shell(config.path.clone())
        .with_scrollback_lines(state.borrow().scrollback_lines);
    // W15c：连接不在 GTK 线程 block_on；后台线程完成后由 16ms poll 收编。
    spawn_background_connect(
        state,
        spec,
        id,
        None,
        ProjectConnectFlow::new(&config),
        config,
        false,
    );
}

fn run_project_flow(state: &Rc<RefCell<UiState>>, config: TargetConfig) {
    run_project_flow_with_intent(state, config, ProjectConnectIntent::CreateIfMissing);
}

fn run_project_flow_with_intent(
    state: &Rc<RefCell<UiState>>,
    config: TargetConfig,
    intent: ProjectConnectIntent,
) {
    let flow = ProjectConnectFlow::new_with_intent(&config, intent);
    step_project_flow(state, config, flow);
}

fn step_project_flow(
    state: &Rc<RefCell<UiState>>,
    config: TargetConfig,
    mut flow: ProjectConnectFlow,
) {
    match flow.state.clone() {
        ProjectConnectState::AttachExisting { session } => {
            attach_tmux(state, config, session, flow, true);
        }
        ProjectConnectState::CreateDetached { session, directory } => {
            let (transport, target) = config.transport.create_backend();
            match CoreBridge::create_workspace(transport, target, None, &session, &directory) {
                Ok(_) => {
                    flow.create_succeeded();
                    step_project_flow(state, config, flow);
                }
                Err(e) => {
                    flow.create_failed(&e.to_string());
                    tracing::error!(target = "muxterm::linux", "create session failed: {e}");
                }
            }
        }
        ProjectConnectState::AttachCreated { session } => {
            attach_tmux(state, config, session, flow, false);
        }
        ProjectConnectState::Done => {}
        ProjectConnectState::Failed(failure) => {
            tracing::error!(
                target = "muxterm::linux",
                "project connect failed at {:?}: {}",
                failure.stage,
                failure.detail
            );
        }
    }
}

fn attach_tmux(
    state: &Rc<RefCell<UiState>>,
    config: TargetConfig,
    session: String,
    mut flow: ProjectConnectFlow,
    existing: bool,
) {
    let id = workspace_id_for_config(&config, &session);
    {
        let mut s = state.borrow_mut();
        if s.pool.get(&id).is_some() {
            activate_existing(&mut s, id);
            if existing {
                flow.attach_existing_succeeded();
            } else {
                flow.attach_created_succeeded();
            }
            return;
        }
    }
    let (transport, alias) = config.transport.attach_backend();
    let is_ssh = transport == "tmux-ssh";
    let scrollback_lines = state.borrow().scrollback_lines;
    // Existing rows carry the target-side socket.  Prefer it over the
    // process-wide default so an attach never silently falls back to another
    // tmux server (especially an isolated `-L` fixture).
    let socket = config
        .socket
        .clone()
        .or_else(|| state.borrow().default_socket.clone());
    let spec = if is_ssh {
        WorkspaceSpec::ssh_tmux(
            alias.expect("SSH alias 必须存在").to_string(),
            Some(session.clone()),
            socket.clone(),
        )
        .with_scrollback_lines(scrollback_lines)
    } else {
        WorkspaceSpec::local_tmux(Some(session.clone()), socket.clone())
            .with_scrollback_lines(scrollback_lines)
    };
    // W15c：SSH 连接可能卡到 ConnectTimeout，绝不能在 GTK 线程 block_on。
    spawn_background_connect(state, spec, id, socket, flow, config, existing);
}

/// TargetConfig + session → 稳定 WorkspaceId。
fn workspace_id_for_config(config: &TargetConfig, session: &str) -> WorkspaceId {
    let alias = match &config.transport {
        TargetTransport::Ssh { name } => Some(name.as_str()),
        TargetTransport::Local => None,
    };
    let transport = if config.transport.is_ssh() {
        "ssh"
    } else {
        "local"
    };
    WorkspaceId::new(
        transport,
        alias,
        session,
        config.runtime.as_str(),
        &config.path,
    )
}

fn activate_existing(s: &mut UiState, id: WorkspaceId) {
    s.pool.activate(&id);
    after_activate(s);
}

fn after_activate(s: &mut UiState) {
    // 切工作区 = 改绑体现：挂载该工作区的像素缓存（没有则新建）。
    let id = s.active_ws_id().clone();
    if s.mounted_ws.as_ref() != Some(&id) {
        if !s.pixel_cache.contains_key(&id) {
            let uses = s.uses_tmux();
            let layout = LayoutHost::new(s.theme.clone(), s.font.clone(), uses, s.scrollback_lines);
            s.pixel_cache.insert(id.clone(), layout);
        }
        // C8：后台 cache 的字号与当前字号不同才补（不在 Ctrl+= 里遍历全部）。
        let needs_font = s
            .pixel_cache
            .get(&id)
            .map(|l| (l.font_size() - s.font.size).abs() > f32::EPSILON)
            .unwrap_or(false);
        if needs_font {
            let font = s.font.clone();
            s.pixel_cache
                .get_mut(&id)
                .expect("layout 必须存在")
                .set_font(&font);
        }
        let root = s
            .pixel_cache
            .get(&id)
            .expect("layout 必须存在")
            .root_box
            .clone();
        // GtkOverlay 的旧主 child 必须先显式摘下；直接用新 child 覆盖时，
        // 嵌套 GtkStack 在部分 GTK4 版本会先 set_parent(new) 再清旧 parent，
        // 触发 gtk_widget_set_parent critical。
        if s.layout_overlay.child().as_ref() != Some(root.upcast_ref()) {
            s.layout_overlay.set_child(None::<&gtk4::Widget>);
            s.layout_overlay.set_child(Some(&root));
        }
        s.mounted_ws = Some(id);
    }
    s.tab_gate = TabSwitchGate::new(Duration::from_millis(1500));
    s.last_client_size = None;
    s.pending_client_size = None;
    s.pending_client_hits = 0;
    s.qc_store
        .replace_recents(&recent_target_configs(&s.pool, 5));
    refresh_ui(s);
    report_all_pane_colours(s);
    maybe_refresh_status(s, true);
}

fn connect_target(state: &Rc<RefCell<UiState>>, config: TargetConfig) {
    connect_target_with_intent(state, config, ProjectConnectIntent::CreateIfMissing);
}

fn connect_target_with_intent(
    state: &Rc<RefCell<UiState>>,
    config: TargetConfig,
    intent: ProjectConnectIntent,
) {
    match config.runtime {
        TargetRuntime::Tmux => run_project_flow_with_intent(state, config, intent),
        TargetRuntime::Shell => {
            if config.transport.is_ssh() {
                run_project_flow_with_intent(state, config, intent);
            } else {
                start_local_shell(state, config);
            }
        }
        TargetRuntime::Herdr => connect_herdr(state, config, intent),
    }
}

/// Herdr 目标：本地直接 socket JSON；SSH 先转发远端 socket 再 attach。
/// Herdr 目标：统一走 Core Catalog 解析 + 打开（W6 §11.2）。
///
/// Project/Recent/Existing 三路共用；后台线程调用 Catalog API，
/// SSH forward 由 HerdrDriver::open 创建，Project/Recent 永不保存临时
/// forward 路径。意图：新建 Project 才 CreateIfMissing，其余 AttachOnly。
fn connect_herdr(state: &Rc<RefCell<UiState>>, config: TargetConfig, intent: ProjectConnectIntent) {
    // 已打开的同 identity slot：直接激活。
    let probe_id = WorkspaceId::new(
        if config.transport.is_ssh() {
            "ssh"
        } else {
            "local"
        },
        match &config.transport {
            TargetTransport::Ssh { name } => Some(name.as_str()),
            TargetTransport::Local => None,
        },
        config.session.as_deref().unwrap_or("default"),
        "herdr",
        &config.path,
    );
    {
        let mut s = state.borrow_mut();
        if s.pool.get(&probe_id).is_some() {
            activate_existing(&mut s, probe_id);
            return;
        }
    }
    // 初次新建 Project 才 CreateIfMissing；Existing/Recent/普通重连 AttachOnly。
    let resolve_intent = match intent {
        ProjectConnectIntent::AttachOnly => ResolveIntent::AttachOnly,
        ProjectConnectIntent::CreateIfMissing => ResolveIntent::CreateIfMissing,
    };
    let handle = state.borrow().rt.handle().clone();
    let (tx, rx) = std::sync::mpsc::channel::<PendingConnect>();
    state.borrow_mut().pending_connects.push_back(rx);
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<Workspace> {
            let mut catalog = crate::core::catalog::Catalog::with_builtins();
            let mut workspace =
                handle.block_on(catalog.open_target_owned(&config, resolve_intent))?;
            handle.block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(10), workspace.connect())
                    .await
                    .map_err(|_| anyhow::anyhow!("herdr connect 超时"))?
            })?;
            Ok(workspace)
        })();
        let id = result.as_ref().map(|w| w.id().clone()).unwrap_or(probe_id);
        let _ = tx.send(PendingConnect {
            id,
            socket: None,
            // Herdr 的 CreateIfMissing 已在 Catalog resolver 中完成；如果
            // Runtime connect 之后失败，绝不能落入 tmux Project fallback。
            flow: ProjectConnectFlow::new_with_intent(&config, ProjectConnectIntent::AttachOnly),
            config,
            existing: true,
            result,
        });
    });
}

/// 池里最近打开的工作区（按 last_used 倒序）→ QuickConnect 目标。
///
/// W6 §11.2：优先读 Core 保存的 `ResolvedTarget.canonical`（含 session /
/// socket / workspace_id）；没有 descriptor 时回退旧五段推导（测试/直开）。
fn recent_target_configs(pool: &WorkspacePool, limit: usize) -> Vec<TargetConfig> {
    pool.list()
        .into_iter()
        .take(limit)
        .map(workspace_to_target_config)
        .collect()
}

/// Workspace → QuickConnect 目标（Recents 列表 / 面板高亮）。
///
/// 读 `resolved_target().canonical`（Catalog 打开时保存）；无 descriptor 时
/// 从 WorkspaceId 推导（测试 mock/CLI 直开路径）。
fn workspace_to_target_config(workspace: &Workspace) -> TargetConfig {
    if let Some(resolved) = workspace.resolved_target() {
        return resolved.canonical.clone();
    }
    let id = workspace.id();
    let name = if id.session.is_empty() {
        QuickConnect::default_name(&id.path)
    } else {
        id.session.clone()
    };
    let runtime = TargetRuntime::from_str(&id.runtime).unwrap_or(TargetRuntime::Tmux);
    let transport = if id.transport == "ssh" {
        if let Some(alias) = &id.alias {
            TargetTransport::Ssh {
                name: alias.clone(),
            }
        } else {
            TargetTransport::Local
        }
    } else {
        TargetTransport::Local
    };
    TargetConfig::new(name, runtime, transport, id.path.clone())
}

fn open_tmux_attach(state: &Rc<RefCell<UiState>>, parent: &Window, _create_only: bool) {
    let socket = state
        .borrow()
        .workspace_sockets
        .get(state.borrow().active_ws_id())
        .cloned()
        .flatten();
    let socket_opt = socket.clone();
    let st = state.clone();
    tmux_dialog::show(parent, socket_opt.as_deref(), move |action| match action {
        TmuxAction::Attach { session } => {
            connect_target(
                &st,
                TargetConfig::tmux_session(session, TargetTransport::Local),
            );
        }
        TmuxAction::NewWorkspace { name } => {
            let session = name.unwrap_or_else(|| "muxterm".into());
            let dir = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            match CoreBridge::create_workspace("local", None, socket.as_deref(), &session, &dir) {
                Ok(created) => connect_target(
                    &st,
                    TargetConfig::tmux_session(created, TargetTransport::Local),
                ),
                Err(e) => tracing::error!(target = "muxterm::linux", "create tmux session: {e}"),
            }
        }
    });
}

fn open_ssh_connect(state: &Rc<RefCell<UiState>>, parent: &Window) {
    let hosts = match CoreBridge::discover_ssh_hosts() {
        Ok(h) if !h.is_empty() => h,
        Ok(_) => {
            tracing::error!(
                target = "muxterm::linux",
                "{}",
                i18n::tr(Key::ErrorNoSshHosts)
            );
            return;
        }
        Err(e) => {
            tracing::error!(target = "muxterm::linux", "SSH host discovery failed: {e}");
            return;
        }
    };
    let items = tmux_dialog::connect_pick_items(&hosts);
    let st = state.clone();
    let win = parent.clone();
    crate::platform::linux::quick_pick::show(
        parent,
        &i18n::tr(Key::ChooseSshHost),
        items,
        move |picked| {
            let Some(item) = picked else {
                return;
            };
            open_connect_sessions(&st, &win, item.id);
        },
    );
}

/// C9：命令面板第二层 = 该 connect 的 runtime list（local 或 SSH alias）。
fn open_connect_sessions(state: &Rc<RefCell<UiState>>, parent: &Window, connect: String) {
    let (transport, target) = if connect == "local" {
        ("local", "")
    } else {
        ("ssh", connect.as_str())
    };
    let sessions =
        CoreBridge::discover_workspaces(transport, Some(target), None).unwrap_or_default();
    let items = tmux_dialog::connect_session_pick_items(&sessions, &connect);
    let st = state.clone();
    let win = parent.clone();
    let connect_for_attach = connect.clone();
    crate::platform::linux::quick_pick::show(
        parent,
        &i18n::tr(Key::ChooseWorkspace),
        items,
        move |picked| {
            let Some(item) = picked else {
                return;
            };
            if tmux_dialog::is_create_session_id(&item.id) {
                let connect = connect_for_attach.clone();
                let st = st.clone();
                crate::platform::linux::pane_switcher::show_rename(&win, "muxterm", move |name| {
                    let dir = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                    let transport = if connect == "local" { "local" } else { "ssh" };
                    let target = if connect == "local" {
                        None
                    } else {
                        Some(connect.as_str())
                    };
                    match CoreBridge::create_workspace(transport, target, None, &name, &dir) {
                        Ok(created) => {
                            let cfg = if connect == "local" {
                                TargetConfig::tmux_session(created, TargetTransport::Local)
                            } else {
                                TargetConfig::tmux_session(
                                    created,
                                    TargetTransport::Ssh {
                                        name: connect.clone(),
                                    },
                                )
                            };
                            connect_target(&st, cfg);
                        }
                        Err(e) => tracing::error!(
                            target = "muxterm::linux",
                            "create remote tmux session: {e}"
                        ),
                    }
                });
            } else {
                let cfg = if connect_for_attach == "local" {
                    TargetConfig::tmux_session(item.id, TargetTransport::Local)
                } else {
                    TargetConfig::tmux_session(
                        item.id,
                        TargetTransport::Ssh {
                            name: connect_for_attach.clone(),
                        },
                    )
                };
                connect_target(&st, cfg);
            }
        },
    );
}

/// 窗口铬（根背景 / tab / status）随主题变化；badge 色保持品牌色。
pub(crate) fn chrome_css(theme: &Theme) -> String {
    let bg = format!(
        "#{:02x}{:02x}{:02x}",
        theme.background.0, theme.background.1, theme.background.2
    );
    let fg = format!(
        "#{:02x}{:02x}{:02x}",
        theme.foreground.0, theme.foreground.1, theme.foreground.2
    );
    format!(
        "
        .muxterm-root {{ background: {bg}; }}
        .tab-bar {{ background: {bg}; }}
        button.tab-button {{
            background-image: none;
            background-color: transparent;
            border: none;
            box-shadow: none;
            min-height: 18px;
            min-width: 0;
            padding: 2px 10px;
            border-radius: 0;
            color: {fg};
            font-weight: 400;
            opacity: 0.55;
        }}
        button.tab-button.tab-active {{
            opacity: 1;
            font-weight: 600;
            background-color: alpha({fg}, 0.12);
            box-shadow: inset 0 -2px 0 {fg};
        }}
        .status-bar {{ color: {fg}; padding: 2px 8px; font-size: 11px; }}
        button.muxterm-status-window {{
            background-image: none;
            background-color: transparent;
            border: none;
            box-shadow: none;
            min-height: 18px;
            padding: 1px 8px;
            border-radius: 3px;
            color: {fg};
            font-weight: 400;
            opacity: 0.6;
        }}
        button.muxterm-status-window.current {{
            opacity: 1;
            font-weight: 600;
            background-color: alpha({fg}, 0.16);
            box-shadow: inset 0 -2px 0 {fg};
        }}
        .qc-badge {{ padding: 0 6px; border-radius: 4px; font-size: 9px; color: #fff; }}
        .qc-badge-recent {{ background: #1e66f5; }}
        .qc-badge-project {{ background: #40a02b; }}
        .qc-badge-current {{ background: #df8e1d; }}
        .qc-current {{ background: alpha(#89b4fa, 0.18); }}
        "
    )
}

fn apply_chrome_css(theme: &Theme) {
    thread_local! {
        static PROVIDER: RefCell<Option<CssProvider>> = const { RefCell::new(None) };
    }
    PROVIDER.with(|slot| {
        let mut slot = slot.borrow_mut();
        let css = slot.get_or_insert_with(|| {
            let provider = CssProvider::new();
            if let Some(display) = gdk::Display::default() {
                gtk4::style_context_add_provider_for_display(
                    &display,
                    &provider,
                    gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
                );
            }
            provider
        });
        css.load_from_data(&chrome_css(theme));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_status_snapshot_reports_pane_count_and_close_hint() {
        // 本地模式中区也画 FFI tab（唯一 status bar）。
        let snap = local_status_snapshot(3, &[(7, "shell".into(), true)]);
        assert!(snap.enabled);
        assert_eq!(snap.justify, "left");
        assert!(snap.left.contains('3'), "left 应含 pane 数: {}", snap.left);
        assert!(!snap.right.is_empty(), "right 应含关闭提示");
        assert_eq!(snap.windows.len(), 1, "本地模式中区应画 FFI tab");
        assert_eq!(snap.windows[0].window_id, 7);
        assert_eq!(snap.interval, 1);
    }

    #[test]
    fn close_intent_maps_quit_and_hide() {
        assert_eq!(close_intent(true), CloseIntent::Quit);
        assert_eq!(close_intent(false), CloseIntent::HideKeepPolling);
    }

    #[test]
    fn should_poll_status_respects_subscription() {
        let last = Instant::now() - Duration::from_secs(5);
        let now = Instant::now();
        // 订阅生效 → 不轮询（值变化走 %subscription-changed 推送）
        assert!(!should_poll_status(true, last, now, Duration::from_secs(1)));
        // 无订阅且到间隔 → 轮询
        assert!(should_poll_status(false, last, now, Duration::from_secs(1)));
        // 无订阅但未到间隔 → 不轮询
        assert!(!should_poll_status(false, now, now, Duration::from_secs(1)));
    }

    #[test]
    fn surface_seed_gate_requires_realized_positive_allocation() {
        assert!(surface_allocation_is_seedable(true, 1, 1));
        assert!(!surface_allocation_is_seedable(false, 80, 24));
        assert!(!surface_allocation_is_seedable(true, 0, 24));
        assert!(!surface_allocation_is_seedable(true, 80, 0));
        assert!(!surface_allocation_is_seedable(true, -1, 24));
        assert!(!surface_allocation_is_seedable(true, 80, -1));
    }

    #[test]
    fn attention_updates_are_runtime_neutral_state_changes() {
        assert_eq!(
            attention_event_pane(&StateChange::PaneOutput {
                pane: PaneId(7),
                data: vec![b'x'],
            }),
            Some(7)
        );
        assert_eq!(
            attention_event_pane(&StateChange::PaneAgentChanged {
                pane: PaneId(9),
                agent: None,
                initial: false,
            }),
            Some(9)
        );
        assert_eq!(
            attention_event_pane(&StateChange::PoolChanged),
            None,
            "platform 只消费通用 StateChange，不按 Runtime 名称分支"
        );
    }

    #[test]
    fn chrome_css_follows_light_and_dark_background() {
        let light = Theme::load("light").unwrap();
        let dark = Theme::load("dark").unwrap();
        let light_css = chrome_css(&light);
        let dark_css = chrome_css(&dark);
        assert!(light_css.contains("#eff1f5"), "{light_css}");
        assert!(dark_css.contains("#1e1e2e"), "{dark_css}");
        assert!(light_css.contains("tab-active"), "{light_css}");
        assert!(
            light_css.contains("muxterm-status-window.current"),
            "{light_css}"
        );
        assert_ne!(light_css, dark_css);
    }

    /// W4：同一批 PaneAdded + LayoutChanged + PaneResized + PaneFrame +
    /// PaneHistory + PaneOutput 时，顺序必须是结构 → baseline → output
    ///（各阶段内部保持原序）。
    #[test]
    fn batch_order_plan_puts_structure_before_frames_before_output() {
        let ev = |kind: &str| match kind {
            "pane_added" => StateChange::PaneAdded {
                pane: PaneId(1),
                tab: TabId(1),
            },
            "layout" => StateChange::LayoutChanged {
                tab: TabId(1),
                layout: crate::core::model::layout::TabLayout {
                    tab: TabId(1),
                    tree: crate::core::model::layout::LayoutNode::leaf(PaneId(1)),
                    active: PaneId(1),
                },
            },
            "resized" => StateChange::PaneResized {
                pane: PaneId(1),
                cols: 80,
                rows: 24,
            },
            "frame" => StateChange::PaneFrame {
                pane: PaneId(1),
                data: b"F".to_vec(),
            },
            "snapshot" => StateChange::PaneSnapshot {
                pane: PaneId(1),
                data: b"S".to_vec(),
            },
            "history" => StateChange::PaneHistory {
                pane: PaneId(1),
                data: b"H".to_vec(),
            },
            "output" => StateChange::PaneOutput {
                pane: PaneId(1),
                data: b"O".to_vec(),
            },
            "tab_added" => StateChange::TabAdded { tab: TabId(1) },
            _ => panic!("unknown kind {kind}"),
        };

        // 输入顺序故意交错：frame、output 夹在结构事件之间。
        let kinds = [
            "pane_added",
            "frame",
            "layout",
            "output",
            "resized",
            "snapshot",
            "history",
            "output",
            "tab_added",
        ];
        let events: Vec<StateChange> = kinds.iter().map(|k| ev(k)).collect();
        let (structure, baseline, output) = batch_order_plan(&events);
        let order: Vec<String> = structure
            .iter()
            .chain(baseline.iter())
            .chain(output.iter())
            .map(|i| kinds[*i].to_string())
            .collect();
        assert_eq!(
            order,
            vec![
                "pane_added",
                "layout",
                "resized",
                "tab_added", // 结构
                "frame",
                "snapshot",
                "history", // baseline
                "output",
                "output", // diff
            ],
            "结构必须整体先于 frame/snapshot，再先于 output"
        );

        // 无结构事件：保持原始顺序（全部在 structure 列表，原序）。
        let plain = ["frame", "history", "output", "frame", "output"];
        let events2: Vec<StateChange> = plain.iter().map(|k| ev(k)).collect();
        let (s2, b2, o2) = batch_order_plan(&events2);
        assert!(b2.is_empty() && o2.is_empty(), "无结构事件不得重排");
        let order2: Vec<&str> = s2.iter().map(|i| plain[*i]).collect();
        assert_eq!(order2, plain, "无结构批次保持原始顺序");
    }

    #[test]
    fn structural_only_batch_still_requires_one_topology_commit() {
        let mut effects = UiBatchEffects::default();
        effects.note_topology();
        assert!(effects.topology_changed);
        effects.note_topology();
        assert!(
            effects.topology_changed,
            "多个 structural event 只能留下一个 coalesced effect"
        );
    }

    #[test]
    fn surface_input_queue_preserves_fifo_and_owner_identity() {
        let queue = Rc::new(RefCell::new(VecDeque::new()));
        let first = WorkspaceId::new("local", None, "first", "tmux", "");
        let second = WorkspaceId::new("local", None, "second", "herdr", "");
        queue.borrow_mut().push_back(SurfaceInput {
            workspace: first.clone(),
            pane: PaneId(7),
            data: b"one".to_vec(),
        });
        queue.borrow_mut().push_back(SurfaceInput {
            workspace: second.clone(),
            pane: PaneId(3),
            data: b"two".to_vec(),
        });
        let drained = take_surface_input(&queue);
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].workspace, first);
        assert_eq!(drained[0].pane, PaneId(7));
        assert_eq!(drained[0].data, b"one");
        assert_eq!(drained[1].workspace, second);
        assert_eq!(drained[1].pane, PaneId(3));
        assert_eq!(drained[1].data, b"two");
        assert!(queue.borrow().is_empty());
    }

    fn fn_src<'a>(src: &'a str, name: &str) -> &'a str {
        let sig = format!("fn {name}(");
        let start = src.find(&sig).unwrap_or_else(|| panic!("missing {sig}"));
        let rest = &src[start..];
        let after = &rest[sig.len()..];
        let mut rel = after.len();
        for pat in ["\nfn ", "\npub fn "] {
            if let Some(i) = after.find(pat) {
                rel = rel.min(i);
            }
        }
        &rest[..sig.len() + rel]
    }

    /// C7：SSH 已有连接探测必须并发，禁止一个 spawn 里串行 map 每个 alias。
    #[test]
    fn spawn_existing_ssh_probe_must_fan_out() {
        let src = include_str!("window.rs");
        let body = fn_src(src, "spawn_existing_ssh_probe");
        let spawns = body.matches("thread::spawn").count() + body.matches("thread::scope").count();
        assert!(
            body.contains("chunks(") || spawns >= 2,
            "spawn_existing_ssh_probe 必须 4 路并发（chunks / scope / 每 host spawn），禁止串行 discover_sessions。body={body}"
        );
    }

    /// C9：已有的连接必须先出 local 行，SSH host 再 4 路并发。
    /// 禁止只调一次 `discover_sessions("all")` 再一次性 send（慢 host 会冻 Loading）。
    #[test]
    fn spawn_local_existing_probe_must_stream_local_then_parallel_ssh() {
        let src = include_str!("window.rs");
        let body = fn_src(src, "spawn_local_existing_probe");
        let local_at = body
            .find("discover_sessions(\"local\"")
            .expect("必须先 discover_sessions(\"local\")");
        let ssh_at = body
            .find("discover_sessions(\"ssh\"")
            .expect("SSH 侧必须 discover_sessions(\"ssh\", alias)");
        assert!(
            local_at < ssh_at,
            "local 必须排在 ssh 扇出之前。body={body}"
        );
        let send_at = body.find("tx.send").expect("必须 send 探测结果");
        assert!(
            send_at < ssh_at,
            "必须先把 local 行 send 再扇出 SSH。body={body}"
        );
        let spawns = body.matches("thread::spawn").count() + body.matches("thread::scope").count();
        assert!(
            body.contains("chunks(") || spawns >= 2,
            "SSH host 必须 4 路并发（chunks / scope）。body={body}"
        );
        assert!(
            !body.contains("discover_sessions(\"all\""),
            "面板探测禁止等 discover_sessions(\"all\") 整表；FFI/Catalog 的 all 仍并行扇出。body={body}"
        );
    }

    /// C9 回归：后台只负责算结果，生产 16ms poll 必须把 channel 收进面板。
    /// GTK e2e 还会只驱动 GLib 主循环，禁止靠 `test_poll_once` 掩盖漏接线。
    #[test]
    fn production_poll_must_drain_existing_probe_results() {
        let src = include_str!("window.rs");
        let start = src
            .find("let id = glib::timeout_add_local")
            .expect("应有生产 16ms poll");
        let rest = &src[start..];
        let end = rest
            .find("state.borrow_mut().poll_source = Some(id)")
            .expect("应保存生产 poll SourceId");
        let body = &rest[..end];
        assert!(
            body.contains("drain_local_existing(&st)"),
            "生产 16ms poll 必须收编已有连接探测结果，测试钩子收编不算。body={body}"
        );
    }

    /// C7：打开面板禁止在调用线程同步 discover_sessions（会冻 GTK）。
    #[test]
    fn open_panel_must_not_discover_sessions_on_caller() {
        let src = include_str!("window.rs");
        let body = fn_src(src, "open_panel");
        assert!(
            !body.contains("discover_sessions"),
            "open_panel 禁止同步 Catalog::discover_sessions；本地列出搬后台线程。body={body}"
        );
    }

    /// C8：字号热路径禁止同步写 config.toml。
    #[test]
    fn adjust_font_must_not_persist_config_synchronously() {
        let src = include_str!("window.rs");
        let body = fn_src(src, "adjust_font");
        assert!(
            !body.contains("persist_config"),
            "adjust_font 禁止同步 persist_config；防抖或后台写盘。body={body}"
        );
    }
}
