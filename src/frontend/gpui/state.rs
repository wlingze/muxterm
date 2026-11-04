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
        if keystroke.modifiers.platform {
            match keystroke.key.as_str() {
                "q" => cx.quit(),
                "t" => self.commands.push(ClientCommand::Task {
                    workspace_id: self.visible.clone(),
                    task: crate::frontend::utils::corebridge::ClientTask::NewTab,
                }),
                "v" => {
                    // 粘贴走剪贴板文本；图片粘贴需要平台读 PNG，暂未接。
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
