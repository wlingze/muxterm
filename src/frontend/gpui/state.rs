//! 前端状态与唯一事件泵循环。
//!
//! `EventPump` 是 Core 事件的唯一消费者；`CommandQueue` 是唯一的反向通道。
//! 两者都不跨线程：C handle 只在 gpui 主线程被持有（见 docs/FRONTEND-GPUI.md §6）。

use std::collections::HashMap;
use std::time::Duration;

use gpui::{Context, FocusHandle, KeyDownEvent, Timer};

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
        if keystroke.modifiers.platform {
            match keystroke.key.as_str() {
                "q" => cx.quit(),
                "t" => self.commands.push(ClientCommand::Task {
                    workspace_id: self.visible.clone(),
                    task: crate::frontend::utils::corebridge::ClientTask::NewTab,
                }),
                "d" => {
                    if let Some(pane_id) = self.focused_pane {
                        self.commands.push(ClientCommand::Task {
                            workspace_id: self.visible.clone(),
                            task: crate::frontend::utils::corebridge::ClientTask::SplitPane {
                                pane_id,
                                horizontal: true,
                            },
                        });
                    }
                }
                _ => {}
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
            self.commands.push(ClientCommand::Input {
                workspace_id: Some(workspace),
                pane_id,
                data: bytes,
                quiet: false,
            });
        }
        cx.notify();
    }
}
