//! 主窗口：FFI 驱动的 GTK4 前端。
//!
//! - 通过共享 `FfiClient` 使用公共 FFI
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
use gtk4::Window;

use crate::frontend::command_queue::CommandQueue;
use crate::frontend::event_pump::EventPump;
use crate::frontend::ffi_client::{
    ClientAttentionStatus, ClientConfig, ClientConfigSnapshot, ClientEventKind, ClientKeyBinding,
    ClientLayout, ClientRuntimeCapability, ClientRuntimeInfo, ClientTask, ClientWorkspaceEvent,
    FfiClient,
};
use crate::frontend::linux::app_shell::{AppShell, HeaderActions};
use crate::frontend::linux::attention_compat::CompatibilityActivity;
use crate::frontend::linux::attention_ui::{GioSink, NotificationSink};
use crate::frontend::linux::command_palette::{parse_palette_action, PaletteAction};
#[cfg(test)]
use crate::frontend::linux::event_batch::batch_order_plan;
use crate::frontend::linux::keymap::{default_keybindings, Action, KeyMap};
use crate::frontend::linux::layout_host::LayoutHost;
use crate::frontend::linux::lifecycle::{should_close_window, OnLastPaneExit};
use crate::frontend::linux::overlay::OverlayLayer;
use crate::frontend::linux::pane_view::{PaneMenuAction, PaneSurface};
use crate::frontend::linux::panel_model::PanelTab;
use crate::frontend::linux::quickconnect::existing::ExistingEntry;
use crate::frontend::linux::quickconnect::font::FontSettings;
use crate::frontend::linux::quickconnect::status_style::StatusBarMode;
use crate::frontend::linux::quickconnect::store::QuickConnectStore;
use crate::frontend::linux::quickconnect_panel::{
    build_root_items, build_search_items, ExistingNav, ExistingPanelState,
};
use crate::frontend::linux::status_bar::StatusBar;
#[cfg(test)]
use crate::frontend::linux::theme::Rgb;
use crate::frontend::linux::theme::{fallback_theme, toggle_target, Theme};
use crate::frontend::linux::view_store::ViewStore;
use crate::frontend::linux::window_input::{connect_close_handler, connect_key_handler};
use crate::frontend::linux::workspace_scenes::WorkspaceScenes;
use crate::frontend::linux::workspace_sidebar::{AgentSidebarItem, WorkspaceSidebar};
use crate::frontend::ssh_probe::SshReach;
#[cfg(test)]
use muxterm_protocol::state::StateChange;
use muxterm_protocol::task::TaskOutcome;
use muxterm_protocol::PaneId;
#[cfg(test)]
use muxterm_protocol::TabId;
use muxterm_protocol::WorkspaceId;

#[path = "window_actions.rs"]
mod window_actions;
#[path = "window_activity.rs"]
mod window_activity;
#[path = "window_appearance.rs"]
mod window_appearance;
#[path = "window_bootstrap.rs"]
mod window_bootstrap;
#[path = "window_chrome.rs"]
mod window_chrome;
#[path = "window_config.rs"]
mod window_config;
#[path = "window_connection.rs"]
mod window_connection;
#[path = "window_discovery.rs"]
mod window_discovery;
#[path = "window_event_pump.rs"]
mod window_event_pump;
#[path = "window_layout.rs"]
mod window_layout;
#[path = "window_overlay.rs"]
mod window_overlay;
#[path = "window_render.rs"]
mod window_render;
#[path = "window_resize.rs"]
mod window_resize;
#[path = "window_scene.rs"]
mod window_scene;
#[path = "window_sidebar.rs"]
mod window_sidebar;
#[path = "window_state.rs"]
mod window_state;
#[path = "window_status.rs"]
mod window_status;
#[path = "window_surface.rs"]
mod window_surface;
#[path = "window_test_api.rs"]
mod window_test_api;
#[path = "window_worktree.rs"]
mod window_worktree;

/// 通过统一 FFI client 的 Core 配置事务写回 config.toml（唯一事实源）。
/// 平台禁止直接解析或写 TOML；失败只记日志，不覆盖用户文件。
pub fn persist_config(client: &FfiClient, dotted: &str, value: serde_json::Value) {
    window_actions::persist_config(client, dotted, value);
}

/// 主窗口。
pub struct AppWindow {
    pub window: Window,
    /// 保持 UI 状态与 Core 连接状态存活（轮询闭包只用 Weak，避免循环引用）。
    _state: Rc<RefCell<UiState>>,
}

struct UiState {
    /// 唯一 Core owner：生产 GTK 不再直接持有 WorkspacePool。
    event_pump: EventPump,
    /// UI → Core 的唯一命令出口；由 GTK poll owner 批量 flush。
    command_queue: RefCell<CommandQueue>,
    /// 每个 workspace 的常驻 LayoutHost 与 GTK Scene 由同一个 owner 管理。
    scenes: WorkspaceScenes,
    /// 前端拥有的 workspace topology/render 快照；Core 不持有其引用。
    view_store: ViewStore,
    /// 前端当前可见的 workspace；不等同于 Core snapshot 的 active 标记。
    visible_workspace: WorkspaceId,
    /// 启动时读取的 provider 能力快照；切场景时不得再查询 Core。
    runtime_info: Vec<ClientRuntimeInfo>,
    /// 当前挂载到窗口的 LayoutHost 对应的工作区。
    mounted_ws: Option<WorkspaceId>,
    /// 本轮结构事件触发 refresh_ui 后，已经从 core snapshot seed 的 pane。
    /// 对应的 PaneSnapshot 事件只需作为通知消费一次，不能再次 reset/feed。
    snapshot_seeded_this_batch: HashSet<u32>,
    qc_store: QuickConnectStore,
    poll_source: Option<glib::SourceId>,
    /// 当前终端字体（config + 运行期偏好）。
    font: FontSettings,
    /// config.toml 的字号，Reset 回到这里。
    config_font_size: f32,
    theme: Theme,
    theme_name: String,
    status: StatusBar,
    /// 标题栏开启的 workspace 侧栏（主分割栏左列）。
    sidebar: WorkspaceSidebar,
    /// 标题栏的快速连接与设置入口。
    header: HeaderActions,
    status_mode: StatusBarMode,
    last_status_at: Instant,
    status_interval: Duration,
    keymap: KeyMap,
    /// 每个 workspace 当前由 frontend 显示的 tab；不等同于 Core 的 active 标记。
    visible_tabs: HashMap<String, u32>,
    /// 用户点击 tab 后的 frontend override；Core 的迟到 active 事件不能覆盖它。
    local_tab_overrides: HashSet<String>,
    active_tab: u32,
    active_pane: u32,
    /// 最近一次同步给后端/PTY 的尺寸。
    /// - SharedClientResize（tmux）：`(None, cols, rows)` 整窗 client size
    /// - 其它 Runtime（shell / Herdr）：`(Some(pane), cols, rows)` 按 pane 跳过，
    ///   避免切 tab 后同像素尺寸被全局缓存吞掉 ResizePane（htop 0826）。
    last_client_size: Option<(Option<u32>, u16, u16)>,
    /// Herdr/shell：每个可见 pane 自己的 GTK 分配。0218.log 只 resize
    /// active pane，分屏里另外两个格子一直停在 snapshot 27×23。
    last_pane_sizes: HashMap<u32, (u16, u16)>,
    /// 切回已有像素缓存后，短暂忽略 ResizePane。Overlay remount 第一帧
    /// 可能把 VTE 量成几行，Herdr Control Hello 会 SIGWINCH 清掉 token。
    hold_pane_resize_until: Option<Instant>,
    /// tmux SharedClientResize：同一尺寸连续命中才 dispatch（约 10×16ms），
    /// 避免 map 时 106→284→142 连发 -C（dogfood 2152）。
    pending_client_size: Option<(u16, u16)>,
    pending_client_hits: u8,
    on_last_pane_exit: OnLastPaneExit,
    /// 事件分发里不能同步 `window.close()`（可能正握着 RefCell）。
    pending_close: bool,
    /// GTK 测试注入的兼容 activity 状态；生产 activity 始终来自 FFI。
    compatibility_activity: CompatibilityActivity,
    /// 本轮进入 blocked 的 workspace 通知日志（测试钩子读取）。
    notification_log: Vec<String>,
    /// 通知出口（生产 GioSink fail-soft；测试可替换）。
    notification_sink: std::boxed::Box<dyn NotificationSink>,
    /// 面板是否打开及当前 tab（测试钩子 + badge 点击入口）。
    panel_open: Option<PanelTab>,
    /// 用户显式 Quit（Ctrl+Q / 命令面板）：close_request 放行真正关闭。
    quit_requested: bool,
    /// WorkspacePool 的软提醒阈值；实际 Workspace 所有权在 Core FFI。
    capacity_limit: usize,
    /// 已针对该 slot 数量显示过一次容量提醒；用户选择保留后不在每个
    /// poll 重复打断，数量变化（新建或关闭）后才重新评估。
    capacity_warning_presented_for_slot_count: Option<usize>,
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
    /// 终端区 Overlay：常驻 workspace scene stack 是主 child，覆盖层浮在上面。
    overlay: OverlayLayer,
    /// 上次看到这里（W18g）：(workspace, pane) → 离开时的最后一行文本。
    last_seen: std::collections::HashMap<(String, u32), String>,
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

/// Decode the stable five-segment FFI workspace identity for the remaining
/// GTK compatibility keys.  The last segment is kept intact because paths
/// may contain `/`.
fn parse_workspace_id(value: &str) -> Option<WorkspaceId> {
    let mut parts = value.splitn(5, '/');
    let transport = parts.next()?;
    let alias = parts.next()?;
    let session = parts.next()?;
    let runtime = parts.next()?;
    let path = parts.next().unwrap_or_default();
    if transport.is_empty() || runtime.is_empty() {
        return None;
    }
    Some(WorkspaceId::new(
        transport,
        (!alias.is_empty()).then_some(alias),
        session,
        runtime,
        path,
    ))
}

fn decode_client_config<T: serde::Serialize>(config: T) -> ClientConfig {
    serde_json::from_value(serde_json::to_value(config).expect("frontend config must serialize"))
        .unwrap_or_default()
}

impl AppWindow {
    /// 有序关闭：停轮询 → 摘掉子树 → destroy 窗口，避免与 PaneView 持有的 VTE 交叉销毁。
    pub fn shutdown(self) {
        crate::frontend::linux::quickconnect_panel::clear_panel_hooks();
        {
            let mut s = self._state.borrow_mut();
            if let Some(id) = s.poll_source.take() {
                id.remove();
            }
            let _ = s.event_pump.client().shutdown();
            // 显式释放全部 LayoutHost/PaneView/VTE：GTK 对象必须在本窗口
            // destroy 前解构，否则 VTE 的 GL 资源残留到下一个测试窗口
            // realize 时才 finalize，与新的 GL 初始化交叉 = 堆损坏
            // （linux_herdr_agent_e2e 连续多测试时可见 double free）。
            s.scenes.shutdown();
            // Popover 挂在状态点按钮上：先解除父子关系，避免 dot 销毁时
            // popover 仍引用它（finalize-with-children 堆损坏）。
            s.status.popover_widget().unparent();
        }
        self.window.set_child(None::<&gtk4::Widget>);
        self.window.destroy();
        // 让 GTK 在窗口销毁后继续跑完 pending finalize，避免跨测试残留。
        while glib::MainContext::default().iteration(false) {}
    }

    pub fn new<T: serde::Serialize>(cfg: T, theme: Theme) -> Self {
        let cfg = decode_client_config(cfg);
        let keybindings = if cfg.keybindings.is_empty() {
            default_keybindings()
        } else {
            cfg.keybindings.clone()
        };
        Self::new_with_keybindings(cfg, theme, keybindings)
    }

    /// Construct the window with effective bindings resolved by Core.
    pub fn new_with_effective_keybindings(
        cfg: ClientConfig,
        theme: Theme,
        keybindings: &[ClientKeyBinding],
    ) -> Self {
        Self::new_with_keybindings(cfg, theme, keybindings.to_vec())
    }
}

/// 当前前台连接的 workspace id（ReplicaStore 键）。
fn active_workspace_id(s: &UiState) -> String {
    workspace_replica_id(&s.active_ws_id())
}

/// Current Core workspace identity used at FFI boundaries.
fn active_workspace_key(s: &UiState) -> String {
    s.active_workspace_key()
}

/// WorkspaceId → ReplicaStore 键（`name@transport`，与 QuickConnect 一致）。
fn workspace_replica_id(id: &WorkspaceId) -> String {
    id.replica_id()
}

/// 按 `(WorkspaceId, PaneId)` 找常驻 PaneView（hidden tab / background
/// workspace 也必须有）；找不到说明 topology 阶段没建好，属于 lifecycle
/// failure，不能静默丢帧。
fn resident_pane_view(
    s: &UiState,
    wid: &WorkspaceId,
    pane: u32,
) -> Option<std::rc::Rc<crate::frontend::linux::pane_view::PaneSurface>> {
    s.scenes
        .get(wid)
        .and_then(|layout| layout.pane(pane).cloned())
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

/// 已有的连接探测增量：先推 local 行，SSH 完成后再推；Done 才清 inflight。
enum ExistingProbeMsg {
    Aliases(Vec<String>),
    Rows(Vec<ExistingEntry>),
    Done,
}

/// W20：SSH 已有连接探测结果（alias → 该 host 的 tmux/Herdr 行）。
type ExistingSshProbeResult = Vec<(String, Vec<ExistingEntry>)>;

#[cfg(test)]
mod tests {
    use super::window_activity::{attention_event_pane, UiBatchEffects};
    use super::window_chrome::chrome_css;
    use super::window_event_pump::take_surface_input;
    use super::window_overlay::workspace_replica_matches;
    use super::window_resize::pending_pane_resizes;
    use super::window_status::local_status_snapshot;
    use super::window_surface::surface_allocation_is_seedable;
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
    fn existing_workspace_activation_is_scene_only() {
        let src = include_str!("window_scene.rs");
        let activation = fn_src(src, "activate_existing");
        assert!(
            activation.contains("show_workspace_scene(s, id, false)"),
            "{activation}"
        );
        assert_scene_navigation_body_is_local(activation);

        let scene = fn_src(src, "show_workspace_scene");
        assert_scene_navigation_body_is_local(scene);
        assert!(!scene.contains("seed_unseeded_pane_for"), "{scene}");
    }

    #[test]
    fn existing_tab_activation_is_scene_only() {
        let src = include_str!("window_scene.rs");
        let activation = fn_src(src, "request_switch_tab");
        assert!(
            activation.contains("show_tab_scene(s, tab_id)"),
            "{activation}"
        );
        assert_scene_navigation_body_is_local(activation);

        let scene = fn_src(src, "show_tab_scene");
        assert!(scene.contains("layout.show_tab(tab_id)"), "{scene}");
        assert_scene_navigation_body_is_local(scene);
    }

    #[test]
    fn workspace_replica_matching_accepts_path_and_session_aliases() {
        let id = WorkspaceId::new("local", None, "session", "tmux", "/worktree");
        assert!(workspace_replica_matches(&id, "session:/worktree@local"));
        assert!(workspace_replica_matches(&id, "session@local"));
        assert!(!workspace_replica_matches(&id, "other@local"));
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
    fn pending_pane_resizes_covers_every_visible_split_leaf() {
        let mut last = HashMap::new();
        last.insert(50, (27, 23));
        let measured = [(50, 120, 40), (54, 120, 20), (57, 120, 19)];
        let pending = pending_pane_resizes(&last, &measured);
        assert_eq!(
            pending,
            vec![(50, 120, 40), (54, 120, 20), (57, 120, 19)],
            "active 以外的 split 格子也必须发 ResizePane"
        );
        last.insert(50, (120, 40));
        last.insert(54, (120, 20));
        last.insert(57, (120, 19));
        assert!(
            pending_pane_resizes(&last, &measured).is_empty(),
            "尺寸未变不得重复 resize"
        );
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
        let light = test_theme("light", Rgb(0xef, 0xf1, 0xf5));
        let dark = test_theme("dark", Rgb(0x1e, 0x1e, 0x2e));
        let light_css = chrome_css(&light);
        let dark_css = chrome_css(&dark);
        assert!(light_css.contains("#eff1f5"), "{light_css}");
        assert!(dark_css.contains("#1e1e2e"), "{dark_css}");
        assert!(light_css.contains("tab-active"), "{light_css}");
        assert!(
            light_css.contains("muxterm-status-window.tab-active"),
            "{light_css}"
        );
        assert!(light_css.contains("inset 0 -3px 0"), "{light_css}");
        assert!(light_css.contains(".quick-pick-root"), "{light_css}");
        assert!(
            light_css.contains(".muxterm-sidebar-workspace-row:hover .muxterm-sidebar-close"),
            "{light_css}"
        );
        assert!(
            light_css.contains("background-color: #eff1f5"),
            "{light_css}"
        );
        assert!(light_css.contains("box-shadow: 0 18px 44px"), "{light_css}");
        assert_ne!(light_css, dark_css);
    }

    fn test_theme(name: &str, background: Rgb) -> Theme {
        Theme {
            name: name.into(),
            background,
            foreground: Rgb(0, 0, 0),
            cursor: Rgb(0, 0, 0),
            colors: [Rgb(0, 0, 0); 16],
        }
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
                layout: muxterm_protocol::layout::TabLayout {
                    tab: TabId(1),
                    tree: muxterm_protocol::layout::LayoutNode::leaf(PaneId(1)),
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
        for pat in ["\nfn ", "\npub fn ", "\npub(super) fn "] {
            if let Some(i) = after.find(pat) {
                rel = rel.min(i);
            }
        }
        &rest[..sig.len() + rel]
    }

    fn assert_scene_navigation_body_is_local(body: &str) {
        for forbidden in [
            "event_pump",
            "command_queue",
            "ClientTask",
            "execute_active_task",
            "activate_workspace",
            "poll_event_store",
            "sync_view_store",
            "flush_command_queue",
            "Mutex",
            ".lock(",
        ] {
            assert!(!body.contains(forbidden), "{forbidden} in {body}");
        }
    }

    /// C7：SSH 已有连接探测必须并发，禁止一个 spawn 里串行 map 每个 alias。
    #[test]
    fn spawn_existing_ssh_probe_must_fan_out() {
        let src = include_str!("window_discovery.rs");
        let body = fn_src(src, "spawn_existing_ssh_probe");
        let spawns = body.matches("thread::spawn").count() + body.matches("thread::scope").count();
        assert!(
            body.contains("chunks(") || spawns >= 2,
            "spawn_existing_ssh_probe 必须 4 路并发（chunks / scope / 每 host spawn），禁止串行 Existing discovery。body={body}"
        );
    }

    /// C9：已有的连接必须先出 local 行，SSH host 再 4 路并发。
    /// 禁止只调一次 `discover_existing("all")` 再一次性 send（慢 host 会冻 Loading）。
    #[test]
    fn spawn_local_existing_probe_must_stream_local_then_parallel_ssh() {
        let src = include_str!("window_discovery.rs");
        let body = fn_src(src, "spawn_local_existing_probe");
        let local_at = body
            .find("discover_existing(\"local\"")
            .expect("必须先 discover_existing(\"local\")");
        let ssh_at = body
            .find("discover_existing(\"ssh\"")
            .expect("SSH 侧必须 discover_existing(\"ssh\", alias)");
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
            !body.contains("discover_existing(\"all\""),
            "面板探测禁止等 discover_existing(\"all\") 整表；FFI/Catalog 的 all 仍并行扇出。body={body}"
        );
    }

    /// C9 回归：后台只负责算结果，生产 16ms poll 必须把 channel 收进面板。
    /// GTK e2e 还会只驱动 GLib 主循环，禁止靠 `test_poll_once` 掩盖漏接线。
    #[test]
    fn production_poll_must_drain_existing_probe_results() {
        let src = include_str!("window_bootstrap.rs");
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

    /// C7：打开面板禁止在调用线程同步 discover_existing（会冻 GTK）。
    #[test]
    fn open_panel_must_not_probe_existing_on_caller() {
        let src = include_str!("window_overlay.rs");
        let body = fn_src(src, "open_panel");
        assert!(
            !body.contains("discover_existing"),
            "open_panel 禁止同步 Existing discovery；本地列出搬后台线程。body={body}"
        );
    }

    /// C8：字号热路径禁止同步写 config.toml。
    #[test]
    fn adjust_font_must_not_persist_config_synchronously() {
        let src = include_str!("window_actions.rs");
        let body = fn_src(src, "adjust_font");
        assert!(
            !body.contains("persist_config"),
            "adjust_font 禁止同步 persist_config；防抖或后台写盘。body={body}"
        );
    }
}
