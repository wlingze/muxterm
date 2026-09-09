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
use gtk4::{
    ApplicationWindow, Box, Button, CheckButton, CssProvider, EventControllerKey, HeaderBar, Label,
    Orientation, Paned, Window,
};
use vte4::prelude::*;

use anyhow::anyhow;

use crate::core::attention::clock::RealClock;
use crate::core::attention::engine::{AttentionEngine, PaneAttention};
use crate::core::attention::signal::{AttentionSignal, AttentionSource};
use crate::core::attention::state::PaneStatus;
use crate::core::config::{Action, Config, KeyBinding, OnLastPaneExit, Theme};
use crate::core::config_service::SettingsService;
#[cfg(test)]
use crate::core::protocol::state::StateChange;
use crate::core::protocol::task::TaskOutcome;
use crate::core::quickconnect::model::QuickConnect;
use crate::core::runtime::RuntimeCapability;
use crate::core::types::PaneId;
#[cfg(test)]
use crate::core::types::TabId;
use crate::core::workspace::id::WorkspaceId;
use crate::core::workspace::pool::WorkspaceCapacityCandidate;
use crate::core::workspace::spec::WorkspaceSpec;
use crate::platform::event_pump::EventPump;
use crate::platform::ffi_client::{
    ClientActivitySnapshot, ClientAttentionConfig, ClientAttentionPane, ClientCandidateRef,
    ClientEventKind, ClientOpenIntent, ClientOpenRequest, ClientOpenedWorkspace, ClientTarget,
    ClientTask, ClientWorkspaceAttention, ClientWorkspaceEvent, FfiClient,
};
use crate::platform::i18n::{self, Key};
use crate::platform::linux::attention_ui::{window_title, GioSink, NotificationSink};
use crate::platform::linux::command_palette::{parse_palette_action, PaletteAction};
#[cfg(test)]
use crate::platform::linux::event_batch::batch_order_plan;
use crate::platform::linux::keymap::KeyMap;
use crate::platform::linux::layout_host::LayoutHost;
use crate::platform::linux::lifecycle::{cycle_pane_id, should_close_window};
use crate::platform::linux::pane_view::{PaneMenuAction, PaneView};
use crate::platform::linux::panel_model::PanelTab;
use crate::platform::linux::quickconnect::event_policy::ClientSizePolicy;
use crate::platform::linux::quickconnect::existing::{ExistingEntry, ExistingTransport};
use crate::platform::linux::quickconnect::font::FontSettings;
use crate::platform::linux::quickconnect::model::{TargetConfig, TargetRuntime, TargetTransport};
use crate::platform::linux::quickconnect::project_flow::ProjectConnectIntent;
use crate::platform::linux::quickconnect::status_style::{StatusBarMode, StatusBarSnapshot};
use crate::platform::linux::quickconnect::store::QuickConnectStore;
use crate::platform::linux::quickconnect::tab_gate::TabSwitchGate;
use crate::platform::linux::quickconnect_panel::{
    build_root_items, build_search_items, ExistingNav, ExistingPanelState, PanelItem,
};
use crate::platform::linux::scene_stack::SceneStack;
use crate::platform::linux::status_bar::{ConnectionSummary, StatusBar};
use crate::platform::linux::tmux_dialog::{self, TmuxAction};
use crate::platform::linux::view_store::ViewStore;
use crate::platform::linux::workspace_sidebar::{
    AgentSidebarItem, CommandSidebarItem, WorkspaceSidebar, WorkspaceSidebarItem,
};
use crate::platform::ssh_probe::{classify_ssh_probe, ssh_probe_args, SshReach};

/// 主窗口。
pub struct AppWindow {
    pub window: Window,
    /// 保持 UI 状态与 Core 连接状态存活（轮询闭包只用 Weak，避免循环引用）。
    _state: Rc<RefCell<UiState>>,
}

struct UiState {
    /// 唯一 Core owner：生产 GTK 不再直接持有 WorkspacePool。
    event_pump: EventPump,
    /// 每个工作区一个像素缓存（VTE 不随切走销毁；Runtime 不在 GUI）。
    pixel_cache: std::collections::HashMap<WorkspaceId, LayoutHost>,
    /// 常驻 Workspace Scene 的产品身份与可见场景。
    scene_stack: SceneStack,
    /// GTK scene container. Every live workspace keeps its LayoutHost page.
    scene_stack_view: gtk4::Stack,
    /// 前端拥有的 workspace topology/render 快照；Core 不持有其引用。
    view_store: ViewStore,
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
    tab_gate: TabSwitchGate,
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
    /// 窗口根容器（挂载当前工作区的 LayoutHost.root_box）。
    root_box: gtk4::Box,
    /// 终端区 Overlay：常驻 workspace scene stack 是主 child，回底按钮浮在上面。
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
        let key = self
            .view_store
            .active_workspace_id()
            .expect("必须有前台连接");
        parse_workspace_id(key).expect("Core workspace id 必须保持五段格式")
    }

    fn active_layout(&self) -> &LayoutHost {
        let id = self.active_ws_id();
        self.pixel_cache
            .get(&id)
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
        self.active_workspace_runtime()
            .is_some_and(|runtime| matches!(runtime, "tmux" | "ssh" | "tmux-ssh"))
    }

    fn active_workspace_runtime(&self) -> Option<&str> {
        let id = self.view_store.active_workspace_id()?;
        self.view_store
            .workspace(id)
            .and_then(|view| view.workspace.as_ref())
            .map(|workspace| workspace.runtime.as_str())
    }

    fn active_supports(&self, capability: RuntimeCapability) -> bool {
        let Some(workspace_id) = self.view_store.active_workspace_id() else {
            return false;
        };
        self.workspace_supports(workspace_id, capability)
    }

    fn workspace_supports(&self, workspace_id: &str, capability: RuntimeCapability) -> bool {
        let Some(runtime) = self
            .view_store
            .workspace(workspace_id)
            .and_then(|view| view.workspace.as_ref())
            .map(|workspace| workspace.runtime.as_str())
        else {
            return false;
        };
        let wanted = format!("{capability:?}");
        self.event_pump
            .client()
            .runtime_list()
            .ok()
            .into_iter()
            .flatten()
            .find(|provider| provider.id == runtime)
            .is_some_and(|provider| provider.support.iter().any(|item| item == &wanted))
    }

    fn execute_active_task(&self, task: ClientTask) -> anyhow::Result<()> {
        let workspace_id = self
            .view_store
            .active_workspace_id()
            .ok_or_else(|| anyhow!("没有激活的 workspace"))?;
        let rc = self
            .event_pump
            .client()
            .execute_workspace_task(workspace_id, task);
        if rc == 0 {
            Ok(())
        } else {
            anyhow::bail!("Core FFI task dispatch failed: workspace={workspace_id}, code={rc}")
        }
    }
}

fn poll_event_store(s: &mut UiState) -> Vec<ClientWorkspaceEvent> {
    let event_pump = &s.event_pump;
    event_pump.poll_into_with_events(&mut s.view_store)
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

    for workspace in s.attention.snapshot() {
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
            let pane_has_compatibility_state = pane.status != PaneStatus::Unknown
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
            let pane = client_attention_pane(&pane);
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

fn client_attention_pane(pane: &PaneAttention) -> ClientAttentionPane {
    ClientAttentionPane {
        workspace_id: pane.workspace_id.clone(),
        pane_id: pane.pane_id,
        status: format!("{:?}", pane.status).to_lowercase(),
        acknowledged: pane.acknowledged,
        last_line: pane.last_line.clone(),
        seq: pane.seq,
        process_name: pane.process_name.clone(),
        process_is_agent: pane.process_is_agent,
        agent_name: pane.agent_name.clone(),
        shell_name: pane.shell_name.clone(),
    }
}

fn panel_attention_rows(snapshot: &ClientActivitySnapshot) -> Vec<PaneAttention> {
    snapshot
        .workspaces
        .iter()
        .flat_map(|workspace| workspace.panes.iter())
        .map(|pane| PaneAttention {
            workspace_id: pane.workspace_id.clone(),
            pane_id: pane.pane_id,
            status: match pane.status.as_str() {
                "idle" => PaneStatus::Idle,
                "working" => PaneStatus::Working,
                "blocked" => PaneStatus::Blocked,
                "done" => PaneStatus::Done,
                _ => PaneStatus::Unknown,
            },
            acknowledged: pane.acknowledged,
            last_line: pane.last_line.clone(),
            seq: pane.seq,
            process_name: pane.process_name.clone(),
            process_is_agent: pane.process_is_agent,
            agent_name: pane.agent_name.clone(),
            shell_name: pane.shell_name.clone(),
            mute_until: None,
            last_regex_eval: Instant::now(),
        })
        .collect()
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
            let _ = s.event_pump.client().shutdown();
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
        Self::new_with_keybindings(cfg.clone(), theme, cfg.keybindings.clone())
    }

    /// Construct the window with shortcuts resolved from the Core shortcut
    /// config (preset + primary key + overrides) instead of the legacy list.
    pub fn new_with_effective_keybindings(
        cfg: Config,
        theme: Theme,
        shortcuts: &crate::core::config_service::ShortcutConfig,
    ) -> Self {
        let keybindings =
            crate::core::config_service::action_catalog::resolve_effective_keybindings(shortcuts);
        Self::new_with_keybindings(cfg, theme, keybindings)
    }

    fn new_with_keybindings(cfg: Config, theme: Theme, keybindings: Vec<KeyBinding>) -> Self {
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
        let attention_config = ClientAttentionConfig {
            enabled: cfg.attention.enabled,
            blocked_regex: cfg.attention.blocked_regex.clone(),
            debounce_ms: cfg.attention.debounce_ms,
        };
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

        let root = Box::builder()
            .orientation(Orientation::Vertical)
            .spacing(0)
            .build();
        root.add_css_class("muxterm-root");

        let theme_name = cfg.theme.name.clone().to_ascii_lowercase();
        let theme = Theme::load(&theme_name).unwrap_or(theme);
        apply_chrome_css(&theme);
        let config_font_size = cfg.font.size;
        let font = FontSettings {
            family: cfg.font.family.clone(),
            size: cfg.font.size,
            fallback: cfg.font.fallback.clone(),
        };
        let status_mode = StatusBarMode::from_toml(Some(&cfg.statusbar.mode));
        let sidebar = WorkspaceSidebar::new();

        let header = HeaderBar::new();
        header.set_widget_name("muxterm-header-bar");
        header.pack_start(&sidebar.toggle);
        let quick_connect_button = Button::with_label("⚡");
        quick_connect_button.set_widget_name("muxterm-quick-connect-button");
        quick_connect_button.set_has_frame(false);
        quick_connect_button.set_can_focus(false);
        header.pack_start(&quick_connect_button);
        let settings_button = Button::with_label("⚙");
        settings_button.set_widget_name("muxterm-settings-button");
        settings_button.set_has_frame(false);
        settings_button.set_can_focus(false);
        header.pack_end(&settings_button);
        let title_label = Label::new(Some("muxterm"));
        title_label.set_widget_name("muxterm-title-label");
        header.set_title_widget(Some(&title_label));
        window.set_titlebar(Some(&header));

        let uses_tmux = view_store
            .workspace(startup_key)
            .and_then(|view| view.workspace.as_ref())
            .is_some_and(|workspace| {
                matches!(workspace.runtime.as_str(), "tmux" | "ssh" | "tmux-ssh")
            });
        let mut pixel_cache = std::collections::HashMap::new();
        let layout = LayoutHost::new(theme.clone(), font.clone(), uses_tmux, cfg.scrollback.lines);
        pixel_cache.insert(startup_id.clone(), layout);
        let status = StatusBar::new(status_mode, theme.clone());
        status.container.add_css_class("status-bar");

        // 唯一 chrome：一条 status bar（LINUX-PLAN §3），没有第二条 TabBar。
        // 终端区包一层 Overlay：回底按钮浮在 VTE 右下角（W16a）。
        let scene_stack_view = gtk4::Stack::builder()
            .hexpand(true)
            .vexpand(true)
            .transition_type(gtk4::StackTransitionType::None)
            .build();
        scene_stack_view.set_widget_name("muxterm-scene-stack");
        scene_stack_view.add_named(
            &pixel_cache
                .get(&startup_id)
                .expect("startup layout")
                .root_box,
            Some(&startup_id.as_str()),
        );
        scene_stack_view.set_visible_child_name(&startup_id.as_str());
        let layout_overlay = gtk4::Overlay::new();
        layout_overlay.set_hexpand(true);
        layout_overlay.set_vexpand(true);
        layout_overlay.set_child(Some(&scene_stack_view));
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
        // 左侧栏与右侧终端 chrome 是同一个水平 Paned 的两列。Tab/status
        // chrome 属于右列，不能延伸到侧栏下方；Paned 的 handle 同时提供
        // 用户可调宽度，避免用一个 hexpand 空壳制造中间空白。
        let terminal_column = Box::builder()
            .orientation(Orientation::Vertical)
            .spacing(0)
            .hexpand(true)
            .vexpand(true)
            .build();
        terminal_column.set_widget_name("muxterm-terminal-column");
        terminal_column.append(&layout_overlay);
        terminal_column.append(&status.container);

        let content = Paned::new(Orientation::Horizontal);
        content.set_widget_name("muxterm-content");
        content.add_css_class("muxterm-main-split");
        content.set_hexpand(true);
        content.set_vexpand(true);
        content.set_wide_handle(false);
        content.set_resize_start_child(false);
        content.set_shrink_start_child(false);
        content.set_resize_end_child(true);
        content.set_shrink_end_child(true);
        content.set_start_child(Some(&sidebar.container));
        content.set_end_child(Some(&terminal_column));
        content.set_position(280);
        root.append(&content);
        window.set_child(Some(&root));

        layout_overlay.add_overlay(&pane_find);
        layout_overlay.add_overlay(&search_highlight);
        layout_overlay.add_overlay(&disconnect_overlay);
        layout_overlay.add_overlay(&last_seen_mark);
        layout_overlay.add_overlay(&cmd_mark_ok);
        layout_overlay.add_overlay(&cmd_mark_fail);
        layout_overlay.add_overlay(&jump_latest);

        let keymap = KeyMap::from_bindings(&keybindings);
        let qc_store =
            QuickConnectStore::new_unified(crate::core::config::Config::user_config_path());
        let state = Rc::new(RefCell::new(UiState {
            event_pump,
            pixel_cache,
            scene_stack: SceneStack::with_visible(startup_id.as_str()),
            scene_stack_view,
            view_store,
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
            status_mode,
            last_status_at: Instant::now()
                .checked_sub(Duration::from_secs(10))
                .unwrap_or_else(Instant::now),
            status_interval: Duration::from_secs(1),
            keymap,
            active_tab: 0,
            active_pane: 0,
            last_client_size: None,
            last_pane_sizes: HashMap::new(),
            hold_pane_resize_until: None,
            pending_client_size: None,
            pending_client_hits: 0,
            tab_gate: TabSwitchGate::new(Duration::from_millis(1500)),
            on_last_pane_exit: cfg.behavior.on_last_pane_exit,
            pending_close: false,
            attention: AttentionEngine::new(cfg.attention.clone(), RealClock),
            notification_log: Vec::new(),
            notification_sink: std::boxed::Box::new(GioSink::new(None)),
            panel_open: None,
            quit_requested: false,
            capacity_limit: cfg.pool.max_slots.max(1) as usize,
            capacity_warning_presented_for_slot_count: None,
            runtime_status: crate::core::protocol::ffi::types::BACKEND_STATUS_CONNECTED,
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

        {
            let st = state.clone();
            let mut s = state.borrow_mut();
            if let Some(layout) = s.pixel_cache.get_mut(&startup_id) {
                layout.set_menu_callback(move |pane_id, action| {
                    handle_pane_menu_action(&st, pane_id, action);
                });
            }
        }

        {
            let st = state.clone();
            let win = window.clone();
            settings_button.connect_clicked(move |_| {
                open_preferences(&st, &win);
            });
        }

        {
            let st = state.clone();
            let win = window.clone();
            quick_connect_button.connect_clicked(move |_| {
                open_quick_connect(&st, &win);
            });
        }

        {
            let st = state.clone();
            state
                .borrow()
                .sidebar
                .connect_workspace_activated(move |id| {
                    let mut s = st.borrow_mut();
                    activate_existing(&mut s, id.clone());
                });
        }

        {
            let st = state.clone();
            state.borrow().sidebar.connect_workspace_closed(move |id| {
                close_sidebar_workspace(&mut st.borrow_mut(), id);
            });
        }

        {
            let st = state.clone();
            state
                .borrow()
                .sidebar
                .connect_agent_activated(move |id, pane| {
                    activate_sidebar_activity(&mut st.borrow_mut(), id, pane);
                });
        }

        {
            let st = state.clone();
            state
                .borrow()
                .sidebar
                .connect_command_activated(move |id, pane| {
                    activate_sidebar_activity(&mut st.borrow_mut(), id, pane);
                });
        }

        {
            let st = state.clone();
            let toggle = state.borrow().sidebar.toggle.clone();
            toggle.connect_toggled(move |button| {
                if button.is_active() {
                    refresh_sidebar_if_open(&mut st.borrow_mut());
                }
            });
        }

        {
            let s = state.borrow();
            let workspaces = sidebar_workspaces(&s);
            let activity = activity_snapshot(&s);
            let agents = sidebar_agents(&s, &activity);
            let commands = sidebar_commands(&s, &activity);
            s.sidebar.set_workspaces(&workspaces);
            s.sidebar.set_agents(&agents);
            s.sidebar.set_commands(&commands);
        }

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
                let workspace_id = active_workspace_id(&s);
                let hit = s.event_pump.client().search_all(&q).ok().and_then(|hits| {
                    hits.into_iter()
                        .find(|hit| hit.workspace_id == workspace_id && hit.pane_id == pane)
                });
                if let Some(hit) = hit {
                    if let Some(row) = s.event_pump.client().workspace_pane_viewport_for_seq(
                        &workspace_id,
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
                let n = activity_snapshot(&st.borrow()).blocked_count;
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
                let s = st.borrow();
                let _ = s.execute_active_task(ClientTask::NewTab);
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
                let outcome = crate::platform::linux::fault_gtk::run("linux.poll", || {
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
        let _ = s.event_pump.send_input(&workspace_key, pane, data);
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
            .active_workspace_runtime()
            .unwrap_or_default()
            .to_string()
    }

    /// 测试用：能力判断必须走 Runtime 契约，不能按 runtime 名字分支。
    pub fn test_active_runtime_supports(&self, capability: RuntimeCapability) -> bool {
        let s = self._state.borrow();
        s.active_supports(capability)
    }

    /// 测试用：对当前 Runtime 执行真实 detach，并保留精确 outcome。
    pub fn test_detach_active_workspace_outcome(&self) -> anyhow::Result<TaskOutcome> {
        let s = self._state.borrow();
        match s.execute_active_task(ClientTask::Detach) {
            Ok(()) => Ok(TaskOutcome::Done),
            Err(error) => Ok(TaskOutcome::Rejected {
                reason: error.to_string(),
            }),
        }
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
        let active_tab = view
            .tabs
            .iter()
            .find(|tab| tab.is_active)
            .map(|tab| tab.id)
            .unwrap_or(s.active_tab);
        let Some(layout) = view.layouts.get(&active_tab) else {
            return Vec::new();
        };
        fn leaves(layout: &crate::platform::ffi_client::ClientLayout, out: &mut Vec<u32>) {
            match layout {
                crate::platform::ffi_client::ClientLayout::Leaf { pane_id } => out.push(*pane_id),
                crate::platform::ffi_client::ClientLayout::Split { first, second, .. } => {
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
        let active_tab = view
            .tabs
            .iter()
            .find(|tab| tab.is_active)
            .map(|tab| tab.id)
            .unwrap_or(s.active_tab);
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

    /// 测试用：绕过 tmux 直接向 Surface/AttentionEngine 注入字节。
    pub fn test_feed_replica(&self, pane_id: u32, bytes: &[u8]) {
        let mut s = self._state.borrow_mut();
        if let Some(view) = s.active_layout().pane(pane_id).cloned() {
            view.feed_output(bytes);
            view.flush_deferred_feed();
        }
        apply_test_replica_attention(&mut s, pane_id, bytes);
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
        status: crate::core::attention::state::PaneStatus,
    ) {
        let mut s = self._state.borrow_mut();
        let ws = active_workspace_id(&s);
        s.attention
            .set_agent_process_name(&ws, pane, Some(process_name.to_string()));
        s.attention.apply(
            &ws,
            pane,
            &[AttentionSignal::AuthoritativeStatus {
                status,
                initial: false,
            }],
            "",
            1,
        );
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

    /// 测试用：后台打开任意 `WorkspaceSpec`（SSH loopback 必须带远端 `-L`）。
    ///
    /// 等连接完成并激活后再返回：测试随后 `wait_ready` / 取 leaf 时看到的是
    /// 新工作区，而不是启动时的本地 shell（W18b 的 pane id 才不会串）。
    pub fn test_open_spec(&self, spec: WorkspaceSpec) {
        let id = spec.id();
        let target = ClientTarget {
            name: spec.name(),
            runtime: spec.runtime.clone(),
            transport: spec.transport.clone(),
            target: spec.alias.clone(),
            path: spec.path.clone(),
            session: (!spec.session.is_empty()).then(|| spec.session.clone()),
            socket: spec.socket.clone(),
        };
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
                    s.workspace_sockets.insert(opened_id, spec.socket.clone());
                }
                if sync_view_store(&mut s).is_ok() {
                    after_activate(&mut s);
                }
            }
            Err(error) => {
                self._state
                    .borrow_mut()
                    .notification_log
                    .push(format!("{}: connect failed: {error}", id.replica_id()));
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
        let workspace_id = active_workspace_key(&s);
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
        let workspace_id = active_workspace_key(&s);
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

fn handle_action(s: &mut UiState, action: Action, window: &Window, state: &Rc<RefCell<UiState>>) {
    match action {
        Action::NewTab | Action::NewWindow => {
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
                let s = state.borrow();
                s.execute_active_task(ClientTask::Detach).is_ok()
            };
            if should_quit {
                request_quit_close(state, window);
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
            let s = state.borrow();
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
            let _ = s.execute_active_task(ClientTask::CloseTab { tab_id: tab });
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

/// 通过 Core SettingsService 事务写回 config.toml（唯一事实源）。
/// 平台禁止直接解析或写 TOML；失败只记日志，不覆盖用户文件。
pub fn persist_config(dotted: &str, value: serde_json::Value) {
    let Some(path) = Config::user_config_path() else {
        return;
    };
    let mut service = match SettingsService::open(&path) {
        Ok(service) => service,
        Err(error) => {
            tracing::warn!(target = "muxterm::config", "打开配置事务失败: {error}");
            return;
        }
    };
    let Ok(pointer) = crate::core::config_service::dotted_pointer(dotted) else {
        return;
    };
    let transaction = service.begin();
    if let Err(error) = service
        .patch(
            &transaction,
            &[crate::core::config_service::JsonPatchOperation {
                op: "replace".into(),
                path: pointer,
                value: Some(value),
            }],
        )
        .and_then(|_| service.commit(&transaction).map(|_| ()))
    {
        tracing::warn!(target = "muxterm::config", "保存设置失败: {error}");
        let _ = service.cancel(&transaction);
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
                persist_config("font.size", serde_json::Value::from(f64::from(size)));
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
    persist_config(
        "font.size",
        serde_json::Value::from(f64::from(s.config_font_size)),
    );
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
    persist_config(
        "theme.name",
        serde_json::Value::String(next_name.to_string()),
    );
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
        serde_json::Value::String(next.as_str().to_string()),
    );
    maybe_refresh_status(s, true);
}

fn report_all_pane_colours(s: &mut UiState) {
    if !s.uses_tmux() {
        return;
    }
    let fg = s.theme.foreground;
    let bg = s.theme.background;
    let _ = (fg, bg);
    let _ = s.event_pump.client().report_all_pane_colours(
        &format!("#{:02x}{:02x}{:02x}", fg.0, fg.1, fg.2),
        &format!("#{:02x}{:02x}{:02x}", bg.0, bg.1, bg.2),
    );
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
        let text = crate::platform::mirror::sanitize_paste(text.as_str(), bracketed);
        let data = crate::platform::mirror::encode_clipboard_paste(&text, bracketed);
        if data.is_empty() {
            return;
        }
        let Some(st) = st.upgrade() else {
            return;
        };
        let s = st.borrow();
        let workspace_id = active_workspace_key(&s);
        let _ = s.event_pump.send_input(&workspace_id, pane_id, &data);
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
    let workspace_id = active_workspace_key(s);
    if let Some(tab_id) = s
        .view_store
        .workspace(&workspace_id)
        .and_then(|view| view.tabs.get(n.saturating_sub(1)).map(|tab| tab.id))
    {
        request_switch_tab(s, tab_id);
    }
}

fn switch_workspace_n(s: &mut UiState, n: usize) {
    let target = s
        .view_store
        .workspaces()
        .nth(n.saturating_sub(1))
        .and_then(|(_, view)| view.workspace.as_ref())
        .and_then(|workspace| parse_workspace_id(&workspace.id));
    if let Some(target) = target {
        if s.active_ws_id() != target {
            activate_existing(s, target);
        }
    }
}

fn request_switch_tab(s: &mut UiState, tab_id: u32) {
    if tab_id == s.active_tab {
        return;
    }
    s.tab_gate.request(tab_id);
    let _ = s.execute_active_task(ClientTask::SwitchTab { tab_id });
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
    let n = activity_snapshot(s).blocked_count;
    s.status.set_attention(n);
    let workspace = s
        .view_store
        .active_workspace_id()
        .and_then(|id| s.view_store.workspace(id))
        .and_then(|view| view.workspace.as_ref())
        .map(|workspace| workspace.name.clone())
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
    let Some(workspace_id) = s.view_store.active_workspace_id() else {
        return;
    };
    let Some(workspace) = s
        .view_store
        .workspace(workspace_id)
        .and_then(|view| view.workspace.as_ref())
    else {
        return;
    };
    let Some(id) = parse_workspace_id(&workspace.id) else {
        return;
    };
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
    let (down, up) = s.event_pump.client().traffic_bytes();
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
    s.view_store
        .active_workspace_id()
        .and_then(parse_workspace_id)
        .map(|id| workspace_replica_id(&id))
        .unwrap_or_default()
}

/// Current Core workspace identity used at FFI boundaries.
fn active_workspace_key(s: &UiState) -> String {
    s.view_store
        .active_workspace_id()
        .unwrap_or_default()
        .to_string()
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
) -> Option<std::rc::Rc<crate::platform::linux::pane_view::PaneView>> {
    s.pixel_cache
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

/// Recreate the two legacy direct-injection cases used by GTK tests without
/// making them a second production event source. Real Runtime output is
/// already applied by Core before the workspace event poll returns.
fn apply_test_replica_attention(s: &mut UiState, pane: u32, bytes: &[u8]) {
    let workspace = active_workspace_id(s);
    let seq = s
        .attention
        .snapshot()
        .into_iter()
        .flat_map(|workspace| workspace.panes)
        .map(|pane| pane.seq)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let text = String::from_utf8_lossy(bytes);
    let last_line = text
        .split('\n')
        .rev()
        .map(|line| line.trim_matches(|ch: char| ch == '\r' || ch.is_control()))
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .to_string();

    let command_start = b"\x1b]133;B\x07";
    let command_end = b"\x1b]133;C\x07";
    if let (Some(start), Some(end)) = (
        find_bytes(bytes, command_start).map(|index| index + command_start.len()),
        find_bytes(bytes, command_end),
    ) {
        if start <= end {
            let command = String::from_utf8_lossy(&bytes[start..end]);
            if !command.trim().is_empty() {
                s.attention
                    .set_process_name(&workspace, pane, Some(command.trim().to_string()));
            }
        }
    }

    let has_osc133 = find_bytes(bytes, b"\x1b]133;").is_some();
    let signal = if let Some(index) = find_bytes(bytes, b"\x1b]133;D") {
        let exit_code = bytes[index + b"\x1b]133;D".len()..]
            .strip_prefix(b";")
            .and_then(|value| value.split(|byte| *byte == b'\x07').next())
            .and_then(|value| std::str::from_utf8(value).ok())
            .and_then(|value| value.parse::<u8>().ok());
        Some(AttentionSignal::CommandDone { exit_code })
    } else if !has_osc133 && bytes.contains(&0x07) {
        Some(AttentionSignal::AttentionRequest {
            source: AttentionSource::Bel,
        })
    } else {
        None
    };
    if let Some(signal) = signal {
        s.attention
            .apply(&workspace, pane, &[signal], &last_line, seq);
    }
    if pane == s.active_pane {
        s.attention.on_became_visible(&workspace, pane);
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    (!needle.is_empty())
        .then(|| {
            haystack
                .windows(needle.len())
                .position(|window| window == needle)
        })
        .flatten()
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
    // events are drained from Core above and never pass through this engine.
    for ws in s.attention.take_new_blocked_notifications() {
        record_attention_notification(s, &ws, "blocked");
    }
    for ws in s.attention.take_new_done_notifications() {
        record_attention_notification(s, &ws, "done");
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
    let wid = s.active_ws_id();
    refresh_workspace_layout(s, &wid);
    maybe_refresh_status(s, true);
    sync_chrome_visibility(s);
}

/// Commit one workspace's final topology to its persistent LayoutHost.
///
/// This function intentionally does not switch the active window for a
/// background workspace.  It only creates/reparents resident PaneViews and
/// feeds an already-realized Surface from the frontend-owned ViewStore state.
fn refresh_workspace_layout(s: &mut UiState, wid: &WorkspaceId) {
    let is_active = s.active_ws_id() == *wid;
    let workspace_key = wid.as_str();
    let Some(view) = s.view_store.workspace(&workspace_key) else {
        return;
    };
    let tab_ids: Vec<u32> = view.tabs.iter().map(|tab| tab.id).collect();
    let active_tab = view
        .tabs
        .iter()
        .find(|tab| tab.is_active)
        .map(|tab| tab.id)
        .or_else(|| tab_ids.first().copied());
    // W4：topology sync 必须为**所有** tab 的 leaves 建立常驻 PaneView，
    // 不能只建 active tab；hidden tab 的 frame/output 隐藏期间继续 feed。
    let layouts = tab_ids
        .iter()
        .filter_map(|tab_id| {
            view.layouts
                .get(tab_id)
                .cloned()
                .map(|layout| (*tab_id, layout))
        })
        .collect::<Vec<_>>();
    let panes: Vec<(u32, u16, u16, bool)> = active_tab
        .and_then(|tab_id| view.panes.get(&tab_id))
        .map(|panes| {
            panes
                .iter()
                .map(|pane| (pane.id, pane.cols, pane.rows, pane.is_active))
                .collect()
        })
        .unwrap_or_default();

    s.scene_stack.ensure(&workspace_key);

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
            for (tab, client_layout) in &layouts {
                layout.apply_client_layout(*tab, client_layout, &input_cb);
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
                    // 临时输入面板存在时不能由 topology refresh 抢走焦点；
                    // 没有输入面板时，键盘归当前 terminal。
                    if s.panel_open.is_none() && !s.pane_find.is_visible() {
                        view.grab_focus();
                    }
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
    let workspace_key = s.active_ws_id().as_str();
    let Some(view) = s.view_store.workspace(&workspace_key) else {
        return;
    };
    let session = view
        .workspace
        .as_ref()
        .map(|workspace| workspace.name.clone())
        .unwrap_or_default();
    let active_tab = view
        .tabs
        .iter()
        .find(|tab| tab.is_active)
        .map(|tab| tab.id)
        .unwrap_or(s.active_tab);
    let npanes = view.panes.get(&active_tab).map(Vec::len).unwrap_or(0);
    let rows: Vec<(u32, String, bool)> = view
        .tabs
        .iter()
        .map(|tab| (tab.id, tab.name.clone(), tab.is_active))
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
    let worktree = s.active_supports(RuntimeCapability::WorktreeList);
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
        if let Err(error) = s
            .event_pump
            .send_input(&workspace_id.as_str(), pane_id.0, &data)
        {
            tracing::warn!(
                target = "muxterm::surface",
                workspace = %workspace_id,
                pane = %pane_id.0,
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
    // Every opened scene consumes its own render mailbox. Hidden workspaces
    // keep feeding their resident surfaces so navigation never needs a
    // recapture or reset.
    let workspace_ids: Vec<String> = s.view_store.workspace_ids().map(str::to_string).collect();
    for workspace_key in workspace_ids {
        let Some(wid) = parse_workspace_id(&workspace_key) else {
            continue;
        };
        let panes: Vec<(u32, u16, u16)> = s
            .view_store
            .workspace(&workspace_key)
            .map(|view| {
                view.panes
                    .values()
                    .flat_map(|panes| panes.iter())
                    .map(|pane| (pane.id, pane.cols, pane.rows))
                    .collect()
            })
            .unwrap_or_default();
        for (pane_id, cols, rows) in panes {
            let Some(view) = resident_pane_view(s, &wid, pane_id) else {
                continue;
            };
            view.ensure_grid_size(cols, rows);
            seed_unseeded_pane_for(s, &wid, &view, pane_id, cols, rows);
            if view.is_seeded() {
                drain_view_store_render_events(s, &wid, &view, pane_id);
                forward_parser_replies_for_key(s, &workspace_key, pane_id);
            }
        }
    }
}

fn refresh_event_workspaces(s: &mut UiState, events: &[ClientWorkspaceEvent]) {
    let workspace_ids: Vec<WorkspaceId> = events
        .iter()
        .filter(|event| event.event.is_topology())
        .filter_map(|event| parse_workspace_id(&event.workspace_id))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    for workspace_id in workspace_ids {
        refresh_workspace_layout(s, &workspace_id);
    }
    apply_attention_visibility_events(s, events);
    mark_active_attention_visible(s);
}

fn apply_attention_visibility_events(s: &UiState, events: &[ClientWorkspaceEvent]) {
    let Some(active_workspace) = s.view_store.active_workspace_id() else {
        return;
    };
    for event in events
        .iter()
        .filter(|event| event.workspace_id == active_workspace)
    {
        let pane = match event.event.type_ {
            crate::ffi::types::STATE_ACTIVE_PANE_CHANGED => Some(event.event.pane_id),
            crate::ffi::types::STATE_PANE_OUTPUT
            | crate::ffi::types::STATE_PANE_FRAME
            | crate::ffi::types::STATE_PANE_SNAPSHOT
            | crate::ffi::types::STATE_PANE_HISTORY
            | crate::ffi::types::STATE_PANE_AGENT_CHANGED
            | crate::ffi::types::STATE_STATUS_SUBSCRIPTION
                if event.event.pane_id == s.active_pane =>
            {
                Some(event.event.pane_id)
            }
            _ => None,
        };
        let Some(pane) = pane else {
            continue;
        };
        let _ = s
            .event_pump
            .client()
            .workspace_attention_on_became_visible(&event.workspace_id, pane);
    }
}

fn mark_active_attention_visible(s: &UiState) {
    let workspace_id = active_workspace_key(s);
    if workspace_id.is_empty() {
        return;
    }
    let _ = s
        .event_pump
        .client()
        .workspace_attention_on_became_visible(&workspace_id, s.active_pane);
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
    let workspace_key = wid.as_str();
    let bytes = s
        .view_store
        .take_pane_baseline(&workspace_key, pane_id)
        .or_else(|| {
            let bytes = s
                .event_pump
                .client()
                .get_workspace_pane_output(&workspace_key, pane_id);
            (!bytes.is_empty()).then_some(bytes)
        });
    if let Some(bytes) = bytes {
        tracing::info!(
            target: "muxterm::surface",
            pane = pane_id,
            bytes = bytes.len(),
            "surface baseline seed from current-generation frame"
        );
        view.seed_raw(&bytes, cols, rows);
        s.snapshot_seeded_this_batch.insert(pane_id);
        drain_view_store_render_events(s, wid, view, pane_id);
    } else {
        tracing::info!(
            target: "muxterm::surface",
            pane = pane_id,
            "pane view unseeded and no queued or compatibility baseline is available"
        );
    }
}

/// Flush render events retained while a pane had no realized Surface.
fn drain_view_store_render_events(
    s: &mut UiState,
    wid: &WorkspaceId,
    view: &std::rc::Rc<PaneView>,
    pane_id: u32,
) {
    let workspace_key = wid.as_str();
    for event in s
        .view_store
        .take_pane_render_events(&workspace_key, pane_id)
    {
        match event.kind() {
            ClientEventKind::PaneHistory => view.prepend_history(&event.data),
            ClientEventKind::PaneFrame => view.feed_full(&event.data),
            ClientEventKind::PaneOutput => view.feed_output(&event.data),
            ClientEventKind::PaneSnapshot
            | ClientEventKind::PaneClosed
            | ClientEventKind::PaneResized
            | ClientEventKind::Other(_) => {}
        }
    }
    view.flush_deferred_history();
    view.flush_deferred_feed();
}

fn sync_pane_grid_size(s: &UiState, pane_id: u32) {
    let Some(view) = s.active_layout().pane(pane_id) else {
        return;
    };
    let workspace_key = s.active_ws_id().as_str();
    let active_tab = s
        .view_store
        .workspace(&workspace_key)
        .and_then(|view| view.tabs.iter().find(|tab| tab.is_active).map(|tab| tab.id))
        .unwrap_or(s.active_tab);
    let Some(pane) = s
        .view_store
        .workspace(&workspace_key)
        .and_then(|workspace| workspace.panes.get(&active_tab))
        .and_then(|panes| panes.iter().find(|pane| pane.id == pane_id))
    else {
        return;
    };
    view.ensure_grid_size(pane.cols, pane.rows);
}

/// 按 `(WorkspaceId, PaneId)` 对齐字符格（hidden tab / background 也适用）。
fn sync_pane_grid_size_for(s: &UiState, wid: &WorkspaceId, pane_id: u32) {
    let Some(view) = resident_pane_view(s, wid, pane_id) else {
        return;
    };
    let workspace_key = wid.as_str();
    let (cols, rows) = s
        .view_store
        .workspace(&workspace_key)
        .and_then(|workspace| {
            workspace
                .panes
                .values()
                .flat_map(|panes| panes.iter())
                .find(|pane| pane.id == pane_id)
        })
        .map(|pane| (pane.cols, pane.rows))
        .unwrap_or((80, 24));
    view.ensure_grid_size(cols, rows);
}

fn forward_parser_replies(s: &mut UiState, pane_id: u32) {
    let workspace_id = active_workspace_key(s);
    forward_parser_replies_for_key(s, &workspace_id, pane_id);
}

/// 按 WorkspaceId 转发 parser replies（background workspace 也 flush）。
fn forward_parser_replies_for(s: &mut UiState, wid: &WorkspaceId, pane_id: u32) {
    let workspace_id = wid.as_str();
    forward_parser_replies_for_key(s, &workspace_id, pane_id);
}

fn forward_parser_replies_for_key(s: &mut UiState, workspace_id: &str, pane_id: u32) {
    // tmux/SSH mirror 的远端 Runtime 已经负责 query reply；把 GTK 无头
    // parser 的应答写回会把 OSC/DA 字节泄漏到用户 shell。
    if s.workspace_supports(workspace_id, RuntimeCapability::SharedClientResize) {
        return;
    }
    let replies = s
        .event_pump
        .client()
        .take_workspace_pane_reply(workspace_id, pane_id);
    if replies.is_empty() {
        return;
    }
    let _ = s
        .event_pump
        .client()
        .send_workspace_input(workspace_id, pane_id, &replies);
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
    let shared_client_resize = s.active_supports(RuntimeCapability::SharedClientResize);
    if !shared_client_resize {
        sync_visible_pane_sizes(s);
        return;
    }
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
    let workspace_key = s.active_ws_id().as_str();
    let active_tab = s
        .view_store
        .workspace(&workspace_key)
        .and_then(|view| view.tabs.iter().find(|tab| tab.is_active).map(|tab| tab.id))
        .unwrap_or(s.active_tab);
    let multi_pane = s
        .view_store
        .workspace(&workspace_key)
        .and_then(|view| view.panes.get(&active_tab))
        .is_some_and(|panes| panes.len() > 1);
    let cols = match ClientSizePolicy::cols(term.column_count(), allocated, root_w, cw, multi_pane)
    {
        Some(cols) => cols,
        None => return,
    };
    let rows = match ClientSizePolicy::rows(root_h, ch) {
        Some(rows) => rows,
        None => return,
    };
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
    let workspace_id = active_workspace_key(s);
    let _ = s
        .event_pump
        .client()
        .resize_workspace_client(&workspace_id, cols, rows);
}

/// Herdr 没有 SharedClientResize：每个可见 split 格子按自己的 VTE 分配
/// 发 ResizePane。只同步 active pane 会让 0218.log 里 54/57 停在 27×12。
fn sync_visible_pane_sizes(s: &mut UiState) {
    if s.hold_pane_resize_until
        .is_some_and(|until| Instant::now() < until)
    {
        return;
    }
    s.hold_pane_resize_until = None;
    let workspace_key = s.active_ws_id().as_str();
    let active_tab = s
        .view_store
        .workspace(&workspace_key)
        .and_then(|view| view.tabs.iter().find(|tab| tab.is_active).map(|tab| tab.id))
        .unwrap_or(s.active_tab);
    let pane_ids: Vec<u32> = s
        .view_store
        .workspace(&workspace_key)
        .and_then(|view| view.panes.get(&active_tab))
        .map(|panes| panes.iter().map(|pane| pane.id).collect())
        .unwrap_or_default();
    let mut measured = Vec::new();
    for pane in pane_ids {
        let Some(view) = s.active_layout().pane(pane) else {
            continue;
        };
        let term = view.terminal();
        let cw = term.char_width();
        let ch = term.char_height();
        if cw <= 0 || ch <= 0 || term.width() <= 0 || term.height() <= 0 {
            continue;
        }
        let cols = (i64::from(term.width()) / cw).clamp(2, i64::from(u16::MAX)) as u16;
        let rows = (i64::from(term.height()) / ch).clamp(1, i64::from(u16::MAX)) as u16;
        view.ensure_grid_size(cols, rows);
        measured.push((pane, cols, rows));
        if pane == s.active_pane {
            s.last_client_size = Some((Some(pane), cols, rows));
        }
    }
    let resizes = pending_pane_resizes(&s.last_pane_sizes, &measured);
    for &(pane, cols, rows) in &resizes {
        s.last_pane_sizes.insert(pane, (cols, rows));
    }
    for (pane, cols, rows) in resizes {
        let workspace_id = active_workspace_key(s);
        let _ = s
            .event_pump
            .client()
            .resize_workspace_pane(&workspace_id, pane, cols, rows);
    }
}

/// 0218.log：三个可见格子都要有自己的 ResizePane；已同步过的尺寸跳过。
fn pending_pane_resizes(
    last: &HashMap<u32, (u16, u16)>,
    measured: &[(u32, u16, u16)],
) -> Vec<(u32, u16, u16)> {
    measured
        .iter()
        .copied()
        .filter(|(pane, cols, rows)| last.get(pane) != Some(&(*cols, *rows)))
        .collect()
}

/// SSH 可达性缓存 TTL（W15d：面板打开时后台探测，TTL 内复用）。
const SSH_PROBE_TTL: Duration = Duration::from_secs(30);

/// 后台探测一个 SSH 别名（`ssh -o BatchMode=yes -o ConnectTimeout=2 <alias> true`）。
fn spawn_ssh_probe(s: &mut UiState, alias: String) {
    let (tx, rx) = std::sync::mpsc::channel::<(String, SshReach)>();
    s.pending_ssh_probes.push_back(rx);
    std::thread::spawn(move || {
        let args = ssh_probe_args(&alias, 2);
        let status = std::process::Command::new("ssh")
            .args(&args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let reach = match status {
            Ok(st) => classify_ssh_probe(st.code()),
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

fn existing_entries(
    candidates: Vec<crate::platform::ffi_client::ExistingCandidate>,
) -> Vec<ExistingEntry> {
    candidates
        .into_iter()
        .filter_map(|candidate| match ExistingEntry::from_candidate(candidate) {
            Ok(entry) => Some(entry),
            Err(error) => {
                tracing::warn!(
                    target = "muxterm::linux",
                    %error,
                    "ignoring unsupported Existing candidate"
                );
                None
            }
        })
        .collect()
}

/// 已有的连接探测增量：先推 local 行，SSH 完成后再推；Done 才清 inflight。
enum ExistingProbeMsg {
    Aliases(Vec<String>),
    Rows(Vec<ExistingEntry>),
    Done,
}

fn merge_existing_entries(ex: &mut ExistingPanelState, entries: Vec<ExistingEntry>) {
    for e in entries {
        match &e.transport {
            ExistingTransport::Ssh { name } => {
                if !ex.hosts.contains(name) {
                    ex.hosts.push(name.clone());
                }
                let bucket = ex.remote.entry(name.clone()).or_default();
                if !bucket.contains(&e) {
                    bucket.push(e);
                }
            }
            ExistingTransport::Local => {
                if !ex.locals.contains(&e) {
                    ex.locals.push(e);
                }
            }
        }
    }
}

fn append_unique_existing_entries(target: &mut Vec<ExistingEntry>, entries: Vec<ExistingEntry>) {
    for entry in entries {
        if !target.contains(&entry) {
            target.push(entry);
        }
    }
}

/// C7/C9：已有的连接探测。先 `discover_existing("local")` 立刻推表，
/// 再按 SSH host 最多 4 路并发。禁止等 `all` 串完才刷新（archmini 上 cd/mac 会冻 Loading）。
fn spawn_local_existing_probe(s: &mut UiState) {
    s.existing.borrow_mut().probe_inflight = true;
    // Catalog 的通用 local discovery 默认只查默认 tmux server。启动配置或
    // 已打开 workspace 可能使用显式 `-L` socket；把这些已知 socket 作为只读
    // discovery 的附加输入，否则 Existing 搜索会漏掉当前用户可链接的会话。
    let local_tmux_sockets: Vec<String> = {
        let mut sockets = Vec::new();
        if let Some(socket) = s
            .default_socket
            .as_deref()
            .filter(|socket| !socket.is_empty())
        {
            sockets.push(socket.to_string());
        }
        for (id, socket) in &s.workspace_sockets {
            if id.transport != "ssh" {
                if let Some(socket) = socket.as_deref().filter(|socket| !socket.is_empty()) {
                    if !sockets.iter().any(|known| known == socket) {
                        sockets.push(socket.to_string());
                    }
                }
            }
        }
        sockets
    };
    let (tx, rx) = std::sync::mpsc::channel::<ExistingProbeMsg>();
    s.pending_local_probe.push_back(rx);
    std::thread::spawn(move || {
        tracing::debug!(target = "muxterm::linux", "existing probe: local start");
        let mut local =
            existing_entries(FfiClient::discover_existing("local", None, None).unwrap_or_default());
        for socket in local_tmux_sockets {
            let entries = FfiClient::discover_tmux_sessions("local", None, Some(&socket))
                .unwrap_or_default()
                .into_iter()
                .map(|session| {
                    ExistingEntry::tmux(
                        session.name,
                        ExistingTransport::Local,
                        Some(socket.clone()),
                    )
                })
                .collect();
            append_unique_existing_entries(&mut local, entries);
        }
        tracing::debug!(
            target = "muxterm::linux",
            n = local.len(),
            "existing probe: local done"
        );
        let _ = tx.send(ExistingProbeMsg::Rows(local));

        let aliases: Vec<String> = FfiClient::discover_ssh_hosts()
            .unwrap_or_default()
            .into_iter()
            .map(|h| h.alias)
            .collect();
        let _ = tx.send(ExistingProbeMsg::Aliases(aliases.clone()));
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
                            let entries = existing_entries(
                                FfiClient::discover_existing("ssh", Some(&alias), None)
                                    .unwrap_or_default(),
                            );
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
                Ok(msg @ ExistingProbeMsg::Aliases(_)) | Ok(msg @ ExistingProbeMsg::Rows(_)) => {
                    Some(msg)
                }
                Ok(ExistingProbeMsg::Done) => {
                    s.pending_local_probe.pop_front();
                    Some(ExistingProbeMsg::Done)
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    wait = true;
                    None
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    s.pending_local_probe.pop_front();
                    Some(ExistingProbeMsg::Done)
                }
            }
        };
        match msg {
            Some(ExistingProbeMsg::Aliases(aliases)) => {
                let s = state.borrow();
                let mut ex = s.existing.borrow_mut();
                ex.ssh_aliases = aliases;
                drop(ex);
                crate::platform::linux::quickconnect_panel::refresh_current();
            }
            Some(ExistingProbeMsg::Rows(entries)) => {
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
            Some(ExistingProbeMsg::Done) => {
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
    let aliases: Vec<String> = FfiClient::discover_ssh_hosts()
        .unwrap_or_default()
        .into_iter()
        .map(|h| h.alias)
        .collect();
    {
        let s = state.borrow();
        s.existing.borrow_mut().ssh_aliases = aliases.clone();
    }
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
                                let entries = existing_entries(
                                    FfiClient::discover_existing("ssh", Some(alias), None)
                                        .unwrap_or_default(),
                                );
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
/// 只重连声明 `SharedClientResize` 的 Runtime；Core 负责所有 live
/// Workspace 的 reconnect，GTK 只提交一次产品级 reconnect 请求。
fn maybe_schedule_reconnect(state: &Rc<RefCell<UiState>>) {
    let should_retry = {
        let mut s = state.borrow_mut();
        if s.reconnecting
            || s.runtime_status == crate::core::protocol::ffi::types::BACKEND_STATUS_CONNECTED
        {
            return;
        }
        let now = Instant::now();
        if s.reconnect_retry_at.is_some_and(|at| now < at) {
            return;
        }
        let Some(id) = s
            .view_store
            .active_workspace_id()
            .and_then(parse_workspace_id)
        else {
            return;
        };
        if !s.workspace_supports(&id.as_str(), RuntimeCapability::SharedClientResize) {
            return;
        }
        s.reconnecting = true;
        true
    };
    if !should_retry {
        return;
    }
    let result = state.borrow().event_pump.client().reconnect();
    let mut s = state.borrow_mut();
    s.reconnecting = false;
    match result {
        Ok(()) => {
            s.reconnect_attempts = 0;
            s.reconnect_retry_at = None;
            s.disconnect_overlay.set_visible(false);
            drop(s);
            handle_reconnect_success(state, false);
        }
        Err(error) => {
            s.reconnect_attempts = s.reconnect_attempts.saturating_add(1);
            let delay = Duration::from_secs(1u64 << s.reconnect_attempts.min(3));
            s.reconnect_retry_at = Some(Instant::now() + delay);
            tracing::warn!(
                target = "muxterm::linux",
                "reconnect failed (attempt {}): {error}; retry in {delay:?}",
                s.reconnect_attempts
            );
        }
    }
}

/// 重连成功：换 Runtime、隐藏水印；断线期间的 BEL 重新推导成 Blocked。
///
/// 只对「仍处于断线状态」的工作区生效：若断线期间用户已重新 attach
/// （新 Runtime 已插入且 Connected），旧重连结果必须丢弃——否则会换掉
/// 更新的 Runtime，并丢失其尚未消费的 capture 事件（PaneBuf 空、搜索
/// 不到断线前 token）。
fn handle_reconnect_success(state: &Rc<RefCell<UiState>>, bell: bool) {
    let mut s = state.borrow_mut();
    s.reconnect_attempts = 0;
    s.reconnect_retry_at = None;
    s.disconnect_overlay.set_visible(false);
    if bell {
        let ws = active_workspace_id(&s);
        let pane = s.active_pane;
        let workspace_id = active_workspace_key(&s);
        let last_line = s
            .event_pump
            .client()
            .workspace_pane_last_n_lines(&workspace_id, pane, 1)
            .ok()
            .and_then(|lines| lines.last().cloned())
            .unwrap_or_default();
        let seq = s
            .event_pump
            .client()
            .workspace_pane_latest_line_seq(&workspace_id, pane)
            .unwrap_or_default();
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
/// 内部自行 borrow：面板回调会再次借用 state，调用方不能同时持有 RefMut。
fn open_panel(state: &Rc<RefCell<UiState>>, window: &Window, initial_tab: PanelTab) {
    let (workspaces, workspace_search_items, agents, attention, win, st, ssh_reach) = {
        let mut s = state.borrow_mut();
        let recents = recent_target_configs(
            &s.view_store,
            &s.workspace_sockets,
            s.view_store.workspace_ids().count(),
        );
        s.qc_store.replace_all_recents(&recents);
        let current = s
            .view_store
            .active_workspace_id()
            .and_then(|workspace_key| {
                let workspace = s
                    .view_store
                    .workspace(workspace_key)
                    .and_then(|view| view.workspace.as_ref())?;
                let id = parse_workspace_id(workspace_key)?;
                let socket = s
                    .workspace_sockets
                    .get(&id)
                    .and_then(|value| value.as_deref());
                Some(workspace_to_target_config(workspace, socket))
            });
        let store = s.qc_store.clone();
        let win = window.clone();
        let st = state.clone();
        let workspaces = build_root_items(&store, current.as_ref());
        let workspace_search_items = build_search_items(&store, current.as_ref());
        let ssh_reach = collect_ssh_reach(&mut s, &workspaces);
        // 临时输入 surface 互斥：QuickConnect 打开后不保留 pane-find。
        s.pane_find.set_visible(false);
        // C7：本地列出搬后台线程（GTK 线程禁止 ssh / 扫 herdr socket），
        // 结果经 16ms poll 收编，和 SSH probe 同一模式。
        spawn_local_existing_probe(&mut s);
        let activity = activity_snapshot(&s);
        let agents = sidebar_agents(&s, &activity);
        let attention = panel_attention_rows(&activity);
        s.panel_open = Some(initial_tab);
        (
            workspaces,
            workspace_search_items,
            agents,
            attention,
            win,
            st,
            ssh_reach,
        )
    };
    if !window.is_visible() {
        window.present();
    }
    crate::platform::linux::quickconnect_panel::show(
        &win,
        crate::platform::linux::quickconnect_panel::PanelShowArgs {
            initial_tab,
            workspaces,
            workspace_search_items,
            agents,
            attention,
            on_connect: {
                let st = st.clone();
                std::boxed::Box::new(move |request| {
                    connect_open_request(&st, request);
                })
            },
            on_existing_connect: {
                let st = st.clone();
                std::boxed::Box::new(move |request| {
                    connect_open_request(&st, request);
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
            on_mute: {
                let st = st.clone();
                let win = win.clone();
                std::boxed::Box::new(move |ws, pane, duration| {
                    let seconds = duration.as_secs();
                    let rc = {
                        let s = st.borrow();
                        attention_workspace_id(&s, &ws)
                            .map(|workspace_id| {
                                s.event_pump.client().workspace_attention_mute(
                                    &workspace_id.as_str(),
                                    pane,
                                    seconds,
                                )
                            })
                            .unwrap_or(-1)
                    };
                    if rc == 0 {
                        let mut s = st.borrow_mut();
                        refresh_sidebar_if_open(&mut s);
                        refresh_attention_chrome(&s, &win);
                    } else {
                        tracing::warn!(
                            target = "muxterm::linux",
                            "Core attention mute failed: workspace={ws}, pane={pane}, code={rc}"
                        );
                    }
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
                    let workspace_id = active_workspace_key(&s);
                    let hits = s
                        .event_pump
                        .client()
                        .search_all(query)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|hit| match scope {
                            crate::platform::linux::panel_model::SearchScope::Pane => {
                                hit.workspace_id == workspace_id && hit.pane_id == s.active_pane
                            }
                            crate::platform::linux::panel_model::SearchScope::Workspace => {
                                hit.workspace_id == workspace_id
                            }
                            crate::platform::linux::panel_model::SearchScope::All => true,
                        })
                        .map(|hit| crate::platform::linux::panel_model::SearchRow {
                            workspace_id: hit.workspace_id,
                            tab_id: hit.tab_id,
                            pane_id: hit.pane_id,
                            seq: hit.seq,
                            line: hit.line,
                        })
                        .collect();
                    hits
                })
            },
            on_close: {
                let st = st.clone();
                std::boxed::Box::new(move || {
                    let active_view = {
                        let mut s = st.borrow_mut();
                        s.panel_open = None;
                        let pane = s.active_pane;
                        s.active_layout().pane(pane).cloned()
                    };
                    if let Some(view) = active_view {
                        view.grab_focus();
                    }
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
    let workspace_key = active_workspace_key(&s);
    // 按 pane 查所在 tab（SearchRow 已带 tab_id，但回调只传 ws/pane；
    // 这里从 owned topology 反查，结果必须切 tab）。
    let tab_id = {
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
    if let Some(tid) = tab_id {
        if tid != s.active_tab {
            request_switch_tab(&mut s, tid);
        }
    }
    // 激活 pane（若已在前台连接中）。
    let _ = s.execute_active_task(ClientTask::SwitchPane { pane_id: pane });
    // 搜索命中：滚到该行并显示客户端高亮（W17c）。
    if seq > 0 {
        let row = s
            .event_pump
            .client()
            .workspace_pane_viewport_for_seq(&workspace_key, pane, seq);
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
        let key = id.as_str();
        if s.event_pump.client().activate_workspace(&key).is_ok() {
            if let Err(error) = sync_view_store(s) {
                tracing::warn!(
                    target = "muxterm::linux",
                    %error,
                    workspace = %key,
                    "workspace activation snapshot refresh failed"
                );
            }
            after_activate(s);
        }
    }
}

/// 按 workspace_id（name@transport）找 WorkspaceId。
fn attention_workspace_id(s: &UiState, ws: &str) -> Option<WorkspaceId> {
    s.view_store
        .workspaces()
        .filter_map(|(_, view)| view.workspace.as_ref())
        .filter_map(|workspace| parse_workspace_id(&workspace.id))
        .find(|id| workspace_replica_id(id) == ws)
}

/// 打开配置页：保存/热加载后重读 config.toml 并应用主题/字体/attention。
fn open_preferences(state: &Rc<RefCell<UiState>>, window: &Window) {
    let Some(path) = Config::user_config_path() else {
        tracing::warn!(target = "muxterm::linux", "无用户配置目录，无法打开配置页");
        return;
    };
    let st = state.clone();
    let hosts = FfiClient::discover_ssh_hosts().unwrap_or_default();
    let runtimes = FfiClient::discover_runtimes().unwrap_or_default();
    let callback_path = path.clone();
    crate::platform::linux::preferences_window::show(
        window,
        path,
        std::boxed::Box::new(move || {
            let mut s = st.borrow_mut();
            // 保存后重新打开 Core 事务中的文档，重建 keymap 并刷新可由
            // SettingsService 验证过的运行期状态；platform 不直接读 config.toml。
            if let Ok(service) = SettingsService::open(&callback_path) {
                let document = service.document();
                let cfg = &document.config;
                let shortcuts = &document.shortcuts;
                let bindings =
                    crate::core::config_service::action_catalog::resolve_effective_keybindings(
                        shortcuts,
                    );
                s.keymap = KeyMap::from_bindings(&bindings);
                let attention_config = ClientAttentionConfig {
                    enabled: cfg.attention.enabled,
                    blocked_regex: cfg.attention.blocked_regex.clone(),
                    debounce_ms: cfg.attention.debounce_ms,
                };
                if let Err(error) = s.event_pump.client().configure_attention(&attention_config) {
                    tracing::warn!(
                        target = "muxterm::linux",
                        %error,
                        "热加载 Core attention 配置失败"
                    );
                }
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
        Some((runtimes, hosts)),
    );
}

fn open_target_config(
    state: &Rc<RefCell<UiState>>,
    window: &Window,
    editing: Option<TargetConfig>,
) {
    let store = state.borrow().qc_store.clone();
    let hosts = FfiClient::discover_ssh_hosts().unwrap_or_default();
    let runtimes = FfiClient::discover_runtimes().unwrap_or_default();
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
        create_worktree(
            &st,
            branch_text.trim().to_string(),
            path_text.trim().to_string(),
        );
        dlg.close();
    });
    dialog.present();
}

/// 通过 Core FFI 创建 native worktree，并在成功后刷新 owned workspace DTO。
fn create_worktree(state: &Rc<RefCell<UiState>>, branch: String, path: String) {
    let source_workspace_id = {
        let s = state.borrow();
        s.view_store.active_workspace_id().map(ToOwned::to_owned)
    };
    let Some(source_workspace_id) = source_workspace_id else {
        return;
    };
    let result = {
        let s = state.borrow();
        s.event_pump.client().create_native_worktree(
            &source_workspace_id,
            &branch,
            &path,
            None,
            None,
        )
    };
    match result {
        Ok(opened) => {
            let mut s = state.borrow_mut();
            if let Err(error) = sync_view_store(&mut s) {
                tracing::warn!(target = "muxterm::linux", %error, "worktree snapshot refresh failed");
                return;
            }
            if parse_workspace_id(&opened.id).is_some() {
                after_activate(&mut s);
            }
        }
        Err(error) => {
            let detail = error.to_string();
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
    if s.active_ws_id() == id {
        return;
    }
    let key = id.as_str();
    if let Err(error) = s.event_pump.client().activate_workspace(&key) {
        tracing::warn!(
            target = "muxterm::linux",
            %error,
            workspace = %key,
            "workspace activation failed"
        );
        return;
    }
    if let Err(error) = sync_view_store(s) {
        tracing::warn!(
            target = "muxterm::linux",
            %error,
            workspace = %key,
            "workspace activation snapshot refresh failed"
        );
        return;
    }
    after_activate(s);
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
            let active = s.view_store.active_workspace_id();
            let mut candidates: Vec<WorkspaceCapacityCandidate> = s
                .view_store
                .workspaces()
                .filter_map(|(_, view)| view.workspace.as_ref())
                .filter(|workspace| active != Some(workspace.id.as_str()))
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
    s.scene_stack.remove(&workspace_key);
    s.view_store.remove_workspace(&workspace_key);
    remove_scene_page(s, &workspace_key);
    if s.mounted_ws.as_ref() == Some(id) {
        s.mounted_ws = None;
    }
    s.pixel_cache.remove(id);
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
            let _ = s.event_pump.client().activate_workspace(&fallback_key);
            let _ = sync_view_store(s);
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

fn remove_scene_page(s: &mut UiState, workspace_id: &str) {
    if let Some(child) = s.scene_stack_view.child_by_name(workspace_id) {
        s.scene_stack_view.remove(&child);
    }
}

fn activate_sidebar_activity(s: &mut UiState, id: &WorkspaceId, pane: u32) {
    if s.active_ws_id() != *id {
        let workspace_key = id.as_str();
        if s.event_pump
            .client()
            .activate_workspace(&workspace_key)
            .is_err()
        {
            return;
        }
        if sync_view_store(s).is_err() {
            return;
        }
        after_activate(s);
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
    let has_attention = s.attention.snapshot().iter().any(|workspace| {
        workspace.workspace_id == workspace_id
            && workspace
                .panes
                .iter()
                .any(|attention| attention.pane_id == pane)
    });
    if has_attention {
        s.attention.acknowledge(workspace_id, pane);
    }
}

fn refresh_sidebar_if_open(s: &mut UiState) {
    if !s.sidebar.is_open() {
        return;
    }
    let workspaces = sidebar_workspaces(s);
    let activity = activity_snapshot(s);
    let agents = sidebar_agents(s, &activity);
    let commands = sidebar_commands(s, &activity);
    s.sidebar.set_workspaces(&workspaces);
    s.sidebar.set_agents(&agents);
    s.sidebar.set_commands(&commands);
}

fn sidebar_workspaces(s: &UiState) -> Vec<WorkspaceSidebarItem> {
    WorkspaceSidebarItem::from_views(&s.view_store)
}

fn sidebar_agents(
    s: &UiState,
    activity: &crate::platform::ffi_client::ClientActivitySnapshot,
) -> Vec<AgentSidebarItem> {
    AgentSidebarItem::from_views(&s.view_store, activity)
}

fn sidebar_commands(
    s: &UiState,
    activity: &crate::platform::ffi_client::ClientActivitySnapshot,
) -> Vec<CommandSidebarItem> {
    CommandSidebarItem::from_views(&s.view_store, activity)
}

fn after_activate(s: &mut UiState) {
    // 切工作区 = 改绑体现：挂载该工作区的像素缓存（没有则新建）。
    let id = s.active_ws_id().clone();
    s.scene_stack.ensure(&id.as_str());
    let _ = s.scene_stack.show(&id.as_str());
    let had_cache = s.pixel_cache.contains_key(&id);
    let switching = s.mounted_ws.as_ref() != Some(&id);
    if switching {
        if !s.pixel_cache.contains_key(&id) {
            let uses = s.uses_tmux();
            let weak = s.self_weak.clone();
            let mut layout =
                LayoutHost::new(s.theme.clone(), s.font.clone(), uses, s.scrollback_lines);
            layout.set_menu_callback(move |pane_id, action| {
                if let Some(state) = weak.upgrade() {
                    handle_pane_menu_action(&state, pane_id, action);
                }
            });
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
        if s.scene_stack_view.child_by_name(&id.as_str()).is_none() {
            let root = s
                .pixel_cache
                .get(&id)
                .expect("layout 必须存在")
                .root_box
                .clone();
            s.scene_stack_view.add_named(&root, Some(&id.as_str()));
        }
        s.scene_stack_view.set_visible_child_name(&id.as_str());
        s.mounted_ws = Some(id);
    }
    s.tab_gate = TabSwitchGate::new(Duration::from_millis(1500));
    if switching && had_cache && !s.uses_tmux() {
        s.hold_pane_resize_until = Some(Instant::now() + Duration::from_millis(400));
    } else {
        s.last_client_size = None;
        s.last_pane_sizes.clear();
        s.pending_client_size = None;
        s.pending_client_hits = 0;
        s.hold_pane_resize_until = None;
    }
    let recents = recent_target_configs(
        &s.view_store,
        &s.workspace_sockets,
        s.view_store.workspace_ids().count(),
    );
    s.qc_store.replace_all_recents(&recents);
    refresh_ui(s);
    mark_active_attention_visible(s);
    refresh_sidebar_if_open(s);
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
    let target = client_target_from_config(&config);
    let open_intent = match intent {
        ProjectConnectIntent::AttachOnly => ClientOpenIntent::AttachOnly,
        ProjectConnectIntent::CreateIfMissing => ClientOpenIntent::CreateIfMissing,
    };
    let result = {
        let s = state.borrow();
        s.event_pump.client().open_target(&target, open_intent)
    };
    match result {
        Ok(opened) => {
            let mut s = state.borrow_mut();
            if let Some(id) = parse_workspace_id(&opened.id) {
                s.workspace_sockets.insert(id, config.socket.clone());
            }
            if let Err(error) = sync_view_store(&mut s) {
                tracing::warn!(target = "muxterm::linux", %error, "workspace snapshot refresh failed after open");
                return;
            }
            after_activate(&mut s);
        }
        Err(error) => {
            let detail = error.to_string();
            tracing::error!(target = "muxterm::linux", "connect failed: {detail}");
            state
                .borrow_mut()
                .notification_log
                .push(format!("{}: connect failed: {detail}", config.name));
        }
    }
}

fn connect_open_request(state: &Rc<RefCell<UiState>>, request: ClientOpenRequest) {
    let request_socket = match &request.candidate {
        ClientCandidateRef::Existing { identity } => identity.socket.clone(),
        _ => None,
    };
    let label = match &request.candidate {
        ClientCandidateRef::Project { project_id } => format!("project {project_id}"),
        ClientCandidateRef::Recent { key } => format!("recent {key}"),
        ClientCandidateRef::Existing { identity } => {
            format!("{} @ {}", identity.runtime_id, identity.target)
        }
        _ => "existing connection".to_string(),
    };
    let result = {
        let s = state.borrow();
        s.event_pump.client().open(&request)
    };
    match result {
        Ok(opened) => {
            let socket = request_socket.or_else(|| opened_workspace_socket(&opened));
            let mut s = state.borrow_mut();
            if let Some(id) = parse_workspace_id(&opened.id) {
                s.workspace_sockets.insert(id, socket);
            }
            if let Err(error) = sync_view_store(&mut s) {
                tracing::warn!(
                    target = "muxterm::linux",
                    %error,
                    "workspace snapshot refresh failed after existing attach"
                );
                return;
            }
            after_activate(&mut s);
        }
        Err(error) => {
            let detail = error.to_string();
            tracing::error!(
                target = "muxterm::linux",
                "existing attach failed: {detail}"
            );
            state
                .borrow_mut()
                .notification_log
                .push(format!("{label}: connect failed: {detail}"));
        }
    }
}

fn opened_workspace_socket(opened: &ClientOpenedWorkspace) -> Option<String> {
    opened
        .resolved_target
        .as_ref()
        .and_then(|target| target.get("canonical"))
        .and_then(|canonical| canonical.get("socket"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

fn client_target_from_config(config: &TargetConfig) -> ClientTarget {
    let (transport, target) = match &config.transport {
        TargetTransport::Local => ("local".to_string(), None),
        TargetTransport::Ssh { name } => ("ssh".to_string(), Some(name.clone())),
    };
    ClientTarget {
        name: config.name.clone(),
        runtime: config.runtime.as_str().to_string(),
        transport,
        target,
        path: config.path.clone(),
        session: config.session.clone(),
        socket: config.socket.clone(),
    }
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
    let mut workspaces: Vec<&crate::platform::ffi_client::ClientWorkspace> = view_store
        .workspaces()
        .filter_map(|(_, view)| view.workspace.as_ref())
        .collect();
    workspaces.sort_by(|left, right| left.id.cmp(&right.id));
    workspaces
        .into_iter()
        .take(limit)
        .map(|workspace| {
            let id = parse_workspace_id(&workspace.id);
            let socket = id
                .as_ref()
                .and_then(|id| workspace_sockets.get(id))
                .and_then(|value| value.as_deref());
            workspace_to_target_config(workspace, socket)
        })
        .collect()
}

/// Owned workspace DTO → QuickConnect 目标（Recents 列表 / 面板高亮）。
///
/// 读 `resolved_target().canonical`（Catalog 打开时保存）；无 descriptor 时
/// 从 WorkspaceId 推导（测试 mock/CLI 直开路径）。
fn workspace_to_target_config(
    workspace: &crate::platform::ffi_client::ClientWorkspace,
    tmux_socket: Option<&str>,
) -> TargetConfig {
    if let Some(canonical) = workspace
        .resolved_target
        .as_ref()
        .and_then(|resolved| resolved.get("canonical"))
    {
        let name = canonical
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&workspace.name);
        let runtime = canonical
            .get("runtime")
            .and_then(serde_json::Value::as_str)
            .and_then(TargetRuntime::from_str)
            .unwrap_or_else(|| {
                TargetRuntime::from_str(&workspace.runtime).unwrap_or(TargetRuntime::Tmux)
            });
        let transport = match canonical
            .get("transport")
            .and_then(serde_json::Value::as_str)
        {
            Some("ssh") => TargetTransport::Ssh {
                name: canonical
                    .get("target")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            },
            _ => TargetTransport::Local,
        };
        let mut config = TargetConfig::new(
            name,
            runtime,
            transport,
            canonical
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default(),
        );
        config.session = canonical
            .get("session")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        config.socket = canonical
            .get("socket")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .or_else(|| tmux_socket.map(str::to_string));
        config.workspace_id = canonical
            .get("workspace_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        return config;
    }
    let Some(id) = parse_workspace_id(&workspace.id) else {
        return TargetConfig::new(
            workspace.name.clone(),
            TargetRuntime::from_str(&workspace.runtime).unwrap_or(TargetRuntime::Tmux),
            TargetTransport::Local,
            "",
        );
    };
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
    let mut config = TargetConfig::new(name, runtime, transport, id.path.clone());
    if runtime == TargetRuntime::Tmux {
        config.session = (!id.session.is_empty()).then(|| id.session.clone());
        config.socket = tmux_socket.map(str::to_owned);
    }
    config
}

fn open_tmux_attach(state: &Rc<RefCell<UiState>>, parent: &Window, _create_only: bool) {
    let active_id = state.borrow().active_ws_id();
    let socket = state
        .borrow()
        .workspace_sockets
        .get(&active_id)
        .cloned()
        .flatten();
    let socket_opt = socket.clone();
    let st = state.clone();
    tmux_dialog::show(parent, socket_opt.as_deref(), move |action| match action {
        TmuxAction::Attach { session } => {
            connect_open_request(
                &st,
                ExistingEntry::tmux(session, ExistingTransport::Local, socket.clone())
                    .open_request(),
            );
        }
        TmuxAction::NewWorkspace { name } => {
            let session = name.unwrap_or_else(|| "muxterm".into());
            let dir = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            match FfiClient::create_workspace("local", None, socket.as_deref(), &session, &dir) {
                Ok(created) => connect_open_request(
                    &st,
                    ExistingEntry::tmux(created, ExistingTransport::Local, socket.clone())
                        .open_request(),
                ),
                Err(e) => tracing::error!(target = "muxterm::linux", "create tmux session: {e}"),
            }
        }
    });
}

fn open_ssh_connect(state: &Rc<RefCell<UiState>>, parent: &Window) {
    let hosts = match FfiClient::discover_ssh_hosts() {
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
    let sessions = FfiClient::discover_existing(transport, Some(target), None).unwrap_or_default();
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
                    match FfiClient::create_workspace(transport, target, None, &name, &dir) {
                        Ok(created) => {
                            let transport = if connect == "local" {
                                ExistingTransport::Local
                            } else {
                                ExistingTransport::Ssh {
                                    name: connect.clone(),
                                }
                            };
                            connect_open_request(
                                &st,
                                ExistingEntry::tmux(created, transport, None).open_request(),
                            );
                        }
                        Err(e) => tracing::error!(
                            target = "muxterm::linux",
                            "create remote tmux session: {e}"
                        ),
                    }
                });
            } else {
                let transport = if connect_for_attach == "local" {
                    ExistingTransport::Local
                } else {
                    ExistingTransport::Ssh {
                        name: connect_for_attach.clone(),
                    }
                };
                connect_open_request(
                    &st,
                    ExistingEntry::tmux(item.id, transport, None).open_request(),
                );
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
        .status-bar {{
            color: {fg};
            padding: 3px 8px;
            font-size: 11px;
            border-top: 1px solid alpha({fg}, 0.12);
        }}
        .muxterm-status-windows {{
            padding: 1px;
            border-radius: 5px;
            background-color: alpha({fg}, 0.035);
        }}
        button.muxterm-status-window {{
            background-image: none;
            background-color: transparent;
            border: 1px solid transparent;
            box-shadow: none;
            min-height: 20px;
            padding: 1px 10px;
            border-radius: 4px;
            color: {fg};
            font-weight: 500;
            opacity: 0.72;
        }}
        button.muxterm-status-window:hover {{
            opacity: 0.92;
            background-color: alpha({fg}, 0.08);
        }}
        button.muxterm-status-window.tab-active {{
            opacity: 1;
            font-weight: 700;
            border-color: alpha({fg}, 0.16);
            background-color: alpha({fg}, 0.20);
            box-shadow: inset 0 -3px 0 {fg};
        }}
        .qc-badge {{ padding: 0 6px; border-radius: 4px; font-size: 9px; color: #fff; }}
        .qc-badge-recent {{ background: #1e66f5; }}
        .qc-badge-project {{ background: #40a02b; }}
        .qc-badge-current {{ background: #df8e1d; }}
        .qc-current {{ background: alpha(#89b4fa, 0.18); }}
        .muxterm-sidebar {{ background: {bg}; border-right: 1px solid alpha({fg}, 0.18); }}
        .muxterm-sidebar-section-header {{
            background-image: none;
            background-color: alpha({fg}, 0.035);
            border: none;
            border-radius: 0;
            min-height: 22px;
            padding: 0;
            color: {fg};
        }}
        .muxterm-sidebar-section-header:hover {{ background-color: alpha({fg}, 0.09); }}
        .muxterm-sidebar-title {{ color: {fg}; font-size: 9.5px; font-weight: 700; letter-spacing: 0.04em; }}
        .muxterm-sidebar-section-arrow {{ color: {fg}; opacity: 0.72; }}
        .muxterm-sidebar-sections > separator {{
            min-height: 1px;
            background: alpha({fg}, 0.18);
        }}
        .muxterm-sidebar-list {{ background: transparent; padding: 1px 3px 3px; }}
        .muxterm-sidebar-row {{ border-radius: 4px; margin: 1px 0; }}
        .muxterm-sidebar-row.active {{ background: alpha({fg}, 0.14); box-shadow: inset 2px 0 0 alpha({fg}, 0.72); }}
        .muxterm-sidebar-close {{
            min-width: 22px;
            min-height: 22px;
            padding: 0;
            border-radius: 4px;
            color: {fg};
            opacity: 0;
        }}
        .muxterm-sidebar-workspace-row:hover .muxterm-sidebar-close {{ opacity: 0.72; }}
        .muxterm-sidebar-close:hover {{
            opacity: 1;
            background-color: alpha({fg}, 0.12);
        }}
        button.muxterm-sidebar-command-visibility {{
            background-image: none;
            background-color: transparent;
            border: none;
            box-shadow: none;
            min-width: 22px;
            min-height: 22px;
            padding: 1px 3px;
            border-radius: 4px;
            color: {fg};
            opacity: 0.58;
        }}
        button.muxterm-sidebar-command-visibility:hover {{
            opacity: 1;
            background-color: alpha({fg}, 0.12);
        }}
        .muxterm-sidebar-row-name {{ color: {fg}; font-size: 11.5px; font-weight: 500; }}
        .muxterm-sidebar-row-detail {{ color: {fg}; opacity: 0.55; font-size: 10px; }}
        .muxterm-sidebar-workspace-shortcut {{
            color: {fg};
            opacity: 0.52;
            font-size: 11px;
            font-weight: 600;
        }}
        .muxterm-sidebar-agent-dot {{ font-size: 10px; }}
        .muxterm-sidebar-agent-dot.running {{ color: #40a02b; }}
        .muxterm-sidebar-agent-dot.done {{ color: #df8e1d; }}
        .muxterm-main-split > separator {{
            min-width: 1px;
            background: alpha({fg}, 0.18);
        }}
        .quick-pick-backdrop {{ background-color: alpha(#000000, 0.42); }}
        .quick-pick-root {{
            background-color: {bg};
            color: {fg};
            border: 1px solid alpha({fg}, 0.24);
            border-radius: 10px;
            box-shadow: 0 18px 44px alpha(#000000, 0.54), inset 0 1px alpha({fg}, 0.08);
        }}
        .quick-pick-entry {{
            min-height: 32px;
            padding: 0 10px;
            background-color: alpha({fg}, 0.065);
            color: {fg};
            border: 1px solid alpha({fg}, 0.20);
            border-radius: 6px;
            box-shadow: inset 0 1px 2px alpha(#000000, 0.22);
        }}
        .quick-pick-entry:focus {{
            border-color: alpha({fg}, 0.42);
            box-shadow: 0 0 0 1px alpha({fg}, 0.12), inset 0 1px 2px alpha(#000000, 0.20);
        }}
        .quick-pick-list {{ background-color: transparent; padding: 2px 6px 5px; }}
        .quick-pick-list row {{
            min-height: 0;
            border-radius: 5px;
            color: {fg};
        }}
        .quick-pick-list row:hover {{ background-color: alpha({fg}, 0.065); }}
        .quick-pick-list row:selected {{ background-color: alpha({fg}, 0.15); }}
        .quick-pick-label, .qc-name {{ font-size: 13px; font-weight: 600; }}
        .quick-pick-detail, .qc-sub {{ font-size: 11px; opacity: 0.62; }}
        .qc-attention-count {{ font-size: 10px; opacity: 0.58; }}
        .quick-pick-root togglebutton {{
            min-height: 24px;
            padding: 1px 9px;
            border-radius: 5px;
        }}
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
        let light = Theme::load("light").unwrap();
        let dark = Theme::load("dark").unwrap();
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
                layout: crate::core::protocol::layout::TabLayout {
                    tab: TabId(1),
                    tree: crate::core::protocol::layout::LayoutNode::leaf(PaneId(1)),
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
            "spawn_existing_ssh_probe 必须 4 路并发（chunks / scope / 每 host spawn），禁止串行 Existing discovery。body={body}"
        );
    }

    /// C9：已有的连接必须先出 local 行，SSH host 再 4 路并发。
    /// 禁止只调一次 `discover_existing("all")` 再一次性 send（慢 host 会冻 Loading）。
    #[test]
    fn spawn_local_existing_probe_must_stream_local_then_parallel_ssh() {
        let src = include_str!("window.rs");
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
        let src = include_str!("window.rs");
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
