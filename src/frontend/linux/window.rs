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
use gtk4::{ApplicationWindow, Box, Button, CheckButton, Label, Orientation, Window};

use crate::frontend::command_queue::{ClientCommand, CommandQueue};
use crate::frontend::event_pump::EventPump;
use crate::frontend::ffi_client::{
    ClientActivitySnapshot, ClientAttentionPane, ClientAttentionStatus, ClientConfig,
    ClientConfigSnapshot, ClientEventKind, ClientKeyBinding, ClientLayout, ClientOpenIntent,
    ClientOpenRequest, ClientRuntimeCapability, ClientRuntimeInfo, ClientTarget, ClientTask,
    ClientWorkspaceAttention, ClientWorkspaceEvent, FfiClient,
};
use crate::frontend::i18n::{self, Key};
use crate::frontend::linux::app_shell::{AppShell, HeaderActions};
use crate::frontend::linux::attention_compat::CompatibilityActivity;
use crate::frontend::linux::attention_ui::{GioSink, NotificationSink};
use crate::frontend::linux::command_palette::{parse_palette_action, PaletteAction};
#[cfg(test)]
use crate::frontend::linux::event_batch::batch_order_plan;
use crate::frontend::linux::keymap::{default_keybindings, Action, KeyMap};
use crate::frontend::linux::layout_host::LayoutHost;
use crate::frontend::linux::lifecycle::{cycle_pane_id, should_close_window, OnLastPaneExit};
use crate::frontend::linux::overlay::OverlayLayer;
use crate::frontend::linux::pane_view::{PaneMenuAction, PaneSurface};
use crate::frontend::linux::panel_model::PanelTab;
use crate::frontend::linux::quickconnect::existing::{ExistingEntry, ExistingTransport};
use crate::frontend::linux::quickconnect::font::FontSettings;
use crate::frontend::linux::quickconnect::model::{TargetConfig, TargetTransport};
use crate::frontend::linux::quickconnect::project_flow::ProjectConnectIntent;
use crate::frontend::linux::quickconnect::status_style::{StatusBarMode, StatusBarSnapshot};
use crate::frontend::linux::quickconnect::store::QuickConnectStore;
use crate::frontend::linux::quickconnect_panel::{
    build_root_items, build_search_items, ExistingNav, ExistingPanelState, PanelItem,
};
use crate::frontend::linux::status_bar::StatusBar;
#[cfg(test)]
use crate::frontend::linux::theme::Rgb;
use crate::frontend::linux::theme::{fallback_theme, toggle_target, Theme};
use crate::frontend::linux::view_store::ViewStore;
use crate::frontend::linux::window_input::{connect_close_handler, connect_key_handler};
use crate::frontend::linux::workspace_scenes::WorkspaceScenes;
use crate::frontend::linux::workspace_sidebar::{AgentSidebarItem, WorkspaceSidebar};
use crate::frontend::ssh_probe::{classify_ssh_probe, ssh_probe_args, SshReach};
#[cfg(test)]
use muxterm_protocol::state::StateChange;
use muxterm_protocol::task::TaskOutcome;
use muxterm_protocol::PaneId;
#[cfg(test)]
use muxterm_protocol::TabId;
use muxterm_protocol::WorkspaceId;

#[path = "window_appearance.rs"]
mod window_appearance;
#[path = "window_chrome.rs"]
mod window_chrome;
#[path = "window_config.rs"]
mod window_config;
#[path = "window_connection.rs"]
mod window_connection;
#[path = "window_discovery.rs"]
mod window_discovery;
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
#[path = "window_status.rs"]
mod window_status;
#[path = "window_surface.rs"]
mod window_surface;
#[path = "window_worktree.rs"]
mod window_worktree;

/// 主窗口。
pub struct AppWindow {
    pub window: Window,
    /// 保持 UI 状态与 Core 连接状态存活（轮询闭包只用 Weak，避免循环引用）。
    _state: Rc<RefCell<UiState>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkspaceCapacityCandidate {
    id: WorkspaceId,
    name: String,
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

impl UiState {
    fn active_ws_id(&self) -> WorkspaceId {
        self.visible_workspace.clone()
    }

    fn active_workspace_key(&self) -> String {
        self.visible_workspace.as_str()
    }

    fn active_layout(&self) -> &LayoutHost {
        let id = self.active_ws_id();
        self.scenes
            .get(&id)
            .expect("active workspace 必须有 layout")
    }

    fn active_layout_mut(&mut self) -> &mut LayoutHost {
        let id = self.active_ws_id().clone();
        self.scenes
            .get_mut(&id)
            .expect("active workspace 必须有 layout")
    }

    /// Return the tab currently shown by the frontend for one workspace.
    ///
    /// Core's `ClientTab::is_active` remains the fallback for startup and for
    /// mutations that Core performs itself.  Once the frontend has shown a
    /// tab, its choice wins until that tab disappears or Core reports a new
    /// active-tab mutation.
    fn visible_tab_id(&self, workspace_key: &str) -> Option<u32> {
        let view = self.view_store.workspace(workspace_key)?;
        self.visible_tabs
            .get(workspace_key)
            .copied()
            .filter(|tab_id| view.tabs.iter().any(|tab| tab.id == *tab_id))
            .or_else(|| view.tabs.iter().find(|tab| tab.is_active).map(|tab| tab.id))
            .or_else(|| view.tabs.first().map(|tab| tab.id))
    }

    fn active_tab_id(&self) -> u32 {
        let workspace_key = self.active_workspace_key();
        self.visible_tab_id(&workspace_key)
            .unwrap_or(self.active_tab)
    }

    /// 当前前台是否 tmux/SSH 控制 client（local shell 不支持 detach）。
    fn uses_tmux(&self) -> bool {
        self.active_workspace_runtime()
            .is_some_and(|runtime| matches!(runtime, "tmux" | "ssh" | "tmux-ssh"))
    }

    fn active_workspace_runtime(&self) -> Option<&str> {
        let id = self.active_workspace_key();
        self.view_store
            .workspace(&id)
            .and_then(|view| view.workspace.as_ref())
            .map(|workspace| workspace.runtime.as_str())
    }

    fn active_supports(&self, capability: ClientRuntimeCapability) -> bool {
        let workspace_id = self.active_workspace_key();
        self.workspace_supports(&workspace_id, capability)
    }

    fn workspace_supports(&self, workspace_id: &str, capability: ClientRuntimeCapability) -> bool {
        let Some(runtime) = self
            .view_store
            .workspace(workspace_id)
            .and_then(|view| view.workspace.as_ref())
            .map(|workspace| workspace.runtime.as_str())
        else {
            return false;
        };
        self.runtime_info
            .iter()
            .find(|provider| provider.id == runtime)
            .is_some_and(|provider| provider.supports(capability))
    }

    fn execute_active_task(&self, task: ClientTask) -> anyhow::Result<()> {
        let workspace_id = self.active_workspace_key();
        if self.view_store.workspace(&workspace_id).is_none() {
            anyhow::bail!("没有激活的 workspace");
        }
        self.command_queue.borrow_mut().push(ClientCommand::Task {
            workspace_id: Some(workspace_id),
            task,
        });
        Ok(())
    }
}

/// Flush commands only from the GTK event-loop owner.
fn flush_command_queue(s: &UiState) -> Vec<i32> {
    let client = s.event_pump.client();
    let results = s.command_queue.borrow_mut().flush(client);
    for code in results.iter().copied().filter(|code| *code != 0) {
        tracing::warn!(
            target = "muxterm::linux",
            code,
            "Core command queue dispatch failed"
        );
    }
    results
}

fn enqueue_workspace_input(
    s: &UiState,
    workspace_id: &str,
    pane_id: u32,
    data: &[u8],
    quiet: bool,
) {
    s.command_queue.borrow_mut().push(ClientCommand::Input {
        workspace_id: Some(workspace_id.to_owned()),
        pane_id,
        data: data.to_vec(),
        quiet,
    });
}

fn enqueue_workspace_resize(
    s: &UiState,
    workspace_id: &str,
    pane_id: Option<u32>,
    cols: u16,
    rows: u16,
) {
    s.command_queue.borrow_mut().push(ClientCommand::Resize {
        workspace_id: Some(workspace_id.to_owned()),
        pane_id,
        cols,
        rows,
    });
}

fn poll_event_store(s: &mut UiState) -> Vec<ClientWorkspaceEvent> {
    let event_pump = &s.event_pump;
    let events = event_pump.poll_into_with_events(&mut s.view_store);
    poll_config_events(s);
    events
}

/// Drain Core configuration events through EventPump and hot-apply committed
/// or reloaded frontend settings. Preview events stay owned by the settings
/// overlay until its transaction commits.
fn poll_config_events(s: &mut UiState) {
    let changed = s
        .event_pump
        .poll_config_events()
        .iter()
        .any(|event| event.changes_values());
    if !changed {
        return;
    }
    match s.event_pump.client().config_describe() {
        Ok(snapshot) => apply_config_snapshot(s, snapshot),
        Err(error) => tracing::warn!(
            target = "muxterm::config",
            %error,
            "读取配置变更快照失败，跳过热应用"
        ),
    }
}

fn sync_view_store(s: &mut UiState) -> anyhow::Result<usize> {
    let (event_pump, view_store) = (&s.event_pump, &mut s.view_store);
    event_pump.sync_view_store(view_store)
}

/// Read Core-owned activity state and merge the small compatibility fixture
/// used by GTK integration hooks. Production attention state is owned by
/// Core; the local engine only has entries when a test deliberately injects
/// bytes or an authoritative status through an AppWindow test hook.
fn activity_snapshot(s: &UiState) -> ClientActivitySnapshot {
    let mut snapshot = s
        .event_pump
        .client()
        .activity_snapshot()
        .unwrap_or_default();
    let core_blocked_workspace_ids: HashSet<String> = snapshot
        .workspaces
        .iter()
        .filter(|workspace| workspace.blocked > 0)
        .map(|workspace| workspace.workspace_id.clone())
        .collect();
    let mut compatibility_workspace_ids = HashSet::new();

    for workspace in s.compatibility_activity.snapshot() {
        let mut has_compatibility_state = false;
        let target = snapshot
            .workspaces
            .iter_mut()
            .find(|item| item.workspace_id == workspace.workspace_id);
        let target = if let Some(target) = target {
            target
        } else {
            snapshot.workspaces.push(ClientWorkspaceAttention {
                workspace_id: workspace.workspace_id.clone(),
                path: String::new(),
                blocked: 0,
                done: 0,
                working: 0,
                panes: Vec::new(),
            });
            snapshot
                .workspaces
                .last_mut()
                .expect("刚插入的 activity workspace 必须存在")
        };
        for pane in workspace.panes {
            let pane_has_compatibility_state = pane.status != "unknown"
                || !pane.last_line.is_empty()
                || pane.seq != 0
                || pane.process_name.is_some()
                || pane.process_is_agent
                || pane.agent_name.is_some()
                || pane.shell_name.is_some();
            if !pane_has_compatibility_state {
                continue;
            }
            has_compatibility_state = true;
            if let Some(existing) = target
                .panes
                .iter_mut()
                .find(|item| item.pane_id == pane.pane_id)
            {
                *existing = pane;
            } else {
                target.panes.push(pane);
            }
        }
        if has_compatibility_state {
            compatibility_workspace_ids.insert(workspace.workspace_id.clone());
        }
    }

    for workspace in &mut snapshot.workspaces {
        if !compatibility_workspace_ids.contains(&workspace.workspace_id) {
            continue;
        }
        workspace.blocked = workspace
            .panes
            .iter()
            .filter(|pane| pane.status == "blocked" && !pane.acknowledged)
            .count();
        workspace.done = workspace
            .panes
            .iter()
            .filter(|pane| pane.status == "done" && !pane.acknowledged)
            .count();
        workspace.working = workspace
            .panes
            .iter()
            .filter(|pane| pane.status == "working")
            .count();
    }
    let compatibility_blocked_count = snapshot
        .workspaces
        .iter()
        .filter(|workspace| {
            compatibility_workspace_ids.contains(&workspace.workspace_id)
                && workspace.blocked > 0
                && !core_blocked_workspace_ids.contains(&workspace.workspace_id)
        })
        .count();
    snapshot.blocked_count = snapshot
        .blocked_count
        .saturating_add(compatibility_blocked_count);
    snapshot
}

fn panel_attention_rows(snapshot: &ClientActivitySnapshot) -> Vec<ClientAttentionPane> {
    snapshot
        .workspaces
        .iter()
        .flat_map(|workspace| workspace.panes.iter())
        .cloned()
        .collect()
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

    fn new_with_keybindings(
        cfg: ClientConfig,
        theme: Theme,
        keybindings: Vec<ClientKeyBinding>,
    ) -> Self {
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
        // Core FFI handle 是唯一 live WorkspacePool owner；GTK 只消费 owned DTO。
        let client = if requested_tmux {
            match FfiClient::new_connect("tmux", socket.as_deref(), session.as_deref(), None, None)
            {
                Ok(client) => client,
                Err(error) => {
                    tracing::error!(target = "muxterm::linux", "启动 tmux Core 失败: {error}");
                    FfiClient::new_connect("local", None, None, None, Some(""))
                        .expect("local runtime 必须可用")
                }
            }
        } else {
            FfiClient::new_connect("local", None, None, None, Some(""))
                .expect("local runtime 必须可用")
        };
        let attention_config = cfg.attention.clone();
        if let Err(error) = client.configure_attention(&attention_config) {
            tracing::warn!(
                target = "muxterm::linux",
                %error,
                "应用当前窗口的 Core attention 配置失败"
            );
        }
        let event_pump = EventPump::new(client);
        let mut view_store = ViewStore::default();
        event_pump
            .sync_view_store(&mut view_store)
            .expect("Core workspace snapshot 必须可用");
        let runtime_info = event_pump.client().runtime_list().unwrap_or_else(|error| {
            tracing::warn!(
                target = "muxterm::linux",
                %error,
                "读取 runtime capability snapshot 失败"
            );
            Vec::new()
        });
        let projects = match event_pump.client().config_describe() {
            Ok(snapshot) => match serde_json::from_value(snapshot.values["projects"].clone()) {
                Ok(projects) => projects,
                Err(error) => {
                    tracing::warn!(
                        target = "muxterm::config",
                        "从 Core FFI 快照读取 Project 失败: {error}"
                    );
                    Vec::new()
                }
            },
            Err(error) => {
                tracing::warn!(
                    target = "muxterm::config",
                    "通过 Core FFI 读取 Project 失败: {error}"
                );
                Vec::new()
            }
        };
        let startup_key = view_store
            .active_workspace_id()
            .or_else(|| view_store.workspace_ids().next())
            .expect("Core 必须返回 startup workspace");
        let startup_id = parse_workspace_id(startup_key)
            .expect("Core workspace id 必须保持五段格式")
            .clone();
        let mut startup_sockets = std::collections::HashMap::new();
        if requested_tmux {
            // 启动 attach 的工作区也要登记 socket（W17a 重连要用）。
            startup_sockets.insert(startup_id.clone(), socket.clone());
        }

        let theme_name = cfg.theme.name.clone().to_ascii_lowercase();
        apply_chrome_css(&theme);
        let config_font_size = cfg.font.size;
        let font = FontSettings {
            family: cfg.font.family.clone(),
            size: cfg.font.size,
            fallback: cfg.font.fallback.clone(),
        };
        let status_mode = StatusBarMode::from_toml(Some(&cfg.statusbar.mode));
        let uses_tmux = view_store
            .workspace(startup_key)
            .and_then(|view| view.workspace.as_ref())
            .is_some_and(|workspace| {
                matches!(workspace.runtime.as_str(), "tmux" | "ssh" | "tmux-ssh")
            });
        let layout = LayoutHost::new(theme.clone(), font.clone(), uses_tmux, cfg.scrollback.lines);
        let scenes = WorkspaceScenes::new(startup_id.clone(), layout);
        let scene_stack_widget = scenes.widget();
        let AppShell {
            sidebar,
            status,
            overlay,
            header,
        } = AppShell::new(&window, &scene_stack_widget, status_mode, theme.clone());

        let keymap = KeyMap::from_bindings(&keybindings);
        let qc_store = QuickConnectStore::from_project_documents(&projects);
        let state = Rc::new(RefCell::new(UiState {
            event_pump,
            command_queue: RefCell::new(CommandQueue::default()),
            scenes,
            view_store,
            visible_workspace: startup_id.clone(),
            runtime_info,
            mounted_ws: Some(startup_id.clone()),
            snapshot_seeded_this_batch: HashSet::new(),
            qc_store,
            poll_source: None,
            font,
            config_font_size,
            theme,
            theme_name,
            status,
            sidebar,
            header,
            status_mode,
            last_status_at: Instant::now()
                .checked_sub(Duration::from_secs(10))
                .unwrap_or_else(Instant::now),
            status_interval: Duration::from_secs(1),
            keymap,
            visible_tabs: HashMap::new(),
            local_tab_overrides: HashSet::new(),
            active_tab: 0,
            active_pane: 0,
            last_client_size: None,
            last_pane_sizes: HashMap::new(),
            hold_pane_resize_until: None,
            pending_client_size: None,
            pending_client_hits: 0,
            on_last_pane_exit: cfg.behavior.on_last_pane_exit,
            pending_close: false,
            compatibility_activity: CompatibilityActivity::new(cfg.attention.clone()),
            notification_log: Vec::new(),
            notification_sink: std::boxed::Box::new(GioSink::new(None)),
            panel_open: None,
            quit_requested: false,
            capacity_limit: cfg.pool.max_slots.max(1) as usize,
            capacity_warning_presented_for_slot_count: None,
            runtime_status: crate::ffi::types::BACKEND_STATUS_CONNECTED,
            status_left: None,
            status_right: None,
            workspace_sockets: startup_sockets,
            last_traffic: None,
            last_traffic_at: None,
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
            overlay,
            last_seen: std::collections::HashMap::new(),
            scrollback_lines: cfg.scrollback.lines,
            default_socket: socket.clone(),
            self_weak: std::rc::Weak::new(),
        }));
        state.borrow_mut().self_weak = Rc::downgrade(&state);

        {
            let st = state.clone();
            let mut s = state.borrow_mut();
            if let Some(layout) = s.scenes.get_mut(&startup_id) {
                layout.set_menu_callback(move |pane_id, action| {
                    handle_pane_menu_action(&st, pane_id, action);
                });
            }
        }

        {
            let quick_state = state.clone();
            let settings_state = state.clone();
            let quick_window = window.clone();
            let settings_window = window.clone();
            let s = state.borrow();
            s.header.connect_actions(
                move || open_quick_connect(&quick_state, &quick_window),
                move || open_preferences(&settings_state, &settings_window),
            );
        }

        {
            let activate_state = state.clone();
            let close_state = state.clone();
            let agent_state = state.clone();
            let command_state = state.clone();
            let toggle_state = state.clone();
            let s = state.borrow();
            s.sidebar.connect_actions(
                move |id| {
                    activate_existing(&mut activate_state.borrow_mut(), id.clone());
                },
                move |id| {
                    close_sidebar_workspace(&mut close_state.borrow_mut(), id);
                },
                move |id, pane| {
                    activate_sidebar_activity(&mut agent_state.borrow_mut(), id, pane);
                },
                move |id, pane| {
                    activate_sidebar_activity(&mut command_state.borrow_mut(), id, pane);
                },
                move |is_active| {
                    if is_active {
                        refresh_sidebar_if_open(&mut toggle_state.borrow_mut());
                    }
                },
            );
        }

        {
            let s = state.borrow();
            let activity = activity_snapshot(&s);
            let active_workspace = s.active_workspace_key();
            s.sidebar
                .refresh_from_views(&s.view_store, Some(&active_workspace), &activity);
        }

        // status bar 业务入口 → 交给 frontend-visible scene / Core command queue。
        {
            let tab_state = state.clone();
            let attention_state = state.clone();
            let new_tab_state = state.clone();
            let worktree_state = state.clone();
            let attention_window = window.clone();
            let worktree_window = window.clone();
            let s = state.borrow();
            s.status.connect_actions(
                move |tab_id| {
                    request_switch_tab(&mut tab_state.borrow_mut(), tab_id);
                },
                move || {
                    let n = activity_snapshot(&attention_state.borrow()).blocked_count;
                    let tab = if n > 0 {
                        PanelTab::Attention
                    } else {
                        PanelTab::Workspaces
                    };
                    open_panel(&attention_state, &attention_window, tab);
                },
                move || {
                    let mut s = new_tab_state.borrow_mut();
                    prepare_core_tab_mutation(&mut s, &ClientTask::NewTab);
                    let _ = s.execute_active_task(ClientTask::NewTab);
                },
                move || {
                    show_worktree_create_dialog(&worktree_state, &worktree_window);
                },
            );
        }

        // 命令刻度点击：滚到对应命令文本所在行（W18h）。
        {
            let ok_state = state.clone();
            let fail_state = state.clone();
            let last_seen_state = state.clone();
            let find_state = state.clone();
            let jump_state = state.clone();
            let ok_text = state.borrow().overlay.command_ok_text.clone();
            let fail_text = state.borrow().overlay.command_fail_text.clone();
            let s = state.borrow();
            s.overlay.connect_actions(
                move || scroll_to_command_text(&ok_state, &ok_text),
                move || scroll_to_command_text(&fail_state, &fail_text),
                move || {
                    let s = last_seen_state.borrow();
                    let ws = active_workspace_id(&s);
                    let pane = s.active_pane;
                    if let Some(text) = s.last_seen.get(&(ws.clone(), pane)).cloned() {
                        let lines = s
                            .event_pump
                            .client()
                            .workspace_pane_last_n_lines(&ws, pane, 10_000)
                            .unwrap_or_default();
                        if let Some(row) = lines.iter().position(|l| l.contains(&text)) {
                            if let Some(view) = s.active_layout().pane(pane).cloned() {
                                if let Some(adj) = view.terminal().vadjustment() {
                                    adj.set_value(adj.lower() + row as f64);
                                }
                            }
                        }
                    }
                    s.overlay.last_seen.set_visible(false);
                },
                move |query| {
                    if query.is_empty() {
                        return;
                    }
                    let s = find_state.borrow();
                    let pane = s.active_pane;
                    let workspace_replica = active_workspace_id(&s);
                    let workspace_key = active_workspace_key(&s);
                    let hit = s
                        .event_pump
                        .client()
                        .search_all(query)
                        .ok()
                        .and_then(|hits| {
                            hits.into_iter().find(|hit| {
                                hit.workspace_id == workspace_replica && hit.pane_id == pane
                            })
                        });
                    if let Some(hit) = hit {
                        if let Some(row) = s.event_pump.client().workspace_pane_viewport_for_seq(
                            &workspace_key,
                            pane,
                            hit.seq,
                        ) {
                            if let Some(view) = s.active_layout().pane(pane).cloned() {
                                if let Some(adj) = view.terminal().vadjustment() {
                                    adj.set_value(adj.lower() + row as f64);
                                }
                            }
                        }
                    }
                },
                move || {
                    let mut s = jump_state.borrow_mut();
                    s.overlay.jump_unseen = 0;
                    if let Some(view) = s.active_layout().pane(s.active_pane).cloned() {
                        if let Some(adj) = view.terminal().vadjustment() {
                            adj.set_value(adj.upper());
                        }
                    }
                },
            );
        }

        // 快捷键
        {
            let st = state.clone();
            let window_for_palette = window.clone();
            connect_key_handler(&window, move |keyval, mods| {
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
        }

        // 关闭窗口：非 Quit 动作隐藏并保持 16ms 轮询；Quit 才真正关闭。
        // try_borrow：命令面板 Detach 可能仍握着 RefMut 时同步 close（dogfood 0826）。
        {
            let st = state.clone();
            let win = window.clone();
            connect_close_handler(&window, move || {
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
            let _ = poll_event_store(&mut s);
            refresh_ui(&mut s);
            mark_active_attention_visible(&s);
            report_all_pane_colours(&mut s);
            maybe_refresh_status(&mut s, true);
        }

        {
            let st_weak = Rc::downgrade(&state);
            let win_weak = window.downgrade();
            let id = glib::timeout_add_local(Duration::from_millis(16), move || {
                // W19e：glib trampoline 不能 unwind；panic 先在这里接住，
                // 报告 + 弹窗后继续轮询（Break 会让轮询停掉 = 假死）。
                let outcome = crate::frontend::linux::fault_gtk::run("linux.poll", || {
                    if win_weak.upgrade().is_none() {
                        return glib::ControlFlow::Break;
                    }
                    let Some(st) = st_weak.upgrade() else {
                        return glib::ControlFlow::Break;
                    };
                    let pending_close = {
                        drain_ssh_probes(&st);
                        drain_local_existing(&st);
                        maybe_schedule_reconnect(&st);
                        if let Some(w) = win_weak.upgrade() {
                            maybe_warn_workspace_capacity(&st, &w);
                        }
                        let mut s = st.borrow_mut();
                        // EventPump 是唯一事件消费者：Core 的 workspace 批次先写入
                        // owned ViewStore，再由常驻 Scene 消费 render mailbox。
                        let events = poll_event_store(&mut s);
                        let structural = events.iter().any(|event| event.event.is_topology());
                        refresh_event_workspaces(&mut s, &events);
                        // blocked 与 done 通知都要在 16ms poll 里收编（W17d）：
                        // test_poll_once 的 drain 可能在 16ms poll 应用信号之前运行，
                        // 只 drain blocked 会让后台 Done 的通知永远等不到下一次 poll。
                        drain_attention_notifications(&mut s);
                        sync_pane_outputs(&mut s);
                        sync_window_size(&mut s);
                        // 输入必须在本轮 topology/snapshot/geometry 收编之后
                        // 写入，避免 attach 新 pane 尚未完成首帧时丢掉 send-keys。
                        drain_surface_input(&mut s);
                        flush_command_queue(&s);
                        maybe_refresh_status(&mut s, structural);
                        refresh_connection_summary(&mut s);
                        update_command_marks(&s);
                        update_jump_latest(&s);
                        refresh_sidebar_if_open(&mut s);
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

    /// 测试用：把字节直接喂进 PaneView（含 reply_state / OSC 52）。
    pub fn test_feed_pane_view(&self, pane: u32, bytes: &[u8]) {
        let s = self._state.borrow();
        if let Some(view) = s.active_layout().pane(pane).cloned() {
            view.feed_output(bytes);
            view.flush_deferred_feed();
        }
    }

    /// 测试用：当前激活 pane 走生产粘贴路径。
    pub fn test_paste_active(&self) {
        let state = self._state.clone();
        let s = state.borrow();
        paste_active_pane(&s, &state);
    }

    /// 测试用：向当前激活 pane 发送原始输入（如 `echo hi\n` / `\x04` Ctrl+D）。
    pub fn test_send_input(&self, data: &[u8]) {
        let mut s = self._state.borrow_mut();
        let ws = active_workspace_id(&s);
        let pane = s.active_pane;
        let workspace_key = active_workspace_key(&s);
        enqueue_workspace_input(&s, &workspace_key, pane, data, false);
        s.compatibility_activity.on_user_input(&ws, pane);
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
            .active_workspace_runtime()
            .unwrap_or_default()
            .to_string()
    }

    /// 测试用：能力判断必须走 Runtime 契约，不能按 runtime 名字分支。
    pub fn test_active_runtime_supports(&self, capability: ClientRuntimeCapability) -> bool {
        let s = self._state.borrow();
        s.active_supports(capability)
    }

    /// 测试用：对当前 Runtime 执行真实 detach，并保留精确 outcome。
    pub fn test_detach_active_workspace_outcome(&self) -> anyhow::Result<TaskOutcome> {
        let s = self._state.borrow();
        if !s.active_supports(ClientRuntimeCapability::PersistDetach) {
            return Ok(TaskOutcome::Rejected {
                reason: "active Runtime does not support PersistDetach".into(),
            });
        }
        match s.execute_active_task(ClientTask::Detach) {
            Ok(()) => {
                let result = flush_command_queue(&s)
                    .into_iter()
                    .find(|code| *code != 0)
                    .map(|code| TaskOutcome::Rejected {
                        reason: format!("Core FFI task dispatch failed: code={code}"),
                    })
                    .unwrap_or(TaskOutcome::Done);
                Ok(result)
            }
            Err(error) => Ok(TaskOutcome::Rejected {
                reason: error.to_string(),
            }),
        }
    }

    /// 测试用：走生产 `adjust_font(+1)`（Ctrl+= 热路径）。
    pub fn test_increase_font(&self) {
        let mut s = self._state.borrow_mut();
        adjust_font(&mut s, &self._state, 1);
    }

    /// 测试用：走生产 `adjust_font(-1)`（Ctrl+- 热路径）。
    pub fn test_decrease_font(&self) {
        let mut s = self._state.borrow_mut();
        adjust_font(&mut s, &self._state, -1);
    }

    /// 测试用：当前 UiState 字号（缩放热路径断言）。
    pub fn test_font_size(&self) -> f32 {
        self._state.borrow().font.size
    }

    /// 测试用：当前激活 pane 的核心输出快照。
    pub fn test_active_pane_output(&self) -> Vec<u8> {
        let s = self._state.borrow();
        let workspace_id = active_workspace_key(&s);
        s.event_pump
            .client()
            .get_workspace_pane_output(&workspace_id, s.active_pane)
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
        let workspace_id = active_workspace_key(&s);
        let Some(view) = s.view_store.workspace(&workspace_id) else {
            return Vec::new();
        };
        let active_tab = s.visible_tab_id(&workspace_id).unwrap_or(s.active_tab);
        let Some(layout) = view.layouts.get(&active_tab) else {
            return Vec::new();
        };
        fn leaves(layout: &crate::frontend::ffi_client::ClientLayout, out: &mut Vec<u32>) {
            match layout {
                crate::frontend::ffi_client::ClientLayout::Leaf { pane_id } => out.push(*pane_id),
                crate::frontend::ffi_client::ClientLayout::Split { first, second, .. } => {
                    leaves(first, out);
                    leaves(second, out);
                }
            }
        }
        let mut ids = Vec::new();
        leaves(layout, &mut ids);
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
        drain_ssh_probes(&self._state);
        drain_existing_ssh(&self._state);
        drain_local_existing(&self._state);
        maybe_schedule_reconnect(&self._state);
        maybe_warn_workspace_capacity(&self._state, &self.window);
        let (n, pending_close) = {
            let mut s = self._state.borrow_mut();
            let events = poll_event_store(&mut s);
            refresh_event_workspaces(&mut s, &events);
            let n = events
                .iter()
                .filter(|event| {
                    matches!(
                        event.event.kind(),
                        ClientEventKind::PaneOutput | ClientEventKind::PaneFrame
                    )
                })
                .count();
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
        let workspace_id = active_workspace_key(&s);
        let Some(view) = s.view_store.workspace(&workspace_id) else {
            return (0, 0);
        };
        let n_tabs = view.tabs.len();
        let active_tab = s.visible_tab_id(&workspace_id).unwrap_or(s.active_tab);
        let n_panes = view.panes.get(&active_tab).map_or(0, Vec::len);
        (n_tabs, n_panes)
    }

    /// 测试用：状态栏文案。
    pub fn test_status_text(&self) -> String {
        self._state.borrow().status.plain_text()
    }

    /// 测试用：手动轮询一次核心事件并刷新输出（不等待 16ms 定时器）。
    pub fn test_poll_once(&self) {
        drain_ssh_probes(&self._state);
        drain_existing_ssh(&self._state);
        drain_local_existing(&self._state);
        maybe_schedule_reconnect(&self._state);
        maybe_warn_workspace_capacity(&self._state, &self.window);
        let pending_close = {
            let mut s = self._state.borrow_mut();
            let events = poll_event_store(&mut s);
            refresh_event_workspaces(&mut s, &events);
            drain_attention_notifications(&mut s);
            sync_pane_outputs(&mut s);
            sync_window_size(&mut s);
            drain_surface_input(&mut s);
            flush_command_queue(&s);
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
    pub fn test_pane_mouse_reporting(&self, pane: u32) -> bool {
        self._state
            .borrow()
            .active_layout()
            .pane(pane)
            .map(|v| v.test_mouse_reporting())
            .unwrap_or(false)
    }

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
        crate::frontend::linux::fault_gtk::inject_fault(token);
    }

    /// 测试用：主窗口本身（供 widget 树断言）。
    pub fn test_window(&self) -> gtk4::Window {
        self.window.clone()
    }

    /// 测试用：QuickConnect 面板是否打开。
    pub fn test_panel_open(&self) -> bool {
        self._state.borrow().panel_open.is_some()
    }

    /// 测试用：当前 pane 的 VTE 是否持有键盘焦点。
    pub fn test_active_terminal_has_focus(&self) -> bool {
        let s = self._state.borrow();
        s.active_layout()
            .pane(s.active_pane)
            .is_some_and(|view| view.terminal().has_focus())
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
        let s = self._state.borrow();
        let workspace_id = active_workspace_key(&s);
        s.view_store
            .workspace(&workspace_id)
            .map(|view| view.tabs.iter().map(|tab| tab.id).collect())
            .unwrap_or_default()
    }

    /// 测试用：全部 tab 名（core 顺序；W7 new_tab_shortcut 断言非空/raw label）。
    pub fn test_tab_names(&self) -> Vec<String> {
        let s = self._state.borrow();
        let workspace_id = active_workspace_key(&s);
        s.view_store
            .workspace(&workspace_id)
            .map(|view| view.tabs.iter().map(|tab| tab.name.clone()).collect())
            .unwrap_or_default()
    }

    /// 测试用：指定 replica 的 herdr 运行时 stream 探针（takeover_watchdog 用）。
    /// 返回 (stream_starts, control_takeover_starts, takeover_suppressed, actual_mode)。
    pub fn test_herdr_probe(&self, replica: &str, pane: u32) -> Option<(u64, u64, bool, String)> {
        let s = self._state.borrow();
        let workspace_key = s
            .view_store
            .workspaces()
            .filter_map(|(_, view)| view.workspace.as_ref())
            .find(|workspace| {
                parse_workspace_id(&workspace.id).is_some_and(|id| id.replica_id() == replica)
            })
            .map(|workspace| workspace.id.clone())?;
        s.event_pump
            .client()
            .herdr_probe(&workspace_key, pane)
            .map(|probe| {
                (
                    probe.stream_starts,
                    probe.control_takeover_starts,
                    probe.takeover_suppressed,
                    probe.actual_mode,
                )
            })
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
        activity_snapshot(&self._state.borrow()).blocked_count
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
        let workspace_id = active_workspace_key(&s);
        s.event_pump
            .client()
            .workspace_pane_last_n_lines(&workspace_id, pane_id, n as u32)
            .unwrap_or_default()
    }

    /// 测试用：绕过 tmux 直接向 Surface/前端 activity 兼容层注入字节。
    pub fn test_feed_replica(&self, pane_id: u32, bytes: &[u8]) {
        let mut s = self._state.borrow_mut();
        if let Some(view) = s.active_layout().pane(pane_id).cloned() {
            view.feed_output(bytes);
            view.flush_deferred_feed();
        }
        let workspace = active_workspace_id(&s);
        let visible = pane_id == s.active_pane;
        s.compatibility_activity
            .apply_output(&workspace, pane_id, bytes, visible);
        refresh_sidebar_if_open(&mut s);
    }

    /// 测试用：本轮进入 blocked 的 workspace 通知记录。
    pub fn test_notifications_recorded(&self) -> Vec<String> {
        self._state.borrow().notification_log.clone()
    }

    /// 测试用：所有工作区 Done pane 数之和（任务完成，不是 blocked）。
    pub fn test_attention_done_count(&self) -> usize {
        let snapshot = activity_snapshot(&self._state.borrow());
        snapshot
            .workspaces
            .iter()
            .map(|workspace| workspace.done)
            .sum()
    }

    /// 测试用：通过通用 Attention 权威状态模拟任意 Runtime 的 agent。
    pub fn test_set_agent_attention(
        &self,
        pane: u32,
        process_name: &str,
        status: ClientAttentionStatus,
    ) {
        let mut s = self._state.borrow_mut();
        let ws = active_workspace_id(&s);
        s.compatibility_activity
            .set_agent_attention(&ws, pane, process_name, status);
        refresh_sidebar_if_open(&mut s);
    }

    /// 测试用：当前激活 pane id。
    pub fn test_active_pane_id(&self) -> u32 {
        self._state.borrow().active_pane
    }

    /// 测试用：SwitchPane（后台完成通知必须打在非前台 pane）。
    pub fn test_switch_pane(&self, pane_id: u32) {
        let _ = self
            ._state
            .borrow()
            .execute_active_task(ClientTask::SwitchPane { pane_id });
    }

    /// 测试用：生产搜索路径 `WorkspacePool::search_all`（不是 Mock PaneBuf）。
    pub fn test_search_all(&self, query: &str) -> Vec<(String, u32, String)> {
        self._state
            .borrow()
            .event_pump
            .client()
            .search_all(query)
            .unwrap_or_default()
            .into_iter()
            .map(|h| (h.workspace_id, h.pane_id, h.line))
            .collect()
    }

    /// 测试用：连接一个 QuickConnect 目标（走生产 connect_target 路径）。
    pub fn test_connect_target(&self, config: TargetConfig) {
        connect_target(&self._state.clone(), config);
    }

    /// 测试用：通过 FFI semantic target attach（SSH loopback 必须带远端 `-L`）。
    ///
    /// 等连接完成并激活后再返回：测试随后 `wait_ready` / 取 leaf 时看到的是
    /// 新工作区，而不是启动时的本地 shell（W18b 的 pane id 才不会串）。
    pub fn test_open_target(
        &self,
        target: ClientTarget,
        workspace_replica_id: String,
        socket: Option<String>,
    ) {
        let result = {
            let s = self._state.borrow();
            s.event_pump
                .client()
                .open_target(&target, ClientOpenIntent::AttachOnly)
        };
        match result {
            Ok(opened) => {
                let mut s = self._state.borrow_mut();
                if let Some(opened_id) = parse_workspace_id(&opened.id) {
                    s.workspace_sockets.insert(opened_id, socket);
                }
                if sync_view_store(&mut s).is_ok() {
                    after_activate(&mut s);
                }
            }
            Err(error) => {
                self._state
                    .borrow_mut()
                    .notification_log
                    .push(format!("{workspace_replica_id}: connect failed: {error}"));
            }
        }
    }

    /// 测试用：当前池里各工作区 replica id（`name@transport`）。
    pub fn test_workspace_replica_ids(&self) -> Vec<String> {
        self._state
            .borrow()
            .view_store
            .workspaces()
            .filter_map(|(_, view)| view.workspace.as_ref())
            .filter_map(|workspace| parse_workspace_id(&workspace.id))
            .map(|id| id.replica_id())
            .collect()
    }

    /// 测试用：池里各工作区的 runtime 种类（断言没误开成本地 tmux）。
    pub fn test_workspace_runtimes(&self) -> Vec<String> {
        self._state
            .borrow()
            .view_store
            .workspaces()
            .filter_map(|(_, view)| view.workspace.as_ref())
            .map(|workspace| workspace.runtime.clone())
            .collect()
    }

    /// 测试用：按 replica id 激活工作区（上次看到这里 / 跨工作区搜索）。
    pub fn test_activate_workspace(&self, replica: &str) {
        let mut s = self._state.borrow_mut();
        activate_attention_workspace(&mut s, replica);
    }

    /// 测试用：只搜当前工作区当前 pane。
    pub fn test_search_pane(&self, pane: u32, query: &str) -> Vec<(String, u32, String)> {
        let s = self._state.borrow();
        let workspace_id = active_workspace_id(&s);
        s.event_pump
            .client()
            .search_all(query)
            .unwrap_or_default()
            .into_iter()
            .filter(|hit| hit.workspace_id == workspace_id && hit.pane_id == pane)
            .map(|h| (h.workspace_id, h.pane_id, h.line))
            .collect()
    }

    /// 测试用：只搜当前工作区全部 pane。
    pub fn test_search_workspace(&self, query: &str) -> Vec<(String, u32, String)> {
        let s = self._state.borrow();
        let workspace_id = active_workspace_id(&s);
        s.event_pump
            .client()
            .search_all(query)
            .unwrap_or_default()
            .into_iter()
            .filter(|hit| hit.workspace_id == workspace_id)
            .map(|h| (h.workspace_id, h.pane_id, h.line))
            .collect()
    }

    /// 测试用：打开当前 pane 内查找条（与 Ctrl+F 同一条生产路径）。
    pub fn test_open_pane_find(&self) {
        open_pane_find(&self._state.clone(), &self.window);
    }
}

/// A Core tab mutation is allowed to change the authoritative active tab.
/// Drop a local display override before dispatching it so the resulting
/// `ActiveTabChanged` event can select the new Core tab.
fn prepare_core_tab_mutation(s: &mut UiState, task: &ClientTask) {
    let clear = match task {
        ClientTask::NewTab => true,
        ClientTask::CloseTab { tab_id } => *tab_id == s.active_tab,
        _ => false,
    };
    if clear {
        let workspace_key = s.active_workspace_key();
        s.visible_tabs.remove(&workspace_key);
        s.local_tab_overrides.remove(&workspace_key);
    }
}

fn handle_action(s: &mut UiState, action: Action, window: &Window, state: &Rc<RefCell<UiState>>) {
    match action {
        Action::NewTab | Action::NewWindow => {
            prepare_core_tab_mutation(s, &ClientTask::NewTab);
            let _ = s.execute_active_task(ClientTask::NewTab);
            // Accepted 不得手工 refresh：等 16ms 批里 LayoutChanged/MutationSettled。
            return;
        }
        Action::NewPane => {
            let pane = s.active_pane;
            let _ = s.execute_active_task(ClientTask::SplitPane {
                pane_id: pane,
                horizontal: true,
            });
            return;
        }
        Action::NewPaneVertical => {
            let pane = s.active_pane;
            let _ = s.execute_active_task(ClientTask::SplitPane {
                pane_id: pane,
                horizontal: false,
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
            let workspace_id = active_workspace_key(s);
            if let Some(tab_id) = s
                .view_store
                .workspace(&workspace_id)
                .and_then(|view| view.tabs.last().map(|tab| tab.id))
            {
                request_switch_tab(s, tab_id);
            }
        }
        Action::SwitchWorkspace1 => switch_workspace_n(s, 1),
        Action::SwitchWorkspace2 => switch_workspace_n(s, 2),
        Action::SwitchWorkspace3 => switch_workspace_n(s, 3),
        Action::SwitchWorkspace4 => switch_workspace_n(s, 4),
        Action::SwitchWorkspace5 => switch_workspace_n(s, 5),
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
        Action::IncreaseFontSize => adjust_font(s, state, 1),
        Action::DecreaseFontSize => adjust_font(s, state, -1),
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
    crate::frontend::linux::command_palette::show_for_runtime(
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
            crate::frontend::linux::command_palette::show_language(&language_parent, move |_| {
                let mut s = callback_state.borrow_mut();
                maybe_refresh_status(&mut s, true);
            });
        }
        PaletteAction::TmuxDetach => {
            // 命令先入队；下一轮 GTK poll flush 后再关闭，避免 close 直接
            // 销毁窗口而丢掉尚未提交的 detach。
            {
                let mut s = state.borrow_mut();
                let accepted = s.execute_active_task(ClientTask::Detach).is_ok();
                if accepted {
                    s.pending_close = true;
                }
            }
        }
        PaletteAction::SshDisconnect => {
            let s = state.borrow();
            if s.uses_tmux() {
                let _ = s.execute_active_task(ClientTask::Detach);
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
            adjust_font(&mut s, state, 1);
        }
        PaletteAction::DecreaseFontSize => {
            let mut s = state.borrow_mut();
            adjust_font(&mut s, state, -1);
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
            prepare_core_tab_mutation(&mut s, &ClientTask::NewTab);
            let _ = s.execute_active_task(ClientTask::NewTab);
            // Accepted 不得手工 refresh：等 LayoutChanged/MutationSettled。
        }
        PaletteAction::NewPane => {
            let s = state.borrow();
            let pane = s.active_pane;
            let _ = s.execute_active_task(ClientTask::SplitPane {
                pane_id: pane,
                horizontal: true,
            });
        }
        PaletteAction::NewPaneVertical => {
            let s = state.borrow();
            let pane = s.active_pane;
            let _ = s.execute_active_task(ClientTask::SplitPane {
                pane_id: pane,
                horizontal: false,
            });
        }
        PaletteAction::ClosePane => {
            let mut s = state.borrow_mut();
            let pane = s.active_pane;
            let _ = s.execute_active_task(ClientTask::ClosePane { pane_id: pane });
            refresh_ui(&mut s);
        }
        PaletteAction::CloseTab => {
            let mut s = state.borrow_mut();
            let tab = s.active_tab;
            let task = ClientTask::CloseTab { tab_id: tab };
            prepare_core_tab_mutation(&mut s, &task);
            let _ = s.execute_active_task(task);
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
        let _ = s.execute_active_task(ClientTask::TogglePaneFullscreen { pane_id: pane });
    } else {
        let next = match s.active_layout().fullscreen_pane() {
            Some(id) if id == pane => None,
            _ => Some(pane),
        };
        s.active_layout_mut().set_fullscreen_pane(next);
    }
}

/// 通过统一 FFI client 的 Core 配置事务写回 config.toml（唯一事实源）。
/// 平台禁止直接解析或写 TOML；失败只记日志，不覆盖用户文件。
pub fn persist_config(client: &FfiClient, dotted: &str, value: serde_json::Value) {
    if let Err(error) = client.config_apply_path(dotted, value) {
        tracing::warn!(target = "muxterm::config", "保存设置失败: {error}");
    }
}

fn adjust_font(s: &mut UiState, state: &Rc<RefCell<UiState>>, direction: i32) {
    window_appearance::adjust_font(s, state, direction);
}

fn reset_font(s: &mut UiState) {
    window_appearance::reset_font(s);
}

fn toggle_theme(s: &mut UiState) {
    window_appearance::toggle_theme(s);
}

fn apply_config_snapshot(s: &mut UiState, snapshot: ClientConfigSnapshot) {
    window_appearance::apply_config_snapshot(s, snapshot);
}

fn toggle_status_mode(s: &mut UiState) {
    window_appearance::toggle_status_mode(s);
}

fn report_all_pane_colours(s: &mut UiState) {
    window_appearance::report_all_pane_colours(s);
}

fn copy_pane(s: &UiState, pane_id: u32) {
    if let Some(view) = s.active_layout().pane(pane_id) {
        view.copy_clipboard();
    }
}

fn paste_pane(s: &UiState, state: &Rc<RefCell<UiState>>, pane_id: u32) {
    let Some(view) = s.active_layout().pane(pane_id).cloned() else {
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
        let text = crate::frontend::mirror::sanitize_paste(text.as_str(), bracketed);
        let data = crate::frontend::mirror::encode_clipboard_paste(&text, bracketed);
        if data.is_empty() {
            return;
        }
        let Some(st) = st.upgrade() else {
            return;
        };
        let s = st.borrow();
        let workspace_id = active_workspace_key(&s);
        enqueue_workspace_input(&s, &workspace_id, pane_id, &data, false);
    });
}

fn copy_active_pane(s: &UiState) {
    copy_pane(s, s.active_pane);
}

fn paste_active_pane(s: &UiState, state: &Rc<RefCell<UiState>>) {
    paste_pane(s, state, s.active_pane);
}

fn split_pane_from_menu(s: &mut UiState, pane_id: u32, vertical: bool) {
    let _ = s.execute_active_task(ClientTask::SplitPane {
        pane_id,
        horizontal: !vertical,
    });
}

fn handle_pane_menu_action(state: &Rc<RefCell<UiState>>, pane_id: u32, action: PaneMenuAction) {
    match action {
        PaneMenuAction::Copy => {
            let s = state.borrow();
            copy_pane(&s, pane_id);
        }
        PaneMenuAction::Paste => {
            let s = state.borrow();
            paste_pane(&s, state, pane_id);
        }
        PaneMenuAction::SplitVertical => {
            let mut s = state.borrow_mut();
            split_pane_from_menu(&mut s, pane_id, true);
        }
        PaneMenuAction::SplitHorizontal => {
            let mut s = state.borrow_mut();
            split_pane_from_menu(&mut s, pane_id, false);
        }
    }
}

fn switch_tab_n(s: &mut UiState, n: usize) {
    window_scene::switch_tab_n(s, n);
}

fn switch_workspace_n(s: &mut UiState, n: usize) {
    window_scene::switch_workspace_n(s, n);
}

/// 切 tab 只显示已经常驻的 GTK Stack page，不通知 Core。
fn show_tab_scene(s: &mut UiState, tab_id: u32) -> bool {
    window_scene::show_tab_scene(s, tab_id)
}

fn request_switch_tab(s: &mut UiState, tab_id: u32) {
    window_scene::request_switch_tab(s, tab_id);
}

/// 与 macOS `movePane` 对齐：用当前 tab 快照算目标，发 SwitchPane。
/// 不要发 NextPane——tmux 布局树若没解析完会落到无效的
/// `select-pane -t @N -N/-P`（2219.log 14:41:29）。
fn switch_pane_offset(s: &mut UiState, forward: bool) {
    let workspace_id = active_workspace_key(s);
    let panes = s
        .view_store
        .workspace(&workspace_id)
        .and_then(|view| view.panes.get(&s.active_tab))
        .cloned()
        .unwrap_or_default();
    let ids: Vec<u32> = panes.iter().map(|pane| pane.id).collect();
    let active = panes
        .iter()
        .find(|pane| pane.is_active)
        .map(|pane| pane.id)
        .unwrap_or(s.active_pane);
    if let Some(target) = cycle_pane_id(&ids, active, forward) {
        let _ = s.execute_active_task(ClientTask::SwitchPane { pane_id: target });
    }
}

/// 刷新状态栏红点与窗口标题（blocked 工作区数）。
fn refresh_attention_chrome(s: &UiState, window: &Window) {
    window_status::refresh_attention_chrome(s, window);
}

/// 回底按钮可见性：VTE 滚离底部时显示，回到尾部隐藏（W16a）。
/// 把当前 pane 滚到包含指定文本的行（命令刻度 / 上次看到这里共用）。
fn scroll_to_command_text(state: &Rc<RefCell<UiState>>, text: &Rc<RefCell<Option<String>>>) {
    let s = state.borrow();
    let Some(text) = text.borrow().clone() else {
        return;
    };
    let pane = s.active_pane;
    let workspace_id = active_workspace_key(&s);
    let lines = s
        .event_pump
        .client()
        .workspace_pane_last_n_lines(&workspace_id, pane, 10_000)
        .unwrap_or_default();
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
    let workspace_id = active_workspace_key(s);
    let marks = s
        .event_pump
        .client()
        .workspace_pane_command_marks(&workspace_id, s.active_pane)
        .unwrap_or_default();
    let ok = marks.iter().rev().find(|m| m.exit_code == Some(0));
    let fail = marks
        .iter()
        .rev()
        .find(|m| m.exit_code.is_some_and(|c| c != 0));
    if let Some(m) = ok {
        s.overlay.command_ok.set_visible(true);
        s.overlay.command_ok.set_tooltip_text(Some(&m.command));
        *s.overlay.command_ok_text.borrow_mut() = Some(m.command.clone());
    } else {
        s.overlay.command_ok.set_visible(false);
        *s.overlay.command_ok_text.borrow_mut() = None;
    }
    if let Some(m) = fail {
        s.overlay.command_fail.set_visible(true);
        s.overlay.command_fail.set_tooltip_text(Some(&m.command));
        *s.overlay.command_fail_text.borrow_mut() = Some(m.command.clone());
    } else {
        s.overlay.command_fail.set_visible(false);
        *s.overlay.command_fail_text.borrow_mut() = None;
    }
}

/// 当前激活 pane 的 VTE 是否在底部（scroll lock / 回底按钮共用）。
fn view_at_bottom(view: &std::rc::Rc<PaneSurface>) -> bool {
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
    s.overlay.jump_latest.set_visible(!at_bottom);
    if at_bottom {
        // 回到尾部：搜索高亮不再有意义（W17c）。
        s.overlay.search_highlight.set_visible(false);
    } else if s.overlay.jump_unseen > 0 {
        s.overlay
            .jump_latest
            .set_label(&format!("↓ +{}", s.overlay.jump_unseen));
    } else {
        s.overlay.jump_latest.set_label("↓");
    }
}

/// 把当前连接摘要刷到状态点 popover（C7.7）。
///
/// 速率由连续两次 `traffic_bytes()` 快照 + 墙钟差出来（W15a），
/// 禁止把累计字节标成 `B/s`。
fn refresh_connection_summary(s: &mut UiState) {
    window_status::refresh_connection_summary(s);
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

#[cfg(test)]
#[derive(Debug, Default)]
struct UiBatchEffects {
    topology_changed: bool,
}

#[cfg(test)]
impl UiBatchEffects {
    fn note_topology(&mut self) {
        self.topology_changed = true;
    }
}

#[cfg(test)]
fn attention_event_pane(event: &StateChange) -> Option<u32> {
    match event {
        StateChange::PaneOutput { pane, .. }
        | StateChange::PaneFrame { pane, .. }
        | StateChange::PaneAgentChanged { pane, .. } => Some(pane.0),
        _ => None,
    }
}

fn mark_pending_close_if_session_ended(s: &mut UiState) {
    let workspace_id = active_workspace_key(s);
    let n_tabs = s
        .view_store
        .workspace(&workspace_id)
        .map_or(0, |view| view.tabs.len());
    if should_close_window(false, n_tabs, s.on_last_pane_exit) {
        s.pending_close = true;
    }
}

/// 取走本轮 blocked / done 通知并交给 sink（测试日志也记录）。
fn drain_attention_notifications(s: &mut UiState) {
    let notifications = s
        .event_pump
        .client()
        .take_activity_notifications()
        .unwrap_or_default();
    if notifications.notifications.is_empty() {
        for ws in notifications.blocked {
            record_attention_notification(s, &ws, "blocked");
        }
        for ws in notifications.done {
            record_attention_notification(s, &ws, "done");
        }
    } else {
        for notification in notifications.notifications {
            record_attention_notification(s, &notification.workspace_id, &notification.kind);
        }
    }

    // Compatibility-only direct injection used by the GTK tests. Runtime
    // events are drained from Core above and never pass through this adapter.
    for notification in s.compatibility_activity.take_notifications() {
        record_attention_notification(s, &notification.workspace_id, &notification.kind);
    }
}

fn record_attention_notification(s: &mut UiState, workspace: &str, kind: &str) {
    match kind {
        "blocked" => {
            tracing::info!(
                target: "muxterm::notify",
                workspace = %workspace,
                "blocked workspace notification"
            );
            s.notification_sink
                .notify_blocked(workspace, "needs attention");
            s.notification_log
                .push(format!("{workspace}: needs attention"));
        }
        "done" => {
            tracing::info!(
                target: "muxterm::notify",
                workspace = %workspace,
                "background task done"
            );
            s.notification_sink.notify_done(workspace, "task complete");
            s.notification_log
                .push(format!("{workspace}: task complete"));
        }
        other => {
            tracing::debug!(
                target: "muxterm::notify",
                workspace = %workspace,
                kind = other,
                "ignored unknown activity notification"
            );
        }
    }
}

fn refresh_ui(s: &mut UiState) {
    window_layout::refresh_ui(s);
}

fn refresh_workspace_layout(s: &mut UiState, wid: &WorkspaceId, seed_from_core: bool) {
    window_layout::refresh_workspace_layout(s, wid, seed_from_core);
}

fn local_status_snapshot(npanes: usize, tabs: &[(u32, String, bool)]) -> StatusBarSnapshot {
    window_status::local_status_snapshot(npanes, tabs)
}

fn maybe_refresh_status(s: &mut UiState, force: bool) {
    window_status::maybe_refresh_status(s, force);
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
    window_status::sync_chrome_visibility(s);
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
        let workspace_key = workspace_id.as_str();
        enqueue_workspace_input(s, &workspace_key, pane_id.0, &data, false);
    }
}

fn take_surface_input(queue: &Rc<RefCell<VecDeque<SurfaceInput>>>) -> Vec<SurfaceInput> {
    queue.borrow_mut().drain(..).collect()
}

fn sync_pane_outputs(s: &mut UiState) {
    window_render::sync_pane_outputs(s);
}

fn refresh_event_workspaces(s: &mut UiState, events: &[ClientWorkspaceEvent]) {
    window_render::refresh_event_workspaces(s, events);
}

fn repair_visible_workspace(s: &mut UiState) {
    window_render::repair_visible_workspace(s);
}

fn apply_attention_visibility_events(s: &UiState, events: &[ClientWorkspaceEvent]) {
    window_render::apply_attention_visibility_events(s, events);
}

fn mark_active_attention_visible(s: &UiState) {
    window_render::mark_active_attention_visible(s);
}

/// 把 core 里已就绪的 attach 快照播种进尚未播种的 VTE。
///
/// 窗口 present/realize 前 feed 会被 VTE 丢弃（白屏），所以只在 widget
/// 已 realized 时播种；未 realized 的 pane 保持 unseeded，等布局挂载后
/// 由下一次 refresh_ui / sync_pane_outputs 补种。
fn seed_unseeded_pane(
    s: &mut UiState,
    view: &std::rc::Rc<PaneSurface>,
    pane_id: u32,
    cols: u16,
    rows: u16,
) {
    window_surface::seed_unseeded_pane(s, view, pane_id, cols, rows);
}

/// VTE 只有在 realize 且二维分配都有效时才能可靠接收首帧。
fn surface_allocation_is_seedable(realized: bool, width: i32, height: i32) -> bool {
    window_surface::surface_allocation_is_seedable(realized, width, height)
}

fn seed_unseeded_pane_for(
    s: &mut UiState,
    wid: &WorkspaceId,
    view: &std::rc::Rc<PaneSurface>,
    pane_id: u32,
    cols: u16,
    rows: u16,
) {
    window_surface::seed_unseeded_pane_for(s, wid, view, pane_id, cols, rows);
}

/// Flush render events retained while a pane had no realized Surface.
fn drain_view_store_render_events(
    s: &mut UiState,
    wid: &WorkspaceId,
    view: &std::rc::Rc<PaneSurface>,
    pane_id: u32,
) {
    window_surface::drain_view_store_render_events(s, wid, view, pane_id);
}

fn sync_pane_grid_size(s: &UiState, pane_id: u32) {
    window_render::sync_pane_grid_size(s, pane_id);
}

/// 按 `(WorkspaceId, PaneId)` 对齐字符格（hidden tab / background 也适用）。
fn sync_pane_grid_size_for(s: &UiState, wid: &WorkspaceId, pane_id: u32) {
    window_render::sync_pane_grid_size_for(s, wid, pane_id);
}

fn forward_parser_replies(s: &mut UiState, pane_id: u32) {
    window_render::forward_parser_replies(s, pane_id);
}

/// 按 WorkspaceId 转发 parser replies（background workspace 也 flush）。
fn forward_parser_replies_for(s: &mut UiState, wid: &WorkspaceId, pane_id: u32) {
    window_render::forward_parser_replies_for(s, wid, pane_id);
}

fn forward_parser_replies_for_key(s: &mut UiState, workspace_id: &str, pane_id: u32) {
    window_render::forward_parser_replies_for_key(s, workspace_id, pane_id);
}

/// 把窗口内容区的新字符格尺寸同步给 Runtime。
///
/// 共享 client viewport 的 Runtime（tmux）收到整个 Workspace 的尺寸；
/// 其它 Runtime（shell / Herdr）收到当前 Surface 的实际字符格尺寸。
/// platform 只问 capability，不按实现名字分支。
///
/// tmux SharedClientResize：同一尺寸连续 ~10 次 poll（约 160ms）才 -C，
/// 过滤窗口 map / VTE preferred 抖动（dogfood 2152：106→284→142）。
fn sync_window_size(s: &mut UiState) {
    window_resize::sync_window_size(s);
}

/// Herdr 没有 SharedClientResize：每个可见 split 格子按自己的 VTE 分配
/// 发 ResizePane。只同步 active pane 会让 0218.log 里 54/57 停在 27×12。
fn sync_visible_pane_sizes(s: &mut UiState) {
    window_resize::sync_visible_pane_sizes(s);
}

/// 0218.log：三个可见格子都要有自己的 ResizePane；已同步过的尺寸跳过。
fn pending_pane_resizes(
    last: &HashMap<u32, (u16, u16)>,
    measured: &[(u32, u16, u16)],
) -> Vec<(u32, u16, u16)> {
    window_resize::pending_pane_resizes(last, measured)
}

fn spawn_ssh_probe(s: &mut UiState, alias: String) {
    window_discovery::spawn_ssh_probe(s, alias);
}

/// 面板打开时收集 SSH 灯：TTL 内用缓存，否则 Unknown 并后台探测。
fn collect_ssh_reach(s: &mut UiState, workspaces: &[PanelItem]) -> HashMap<String, SshReach> {
    window_discovery::collect_ssh_reach(s, workspaces)
}

/// 收编后台 SSH 探测结果（16ms poll 与 test_poll_once 共用）。
fn drain_ssh_probes(state: &Rc<RefCell<UiState>>) {
    window_discovery::drain_ssh_probes(state);
}

fn existing_entries(
    candidates: Vec<crate::frontend::ffi_client::ExistingCandidate>,
) -> Vec<ExistingEntry> {
    window_discovery::existing_entries(candidates)
}

/// 已有的连接探测增量：先推 local 行，SSH 完成后再推；Done 才清 inflight。
enum ExistingProbeMsg {
    Aliases(Vec<String>),
    Rows(Vec<ExistingEntry>),
    Done,
}

fn merge_existing_entries(ex: &mut ExistingPanelState, entries: Vec<ExistingEntry>) {
    window_discovery::merge_existing_entries(ex, entries);
}

fn append_unique_existing_entries(target: &mut Vec<ExistingEntry>, entries: Vec<ExistingEntry>) {
    window_discovery::append_unique_existing_entries(target, entries);
}

/// C7/C9：已有的连接探测。先 `discover_existing("local")` 立刻推表，
/// 再按 SSH host 最多 4 路并发。禁止等 `all` 串完才刷新（archmini 上 cd/mac 会冻 Loading）。
fn spawn_local_existing_probe(s: &mut UiState) {
    window_discovery::spawn_local_existing_probe(s);
}

/// 收编已有连接探测结果（16ms poll 与 test_poll_once 共用）。
fn drain_local_existing(state: &Rc<RefCell<UiState>>) {
    window_discovery::drain_local_existing(state);
}

/// W20：SSH 已有的连接探测（tmux + Herdr），后台线程，最多 4 路并发。
fn spawn_existing_ssh_probe(state: &Rc<RefCell<UiState>>) {
    window_discovery::spawn_existing_ssh_probe(state);
}

/// 收编 SSH 已有连接探测结果（16ms poll 与 test_poll_once 共用）。
fn drain_existing_ssh(state: &Rc<RefCell<UiState>>) {
    window_discovery::drain_existing_ssh(state);
}

/// W17a：tmux 控制 client 掉线后自动重连。
///
/// 只重连声明 `SharedClientResize` 的 Runtime；Core 负责所有 live
/// Workspace 的 reconnect，GTK 只提交一次产品级 reconnect 请求。
fn maybe_schedule_reconnect(state: &Rc<RefCell<UiState>>) {
    window_discovery::maybe_schedule_reconnect(state);
}

/// 重连成功：换 Runtime、隐藏水印；断线期间的 BEL 重新推导成 Blocked。
///
/// 只对「仍处于断线状态」的工作区生效：若断线期间用户已重新 attach
/// （新 Runtime 已插入且 Connected），旧重连结果必须丢弃——否则会换掉
/// 更新的 Runtime，并丢失其尚未消费的 capture 事件（PaneBuf 空、搜索
/// 不到断线前 token）。
fn handle_reconnect_success(state: &Rc<RefCell<UiState>>) {
    window_discovery::handle_reconnect_success(state);
}

/// 打开当前 pane 内查找条（W18f：Ctrl+F 与 test_open_pane_find 共用）。
fn open_pane_find(state: &Rc<RefCell<UiState>>, _window: &Window) {
    window_overlay::open_pane_find(state);
}

fn open_quick_connect(state: &Rc<RefCell<UiState>>, window: &Window) {
    window_overlay::open_quick_connect(state, window);
}

/// 打开三 tab 面板（initial_tab 由入口决定：Alt+Q → Workspaces，红点 → Attention）。
///
/// 内部自行 borrow：面板回调会再次借用 state，调用方不能同时持有 RefMut。
fn open_panel(state: &Rc<RefCell<UiState>>, window: &Window, initial_tab: PanelTab) {
    window_overlay::open_panel(state, window, initial_tab);
}

/// 跳到注意力 pane：若目标工作区不是当前前台连接，先切连接；
/// 命中在别的 tab 时先 `SwitchTab` 再 `SwitchPane`（W15b）。
/// `seq` 是搜索命中的 PaneBuf 行号（W17c）：切完后把 VTE 滚到该行并显示高亮。
fn jump_to_attention_pane(state: &Rc<RefCell<UiState>>, ws: &str, pane: u32, seq: u64) {
    window_overlay::jump_to_attention_pane(state, ws, pane, seq);
}

/// 目标工作区不是当前前台时切连接；相同则不动（避免无谓的 layout 重建）。
fn activate_attention_workspace(s: &mut UiState, ws: &str) {
    window_overlay::activate_attention_workspace(s, ws);
}

/// 按 workspace_id（name@transport）找 WorkspaceId。
fn attention_workspace_id(s: &UiState, ws: &str) -> Option<WorkspaceId> {
    window_overlay::attention_workspace_id(s, ws)
}

fn workspace_replica_matches(id: &WorkspaceId, requested: &str) -> bool {
    window_overlay::workspace_replica_matches(id, requested)
}

/// 打开配置页：保存/热加载后重读 config.toml 并应用主题/字体/attention。
fn open_preferences(state: &Rc<RefCell<UiState>>, window: &Window) {
    window_config::open_preferences(state, window);
}

fn open_target_config(
    state: &Rc<RefCell<UiState>>,
    window: &Window,
    editing: Option<TargetConfig>,
) {
    window_config::open_target_config(state, window, editing);
}

/// W20：SSH 已有连接探测结果（alias → 该 host 的 tmux/Herdr 行）。
type ExistingSshProbeResult = Vec<(String, Vec<ExistingEntry>)>;

/// worktree 创建对话框：分支 + 路径，Create 后后台建 checkout 并开新格。
fn show_worktree_create_dialog(state: &Rc<RefCell<UiState>>, parent: &gtk4::Window) {
    window_worktree::show_worktree_create_dialog(state, parent);
}

/// TargetConfig + session → 稳定 WorkspaceId。
fn workspace_id_for_config(config: &TargetConfig, session: &str) -> WorkspaceId {
    window_connection::workspace_id_for_config(config, session)
}

fn activate_existing(s: &mut UiState, id: WorkspaceId) {
    window_scene::activate_existing(s, id);
}

/// 超过 soft capacity 时提醒用户选择关闭最久未使用的后台 Workspace。
///
/// 提醒状态按 slot 数量去重：点“全部保留”不会在每个 16ms poll 重复弹窗，
/// 新建或关闭导致数量变化后才重新评估。当前活动 Workspace 只会是 active，
/// 不会进入 `oldest_background_candidates`。
fn maybe_warn_workspace_capacity(state: &Rc<RefCell<UiState>>, parent: &Window) {
    const MAX_CANDIDATES: usize = 8;

    let details = {
        let mut s = state.borrow_mut();
        let count = s.view_store.workspace_ids().count();
        let over_capacity = count > s.capacity_limit;
        if !over_capacity || s.capacity_warning_presented_for_slot_count == Some(count) {
            if !over_capacity {
                s.capacity_warning_presented_for_slot_count = None;
            }
            None
        } else {
            let overflow = count.saturating_sub(s.capacity_limit).max(1);
            let active = s.active_workspace_key();
            let mut candidates: Vec<WorkspaceCapacityCandidate> = s
                .view_store
                .workspaces()
                .filter_map(|(_, view)| view.workspace.as_ref())
                .filter(|workspace| active != workspace.id)
                .filter_map(|workspace| {
                    Some(WorkspaceCapacityCandidate {
                        id: parse_workspace_id(&workspace.id)?,
                        name: workspace.name.clone(),
                    })
                })
                .collect();
            candidates.sort_by_key(|candidate| candidate.id.as_str());
            candidates.truncate(overflow.min(MAX_CANDIDATES));
            if candidates.is_empty() {
                None
            } else {
                s.capacity_warning_presented_for_slot_count = Some(count);
                Some((count, s.capacity_limit, candidates))
            }
        }
    };
    let Some((count, limit, candidates)) = details else {
        return;
    };

    let dialog = Window::builder()
        .title(i18n::tr(Key::WorkspaceCapacityTitle))
        .modal(true)
        .transient_for(parent)
        .default_width(500)
        .default_height(300)
        .build();
    let root = Box::new(Orientation::Vertical, 10);
    root.set_margin_top(16);
    root.set_margin_bottom(16);
    root.set_margin_start(16);
    root.set_margin_end(16);

    let count_text = count.to_string();
    let limit_text = limit.to_string();
    let message = Label::new(Some(&i18n::tr_args(
        Key::WorkspaceCapacityMessage,
        &[
            ("count", count_text.as_str()),
            ("limit", limit_text.as_str()),
        ],
    )));
    message.set_halign(gtk4::Align::Start);
    message.set_wrap(true);
    root.append(&message);

    let candidate_box = Box::new(Orientation::Vertical, 4);
    let choices: Vec<(WorkspaceCapacityCandidate, CheckButton)> = candidates
        .iter()
        .map(|candidate| {
            let check = CheckButton::with_label(&format_capacity_candidate(candidate));
            (candidate.clone(), check)
        })
        .collect();
    for (_, check) in &choices {
        candidate_box.append(check);
    }
    root.append(&candidate_box);

    let actions = Box::new(Orientation::Horizontal, 8);
    actions.set_halign(gtk4::Align::End);
    let keep = Button::with_label(&i18n::tr(Key::WorkspaceCapacityKeepAll));
    let close_selected = Button::with_label(&i18n::tr(Key::WorkspaceCapacityCloseSelected));
    actions.append(&keep);
    actions.append(&close_selected);
    root.append(&actions);
    dialog.set_child(Some(&root));

    let dialog_for_keep = dialog.clone();
    keep.connect_clicked(move |_| dialog_for_keep.close());

    let dialog_for_close = dialog.clone();
    let state_for_close = state.clone();
    close_selected.connect_clicked(move |_| {
        let ids: Vec<WorkspaceId> = choices
            .iter()
            .filter(|(_, check)| check.is_active())
            .map(|(candidate, _)| candidate.id.clone())
            .collect();
        let mut s = state_for_close.borrow_mut();
        for id in ids {
            close_sidebar_workspace(&mut s, &id);
        }
        s.capacity_warning_presented_for_slot_count =
            if s.view_store.workspace_ids().count() > s.capacity_limit {
                Some(s.view_store.workspace_ids().count())
            } else {
                None
            };
        dialog_for_close.close();
    });

    dialog.present();
}

fn format_capacity_candidate(candidate: &WorkspaceCapacityCandidate) -> String {
    let target = candidate
        .id
        .alias
        .as_deref()
        .filter(|alias| !alias.is_empty())
        .unwrap_or("local");
    format!("{} · {} @ {}", candidate.name, candidate.id.runtime, target)
}

fn close_sidebar_workspace(s: &mut UiState, id: &WorkspaceId) {
    let mut ordered: Vec<WorkspaceId> = s
        .view_store
        .workspaces()
        .filter_map(|(_, view)| view.workspace.as_ref())
        .filter_map(|workspace| parse_workspace_id(&workspace.id))
        .collect();
    ordered.sort_by_key(|workspace| workspace.as_str());
    let Some(index) = ordered.iter().position(|candidate| candidate == id) else {
        return;
    };
    let was_active = s.active_ws_id() == *id;
    let fallback = if was_active {
        ordered
            .get(index + 1)
            .or_else(|| {
                index
                    .checked_sub(1)
                    .and_then(|previous| ordered.get(previous))
            })
            .cloned()
    } else {
        None
    };
    let workspace_key = id.as_str();
    if let Err(error) = s.event_pump.client().close_workspace(&workspace_key) {
        tracing::warn!(
            target = "muxterm::linux",
            %error,
            workspace = %workspace_key,
            "workspace close failed"
        );
        return;
    }

    let replica_id = id.replica_id();
    s.last_seen
        .retain(|(workspace, _), _| workspace != &replica_id);
    s.surface_input_queue
        .borrow_mut()
        .retain(|input| &input.workspace != id);
    s.scenes.remove(id);
    s.view_store.remove_workspace(&workspace_key);
    s.visible_tabs.remove(&workspace_key);
    s.local_tab_overrides.remove(&workspace_key);
    if s.mounted_ws.as_ref() == Some(id) {
        s.mounted_ws = None;
    }
    s.workspace_sockets.remove(id);
    if let Err(error) = sync_view_store(s) {
        tracing::warn!(target = "muxterm::linux", %error, "workspace list refresh failed after close");
    }
    let recents = recent_target_configs(
        &s.view_store,
        &s.workspace_sockets,
        s.view_store.workspace_ids().count(),
    );
    s.qc_store.replace_all_recents(&recents);

    if was_active {
        if let Some(fallback) = fallback {
            let fallback_key = fallback.as_str();
            let _ = sync_view_store(s);
            if s.view_store.workspace(&fallback_key).is_some() {
                show_workspace_scene(s, fallback, false);
            }
        } else {
            // 主窗口始终需要一个可轮询的前台 Workspace。关闭最后一格时
            // 立即回到一格空本地 shell；Core 负责旧 Runtime 的 detach/
            // shutdown 语义，GTK 只提交产品级 target。
            let target = ClientTarget {
                name: "shell".into(),
                runtime: "shell".into(),
                transport: "local".into(),
                target: None,
                path: String::new(),
                session: None,
                socket: None,
            };
            if let Err(error) = s
                .event_pump
                .client()
                .open_target(&target, ClientOpenIntent::CreateIfMissing)
            {
                tracing::error!(
                    target = "muxterm::linux",
                    "关闭最后工作区后创建本地 shell 失败: {error}"
                );
                return;
            }
            let _ = sync_view_store(s);
        }
        after_activate(s);
    } else {
        refresh_sidebar_if_open(s);
        maybe_refresh_status(s, true);
    }
}

fn activate_sidebar_activity(s: &mut UiState, id: &WorkspaceId, pane: u32) {
    if s.active_ws_id() != *id {
        let workspace_key = id.as_str();
        if s.view_store.workspace(&workspace_key).is_none() {
            return;
        }
        show_workspace_scene(s, id.clone(), false);
    }
    let workspace_key = active_workspace_key(s);
    let tab = {
        s.view_store
            .workspace(&workspace_key)
            .and_then(|view| {
                view.tabs.iter().find(|tab| {
                    view.panes
                        .get(&tab.id)
                        .is_some_and(|panes| panes.iter().any(|candidate| candidate.id == pane))
                })
            })
            .map(|tab| tab.id)
    };
    if let Some(tab) = tab {
        if tab != s.active_tab {
            request_switch_tab(s, tab);
        }
        let _ = s.execute_active_task(ClientTask::SwitchPane { pane_id: pane });
    }
    let _ = s
        .event_pump
        .client()
        .workspace_attention_acknowledge(&workspace_key, pane);
    acknowledge_compatibility_attention(s, &id.replica_id(), pane);
    refresh_sidebar_if_open(s);
}

fn acknowledge_compatibility_attention(s: &mut UiState, workspace_id: &str, pane: u32) {
    s.compatibility_activity.acknowledge(workspace_id, pane);
}

fn refresh_sidebar_if_open(s: &mut UiState) {
    if !s.sidebar.is_open() {
        return;
    }
    let activity = activity_snapshot(s);
    let active_workspace = s.active_workspace_key();
    s.sidebar
        .refresh_from_views(&s.view_store, Some(&active_workspace), &activity);
}

fn refresh_sidebar_workspaces_if_open(s: &UiState) {
    if s.sidebar.is_open() {
        let active_workspace = s.active_workspace_key();
        s.sidebar
            .refresh_workspaces_from_views(&s.view_store, Some(&active_workspace));
    }
}

/// Core open/activate 完成后，把 Core snapshot 的 active workspace 交给
/// frontend-visible Scene；Core activation 本身只发生在 open/close 等生命周期。
fn after_activate(s: &mut UiState) {
    window_scene::after_activate(s);
}

/// 切工作区只改 GtkStack 可见页和前端缓存，不调用 Core。
fn show_workspace_scene(s: &mut UiState, id: WorkspaceId, seed_from_core: bool) {
    window_scene::show_workspace_scene(s, id, seed_from_core);
}

fn connect_target(state: &Rc<RefCell<UiState>>, config: TargetConfig) {
    window_connection::connect_target(state, config);
}

fn connect_target_with_intent(
    state: &Rc<RefCell<UiState>>,
    config: TargetConfig,
    intent: ProjectConnectIntent,
) {
    window_connection::connect_target_with_intent(state, config, intent);
}

fn connect_open_request(state: &Rc<RefCell<UiState>>, request: ClientOpenRequest) {
    window_connection::connect_open_request(state, request);
}

/// 最近打开的工作区 → QuickConnect 目标。
///
/// W6 §11.2：优先读 Core 保存的 `ResolvedTarget.canonical`（含 session /
/// socket / workspace_id）；没有 descriptor 时回退旧五段推导（测试/直开）。
fn recent_target_configs(
    view_store: &ViewStore,
    workspace_sockets: &HashMap<WorkspaceId, Option<String>>,
    limit: usize,
) -> Vec<TargetConfig> {
    window_connection::recent_target_configs(view_store, workspace_sockets, limit)
}

/// Owned workspace DTO → QuickConnect 目标（Recents 列表 / 面板高亮）。
///
/// 读 `resolved_target().canonical`（Catalog 打开时保存）；无 descriptor 时
/// 从 WorkspaceId 推导（测试 mock/CLI 直开路径）。
fn workspace_to_target_config(
    workspace: &crate::frontend::ffi_client::ClientWorkspace,
    tmux_socket: Option<&str>,
) -> TargetConfig {
    window_connection::workspace_to_target_config(workspace, tmux_socket)
}

fn open_tmux_attach(state: &Rc<RefCell<UiState>>, parent: &Window, _create_only: bool) {
    window_connection::open_tmux_attach(state, parent, _create_only);
}

fn open_ssh_connect(state: &Rc<RefCell<UiState>>, parent: &Window) {
    window_connection::open_ssh_connect(state, parent);
}

pub(crate) fn chrome_css(theme: &Theme) -> String {
    window_chrome::chrome_css(theme)
}

fn apply_chrome_css(theme: &Theme) {
    window_chrome::apply_chrome_css(theme);
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
    fn existing_workspace_activation_is_scene_only() {
        let src = include_str!("window_scene.rs");
        let activation = fn_src(src, "activate_existing");
        assert!(
            activation.contains("show_workspace_scene(s, id, false)"),
            "{activation}"
        );
        assert!(!activation.contains("activate_workspace"), "{activation}");
        assert!(!activation.contains("sync_view_store"), "{activation}");

        let scene = fn_src(src, "show_workspace_scene");
        assert!(!scene.contains("event_pump"), "{scene}");
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
        assert!(
            !activation.contains("ClientTask::SwitchTab"),
            "{activation}"
        );
        assert!(!activation.contains("command_queue"), "{activation}");

        let scene = fn_src(src, "show_tab_scene");
        assert!(scene.contains("layout.show_tab(tab_id)"), "{scene}");
        assert!(!scene.contains("event_pump"), "{scene}");
        assert!(!scene.contains("execute_active_task"), "{scene}");
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
        let src = include_str!("window.rs");
        let body = fn_src(src, "adjust_font");
        assert!(
            !body.contains("persist_config"),
            "adjust_font 禁止同步 persist_config；防抖或后台写盘。body={body}"
        );
    }
}
