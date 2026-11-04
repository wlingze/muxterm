//! 前端状态与唯一事件泵循环。
//!
//! `EventPump` 是 Core 事件的唯一消费者；`CommandQueue` 是唯一的反向通道。
//! 两者都不跨线程：C handle 只在 gpui 主线程被持有（见 docs/FRONTEND-GPUI.md §6）。

use std::collections::HashMap;
use std::time::Duration;

use gpui::{
    Bounds, Context, EntityInputHandler, FocusHandle, KeyDownEvent, Pixels, Point, Size, Timer,
    UTF16Selection, Window,
};

use crate::frontend::command_queue::{ClientCommand, CommandQueue};
use crate::frontend::event_pump::EventPump;
use crate::frontend::gpui::model::scroll::{wheel_lines, wheel_outcome, WheelOutcome};
use crate::frontend::gpui::terminal::mouse::{self, MouseEventKind};
use crate::frontend::gpui::terminal::{encode, KeyPress, Surface};
use crate::frontend::gpui::theme::Theme;
use crate::frontend::utils::corebridge::{ClientEventKind, ClientLayout, ClientTab};
use crate::frontend::view_store::ViewStore;

/// 事件泵间隔：约 120Hz，够跟住输出又不空转。
const PUMP_INTERVAL: Duration = Duration::from_millis(8);

/// 一个已打开 pane 的常驻 Surface key。
type SurfaceKey = (String, u32);

/// 一次滚轮事件的入参（屏幕坐标已经折算成格子）。
#[derive(Debug, Clone, Copy)]
pub struct WheelInput {
    pub pane_id: u32,
    /// 正数往历史上滚（与 macOS `scrollingDeltaY` 同号）。
    pub delta_y: f32,
    /// 触控板给的像素级 delta 需要先攒整格。
    pub precise: bool,
    /// Shift 绕过应用鼠标，落回本地历史。
    pub shift: bool,
    pub col: usize,
    pub row: usize,
}

/// 窗口级状态。一个窗口一份。
pub struct GpuiApp {
    pump: Option<EventPump>,
    store: ViewStore,
    commands: CommandQueue,
    surfaces: HashMap<SurfaceKey, Surface>,
    visible: Option<String>,
    focused_pane: Option<u32>,
    last_sizes: HashMap<SurfaceKey, (u16, u16)>,
    /// 触控板的零头：不足一格的部分留到下一次事件，见 `wheel_lines`。
    wheel_accumulators: HashMap<SurfaceKey, f32>,
    focus_handle: FocusHandle,
    theme: Theme,
    status: String,
    /// IME 组合中的文本（尚未提交）。只在聚焦 pane 的光标处画。
    marked_text: String,
    /// tab 栏位置，默认顶部。
    pub(crate) tab_bar_position: crate::frontend::gpui::chrome::tab_bar_view::TabBarPosition,
    /// 侧栏是否显示（Cmd-B 切换，与 Swift 一致）。
    pub(crate) sidebar_visible: bool,
    /// 注意力快照（Agents/Commands 段的数据源）。FFI 快照接入前保持 `None`。
    pub(crate) attention: Option<crate::frontend::gpui::model::attention::AttentionSnapshot>,
    /// 侧栏四段的展开状态。
    pub(crate) sidebar_expansion: crate::frontend::gpui::chrome::sidebar_view::SectionExpansion,
    /// 用户手动隐藏的命令行（只在前端生效）。
    pub(crate) hidden_commands:
        std::collections::HashSet<crate::frontend::gpui::chrome::sidebar_view::CommandKey>,
}

impl GpuiApp {
    pub fn new(
        pump: EventPump,
        store: ViewStore,
        commands: CommandQueue,
        cx: &mut Context<Self>,
    ) -> Self {
        let visible = store
            .active_workspace_id()
            .map(str::to_string)
            .or_else(|| store.workspace_ids().next().map(str::to_string));
        let mut app = Self {
            pump: Some(pump),
            store,
            commands,
            surfaces: HashMap::new(),
            visible,
            focused_pane: None,
            last_sizes: HashMap::new(),
            wheel_accumulators: HashMap::new(),
            focus_handle: cx.focus_handle(),
            marked_text: String::new(),
            tab_bar_position: crate::frontend::gpui::chrome::tab_bar_view::TabBarPosition::default(
            ),
            sidebar_visible: true,
            attention: None,
            sidebar_expansion:
                crate::frontend::gpui::chrome::sidebar_view::SectionExpansion::default(),
            hidden_commands: std::collections::HashSet::new(),
            theme: Theme::default(),
            status: String::from("muxterm · gpui"),
        };
        app.focus_default_pane();
        app.start_pump(cx);
        app
    }

    fn start_pump(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            Timer::after(PUMP_INTERVAL).await;
            let alive = this.update(cx, |app, cx| {
                if app.pump_once(cx) {
                    cx.notify();
                }
            });
            if alive.is_err() {
                break;
            }
        })
        .detach();
    }

    /// 收一批事件、喂 Surface、冲刷命令。返回是否有可见变化。
    pub fn pump_once(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(pump) = self.pump.as_ref() else {
            return false;
        };
        let events = pump.poll_into_with_events(&mut self.store);
        let mut dirty = events.iter().any(|event| event.event.is_topology());

        let workspaces: Vec<String> = self.store.workspace_ids().map(str::to_string).collect();
        for workspace in workspaces {
            let pane_ids: Vec<(u32, u16, u16)> = self
                .store
                .workspace(&workspace)
                .map(|view| {
                    view.panes
                        .values()
                        .flatten()
                        .map(|pane| (pane.id, pane.cols, pane.rows))
                        .collect()
                })
                .unwrap_or_default();
            for (pane_id, cols, rows) in pane_ids {
                let pending = self.store.take_pane_render_events(&workspace, pane_id);
                if pending.is_empty() {
                    continue;
                }
                let surface = self
                    .surfaces
                    .entry((workspace.clone(), pane_id))
                    .or_insert_with(|| Surface::new(cols as usize, rows as usize));
                for event in pending {
                    match event.kind() {
                        ClientEventKind::PaneSnapshot | ClientEventKind::PaneFrame => {
                            surface.feed_full(&event.data)
                        }
                        ClientEventKind::PaneOutput => surface.feed(&event.data),
                        ClientEventKind::PaneHistory
                        | ClientEventKind::PaneClosed
                        | ClientEventKind::PaneResized
                        | ClientEventKind::Other(_) => {}
                    }
                }
                dirty = true;
            }
        }

        let client = self.store.active_workspace_id().map(str::to_string);
        if self.visible.is_none() {
            self.visible = client;
        }
        self.commands.flush(pump.client());
        let _ = cx;
        dirty
    }

    fn focus_default_pane(&mut self) {
        self.focused_pane = self
            .store
            .workspace(self.visible.as_deref().unwrap_or_default())
            .and_then(|view| {
                let tab = view.active_tab_id()?;
                view.panes_for_tab(tab)
                    .iter()
                    .find(|pane| pane.is_active)
                    .map(|pane| pane.id)
                    .or_else(|| view.panes_for_tab(tab).first().map(|pane| pane.id))
            });
    }

    /// 无头测试用的构造器：不连 Core，也不启动事件泵。
    ///
    /// 生产路径必须走 `new`（EventPump 是唯一的事件消费者）。
    #[doc(hidden)]
    pub fn new_headless(store: ViewStore, cx: &mut Context<Self>) -> Self {
        let visible = store
            .active_workspace_id()
            .map(str::to_string)
            .or_else(|| store.workspace_ids().next().map(str::to_string));
        let mut app = Self {
            pump: None,
            store,
            commands: CommandQueue::default(),
            surfaces: HashMap::new(),
            visible,
            focused_pane: None,
            last_sizes: HashMap::new(),
            wheel_accumulators: HashMap::new(),
            focus_handle: cx.focus_handle(),
            marked_text: String::new(),
            tab_bar_position: crate::frontend::gpui::chrome::tab_bar_view::TabBarPosition::default(
            ),
            sidebar_visible: true,
            attention: None,
            sidebar_expansion:
                crate::frontend::gpui::chrome::sidebar_view::SectionExpansion::default(),
            hidden_commands: std::collections::HashSet::new(),
            theme: Theme::default(),
            status: String::from("muxterm · gpui"),
        };
        app.focus_default_pane();
        app
    }

    /// 无头测试用的注意力快照注入（FFI 快照接入前只有测试会写）。
    #[doc(hidden)]
    pub fn set_attention(
        &mut self,
        attention: Option<crate::frontend::gpui::model::attention::AttentionSnapshot>,
    ) {
        self.attention = attention;
    }

    /// 无头测试用：取走待发送给 Core 的命令。
    #[doc(hidden)]
    pub fn take_commands_for_tests(&mut self) -> Vec<ClientCommand> {
        self.commands.drain()
    }

    /// 侧栏是否显示。
    pub fn sidebar_visible(&self) -> bool {
        self.sidebar_visible
    }

    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    /// 当前 workspace 每个 tab 的 pane 数，用于 tab 标题后缀。
    pub fn pane_counts(&self) -> HashMap<u32, usize> {
        self.store
            .workspace(self.visible.as_deref().unwrap_or_default())
            .map(|view| {
                view.panes
                    .iter()
                    .map(|(tab, panes)| (*tab, panes.len()))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn visible_workspace(&self) -> Option<&str> {
        self.visible.as_deref()
    }

    pub fn workspace_name(&self, id: &str) -> String {
        self.store
            .workspace(id)
            .and_then(|view| view.workspace.as_ref())
            .map(|workspace| workspace.name.clone())
            .unwrap_or_else(|| id.to_string())
    }

    pub fn workspace_ids(&self) -> Vec<String> {
        self.store.workspace_ids().map(str::to_string).collect()
    }

    pub fn workspace_runtime(&self, id: &str) -> String {
        self.store
            .workspace(id)
            .and_then(|view| view.workspace.as_ref())
            .map(|workspace| workspace.runtime.clone())
            .unwrap_or_default()
    }

    pub fn tabs(&self) -> Vec<ClientTab> {
        self.store
            .workspace(self.visible.as_deref().unwrap_or_default())
            .map(|view| view.tabs.clone())
            .unwrap_or_default()
    }

    pub fn active_tab_id(&self) -> Option<u32> {
        self.store
            .workspace(self.visible.as_deref().unwrap_or_default())
            .and_then(|view| view.active_tab_id())
    }

    pub fn layout(&self) -> Option<ClientLayout> {
        let view = self.store.workspace(self.visible.as_deref()?)?;
        let tab = view.active_tab_id()?;
        view.layouts.get(&tab).cloned()
    }

    pub fn pane_geometry(&self, pane_id: u32) -> Option<(u16, u16)> {
        self.store
            .workspace(self.visible.as_deref()?)
            .and_then(|view| {
                view.panes
                    .values()
                    .flatten()
                    .find(|pane| pane.id == pane_id)
                    .map(|pane| (pane.cols, pane.rows))
            })
    }

    /// 侧栏工作区行的输入：顺序、快捷键、连接状态都来自 ViewStore。
    pub fn sidebar_items(&self) -> Vec<crate::frontend::gpui::model::sidebar::SidebarItem> {
        use crate::frontend::gpui::model::sidebar::{shortcut_index_by_workspace_id, SidebarItem};
        let ids = self.workspace_ids();
        let shortcuts =
            shortcut_index_by_workspace_id(&ids.iter().map(String::as_str).collect::<Vec<_>>());
        ids.iter()
            .map(|id| {
                let view = self.store.workspace(id);
                let workspace = view.and_then(|view| view.workspace.as_ref());
                let mut item = SidebarItem::new(id.clone(), self.workspace_name(id));
                item.runtime = self.workspace_runtime(id);
                item.transport = workspace.map(transport_label).unwrap_or_default();
                item.is_active = self.visible.as_deref() == Some(id.as_str());
                item.backend_status = view
                    .and_then(|view| view.backend_status)
                    .unwrap_or(crate::frontend::gpui::model::sidebar::backend_status::DISCONNECTED);
                item.shortcut = shortcuts.get(id).copied();
                // pane → tab 的映射来自拓扑快照：侧栏点击和 tab 指示灯都要它。
                if let Some(view) = view {
                    for (tab_id, panes) in &view.panes {
                        for pane in panes {
                            item.tab_id_by_pane.insert(pane.id, *tab_id);
                        }
                    }
                    for (number, tab) in self
                        .store
                        .workspace(id)
                        .map(|view| view.tabs.clone())
                        .unwrap_or_default()
                        .iter()
                        .enumerate()
                    {
                        for pane in view.panes.get(&tab.id).map(Vec::as_slice).unwrap_or(&[]) {
                            item.tab_number_by_pane.insert(pane.id, number as u32 + 1);
                        }
                    }
                }
                item
            })
            .collect()
    }

    /// 侧栏模型：四段 + 行。Agents/Commands 目前只有 attention 数据源时有内容。
    pub fn sidebar_model(
        &self,
    ) -> Vec<crate::frontend::gpui::chrome::sidebar_view::SidebarSectionModel> {
        use crate::frontend::gpui::chrome::sidebar_view::{build, SidebarInputs};
        let workspaces = self.sidebar_items();
        build(&SidebarInputs {
            workspaces: &workspaces,
            attention: self.attention.as_ref(),
            expansion: self.sidebar_expansion,
            hidden_commands: &self.hidden_commands,
        })
    }

    /// 执行窗口级快捷键。返回是否已经处理（未处理的继续走原始按键编码）。
    ///
    /// 动作表来自 `model::shortcuts::builtin_action`，这里只接 gpui 侧的效果。
    pub fn run_action(
        &mut self,
        action: crate::frontend::gpui::model::shortcuts::KeyAction,
        cx: &mut Context<Self>,
    ) -> bool {
        use crate::frontend::gpui::model::shortcuts::KeyAction;
        use crate::frontend::utils::corebridge::ClientTask;
        match action {
            KeyAction::Quit => {
                cx.quit();
                true
            }
            KeyAction::NewTab => {
                self.commands.push(ClientCommand::Task {
                    workspace_id: self.visible.clone(),
                    task: ClientTask::NewTab,
                });
                true
            }
            KeyAction::SplitHorizontal | KeyAction::SplitVertical => {
                let Some(pane_id) = self.focused_pane else {
                    return false;
                };
                // Swift：Cmd-D 左右、Cmd-Shift-D 上下。水平布局 = 并排。
                let horizontal = action == KeyAction::SplitHorizontal;
                self.commands.push(ClientCommand::Task {
                    workspace_id: self.visible.clone(),
                    task: ClientTask::SplitPane {
                        pane_id,
                        horizontal,
                    },
                });
                true
            }
            KeyAction::SwitchTab(index) => {
                let tabs = self.tabs();
                let Some(tab) = tabs.get(index.saturating_sub(1)) else {
                    return false;
                };
                let tab_id = tab.id;
                self.select_tab(tab_id, cx);
                true
            }
            KeyAction::SwitchLastTab => {
                let tabs = self.tabs();
                let Some(tab) = tabs.last() else {
                    return false;
                };
                let tab_id = tab.id;
                self.select_tab(tab_id, cx);
                true
            }
            KeyAction::PreviousTab | KeyAction::NextTab => {
                let tabs = self.tabs();
                if tabs.len() < 2 {
                    return false;
                }
                let current = tabs.iter().position(|tab| tab.is_active).unwrap_or(0);
                let next = if action == KeyAction::PreviousTab {
                    (current + tabs.len() - 1) % tabs.len()
                } else {
                    (current + 1) % tabs.len()
                };
                let tab_id = tabs[next].id;
                self.select_tab(tab_id, cx);
                true
            }
            KeyAction::SwitchWorkspace(index) => {
                let ids = self.workspace_ids();
                let target = if index == 0 {
                    ids.last().cloned()
                } else {
                    ids.get(index - 1).cloned()
                };
                let Some(target) = target else {
                    return false;
                };
                self.select_workspace(target, cx);
                true
            }
            KeyAction::ToggleSidebar => {
                self.sidebar_visible = !self.sidebar_visible;
                true
            }
            _ => false,
        }
    }

    /// 关闭工作区：只发命令，回来后由 Core 事件删掉这一行。
    pub fn close_workspace(&mut self, workspace_id: String, cx: &mut Context<Self>) {
        self.commands
            .push(ClientCommand::CloseWorkspace(workspace_id));
        cx.notify();
    }

    /// tab 栏投影：活动指示来自 attention，数量后缀来自 pane 数。
    pub fn tab_items(&self) -> Vec<crate::frontend::gpui::chrome::tab_bar_view::TabBarItem> {
        use crate::frontend::gpui::chrome::tab_bar_view;
        let Some(workspace_id) = self.visible.clone() else {
            return Vec::new();
        };
        let tabs = self.tabs();
        let counts: std::collections::HashMap<u32, usize> = self.pane_counts();
        let mut hidden = std::collections::HashSet::new();
        // 前端隐藏身份只作用于 commands；这里的 tab 指示跟随同一份集合。
        for key in &self.hidden_commands {
            hidden.insert(
                crate::frontend::gpui::model::attention::AttentionVisibilityKey {
                    workspace_id: key.workspace_id.clone(),
                    pane_id: key.pane_id,
                },
            );
        }
        let workspaces = self.sidebar_items();
        tab_bar_view::build(
            &workspace_id,
            &tabs,
            &counts,
            &workspaces,
            self.attention.as_ref(),
            &hidden,
        )
    }

    /// 状态栏右侧的注意力徽标：数「主导色」的条目，不是总数。
    pub fn status_attention(&self) -> crate::frontend::gpui::model::status_bar::StatusBarAttention {
        use crate::frontend::gpui::model::status_bar::StatusBarAttention;
        let Some(attention) = self.attention.as_ref() else {
            return StatusBarAttention::new(0, None);
        };
        let indicators: Vec<crate::frontend::gpui::model::sidebar::Indicator> = attention
            .workspaces
            .iter()
            .flat_map(|workspace| workspace.panes.iter())
            .filter(|pane| !pane.acknowledged && !pane.muted)
            .filter_map(|pane| match pane.status {
                crate::frontend::gpui::model::attention::PaneAttentionStatus::Blocked => {
                    Some(crate::frontend::gpui::model::sidebar::Indicator::Blocked)
                }
                crate::frontend::gpui::model::attention::PaneAttentionStatus::Done => {
                    Some(crate::frontend::gpui::model::sidebar::Indicator::Done)
                }
                _ => None,
            })
            .collect();
        StatusBarAttention::from_indicators(&indicators)
    }

    /// tab 栏位置（`[ui] tab_bar_position`）。
    pub fn tab_bar_position(&self) -> crate::frontend::gpui::chrome::tab_bar_view::TabBarPosition {
        self.tab_bar_position
    }

    /// 折叠/展开一段。
    pub fn toggle_sidebar_section(
        &mut self,
        section: crate::frontend::gpui::chrome::sidebar_view::Section,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_expansion.toggle(section);
        cx.notify();
    }

    /// 在 Commands / Hidden Commands 之间切换一行。
    pub fn toggle_command_visibility(
        &mut self,
        key: crate::frontend::gpui::chrome::sidebar_view::CommandKey,
        cx: &mut Context<Self>,
    ) {
        if !self.hidden_commands.remove(&key) {
            self.hidden_commands.insert(key);
        }
        cx.notify();
    }

    /// 点侧栏一行：切工作区，或跳到目标 pane。
    pub fn activate_sidebar_row(
        &mut self,
        target: crate::frontend::gpui::chrome::sidebar_view::RowTarget,
        cx: &mut Context<Self>,
    ) {
        use crate::frontend::gpui::chrome::sidebar_view::RowTarget;
        match target {
            RowTarget::Workspace(id) => self.select_workspace(id, cx),
            RowTarget::Pane {
                workspace_id,
                pane_id,
                tab_id,
            } => {
                if self.visible.as_deref() != Some(workspace_id.as_str()) {
                    self.select_workspace(workspace_id, cx);
                }
                if let Some(tab_id) = tab_id {
                    self.select_tab(tab_id, cx);
                }
                self.focus_pane(pane_id, cx);
            }
        }
    }

    /// IME 组合中的文本。
    pub fn marked_text(&self) -> &str {
        &self.marked_text
    }

    /// 聚焦 pane 的光标格子 `(row, col, cols)`。
    pub fn cursor_cell(&self) -> Option<(usize, usize, usize)> {
        self.focused_cursor()
    }

    /// 聚焦 pane 的光标格子 `(row, col, cols)`。
    fn focused_cursor(&self) -> Option<(usize, usize, usize)> {
        let workspace = self.visible.as_ref()?;
        let pane_id = self.focused_pane?;
        let surface = self.surface(workspace, pane_id)?;
        let grid = surface.grid();
        let (row, col) = grid.cursor();
        Some((row, col, grid.cols()))
    }

    /// IME 提交：占位符不进 PTY，混排只丢占位符。
    ///
    /// 对照 macOS `MuxTerminalView.insertText`。
    pub fn commit_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(bytes) = crate::frontend::gpui::model::ime::commit_bytes(text) else {
            return;
        };
        let (Some(pane_id), Some(workspace)) = (self.focused_pane, self.visible.clone()) else {
            return;
        };
        self.commands.push(ClientCommand::Input {
            workspace_id: Some(workspace),
            pane_id,
            data: bytes,
            quiet: false,
        });
        cx.notify();
    }

    /// 粘贴文本：按应用是否开了括号粘贴包标记，超过一批的按批写回。
    ///
    /// 对照 macOS `TerminalView.pastePayload` 与 `TerminalManager` 的大段粘贴分批。
    pub fn handle_paste(&mut self, text: &str, cx: &mut Context<Self>) {
        let (Some(pane_id), Some(workspace)) = (self.focused_pane, self.visible.clone()) else {
            return;
        };
        let bracketed = self
            .surfaces
            .get(&(workspace.clone(), pane_id))
            .map(|surface| surface.grid().bracketed_paste())
            .unwrap_or(false);
        let payload = crate::frontend::gpui::model::input_encoding::paste_payload(text, bracketed);
        if payload.is_empty() {
            return;
        }
        // 括号粘贴只包一层：分批是传输切片，不改变语义。
        let chunks = crate::frontend::gpui::model::surface_feed::large_paste::chunks(&payload);
        for chunk in chunks {
            self.commands.push(ClientCommand::Input {
                workspace_id: Some(workspace.clone()),
                pane_id,
                data: chunk.to_vec(),
                quiet: false,
            });
        }
        cx.notify();
    }

    /// 更新 IME 组合文本（空字符串表示组合结束）。
    pub fn set_marked_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let next = crate::frontend::gpui::model::ime::strip_placeholders(text);
        if next == self.marked_text {
            return;
        }
        self.marked_text = next;
        cx.notify();
    }

    /// 光标在 pane 元素内的坐标（像素）。
    ///
    /// 候选窗锚点走 `ime::candidate_anchor`：最后一列退回上一行行首。
    pub fn caret_rect(&self) -> Option<(f32, f32, f32, f32)> {
        let (row, col, cols) = self.focused_cursor()?;
        let cell_width = self.theme.cell_width();
        let cell_height = self.theme.cell_height();
        let caret = crate::frontend::gpui::model::ime::CaretRect {
            x: col as f32 * cell_width,
            y: row as f32 * cell_height,
            width: cell_width,
            height: cell_height,
        };
        let anchor = crate::frontend::gpui::model::ime::candidate_anchor(caret, col, cols);
        Some((anchor.x, anchor.y, anchor.width, anchor.height))
    }

    pub fn focused_pane(&self) -> Option<u32> {
        self.focused_pane
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn surface(&self, workspace: &str, pane_id: u32) -> Option<&Surface> {
        self.surfaces.get(&(workspace.to_string(), pane_id))
    }

    /// 布局算出来的尺寸写回 Surface，并在变化时排一条 Resize。
    pub fn apply_sizes(&mut self, sizes: &[(u32, u16, u16)]) {
        let Some(workspace) = self.visible.clone() else {
            return;
        };
        for (pane_id, cols, rows) in sizes {
            let cols = (*cols).max(1);
            let rows = (*rows).max(1);
            let key = (workspace.clone(), *pane_id);
            if self.last_sizes.get(&key) == Some(&(cols, rows)) {
                continue;
            }
            self.last_sizes.insert(key.clone(), (cols, rows));
            self.surfaces
                .entry(key)
                .or_insert_with(|| Surface::new(cols as usize, rows as usize))
                .resize(cols as usize, rows as usize);
            self.commands.push(ClientCommand::Resize {
                workspace_id: Some(workspace.clone()),
                pane_id: Some(*pane_id),
                cols,
                rows,
            });
        }
    }

    /// 滚轮：先问服务端，再看应用要不要鼠标，最后才动本地历史。
    pub fn handle_wheel(&mut self, wheel: WheelInput, cx: &mut Context<Self>) {
        let WheelInput {
            pane_id,
            delta_y,
            precise,
            shift,
            col,
            row,
        } = wheel;
        let Some(workspace) = self.visible.clone() else {
            return;
        };
        let key = (workspace.clone(), pane_id);
        let cell_height = self.theme.cell_height();
        let accumulator = self.wheel_accumulators.entry(key.clone()).or_insert(0.0);
        let lines = wheel_lines(delta_y, precise, cell_height, accumulator);
        let Some(surface) = self.surfaces.get(&key) else {
            return;
        };
        let grid = surface.grid();
        let outcome = wheel_outcome(
            grid.mouse_mode() != crate::frontend::gpui::terminal::MouseMode::Off,
            shift,
            grid.alternate_screen(),
            // gpui 目前没有 Herdr 的服务端滚动通道。
            false,
            lines,
        );
        // 应用鼠标路径固定用 SGR：macOS 同样不跟随 1006 模式。
        let sgr_mode = crate::frontend::gpui::terminal::MouseMode::Normal;
        let payload = match outcome {
            None => return,
            Some(WheelOutcome::Server(_)) => return,
            Some(WheelOutcome::MouseReport(count)) => mouse::encode(
                MouseEventKind::Wheel(count > 0, count.unsigned_abs() as u8),
                col,
                row,
                sgr_mode,
                true,
            ),
            Some(WheelOutcome::Arrows(count)) => {
                // alternate screen 且应用没要鼠标：滚轮 = CSI 方向键。
                let arrow = if count > 0 {
                    &b"\x1b[A"[..]
                } else {
                    &b"\x1b[B"[..]
                };
                let mut bytes = Vec::with_capacity(arrow.len() * count.unsigned_abs() as usize);
                for _ in 0..count.unsigned_abs() {
                    bytes.extend_from_slice(arrow);
                }
                Some(bytes)
            }
            Some(WheelOutcome::History(count)) => {
                if let Some(surface) = self.surfaces.get_mut(&key) {
                    if surface.scroll_by(count) {
                        cx.notify();
                    }
                }
                return;
            }
        };
        if let Some(data) = payload {
            self.commands.push(ClientCommand::Input {
                workspace_id: Some(workspace),
                pane_id,
                data,
                quiet: false,
            });
            cx.notify();
        }
    }

    pub fn select_workspace(&mut self, workspace: String, cx: &mut Context<Self>) {
        if self.visible.as_deref() == Some(workspace.as_str()) {
            return;
        }
        self.visible = Some(workspace.clone());
        self.focused_pane = None;
        self.last_sizes.clear();
        self.commands
            .push(ClientCommand::ActivateWorkspace(workspace));
        self.focus_default_pane();
        cx.notify();
    }

    pub fn select_tab(&mut self, tab_id: u32, cx: &mut Context<Self>) {
        self.commands.push(ClientCommand::Task {
            workspace_id: self.visible.clone(),
            task: crate::frontend::utils::corebridge::ClientTask::SwitchTab { tab_id },
        });
        cx.notify();
    }

    pub fn focus_pane(&mut self, pane_id: u32, cx: &mut Context<Self>) {
        if self.focused_pane == Some(pane_id) {
            return;
        }
        self.focused_pane = Some(pane_id);
        self.commands.push(ClientCommand::Task {
            workspace_id: self.visible.clone(),
            task: crate::frontend::utils::corebridge::ClientTask::SwitchPane { pane_id },
        });
        cx.notify();
    }

    /// 键 → 终端字节。Cmd 组合留给 chrome。
    pub fn handle_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let chord = crate::frontend::gpui::model::shortcuts::KeyChord {
            command: keystroke.modifiers.platform,
            shift: keystroke.modifiers.shift,
            option: keystroke.modifiers.alt,
            control: keystroke.modifiers.control,
            key: keystroke.key.clone(),
        };
        if let Some(action) = crate::frontend::gpui::model::shortcuts::builtin_action(&chord) {
            if self.run_action(action, cx) {
                cx.notify();
                return;
            }
        }
        if keystroke.modifiers.platform {
            // 粘贴走剪贴板文本；图片粘贴需要平台读 PNG，暂未接（见 terminal.md）。
            if keystroke.key == "v" {
                let text = cx.read_from_clipboard().and_then(|item| {
                    item.entries().iter().find_map(|entry| match entry {
                        gpui::ClipboardEntry::String(text) => Some(text.text().to_string()),
                        gpui::ClipboardEntry::Image(_) => None,
                    })
                });
                if let Some(text) = text {
                    self.handle_paste(&text, cx);
                }
            }
            cx.notify();
            return;
        }

        let Some(pane_id) = self.focused_pane else {
            return;
        };
        let Some(workspace) = self.visible.clone() else {
            return;
        };
        let app_cursor = self
            .surfaces
            .get(&(workspace.clone(), pane_id))
            .map(|surface| surface.grid().app_cursor_keys())
            .unwrap_or(false);
        let press = KeyPress {
            key: keystroke.key.clone(),
            key_char: keystroke.key_char.clone(),
            ctrl: keystroke.modifiers.control,
            alt: keystroke.modifiers.alt,
            shift: keystroke.modifiers.shift,
            cmd: false,
        };
        if let Some(bytes) = encode(&press, app_cursor) {
            let forwarded = !bytes.is_empty();
            self.commands.push(ClientCommand::Input {
                workspace_id: Some(workspace),
                pane_id,
                data: bytes,
                quiet: false,
            });
            if forwarded {
                // 已经按原始按键发过了：别再让平台把同一段文本当成 IME 提交送一遍。
                cx.stop_propagation();
            }
        }
        cx.notify();
    }
}

/// IME 接入：终端没有可编辑缓冲，组合文本只在这里中转。
///
/// 对照 macOS `MuxTerminalView.firstRect(forCharacterRange:)` 与
/// `insertText(_:replacementRange:)`。
impl EntityInputHandler for GpuiApp {
    fn text_for_range(
        &mut self,
        _range: std::ops::Range<usize>,
        _adjusted_range: &mut Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        // 终端不维护可被 IME 读回的文本。
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        // 空选区在光标处：macOS 需要它才能摆候选窗。
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        if self.marked_text.is_empty() {
            None
        } else {
            Some(0..self.marked_text.encode_utf16().count())
        }
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.set_marked_text("", cx);
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_marked_text("", cx);
        self.commit_text(text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_marked_text(new_text, cx);
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: std::ops::Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let (x, y, width, height) = self.caret_rect()?;
        Some(Bounds::new(
            element_bounds.origin + Point::new(gpui::px(x), gpui::px(y)),
            Size::new(gpui::px(width), gpui::px(height)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(0)
    }
}

/// 工作区行展示的机器名：`TargetTransport.label`，SSH 用别名、本机是 `local`。
///
/// 对照 macOS `WorkspaceSidebarItem.transport`（值来自 `TargetConfig.transport.label`）。
fn transport_label(workspace: &crate::frontend::utils::corebridge::ClientWorkspace) -> String {
    use crate::frontend::gpui::model::target::TargetTransport;
    let canonical = workspace
        .resolved_target
        .as_ref()
        .and_then(|target| target.get("canonical"));
    let transport = canonical
        .and_then(|canonical| canonical.get("transport"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("local");
    if transport == "ssh" {
        let alias = canonical
            .and_then(|canonical| canonical.get("alias"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("ssh");
        return TargetTransport::Ssh {
            name: alias.to_string(),
        }
        .label()
        .to_string();
    }
    TargetTransport::Local.label().to_string()
}
