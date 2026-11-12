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
use crate::frontend::gpui::model::composer::{ComposerTarget, ComposerTargetKind};
use crate::frontend::gpui::model::quick_connect::TargetConfig;
use crate::frontend::gpui::model::scroll::{wheel_lines, wheel_outcome, WheelOutcome};
use crate::frontend::gpui::terminal::mouse::{self, MouseEventKind};
use crate::frontend::gpui::terminal::{encode, KeyPress, Surface};
use crate::frontend::gpui::theme::Theme;
use crate::frontend::utils::corebridge::{ClientEventKind, ClientLayout, ClientTab};
use crate::frontend::view_store::ViewStore;

/// 事件泵间隔：约 120Hz，够跟住输出又不空转。
const PUMP_INTERVAL: Duration = Duration::from_millis(8);
/// 空闲轮询上限：没有事件时 8ms → 16 → … → 64ms。
const IDLE_PUMP_INTERVAL: Duration = Duration::from_millis(64);
/// 活动/注意力快照的刷新间隔（侧栏 Agents/Commands 的数据源）。
const ATTENTION_INTERVAL: Duration = Duration::from_millis(1_000);

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

#[path = "state/picker.rs"]
mod picker;

#[path = "state/palette_actions.rs"]
mod palette_actions;

pub use palette_actions::{PaletteMenu, PromptState, PromptTarget, StatusBarMode};

/// 设置浮层的键盘落点：搜索框或第 n 个字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsFocus {
    #[default]
    Search,
    Field(usize),
}

/// 正在编辑的文本字段。值先写进 `buffer`，再按控件类型解析进草稿。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsEdit {
    pub path: String,
    pub buffer: String,
}

/// 窗口级状态。一个窗口一份。
/// 选区归属：一个 pane + 一份选区。切 pane / 滚动时整体清掉。
#[derive(Debug, Clone, Copy)]
struct PaneSelection {
    pane_id: u32,
    selection: crate::frontend::gpui::model::selection::Selection,
}

pub struct GpuiApp {
    pump: Option<EventPump>,
    store: ViewStore,
    commands: CommandQueue,
    surfaces: HashMap<SurfaceKey, Surface>,
    visible: Option<String>,
    focused_pane: Option<u32>,
    /// 上次量格子用的字体参数（没变就不重量）。
    cell_metrics_key: Option<(gpui::SharedString, u32, u32, u32)>,
    /// 刚点过、还没被 Core 确认的焦点（同步时让路，避免来回跳）。
    pending_focus: Option<u32>,
    /// 本机 shell 工作区空了之后已经要过新 tab，避免每拍都发一遍。
    shell_restart_requested: bool,
    /// 用户刚关掉的 workspace：Core 不发 topology 事件，Open 完成时的全量
    /// 同步可能把它塞回来（同一拍里 close 还没落盘），用墓碑挡掉。
    closed_workspaces: std::collections::HashSet<String>,
    /// 所有 workspace 都已结束：下一次泵让窗口退出（对照 Swift `closeSessionWindow`）。
    quit_requested: bool,
    last_sizes: HashMap<SurfaceKey, (u16, u16)>,
    /// 触控板的零头：不足一格的部分留到下一次事件，见 `wheel_lines`。
    wheel_accumulators: HashMap<SurfaceKey, f32>,
    focus_handle: FocusHandle,
    theme: Theme,
    /// 渲染期量出来的格子像素尺寸；Core 上报与鼠标命中共用。
    cell: (f32, f32),
    /// 窗口级字符网格（`refresh-client -C`）上一次上报的值。
    last_client_size: Option<(String, u16, u16)>,
    /// `[font] size` 的配置值：Cmd-0 恢复到这个字号。
    configured_font_size: f32,
    /// Cmd-Shift-P 命令面板。
    pub(crate) palette: crate::frontend::gpui::chrome::palette_view::PaletteState,
    /// Muxterm Info 浮层：`None` 表示没打开。
    info: Option<crate::frontend::utils::corebridge::ClientBuildInfo>,
    /// Info 浮层的复制反馈（「已复制…」）。
    info_status: Option<String>,
    /// 命令面板的二级列表（语言）。
    palette_menu: Option<palette_actions::PaletteMenu>,
    /// 重命名输入浮层。
    prompt: Option<palette_actions::PromptState>,
    /// 界面语言（`[ui] language`）。
    language: crate::frontend::utils::i18n::Language,
    /// 状态栏来源（`[statusbar] mode`）。
    status_bar_mode: palette_actions::StatusBarMode,
    /// tmux 自己的 status 左右段。
    status_snapshot: Option<crate::frontend::gpui::model::status_bar::StatusBarSnapshot>,
    status_snapshot_at: Option<std::time::Instant>,
    /// 上一次拉活动/注意力快照的时间。
    attention_at: Option<std::time::Instant>,
    /// 设置浮层的键盘落点（搜索框 / 字段）。
    pub(crate) settings_focus: SettingsFocus,
    /// 正在输入的文本字段；`None` 表示没有编辑中的值。
    pub(crate) settings_edit: Option<SettingsEdit>,
    /// 有未保存修改时按 Esc / 取消：等用户确认「放弃修改」。
    pub(crate) settings_discard_pending: bool,
    /// 刚写过配置，下一轮事件泵要重读主题 / 语言 / 状态栏模式。
    config_reload_pending: bool,
    /// 刚给 PTY 写过输入：这段时间泵要保持快轮询，否则回显要等空闲退避。
    input_urgent_until: Option<std::time::Instant>,
    /// 图片粘贴的阶段与横幅（编码 → Core 上传 → 路径写回 pane）。
    image_paste: crate::frontend::gpui::model::image_paste::ImagePasteTimeline,
    /// `[ui] show_title_bar`：关掉后窗口标题栏不显示任何文字。
    show_title_bar: bool,
    /// 已经写到原生窗口上的标题，避免每帧重复设置。
    applied_window_title: String,
    /// 鼠标是否停在某个 pane 的标题条带（顶部 22pt）。
    pane_title_hover: Option<u32>,
    /// 已打开的 pane `⋯` 菜单（绑定到具体 pane）。
    pub(crate) pane_menu: Option<crate::frontend::gpui::model::pane_menu::PaneMenu>,
    /// 鼠标选区（只属于一个 pane；切 pane 时清掉）。
    selection: Option<PaneSelection>,
    /// 正在拖选。
    dragging_selection: bool,
    status: String,
    /// IME 组合中的文本（尚未提交）。只在聚焦 pane 的光标处画。
    marked_text: String,
    /// Cmd-P 统一面板的状态。
    pub(crate) panel: crate::frontend::gpui::chrome::panel_view::PanelState,
    /// 目标配置窗口（新建 / 编辑 Project）的草稿。
    pub(crate) target_editor: Option<crate::frontend::gpui::model::target_editor::TargetEditor>,
    /// 目标配置窗口最近一次校验失败的原因。
    pub target_editor_error: Option<String>,
    /// 目标配置窗口里正在编辑的字段。
    pub target_editor_focus: crate::frontend::gpui::model::target_editor::TargetEditorFocus,
    /// 设置窗口是否打开。
    pub settings_open: bool,
    /// 上次按键是否已被前端消费（不再交给原生菜单再执行一遍）。
    /// 测试用：回归「按一次 Cmd-D 只能分一次屏」。
    key_consumed: bool,
    /// 上一个按键是否让给了平台输入法（中文组词需要先看到首字母）。
    ime_deferred: bool,
    /// Core manifest 解析出来的分类与字段。
    pub(crate) settings_manifest: crate::frontend::gpui::model::settings::SettingsManifest,
    /// 设置草稿：值与 baseline 的差就是 Apply 要提交的 patch。
    pub(crate) settings_draft: crate::frontend::gpui::model::settings::SettingsDraft,
    /// 独立输入框：每个 pane 一份草稿，关窗不清。
    pub(crate) composer: crate::frontend::gpui::model::composer::ComposerDraftStore,
    /// 输入框是否打开。
    pub composer_open: bool,
    /// 当前草稿页的 UTF-16 光标（图片路径按它插入）。
    pub(crate) composer_selection: crate::frontend::gpui::model::composer::TextRange,
    /// 当前草稿页身份；没有打开过时是 `None`。
    pub(crate) composer_key: Option<crate::frontend::gpui::model::composer::ComposerDraftKey>,
    /// 输入框底部状态行（发送成功 / 没有目标 / 没有内容）。
    pub composer_status: Option<String>,
    /// 搜索命中快照（FFI 接入前保持 `None`，面板 Search tab 为空）。
    pub(crate) search_snapshot: Option<crate::frontend::gpui::model::search::SearchSnapshot>,
    /// tab 栏位置，默认顶部。
    pub(crate) tab_bar_position: crate::frontend::gpui::chrome::tab_bar_view::TabBarPosition,
    /// 侧栏是否显示（Cmd-B 切换，与 Swift 一致）。
    pub(crate) sidebar_visible: bool,
    /// 是否直读系统剪贴板（`NSPasteboard`）。无头测试关掉它：并行测试读真实
    /// 剪贴板既不确定（取决于用户此刻复制了什么），也不是线程安全的。
    system_pasteboard: bool,
    /// 注意力快照（Agents/Commands 段的数据源）。FFI 快照接入前保持 `None`。
    pub(crate) attention: Option<crate::frontend::gpui::model::attention::AttentionSnapshot>,
    /// Cmd-P 面板的「现有 runtime」：来自 Core discovery，不手工拼。
    pub(crate) existing: Vec<crate::frontend::gpui::model::existing::ExistingRuntime>,
    /// discovery 是否已经跑过一次（面板打开时只跑一次，失败可重试）。
    existing_loaded: bool,
    /// discovery 是否正在跑，避免连续按键重复发查询。
    existing_loading: bool,
    /// 连接时用的 tmux socket（`-L`）；隔离 server 的 session 只在它上面可见。
    socket: Option<String>,
    panel_scroll: gpui::ScrollHandle,
    open_pending: bool,
    /// 打开请求的起始时刻：Core 不回应时用它兜底解卡。
    open_started: Option<std::time::Instant>,
    projects_pending: bool,
    aggregate: Option<crate::frontend::gpui::chrome::panel_view::AggregateKind>,
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
        socket: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let visible = store
            .active_workspace_id()
            .map(str::to_string)
            .or_else(|| store.workspace_ids().next().map(str::to_string));
        // theme / font 一律来自 Core 快照；拿不到就退回浅色默认，不自己解析配置名。
        let theme = pump
            .client()
            .config_describe()
            .ok()
            .map(|snapshot| theme_from_snapshot(&snapshot))
            .unwrap_or_default();
        let cell = (theme.cell_width(), theme.cell_height());
        let configured_font_size = theme.font_size;
        // `[ui] tab_bar_position`：Swift 默认底部；gpui 之前默认顶部，这里对齐。
        let tab_bar_position = pump
            .client()
            .config_describe()
            .ok()
            .map(|snapshot| tab_bar_position_from_snapshot(&snapshot))
            .unwrap_or_default();
        // 语言与状态栏来源同样来自 Core 配置；读不到就用进程默认。
        let (language, status_bar_mode) = pump
            .client()
            .config_describe()
            .ok()
            .map(|snapshot| {
                (
                    palette_actions::language_from_config(
                        snapshot.values["ui"]["language"]
                            .as_str()
                            .unwrap_or("system"),
                    ),
                    palette_actions::StatusBarMode::from_config(
                        snapshot.values["statusbar"]["mode"]
                            .as_str()
                            .unwrap_or("tmux"),
                    ),
                )
            })
            .unwrap_or((
                crate::frontend::utils::i18n::Language::System,
                palette_actions::StatusBarMode::default(),
            ));
        let mut app = Self {
            pump: Some(pump),
            store,
            commands,
            surfaces: HashMap::new(),
            visible,
            focused_pane: None,
            cell_metrics_key: None,
            pending_focus: None,
            shell_restart_requested: false,
            closed_workspaces: std::collections::HashSet::new(),
            quit_requested: false,
            last_sizes: HashMap::new(),
            wheel_accumulators: HashMap::new(),
            focus_handle: cx.focus_handle(),
            marked_text: String::new(),
            panel: crate::frontend::gpui::chrome::panel_view::PanelState::default(),
            target_editor: None,
            target_editor_error: None,
            target_editor_focus:
                crate::frontend::gpui::model::target_editor::TargetEditorFocus::Path,
            settings_open: false,
            settings_manifest: crate::frontend::gpui::model::settings::SettingsManifest::default(),
            settings_draft: crate::frontend::gpui::model::settings::SettingsDraft::default(),
            settings_focus: SettingsFocus::default(),
            settings_edit: None,
            settings_discard_pending: false,
            key_consumed: false,
            ime_deferred: false,
            composer: crate::frontend::gpui::model::composer::ComposerDraftStore::new(),
            composer_open: false,
            composer_selection: crate::frontend::gpui::model::composer::TextRange::default(),
            composer_key: None,
            composer_status: None,
            search_snapshot: None,
            tab_bar_position,
            sidebar_visible: true,
            system_pasteboard: true,
            attention: None,
            existing: Vec::new(),
            existing_loaded: false,
            existing_loading: false,
            socket: socket.clone(),
            panel_scroll: gpui::ScrollHandle::new(),
            open_pending: false,
            open_started: None,
            projects_pending: false,
            aggregate: None,
            sidebar_expansion:
                crate::frontend::gpui::chrome::sidebar_view::SectionExpansion::default(),
            hidden_commands: std::collections::HashSet::new(),
            palette: crate::frontend::gpui::chrome::palette_view::PaletteState::default(),
            info: None,
            info_status: None,
            palette_menu: None,
            prompt: None,
            language,
            status_bar_mode,
            status_snapshot: None,
            status_snapshot_at: None,
            attention_at: None,
            config_reload_pending: false,
            input_urgent_until: None,
            image_paste: crate::frontend::gpui::model::image_paste::ImagePasteTimeline::new(),
            show_title_bar: true,
            applied_window_title: String::new(),
            pane_title_hover: None,
            pane_menu: None,
            selection: None,
            dragging_selection: false,
            theme,
            cell,
            last_client_size: None,
            configured_font_size,
            status: String::from("muxterm · gpui"),
        };
        app.sync_aggregate_presentation();
        app.focus_default_pane();
        // 配置里的语言在启动时立刻生效（越早越好，避免第一帧是错的文案）。
        crate::frontend::utils::i18n::set_language(app.language);
        if let Ok(snapshot) = app.pump.as_ref().unwrap().client().config_describe() {
            app.panel.picker.projects =
                crate::frontend::gpui::model::workspace_picker::ProjectList::new(
                    &snapshot.values["projects"],
                );
        }
        app.start_pump(cx);
        app
    }

    /// 重新读一次 Core 配置并刷新主题/字体（设置 Apply 之后调用）。
    pub fn reload_appearance(&mut self, cx: &mut Context<Self>) {
        let Some(snapshot) = self
            .pump
            .as_ref()
            .and_then(|pump| pump.client().config_describe().ok())
        else {
            return;
        };
        self.apply_appearance_snapshot(&snapshot, cx);
    }

    /// 用一份配置快照重建主题与字体。终端调色板与 chrome 明暗都只由它决定。
    pub fn apply_appearance_snapshot(
        &mut self,
        snapshot: &crate::frontend::utils::corebridge::ClientConfigSnapshot,
        cx: &mut Context<Self>,
    ) {
        let theme = theme_from_snapshot(snapshot);
        self.show_title_bar = ui_config_from_snapshot(snapshot).show_title_bar;
        self.cell = (theme.cell_width(), theme.cell_height());
        self.configured_font_size = theme.font_size;
        self.theme = theme;
        cx.notify();
    }

    /// 渲染期从 `TextSystem` 量出真实字形尺寸。
    ///
    /// 前端上报给 Core 的 cols/rows 与鼠标命中都用这个值，所以不能长期停在
    /// 0.6em 的估算上（字体一换就会错列）。
    pub fn measure_cell(&mut self, window: &mut Window) {
        // 字体 / 字号 / 行距没变就不用再量：每帧三次 shaping 在 60fps 下
        // 是纯浪费，而且结果不会变。
        let key = (
            self.theme.font_family.clone(),
            self.theme.font_size.to_bits(),
            self.theme.line_spacing.to_bits(),
            self.theme.column_spacing.to_bits(),
        );
        if self.cell_metrics_key.as_ref() == Some(&key) {
            return;
        }
        let (width, height) = measure_cell_size(&self.theme, window.text_system());
        self.cell_metrics_key = Some(key);
        if (width - self.cell.0).abs() > 0.01 || (height - self.cell.1).abs() > 0.01 {
            self.cell = (width, height);
        }
    }

    /// 当前格子的像素尺寸（宽, 高）。
    pub fn cell(&self) -> (f32, f32) {
        self.cell
    }

    /// 最近是否刚写过输入（决定事件泵要不要保持快轮询）。
    pub fn input_is_pending(&mut self) -> bool {
        match self.input_urgent_until {
            Some(deadline) if std::time::Instant::now() < deadline => true,
            Some(_) => {
                self.input_urgent_until = None;
                false
            }
            None => false,
        }
    }

    /// 记一笔「刚写过输入」：从现在起一个短窗口内保持快轮询。
    fn note_input_sent(&mut self) {
        self.input_urgent_until = Some(std::time::Instant::now() + INPUT_URGENT_WINDOW);
    }

    /// 把原生菜单栏的点击接回来：菜单只发 `KeyAction`，执行仍走 `run_action`。
    pub fn watch_menu(
        &mut self,
        mut receiver: futures::channel::mpsc::UnboundedReceiver<
            crate::frontend::gpui::model::shortcuts::KeyAction,
        >,
        cx: &mut Context<Self>,
    ) {
        use futures::StreamExt;
        cx.spawn(async move |_this, cx| {
            while let Some(action) = receiver.next().await {
                let Ok(windows) = cx.update(|cx| cx.windows()) else {
                    break;
                };
                for handle in windows {
                    let _ = handle.update(cx, |view, window, cx| {
                        if let Ok(entity) = view.downcast::<crate::frontend::gpui::state::GpuiApp>()
                        {
                            entity.update(cx, |app, cx| {
                                app.run_action(action, window, cx);
                            });
                        }
                    });
                }
            }
        })
        .detach();
    }

    fn start_pump(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut delay = PUMP_INTERVAL;
            loop {
                Timer::after(delay).await;
                let outcome = this.update(cx, |app, cx| {
                    let dirty = app.pump_once(cx);
                    if dirty {
                        cx.notify();
                    }
                    let quit = app.quit_requested();
                    (dirty, app.input_is_pending(), quit)
                });
                let Ok((dirty, pending, quit)) = outcome else {
                    break;
                };
                if quit {
                    let _ = cx.update(|cx| cx.quit());
                    break;
                }
                delay = next_pump_delay(delay, dirty, pending);
            }
        })
        .detach();
    }

    /// 收一批事件、喂 Surface、冲刷命令。返回是否有可见变化。
    /// 某个 pane 现有多少行历史（测试用来确认 `PaneHistory` 真的落地了）。
    /// 测试注入：直接给某个 pane 一块画面（不经过 FFI / 事件泵）。
    pub fn feed_pane_for_tests(
        &mut self,
        workspace: &str,
        pane_id: u32,
        cols: u16,
        rows: u16,
        ansi: &[u8],
        cx: &mut Context<Self>,
    ) {
        let surface = self
            .surfaces
            .entry((workspace.to_string(), pane_id))
            .or_insert_with(|| Surface::new(cols as usize, rows as usize));
        surface.resize(cols as usize, rows as usize);
        surface.feed_full(ansi);
        cx.notify();
    }

    pub fn history_depth_for_tests(&self, workspace: &str, pane_id: u32) -> usize {
        self.surfaces
            .get(&(workspace.to_string(), pane_id))
            .map(|surface| surface.max_scroll_offset() as usize)
            .unwrap_or(0)
    }

    /// 某个 pane 当前的视口偏移（0 = 贴着 live 屏幕）。
    pub fn history_offset_for_tests(&self, workspace: &str, pane_id: u32) -> u32 {
        self.surfaces
            .get(&(workspace.to_string(), pane_id))
            .map(|surface| surface.scroll_offset())
            .unwrap_or(0)
    }

    /// 剪贴板里是图片就交给 Core 上传，返回是否已经接管这次粘贴。
    ///
    /// 对照 Swift `ImagePasteController`：前端只负责读剪贴板、编码成 PNG、
    /// 显示阶段横幅；真正把文件送到目标机器的是 Core（`start_image_paste`），
    /// 前端不拼 `scp`、不自己起进程。
    pub fn paste_clipboard_image(&mut self, cx: &mut Context<Self>) -> bool {
        let now = std::time::Instant::now();

        // 与 Swift `MuxTerminalView.paste(from:)` 同一优先级：系统剪贴板里
        // 有 PNG/TIFF 就粘图，哪怕同一个 pasteboard 还带着文本（浏览器复制
        // 图片、Keynote 等都会这样）。gpui 的 `read_from_clipboard` 先看文本，
        // 只走它会把图片当 URL / 文件名粘进终端。
        #[cfg(target_os = "macos")]
        if let Some(result) = self
            .system_pasteboard
            .then(crate::frontend::gpui::platform::clipboard_image::clipboard_image_png)
            .flatten()
        {
            // 已经有一张在传：Swift 在这个窗口里回 `ImagePasteBusy`，这里同样不排队。
            if self.image_paste.is_busy() {
                return true;
            }
            match result {
                Ok(png) => self.begin_image_paste(png, now, cx),
                Err(reason) => self.fail_image_paste(reason, now, cx),
            }
            return true;
        }

        let Some(item) = cx.read_from_clipboard() else {
            return false;
        };
        let Some(image) = item.entries().iter().find_map(|entry| match entry {
            gpui::ClipboardEntry::Image(image) => Some(image),
            gpui::ClipboardEntry::String(_) => None,
        }) else {
            return false;
        };
        if self.image_paste.is_busy() {
            return true;
        }
        // 剪贴板给的是 TIFF/JPEG 也要转成 PNG（Core 只收 PNG）。转不了就
        // 明确告诉用户，而不是悄悄当文本粘贴、什么都不发生。
        let Some(png) =
            crate::frontend::gpui::platform::clipboard_image::to_png(image.format, &image.bytes)
        else {
            self.fail_image_paste(
                format!(
                    "unsupported clipboard image ({:?}); copy it as PNG",
                    image.format
                ),
                now,
                cx,
            );
            return true;
        };
        self.begin_image_paste(png, now, cx);
        true
    }

    /// 把一张已经编码好的 PNG 交给 Core（前端只做横幅，不拼 scp）。
    fn begin_image_paste(&mut self, png: Vec<u8>, now: std::time::Instant, cx: &mut Context<Self>) {
        use crate::frontend::gpui::model::image_paste::{ImagePasteStage, ImagePasteTimeline};
        let (Some(workspace), Some(pane)) = (self.visible.clone(), self.focused_pane) else {
            self.fail_image_paste("no active pane for the image".into(), now, cx);
            return;
        };
        let Some(pump) = self.pump.as_ref() else {
            self.fail_image_paste("not connected to Core".into(), now, cx);
            return;
        };
        let mut timeline = ImagePasteTimeline::new();
        timeline.advance(ImagePasteStage::Encoding, now);
        match pump
            .client()
            .start_image_paste(&workspace, pane, png.as_slice())
        {
            Ok(()) => {
                timeline.advance(ImagePasteStage::Uploading, now);
                self.image_paste = timeline;
            }
            Err(error) => {
                timeline.advance(ImagePasteStage::Failed(error.to_string()), now);
                self.image_paste = timeline;
            }
        }
        cx.notify();
    }

    /// 图片粘贴失败：左侧阶段 + 横幅同时更新（用户永远能看到发生什么）。
    fn fail_image_paste(
        &mut self,
        reason: String,
        now: std::time::Instant,
        cx: &mut Context<Self>,
    ) {
        let mut timeline = crate::frontend::gpui::model::image_paste::ImagePasteTimeline::new();
        timeline.advance(
            crate::frontend::gpui::model::image_paste::ImagePasteStage::Failed(reason),
            now,
        );
        self.image_paste = timeline;
        cx.notify();
    }

    /// 每个 tick 拉一次上传结果：Core 拿到路径才算完成。
    pub fn poll_image_paste(&mut self) -> bool {
        use crate::frontend::gpui::model::image_paste::ImagePasteStage;
        let now = std::time::Instant::now();
        let before = self.image_paste.visible_stage(now);
        self.image_paste.tick(now);
        if !before.is_progress() {
            return before != self.image_paste.visible_stage(now);
        }
        let Some(pump) = self.pump.as_ref() else {
            return false;
        };
        match pump.client().poll_image_paste() {
            Ok(Some(path)) => self.image_paste.advance(ImagePasteStage::Done(path), now),
            Ok(None) => {}
            Err(error) => self
                .image_paste
                .advance(ImagePasteStage::Failed(error.to_string()), now),
        }
        before != self.image_paste.visible_stage(now)
    }

    /// 无头测试用：直接设定横幅阶段，避免依赖真实剪贴板与上传。
    pub fn set_image_paste_stage_for_tests(
        &mut self,
        stage: crate::frontend::gpui::model::image_paste::ImagePasteStage,
    ) {
        self.image_paste.advance(stage, std::time::Instant::now());
    }

    /// 横幅内容（`Idle` 时 None）。
    pub fn image_paste_feedback(
        &self,
    ) -> Option<crate::frontend::gpui::model::image_paste::ImagePasteFeedback> {
        use crate::frontend::gpui::model::image_paste::ImagePasteFeedback;
        let stage = self.image_paste.visible_stage(std::time::Instant::now());
        ImagePasteFeedback::make(&stage)
    }

    pub fn pump_once(&mut self, cx: &mut Context<Self>) -> bool {
        let mut dirty = false;
        if let Some(pump) = self.pump.as_ref() {
            let events = pump.poll_into_with_events(&mut self.store);
            dirty |= events.iter().any(|event| event.event.is_topology());
            // `commands` 与 `pump` 是不同字段，借用互不冲突。
            let outcomes = self.commands.flush(pump.client());
            if outcomes.iter().any(|code| *code != 0)
                && (self.open_pending || self.projects_pending)
            {
                self.panel.picker.error =
                    Some("Core rejected the operation; no changes were applied".into());
                self.open_pending = false;
                self.projects_pending = false;
            }
        }
        let active = self.store.active_workspace_id().map(str::to_string);
        if self.visible.is_none() {
            self.visible = active;
            dirty |= self.sync_aggregate_presentation();
        }
        dirty |= self.reconcile_closed_panes();
        dirty |= self.reconcile_workspace_lifecycle(cx);
        dirty |= self.poll_panel_operations(cx);
        dirty |= self.drop_tombstoned_workspaces(cx);
        dirty |= self.apply_pending_config();
        dirty |= self.refresh_status_snapshot_if_due();
        dirty |= self.refresh_attention_if_due();
        dirty |= self.poll_image_paste();
        // 焦点跟着 Core 的 active pane 走：切 tab、分屏、Cmd-[ / Cmd-] 之后
        // 键盘输入必须落在屏幕上真正活动的那个 pane 里，否则用户看到的是
        // 「切了 / 分了但光标不见了、打字没反应」。
        dirty |= self.sync_focused_pane();
        dirty |= self.drain_render_events();
        self.sync_menu_state();
        let _ = cx;
        dirty
    }

    /// 把运行时可变的菜单状态（勾选 / 置灰）同步到原生菜单栏。
    fn sync_menu_state(&self) {
        use crate::frontend::gpui::model::menu::MenuState;
        let state = MenuState {
            tab_bar_on_top: self.tab_bar_position()
                == crate::frontend::gpui::chrome::tab_bar_view::TabBarPosition::Top,
            has_selection: self.selection.is_some(),
            has_pane: self.focused_pane.is_some(),
        };
        let _ = crate::frontend::gpui::platform::refresh_menu(state);
    }

    /// 把焦点同步到当前 tab 的活动 pane。
    ///
    /// 返回是否发生变化（调用方据此重绘光标）。用户刚点过某个 pane 时
    /// `pending_focus` 还没被 Core 确认，这个窗口期不抢焦点，避免来回跳。
    pub fn sync_focused_pane(&mut self) -> bool {
        let Some(workspace) = self.visible.clone() else {
            return false;
        };
        let Some(view) = self.store.workspace(&workspace) else {
            return false;
        };
        let Some(tab) = view.active_tab_id() else {
            return false;
        };
        let panes = view.panes_for_tab(tab);
        if panes.is_empty() {
            return false;
        }
        let core_active = panes.iter().find(|pane| pane.is_active).map(|pane| pane.id);
        if let Some(pending) = self.pending_focus {
            if core_active == Some(pending) {
                self.pending_focus = None;
            } else if panes.iter().any(|pane| pane.id == pending) {
                return false;
            } else {
                // Core 没听（pane 已经没了）：放弃这次待定焦点。
                self.pending_focus = None;
            }
        }
        let target = core_active.or_else(|| panes.first().map(|pane| pane.id));
        let Some(target) = target else {
            return false;
        };
        if self.focused_pane == Some(target) {
            return false;
        }
        self.focused_pane = Some(target);
        true
    }

    /// 首屏就是本机 shell 时按 Shells 聚合展示。
    ///
    /// 对照 Swift 启动路径：初始 workspace 是 shell 时
    /// `workspacePresentation = .shells(...)`，所以侧栏高亮在 Shells 上，
    /// 而不是没有行可选。
    fn sync_aggregate_presentation(&mut self) -> bool {
        use crate::frontend::gpui::chrome::panel_view::AggregateKind;
        if self.aggregate.is_some() {
            return false;
        }
        let Some(workspace) = self.visible.as_deref() else {
            return false;
        };
        if self.workspace_runtime(workspace) != "shell" {
            return false;
        }
        self.aggregate = Some(AggregateKind::Shells);
        true
    }

    /// 本机 shell 补 tab / 已结束的 workspace 退出 / 焦点回到活着的 workspace。
    ///
    /// 对照 Swift `maybeCloseIfSessionEnded()`：
    /// - 本机 shell 空（最后一个 pane 退出）→ 补一个新 tab，窗口不留空白；
    /// - 其他 runtime（tmux / herdr / ssh）后端已 `Exited` → 这个 workspace
    ///   的 pane 全是僵尸，收起它并切到邻近 workspace；没有别的就退出窗口。
    ///
    /// 旧实现只处理了本机 shell：远端 shell 退出（`%exit`）后页面停在最后
    /// 一帧，键盘没有任何反应，看起来就是「卡住」。
    fn reconcile_workspace_lifecycle(&mut self, cx: &mut Context<Self>) -> bool {
        let mut dirty = false;
        // 先收起所有已结束的非本机 shell workspace；可见的那个要切走。
        let ended: Vec<String> = self
            .store
            .workspace_ids()
            .filter(|workspace_id| self.workspace_has_ended(workspace_id))
            .map(str::to_string)
            .collect();
        for workspace_id in ended {
            let visible = self.visible.as_deref() == Some(workspace_id.as_str());
            self.drop_workspace(&workspace_id);
            dirty = true;
            if visible {
                dirty |= self.switch_to_next_live_workspace(cx);
            }
        }

        let Some(workspace) = self.visible.clone() else {
            return dirty;
        };
        let Some(view) = self.store.workspace(&workspace) else {
            // Core 已经把 workspace 删除（会话结束）：换到邻近 workspace。
            return self.switch_to_next_live_workspace(cx) || dirty;
        };
        let Some(client) = view.workspace.as_ref() else {
            return dirty;
        };
        let empty = view.tabs.is_empty() && view.panes.values().all(Vec::is_empty);
        if !empty {
            self.shell_restart_requested = false;
            return dirty;
        }
        if !(client.runtime == "shell" && transport_label(client) == "local") {
            // 其他 runtime 的空 workspace 没有东西可显示，按同一个规则换走。
            return self.switch_to_next_live_workspace(cx) || dirty;
        }
        // 只补「shell 已经退出了」的空工作区：启动中的空快照不需要抢 tab。
        if view.backend_status
            != Some(crate::frontend::gpui::model::sidebar::backend_status::EXITED)
        {
            return dirty;
        }
        // Core 回包前不要每拍都重发；新 tab 出现后自动复位。
        if self.shell_restart_requested {
            return dirty;
        }
        self.shell_restart_requested = true;
        self.commands.push(ClientCommand::Task {
            workspace_id: Some(workspace),
            task: crate::frontend::utils::corebridge::ClientTask::NewTab,
        });
        dirty
    }

    /// 后端已经退出、且不是会自动补 tab 的本机 shell。
    fn workspace_has_ended(&self, workspace_id: &str) -> bool {
        use crate::frontend::gpui::model::sidebar::backend_status;
        let Some(view) = self.store.workspace(workspace_id) else {
            return false;
        };
        let Some(client) = view.workspace.as_ref() else {
            return false;
        };
        if client.runtime == "shell" && transport_label(client) == "local" {
            return false;
        }
        view.backend_status == Some(backend_status::EXITED)
    }

    /// 把结束的 workspace 从**前端**收起：Core 的 DTO 可能还留着（重连），
    /// 但 pane 已经不再产生画面，继续显示只会是一帧冻住的终端。
    fn drop_workspace(&mut self, workspace_id: &str) {
        self.store.remove_workspace(workspace_id);
        self.surfaces
            .retain(|(workspace, _), _| workspace != workspace_id);
        if self
            .selection
            .is_some_and(|_| self.visible.as_deref() == Some(workspace_id))
        {
            self.selection = None;
            self.dragging_selection = false;
        }
        if self.pane_menu.is_some() && self.visible.as_deref() == Some(workspace_id) {
            self.pane_menu = None;
        }
        self.last_sizes
            .retain(|(workspace, _), _| workspace != workspace_id);
        self.wheel_accumulators
            .retain(|(workspace, _), _| workspace != workspace_id);
    }

    /// 切到按 store 顺序的下一个还有 pane 的 workspace；都没有就退出窗口。
    fn switch_to_next_live_workspace(&mut self, cx: &mut Context<Self>) -> bool {
        let order: Vec<String> = self.store.workspace_ids().map(str::to_string).collect();
        let current = self.visible.clone();
        let start = current
            .as_ref()
            .and_then(|current| order.iter().position(|id| id == current))
            .map(|index| index + 1)
            .unwrap_or(0)
            .min(order.len());
        let target = order[start..]
            .iter()
            .chain(order[..start].iter())
            .find(|workspace_id| {
                Some(workspace_id.as_str()) != current.as_deref()
                    && self.workspace_has_panes(workspace_id)
            })
            .cloned();
        let Some(target) = target else {
            // 没有活着的 workspace：按 Swift 关窗口，不能留一个无响应的空白页。
            // 首帧还没回来（visible 为空）时不能退出，否则一启动就关窗。
            if current.is_some() && !self.quit_requested {
                self.visible = None;
                self.focused_pane = None;
                self.quit_requested = true;
                return true;
            }
            return false;
        };
        self.visible = Some(target.clone());
        self.selection = None;
        self.dragging_selection = false;
        self.pane_menu = None;
        self.pending_focus = None;
        self.focused_pane = None;
        self.commands
            .push(ClientCommand::ActivateWorkspace(target.clone()));
        self.focus_default_pane();
        self.last_sizes.clear();
        let _ = self.sync_aggregate_presentation();
        cx.notify();
        true
    }

    fn workspace_has_panes(&self, workspace_id: &str) -> bool {
        self.store.workspace(workspace_id).is_some_and(|view| {
            view.tabs
                .iter()
                .any(|tab| !view.panes_for_tab(tab.id).is_empty())
        })
    }

    /// 拓扑里已经不存在的 pane：把前端缓存（Surface / 选区 / 菜单）一起丢掉。
    ///
    /// 只清「拓扑和渲染队列都没有」的 pane：本地 shell 可能在
    /// `workspace_list` 回来之前就输出，不能把还没进拓扑的 pane 当死 pane。
    fn reconcile_closed_panes(&mut self) -> bool {
        let mut live: std::collections::HashSet<(String, u32)> = std::collections::HashSet::new();
        for workspace_id in self
            .store
            .workspace_ids()
            .map(str::to_string)
            .collect::<Vec<_>>()
        {
            if let Some(view) = self.store.workspace(&workspace_id) {
                for pane in view.panes.values().flatten() {
                    live.insert((workspace_id.clone(), pane.id));
                }
            }
            for pane_id in self.store.render_mailbox_pane_ids(&workspace_id) {
                live.insert((workspace_id.clone(), pane_id));
            }
        }
        let before = self.surfaces.len();
        self.surfaces.retain(|key, _| live.contains(key));
        let mut dirty = self.surfaces.len() != before;
        if let Some(selection) = self.selection {
            let key = (self.visible.clone().unwrap_or_default(), selection.pane_id);
            if !live.contains(&key) {
                self.selection = None;
                self.dragging_selection = false;
                dirty = true;
            }
        }
        if let Some(menu) = self.pane_menu {
            let key = (self.visible.clone().unwrap_or_default(), menu.pane_id);
            if !live.contains(&key) {
                self.pane_menu = None;
                dirty = true;
            }
        }
        dirty
    }

    /// 把 store 里排队的渲染事件喂给各自的 Surface。
    ///
    /// 从 `pump_once` 拆出来，无头测试才能在没有 FFI pump 的情况下验证
    /// `PaneHistory` 这类事件真的落了地。
    pub fn drain_render_events(&mut self) -> bool {
        let mut dirty = false;
        let workspaces: Vec<String> = self.store.workspace_ids().map(str::to_string).collect();
        for workspace in workspaces {
            // 拓扑里列出的 pane + 已经有输出排队的 pane：后者可能还没进
            // `view.panes`（本地 shell 抢在 `workspace_list` 前面输出）。
            let mut pane_ids: Vec<(u32, u16, u16)> = self
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
            for pane_id in self.store.render_mailbox_pane_ids(&workspace) {
                if pane_ids.iter().any(|(id, _, _)| *id == pane_id) {
                    continue;
                }
                // 尺寸未知时用已有 Surface 的，最后退回 80x24。
                let size = self
                    .surfaces
                    .get(&(workspace.clone(), pane_id))
                    .map(|surface| (surface.grid().cols() as u16, surface.grid().rows() as u16))
                    .unwrap_or((80, 24));
                pane_ids.push((pane_id, size.0, size.1));
            }
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
                        // Core 的 attach 历史回填：纯文本行，直接进 scrollback。
                        ClientEventKind::PaneHistory => {
                            let added = surface.feed_history(&event.data);
                            tracing::debug!(pane_id, added, "pane history");
                        }
                        ClientEventKind::PaneClosed
                        | ClientEventKind::PaneResized
                        | ClientEventKind::Other(_) => {}
                    }
                }
                dirty = true;
            }
        }
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
            cell_metrics_key: None,
            pending_focus: None,
            shell_restart_requested: false,
            closed_workspaces: std::collections::HashSet::new(),
            quit_requested: false,
            last_sizes: HashMap::new(),
            wheel_accumulators: HashMap::new(),
            focus_handle: cx.focus_handle(),
            marked_text: String::new(),
            panel: crate::frontend::gpui::chrome::panel_view::PanelState::default(),
            target_editor: None,
            target_editor_error: None,
            target_editor_focus:
                crate::frontend::gpui::model::target_editor::TargetEditorFocus::Path,
            settings_open: false,
            settings_manifest: crate::frontend::gpui::model::settings::SettingsManifest::default(),
            settings_draft: crate::frontend::gpui::model::settings::SettingsDraft::default(),
            settings_focus: SettingsFocus::default(),
            settings_edit: None,
            settings_discard_pending: false,
            key_consumed: false,
            ime_deferred: false,
            composer: crate::frontend::gpui::model::composer::ComposerDraftStore::new(),
            composer_open: false,
            composer_selection: crate::frontend::gpui::model::composer::TextRange::default(),
            composer_key: None,
            // 无头环境不碰真实剪贴板：测试里的 ClipboardItem 才是唯一输入。
            system_pasteboard: false,
            composer_status: None,
            search_snapshot: None,
            tab_bar_position: crate::frontend::gpui::chrome::tab_bar_view::TabBarPosition::default(
            ),
            sidebar_visible: true,
            attention: None,
            attention_at: None,
            existing: Vec::new(),
            existing_loaded: false,
            existing_loading: false,
            socket: None,
            panel_scroll: gpui::ScrollHandle::new(),
            open_pending: false,
            open_started: None,
            projects_pending: false,
            aggregate: None,
            sidebar_expansion:
                crate::frontend::gpui::chrome::sidebar_view::SectionExpansion::default(),
            hidden_commands: std::collections::HashSet::new(),
            palette: crate::frontend::gpui::chrome::palette_view::PaletteState::default(),
            info: None,
            info_status: None,
            palette_menu: None,
            prompt: None,
            language: crate::frontend::utils::i18n::Language::System,
            status_bar_mode: palette_actions::StatusBarMode::default(),
            status_snapshot: None,
            status_snapshot_at: None,
            config_reload_pending: false,
            input_urgent_until: None,
            image_paste: crate::frontend::gpui::model::image_paste::ImagePasteTimeline::new(),
            show_title_bar: true,
            applied_window_title: String::new(),
            pane_title_hover: None,
            pane_menu: None,
            selection: None,
            dragging_selection: false,
            theme: Theme::default(),
            cell: (
                Theme::default().cell_width(),
                Theme::default().cell_height(),
            ),
            last_client_size: None,
            configured_font_size: crate::frontend::gpui::theme::DEFAULT_FONT_SIZE,
            status: String::from("muxterm · gpui"),
        };
        app.sync_aggregate_presentation();
        app.focus_default_pane();
        app
    }

    /// 无头测试用的注意力快照注入。
    #[doc(hidden)]
    pub fn set_attention(
        &mut self,
        attention: Option<crate::frontend::gpui::model::attention::AttentionSnapshot>,
    ) {
        self.attention = attention;
    }

    /// 无头测试用：注入一份 Core 搜索快照。
    #[doc(hidden)]
    pub fn set_search_for_tests(
        &mut self,
        snapshot: crate::frontend::gpui::model::search::SearchSnapshot,
    ) {
        self.search_snapshot = Some(snapshot);
    }

    /// 无头测试用：注入一份 Core 活动快照，走与生产同一条映射。
    #[doc(hidden)]
    pub fn set_activity_snapshot_for_tests(
        &mut self,
        snapshot: &crate::frontend::utils::corebridge::ClientActivitySnapshot,
    ) -> bool {
        let mut next =
            crate::frontend::gpui::model::attention::AttentionSnapshot::snapshot_from_activity(
                snapshot,
            );
        self.join_attention_to_workspaces(&mut next);
        if self.attention.as_ref() == Some(&next) {
            return false;
        }
        self.attention = Some(next);
        true
    }

    /// 把活动快照的 replica id 换成本前端的产品 workspace id。
    ///
    /// Core 的 `attention_snapshot` 用 `WorkspaceId::replica_id()`
    /// （`session:path@transport`），侧栏用产品 id（`transport/alias/session/runtime/path`）。
    /// 不换就永远 join 不上——这正是「跑着 pi，Agents 还是空」的原因。
    fn join_attention_to_workspaces(
        &self,
        snapshot: &mut crate::frontend::gpui::model::attention::AttentionSnapshot,
    ) {
        use crate::frontend::gpui::model::replica_id::replica_id_for_workspace_id;
        let ids = self
            .store
            .workspace_ids()
            .map(str::to_string)
            .collect::<Vec<_>>();
        for workspace in &mut snapshot.workspaces {
            let Some(id) = ids
                .iter()
                .find(|id| replica_id_for_workspace_id(id) == workspace.workspace_id)
                .cloned()
            else {
                continue;
            };
            workspace.name = self.workspace_name(&id);
            workspace.transport = self
                .store
                .workspace(&id)
                .and_then(|view| view.workspace.as_ref())
                .map(transport_label)
                .unwrap_or_default();
            workspace.workspace_id = id;
        }
    }

    /// 拉一次 Core 的活动/注意力快照（Agents / Commands / Attention 的数据源）。
    ///
    /// 返回是否有变化。查不到就保留上一次的快照，不把侧栏清空。
    pub fn refresh_attention(&mut self) -> bool {
        let Some(pump) = self.pump.as_ref() else {
            return false;
        };
        let Ok(snapshot) = pump.client().activity_snapshot() else {
            return false;
        };
        self.set_activity_snapshot_for_tests(&snapshot)
    }

    /// 按 `ATTENTION_INTERVAL` 节流拉活动快照。
    pub fn refresh_attention_if_due(&mut self) -> bool {
        let now = std::time::Instant::now();
        if let Some(last) = self.attention_at {
            if now.duration_since(last) < ATTENTION_INTERVAL {
                return false;
            }
        }
        self.attention_at = Some(now);
        self.refresh_attention()
    }

    /// 验收测试用：拿到生产路径上的 Core 客户端（只读操作）。
    #[doc(hidden)]
    pub fn client_for_tests(&self) -> Option<&crate::frontend::utils::corebridge::FfiClient> {
        self.pump.as_ref().map(|pump| pump.client())
    }

    /// 当前活动快照里的 workspace id（验收测试断言 join 用）。
    #[doc(hidden)]
    pub fn attention_workspace_ids(&self) -> Vec<String> {
        self.attention
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .workspaces
                    .iter()
                    .map(|workspace| workspace.workspace_id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 无头测试用：替换整个 ViewStore（模拟 Core 回了新拓扑）。
    #[doc(hidden)]
    pub fn replace_store_for_tests(
        &mut self,
        store: crate::frontend::view_store::ViewStore,
        cx: &mut Context<Self>,
    ) {
        self.store = store;
        self.last_sizes.clear();
        self.focused_pane = None;
        self.focus_default_pane();
        cx.notify();
    }

    /// 无头测试用：把一条 Core 事件塞进 ViewStore（走生产的 mailbox 路径）。
    #[doc(hidden)]
    pub fn store_for_tests_event(
        &mut self,
        event: crate::frontend::utils::corebridge::ClientWorkspaceEvent,
    ) {
        self.store.apply_workspace_event(event);
    }

    /// 无头测试用：取走待发送给 Core 的命令。
    #[doc(hidden)]
    pub fn take_commands_for_tests(&mut self) -> Vec<ClientCommand> {
        self.commands.drain()
    }

    /// 无头测试用：上一个按键有没有被消费（消费后才不会被原生菜单再跑一遍）。
    #[doc(hidden)]
    pub fn take_key_consumed_for_tests(&mut self) -> bool {
        std::mem::take(&mut self.key_consumed)
    }

    /// 无头测试用：上一个按键有没有让给平台输入法（中文组词的入口）。
    #[doc(hidden)]
    pub fn take_ime_deferred_for_tests(&mut self) -> bool {
        std::mem::take(&mut self.ime_deferred)
    }

    /// 是否应该关窗口（最后一个活着的 workspace 也结束了）。
    pub fn quit_requested(&self) -> bool {
        self.quit_requested
    }

    /// 侧栏是否显示。
    /// 当前 workspace 的后端状态（状态栏状态点用）。
    pub fn visible_backend_status(&self) -> u32 {
        self.visible
            .as_deref()
            .and_then(|id| self.store.workspace(id))
            .and_then(|view| view.backend_status)
            .unwrap_or(crate::frontend::gpui::model::sidebar::backend_status::DISCONNECTED)
    }

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

    /// 是否显示窗口标题（`[ui] show_title_bar`）。
    pub fn show_title_bar(&self) -> bool {
        self.show_title_bar
    }

    /// 设置窗口标题开关（`[ui] show_title_bar`）。
    pub fn set_show_title_bar(&mut self, show: bool) {
        self.show_title_bar = show;
    }

    /// 已写到原生窗口上的标题。
    pub fn applied_window_title(&self) -> &str {
        &self.applied_window_title
    }

    pub fn set_applied_window_title(&mut self, title: String) {
        self.applied_window_title = title;
    }

    /// 当前 workspace 的显示名（窗口标题用）。
    pub fn visible_workspace_name(&self) -> String {
        let Some(workspace) = self.visible.clone() else {
            return String::new();
        };
        self.workspace_name(&workspace)
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

    /// 参与编号的 workspace：按打开顺序，剔除本机 shell 的原始 workspace。
    ///
    /// 侧栏、面板行首数字与 `Cmd-Ctrl-1..9` 都必须用**同一份**列表，
    /// 否则本机 shell 占着第 1 位，用户按 1 却跳到侧栏里显示为 2 的行。
    pub fn numbered_workspace_ids(&self) -> Vec<String> {
        self.workspace_ids()
            .into_iter()
            .filter(|id| self.workspace_runtime(id) != "shell")
            .collect()
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
        if self.aggregate.is_some() && self.aggregate_pages().is_empty() {
            return None;
        }
        let view = self.store.workspace(self.visible.as_deref()?)?;
        let tab = view.active_tab_id()?;
        view.layouts.get(&tab).cloned()
    }

    /// 当前 tab 可见的 pane 数量（标题条只在 > 1 时允许出现）。
    pub fn visible_pane_count(&self) -> usize {
        self.layout()
            .map(|layout| {
                crate::frontend::gpui::model::pane_navigation::layout_leaf_ids(&layout).len()
            })
            .unwrap_or(1)
    }

    /// 标题条当前是否显示：多 pane、鼠标停在顶部，或这个 pane 的菜单开着。
    ///
    /// 对照 Swift `PaneHostView.updateTitleHover`（只在鼠标进入顶部 22pt 时显示）
    /// 与 `showActionMenu`（菜单打开时标题条保持可见）。
    pub fn shows_pane_title(&self, pane_id: u32) -> bool {
        crate::frontend::gpui::model::pane_title::shows_title_bar(self.visible_pane_count())
            && (self.pane_title_hover == Some(pane_id)
                || self
                    .pane_menu
                    .as_ref()
                    .is_some_and(|menu| menu.pane_id == pane_id))
    }

    /// 鼠标当前悬停的标题条 pane（测试与渲染共用）。
    pub fn hovered_pane_title(&self) -> Option<u32> {
        self.pane_title_hover
    }

    /// 鼠标移动后的悬停状态：只有 pane 顶部 `TITLE_BAR_HEIGHT` 内才算。
    pub fn note_pane_hover(&mut self, pane_id: u32, hovered: bool, cx: &mut Context<Self>) {
        let next = hovered.then_some(pane_id);
        if self.pane_title_hover != next {
            self.pane_title_hover = next;
            cx.notify();
        }
    }

    /// 鼠标离开 pane：清掉悬停（菜单开着时保持标题条可见）。
    pub fn clear_pane_hover(&mut self, pane_id: u32, cx: &mut Context<Self>) {
        if self.pane_title_hover == Some(pane_id) {
            self.pane_title_hover = None;
            cx.notify();
        }
    }

    /// 点 `⋯`：先把焦点交给这个 pane，再开菜单。
    ///
    /// 先后顺序不能反：Swift `showActionMenu` 也是先 `onActivate?()`；
    /// 否则菜单动作会落到另外那个 pane 上。
    pub fn open_pane_menu(&mut self, pane_id: u32, cx: &mut Context<Self>) {
        self.focus_pane(pane_id, cx);
        self.pane_menu = Some(crate::frontend::gpui::model::pane_menu::PaneMenu::new(
            pane_id,
        ));
        cx.notify();
    }

    pub fn close_pane_menu(&mut self, cx: &mut Context<Self>) {
        if self.pane_menu.take().is_some() {
            cx.notify();
        }
    }

    /// 菜单条目（含分隔线），渲染与测试共用。
    pub fn pane_menu_items(
        &self,
    ) -> Option<Vec<crate::frontend::gpui::model::pane_menu::PaneMenuItem>> {
        self.pane_menu
            .as_ref()
            .map(|_| crate::frontend::gpui::model::pane_menu::MENU_ITEMS.to_vec())
    }

    /// 菜单键盘选择上/下移。
    pub fn move_pane_menu(&mut self, delta: i32, cx: &mut Context<Self>) {
        if let Some(menu) = self.pane_menu.as_mut() {
            menu.move_selection(delta);
            cx.notify();
        }
    }

    /// 执行菜单动作：先确保焦点在这个 pane，再走统一的 `run_action`。
    ///
    /// `MoveToNewTab` 没有快捷键（Swift 也只有菜单），直接发 Core 的
    /// `Task::BreakPane`。
    pub fn run_pane_title_action(
        &mut self,
        pane_id: u32,
        action: crate::frontend::gpui::model::pane_title::PaneTitleAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        use crate::frontend::gpui::model::pane_title::PaneTitleAction;
        self.pane_menu = None;
        self.focus_pane(pane_id, cx);
        if action == PaneTitleAction::MoveToNewTab {
            self.commands.push(ClientCommand::Task {
                workspace_id: self.visible.clone(),
                task: crate::frontend::utils::corebridge::ClientTask::BreakPane { pane_id },
            });
            cx.notify();
            return true;
        }
        match action.key_action() {
            Some(key_action) => self.run_action(key_action, window, cx),
            None => false,
        }
    }

    /// 某个 pane 的选区（渲染用）。
    pub fn selection_for(
        &self,
        pane_id: u32,
    ) -> Option<crate::frontend::gpui::model::selection::Selection> {
        self.selection
            .filter(|selection| selection.pane_id == pane_id)
            .map(|selection| selection.selection)
    }

    /// 按下鼠标：按点击次数决定普通拖选 / 选词 / 整行。
    pub fn begin_selection(
        &mut self,
        pane_id: u32,
        row: usize,
        col: usize,
        click_count: usize,
        cx: &mut Context<Self>,
    ) {
        use crate::frontend::gpui::model::selection as model;
        let Some(workspace) = self.visible.clone() else {
            return;
        };
        let cells: Vec<char> = self
            .surface(&workspace, pane_id)
            .and_then(|surface| surface.visible_row(row))
            .map(|cells| cells.iter().map(|cell| cell.ch).collect())
            .unwrap_or_default();
        let cols = self
            .surface(&workspace, pane_id)
            .map(|surface| surface.grid().cols())
            .unwrap_or(80);
        let pos = model::CellPos::new(row, col);
        let selection = match click_count {
            2 => model::word_selection(&cells, pos),
            3.. => model::line_selection(row, cols),
            _ => model::Selection::new(pos, pos, model::Kind::Simple),
        };
        self.selection = Some(PaneSelection { pane_id, selection });
        self.dragging_selection = click_count < 2;
        cx.notify();
    }

    /// 拖拽：把焦点移到新格子。
    ///
    /// 双击选词后再拖，按字符扩展（与 SwiftTerm 一致：只有双击那一下按词）。
    pub fn extend_selection(
        &mut self,
        pane_id: u32,
        row: usize,
        col: usize,
        cx: &mut Context<Self>,
    ) {
        use crate::frontend::gpui::model::selection as model;
        if !self.dragging_selection {
            return;
        }
        let Some(current) = self.selection.filter(|current| current.pane_id == pane_id) else {
            return;
        };
        let focus = model::CellPos::new(row, col);
        if current.selection.focus == focus {
            return;
        }
        self.selection = Some(PaneSelection {
            pane_id,
            selection: current.selection.with_focus(focus),
        });
        cx.notify();
    }

    /// 松开鼠标：单格点击不算选区（否则 Cmd-C 会复制一个字符）。
    pub fn finish_drag(&mut self, cx: &mut Context<Self>) {
        self.dragging_selection = false;
        if self
            .selection
            .is_some_and(|current| current.selection.is_single_cell())
        {
            self.selection = None;
            cx.notify();
        }
    }

    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        if self.selection.take().is_some() {
            cx.notify();
        }
        self.dragging_selection = false;
    }

    /// 选区文本（宽字符占位格跳过，行尾空白去掉）。
    pub fn selected_text(&self) -> Option<String> {
        let current = self.selection?;
        let workspace = self.visible.clone()?;
        let surface = self.surface(&workspace, current.pane_id)?;
        let text = current.selection.text(|row| {
            surface
                .visible_row(row)
                .map(|cells| cells.iter().map(|cell| cell.ch).collect())
        });
        (!text.is_empty()).then_some(text)
    }

    /// Edit → Select All：选中当前 pane 的整个视口（含滚出去的历史）。
    ///
    /// 对照 macOS `NSText.selectAll`：整块屏幕 + 当前滚动位置以上的历史
    /// 都算。这里用「视口最后一行、最后一列 → 视口第 0 行、第 0 列」表示，
    /// 复制路径与拖选完全一致。
    pub fn select_all(&mut self, cx: &mut Context<Self>) -> bool {
        let (Some(workspace), Some(pane_id)) = (self.visible.clone(), self.focused_pane) else {
            return false;
        };
        let Some(surface) = self.surface(&workspace, pane_id) else {
            return false;
        };
        let rows = surface.grid().rows();
        let cols = surface.grid().cols();
        if rows == 0 || cols == 0 {
            return false;
        }
        let selection = crate::frontend::gpui::model::selection::Selection::new(
            crate::frontend::gpui::model::selection::CellPos::new(0, 0),
            crate::frontend::gpui::model::selection::CellPos::new(rows - 1, cols - 1),
            crate::frontend::gpui::model::selection::Kind::Simple,
        );
        self.selection = Some(PaneSelection { pane_id, selection });
        self.dragging_selection = false;
        cx.notify();
        true
    }

    /// 菜单 App → Check for Updates…：走 Core 的 `update_check`。
    ///
    /// 结果写进 Info 浮层底部的状态行，同时打开浮层 —— 否则用户点了菜单
    /// 看不到任何反馈。
    pub fn check_for_updates(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(pump) = self.pump.as_ref() else {
            return false;
        };
        let message = match pump.client().update_check() {
            Ok(status) => {
                use crate::frontend::utils::corebridge::ClientUpdatePhase;
                match status.phase {
                    ClientUpdatePhase::Available => match status.version {
                        Some(version) => format!("Update available: {version}"),
                        None => "Update available".to_string(),
                    },
                    ClientUpdatePhase::UpToDate => {
                        format!("Muxterm {} is up to date", status.current_version)
                    }
                    ClientUpdatePhase::Installed => {
                        "Update installed; restart to apply".to_string()
                    }
                    ClientUpdatePhase::Failed => status
                        .message
                        .unwrap_or_else(|| "Update check failed".to_string()),
                    _ => status
                        .message
                        .unwrap_or_else(|| "Update check finished".to_string()),
                }
            }
            Err(error) => format!("Update check failed: {error}"),
        };
        self.info = Some(crate::frontend::utils::corebridge::ClientBuildInfo::load());
        self.info_status = Some(message);
        cx.notify();
        true
    }

    /// Cmd-C / Edit → Copy：把选区写进系统剪贴板。
    pub fn copy_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(text) = self.selected_text() else {
            return false;
        };
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        true
    }

    /// 某个 pane 的标题（Core 拓扑快照里的名字）。
    pub fn pane_title(&self, pane_id: u32) -> Option<String> {
        let workspace = self.store.workspace(self.visible.as_deref()?)?;
        let tab = workspace.active_tab_id()?;
        workspace
            .panes_for_tab(tab)
            .iter()
            .find(|pane| pane.id == pane_id)
            .map(|pane| pane.title.clone())
    }

    /// 侧栏工作区行的输入：顺序、快捷键、连接状态都来自 ViewStore。
    ///
    /// 对照 Swift `MainWindow.sidebarItems()`：
    /// - 两个固定聚合槽（Shells / Agents）永远排在最前面，不可关闭/拖动；
    /// - `shell` runtime 的原始 workspace 不单独占行，它的 tab 只经 Shells 投影；
    /// - 编号快捷键只在真实 workspace 上分配。
    pub fn sidebar_items(&self) -> Vec<crate::frontend::gpui::model::sidebar::SidebarItem> {
        use crate::frontend::gpui::chrome::panel_view::AggregateKind;
        use crate::frontend::gpui::model::sidebar::{
            shortcut_index_by_workspace_id, SidebarItem, AGGREGATE_AGENTS, AGGREGATE_SHELLS,
        };
        let ids = self.numbered_workspace_ids();
        let shortcuts =
            shortcut_index_by_workspace_id(&ids.iter().map(String::as_str).collect::<Vec<_>>());
        let mut items = vec![
            SidebarItem::new(AGGREGATE_SHELLS, "Shells").with(|item| {
                item.runtime = "aggregate".into();
                item.transport = "local + ssh".into();
                item.is_active = self.aggregate == Some(AggregateKind::Shells);
                item.is_closable = false;
                item.is_reorderable = false;
            }),
            SidebarItem::new(AGGREGATE_AGENTS, "Agents").with(|item| {
                item.runtime = "aggregate".into();
                item.transport = "all workspaces".into();
                item.is_active = self.aggregate == Some(AggregateKind::Agents);
                item.is_closable = false;
                item.is_reorderable = false;
            }),
        ];
        items.extend(ids.iter().map(|id| {
            let view = self.store.workspace(id);
            let workspace = view.and_then(|view| view.workspace.as_ref());
            let mut item = SidebarItem::new(id.clone(), self.workspace_name(id));
            item.runtime = self.workspace_runtime(id);
            item.transport = workspace.map(transport_label).unwrap_or_default();
            item.is_active =
                self.aggregate.is_none() && self.visible.as_deref() == Some(id.as_str());
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
                for (number, tab) in view.tabs.iter().enumerate() {
                    for pane in view.panes.get(&tab.id).map(Vec::as_slice).unwrap_or(&[]) {
                        item.tab_number_by_pane.insert(pane.id, number as u32 + 1);
                    }
                }
            }
            item
        }));
        items
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        use crate::frontend::gpui::model::shortcuts::KeyAction;
        use crate::frontend::utils::corebridge::ClientTask;
        match action {
            KeyAction::Quit => {
                cx.quit();
                true
            }
            KeyAction::CloseWindow => {
                window.remove_window();
                true
            }
            KeyAction::ToggleWindowFullscreen => {
                window.toggle_fullscreen();
                true
            }
            KeyAction::ToggleWindowZoom => {
                window.zoom_window();
                true
            }
            KeyAction::Copy => self.copy_selection(cx),
            KeyAction::Paste => {
                // 剪贴板里是图片时交给 Core：它自己把 PNG 传到目标机器并把路径
                // 写进 pane。前端只做 PNG 编码与横幅，绝不自己拼 scp 命令。
                if self.paste_clipboard_image(cx) {
                    return true;
                }
                // 菜单 Edit → Paste：与 Cmd-V 走同一条粘贴路径。
                let text = cx.read_from_clipboard().and_then(|item| {
                    item.entries().iter().find_map(|entry| match entry {
                        gpui::ClipboardEntry::String(text) => Some(text.text().to_string()),
                        gpui::ClipboardEntry::Image(_) => None,
                    })
                });
                if let Some(text) = text {
                    self.handle_paste(&text, cx);
                }
                true
            }
            KeyAction::IncreaseFontSize
            | KeyAction::DecreaseFontSize
            | KeyAction::ResetFontSize => {
                self.zoom_font(action, cx);
                true
            }
            KeyAction::NewTab => {
                // 对照 Swift `newTab()`：Shells 投影里的 Cmd-T 必须落在一个
                // 真实本机 shell 上；连 shell 都没有就先开一个。
                if self.aggregate
                    == Some(crate::frontend::gpui::chrome::panel_view::AggregateKind::Shells)
                {
                    let Some(local) = self.local_shell_workspace() else {
                        self.open_local_shell(cx);
                        return true;
                    };
                    self.commands.push(ClientCommand::Task {
                        workspace_id: Some(local),
                        task: ClientTask::NewTab,
                    });
                    return true;
                }
                self.commands.push(ClientCommand::Task {
                    workspace_id: self.visible.clone(),
                    task: ClientTask::NewTab,
                });
                true
            }
            KeyAction::SplitHorizontal | KeyAction::SplitVertical | KeyAction::SplitAuto => {
                let Some(pane_id) = self.focused_pane else {
                    return false;
                };
                // Swift：Cmd-D 左右、Cmd-Shift-D 上下。水平布局 = 并排。
                // `split_auto` 按像素长宽比选方向（`AutoSplitPolicy`）。
                let horizontal = match action {
                    KeyAction::SplitHorizontal => true,
                    KeyAction::SplitVertical => false,
                    _ => {
                        let (cols, rows) = self.focused_pane_grid_size().unwrap_or((80, 24));
                        let (cell_width, cell_height) = self.cell;
                        crate::frontend::gpui::model::layout_projection::auto_split_horizontal(
                            i32::from(cols),
                            i32::from(rows),
                            f64::from(cell_width),
                            f64::from(cell_height),
                        )
                    }
                };
                self.commands.push(ClientCommand::Task {
                    workspace_id: self.visible.clone(),
                    task: ClientTask::SplitPane {
                        pane_id,
                        horizontal,
                    },
                });
                true
            }
            KeyAction::ClosePane => {
                let Some(pane_id) = self.focused_pane else {
                    return false;
                };
                self.commands.push(ClientCommand::Task {
                    workspace_id: self.visible.clone(),
                    task: ClientTask::ClosePane { pane_id },
                });
                true
            }
            KeyAction::CloseTab => {
                let Some(tab_id) = self.active_tab_id() else {
                    return false;
                };
                self.commands.push(ClientCommand::Task {
                    workspace_id: self.visible.clone(),
                    task: ClientTask::CloseTab { tab_id },
                });
                true
            }
            KeyAction::TogglePaneFullscreen => {
                let Some(pane_id) = self.focused_pane else {
                    return false;
                };
                self.commands.push(ClientCommand::Task {
                    workspace_id: self.visible.clone(),
                    task: ClientTask::TogglePaneFullscreen { pane_id },
                });
                true
            }
            KeyAction::CycleLayout => {
                let Some(tab_id) = self.active_tab_id() else {
                    return false;
                };
                self.commands.push(ClientCommand::Task {
                    workspace_id: self.visible.clone(),
                    task: ClientTask::CycleLayout { tab_id },
                });
                true
            }
            KeyAction::NextPane | KeyAction::PrevPane => {
                // 对照 Swift `MainWindow.movePane(offset:)`：导航范围是当前 tab
                // 的布局叶子，结果用 `SwitchPane` 定位，不能交给全局 active pane 推断。
                let offset = if action == KeyAction::NextPane { 1 } else { -1 };
                self.move_pane_focus(offset, cx)
            }
            KeyAction::SwitchTab(index) => {
                // 聚合投影里的 tab 条是「1..N 个页面」（跨 workspace），不是当前
                // workspace 的真实 tab id：拿真实 id 去查页会跳错行甚至不跳。
                if self.aggregate.is_some() {
                    return self.select_aggregate_tab(index as u32, cx);
                }
                let tabs = self.tabs();
                let Some(tab) = tabs.get(index.saturating_sub(1)) else {
                    return false;
                };
                let tab_id = tab.id;
                self.select_tab(tab_id, cx);
                true
            }
            KeyAction::SwitchLastTab => {
                if self.aggregate.is_some() {
                    let last = self.aggregate_pages().len() as u32;
                    return last > 0 && self.select_aggregate_tab(last, cx);
                }
                let tabs = self.tabs();
                let Some(tab) = tabs.last() else {
                    return false;
                };
                let tab_id = tab.id;
                self.select_tab(tab_id, cx);
                true
            }
            KeyAction::PreviousTab | KeyAction::NextTab => {
                if self.aggregate.is_some() {
                    let pages = self.aggregate_pages();
                    if pages.len() < 2 {
                        return false;
                    }
                    let current = self.aggregate_page_index().unwrap_or(0);
                    let next = if action == KeyAction::PreviousTab {
                        (current + pages.len() - 1) % pages.len()
                    } else {
                        (current + 1) % pages.len()
                    };
                    return self.select_aggregate_tab(next as u32 + 1, cx);
                }
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
            KeyAction::OpenShells | KeyAction::OpenAgents => {
                use crate::frontend::gpui::chrome::panel_view::AggregateKind;
                if self.panel.open && action == KeyAction::OpenShells {
                    self.toggle_panel_actions(cx);
                } else {
                    self.select_aggregate(
                        if action == KeyAction::OpenShells {
                            AggregateKind::Shells
                        } else {
                            AggregateKind::Agents
                        },
                        cx,
                    );
                }
                true
            }
            KeyAction::SwitchWorkspace(index) => {
                let ids = self.numbered_workspace_ids();
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
            KeyAction::OpenSettings => {
                // Swift：Cmd-, 打开/聚焦设置。已经开着就按取消处理（有改动先确认）。
                if self.settings_open {
                    self.settings_cancel(cx);
                    return true;
                }
                // manifest 与当前值都来自 Core 的一次 describe；拿不到就开空窗口，
                // 不编造一份配置树。无头测试用 `open_settings` 直接注入。
                let snapshot = self
                    .pump
                    .as_ref()
                    .and_then(|pump| pump.client().config_describe().ok());
                match snapshot {
                    Some(snapshot) => self.open_settings(&snapshot.manifest, &snapshot.values, cx),
                    None => {
                        self.open_settings(
                            &serde_json::json!({ "groups": [] }),
                            &serde_json::json!({}),
                            cx,
                        );
                    }
                }
                true
            }
            KeyAction::OpenComposer => {
                self.open_composer(cx);
                true
            }
            // Cmd-P 的 QuickConnect 就是统一面板的 Workspaces tab；
            // Cmd-Shift-P 的旧命令面板还没有窗口（见 README 波次 6）。
            KeyAction::QuickConnect => {
                self.show_panel(
                    crate::frontend::gpui::model::panel::PanelTab::Workspaces,
                    cx,
                );
                true
            }
            // Cmd-Shift-P：与 Swift 的 CommandPalette 同一条目录。
            KeyAction::CommandPalette => {
                self.open_palette(cx);
                true
            }
            KeyAction::Attention => {
                self.show_panel(crate::frontend::gpui::model::panel::PanelTab::Attention, cx);
                true
            }
            KeyAction::SearchWorkspace => {
                self.show_panel(crate::frontend::gpui::model::panel::PanelTab::Search, cx);
                true
            }
            KeyAction::ToggleSidebar => {
                self.sidebar_visible = !self.sidebar_visible;
                true
            }
            // 菜单专属动作（Swift 的 selector 一一对应）。
            KeyAction::RenameWorkspace => {
                self.open_rename_workspace(cx);
                true
            }
            KeyAction::RenameTab => {
                self.open_rename_tab(cx);
                true
            }
            KeyAction::MoveTabLeft => {
                self.move_active_tab(-1, cx);
                true
            }
            KeyAction::MoveTabRight => {
                self.move_active_tab(1, cx);
                true
            }
            KeyAction::MovePaneToNewTab => {
                let Some(pane_id) = self.focused_pane else {
                    return false;
                };
                self.commands.push(ClientCommand::Task {
                    workspace_id: self.visible.clone(),
                    task: ClientTask::BreakPane { pane_id },
                });
                true
            }
            KeyAction::SelectAll => self.select_all(cx),
            KeyAction::TabBarTop => {
                self.set_tab_bar_position(
                    crate::frontend::gpui::chrome::tab_bar_view::TabBarPosition::Top,
                    cx,
                );
                true
            }
            KeyAction::TabBarBottom => {
                self.set_tab_bar_position(
                    crate::frontend::gpui::chrome::tab_bar_view::TabBarPosition::Bottom,
                    cx,
                );
                true
            }
            KeyAction::About => {
                self.open_info(cx);
                true
            }
            KeyAction::CheckForUpdates => self.check_for_updates(cx),
            _ => false,
        }
    }

    /// 关闭工作区：先按 Swift `closeWorkspace` 在本地收起并切走，再让 Core 关。
    ///
    /// Core 不会为 close 发 topology 事件（`workspace_list` 之外什么都收不到），
    /// 旧实现只把命令塞进队列，侧栏行和画面就永远留在那里 —— 用户看到的就是
    /// 「关闭 workspace 失败」。这里改成和 Swift 一样的乐观路径：
    /// 1. 本地移除这一格（行、tab、Surface、选区）；
    /// 2. 可见的 workspace 被关时立刻切到邻近 workspace；
    /// 3. 关的是最后一个可见 workspace 时走关窗（窗口退出会连带关闭 Core handle），
    ///    不再发 close。
    pub fn close_workspace(&mut self, workspace_id: String, cx: &mut Context<Self>) {
        // 本机 shell 是聚合入口的锚点，侧栏里也不给它关闭按钮：防御性忽略。
        let closeable = self.store.workspace(&workspace_id).is_some_and(|view| {
            view.workspace.as_ref().is_some_and(|client| {
                !(client.runtime == "shell" && transport_label(client) == "local")
            })
        });
        if !closeable {
            return;
        }
        let was_visible = self.visible.as_deref() == Some(workspace_id.as_str());
        let has_fallback = self
            .store
            .workspace_ids()
            .any(|id| id != workspace_id && self.workspace_has_panes(id));
        // 最后一个可见 workspace：Swift 交给关窗路径，不发 close。
        if !was_visible || has_fallback {
            self.commands
                .push(ClientCommand::CloseWorkspace(workspace_id.clone()));
        }
        self.closed_workspaces.insert(workspace_id.clone());
        self.drop_workspace(&workspace_id);
        if was_visible {
            self.switch_to_next_live_workspace(cx);
        }
        cx.notify();
    }

    /// 把被关掉的 workspace 从任何迟到的全量同步里再拿掉一次。
    fn drop_tombstoned_workspaces(&mut self, cx: &mut Context<Self>) -> bool {
        let stale: Vec<String> = self
            .closed_workspaces
            .iter()
            .filter(|workspace_id| self.store.workspace(workspace_id).is_some())
            .cloned()
            .collect();
        let mut dirty = false;
        for workspace_id in stale {
            let visible = self.visible.as_deref() == Some(workspace_id.as_str());
            self.drop_workspace(&workspace_id);
            dirty = true;
            if visible {
                dirty |= self.switch_to_next_live_workspace(cx);
            }
        }
        dirty
    }

    /// 测试用：清掉焦点 pane（模拟没有 pane 的窗口）。
    #[doc(hidden)]
    pub fn clear_focus_for_tests(&mut self) {
        self.focused_pane = None;
    }

    /// 测试用：Core 当前 pool 里的 workspace 数量（没有 pump 时 None）。
    #[doc(hidden)]
    pub fn core_workspace_count_for_tests(&self) -> Option<usize> {
        let pump = self.pump.as_ref()?;
        pump.client().workspace_list().ok().map(|list| list.len())
    }

    // ------------------------------------------------------------------
    // Cmd-Shift-P 命令面板
    // ------------------------------------------------------------------

    /// 面板里的命令目录。tab 名字来自当前 workspace，顺序与 macOS 一致。
    pub fn palette_items(&self) -> Vec<crate::frontend::gpui::chrome::palette_view::PaletteItem> {
        if matches!(
            self.palette_menu,
            Some(palette_actions::PaletteMenu::Languages)
        ) {
            return self.language_menu_items();
        }
        let tabs: Vec<String> = self.tabs().into_iter().map(|tab| tab.name).collect();
        crate::frontend::gpui::chrome::palette_view::root_items(&tabs)
    }

    /// 过滤后的可见命令。
    pub fn palette_visible(&self) -> Vec<crate::frontend::gpui::chrome::palette_view::PaletteItem> {
        crate::frontend::gpui::chrome::palette_view::filter(
            &self.palette_items(),
            &self.palette.query(),
        )
        .into_iter()
        .cloned()
        .collect()
    }

    /// 测试用：按动作选中一条命令（标题会随语言变，动作不会）。
    pub fn palette_select_action_for_tests(
        &mut self,
        action: crate::frontend::gpui::chrome::palette_view::PaletteAction,
    ) -> bool {
        let visible = self.palette_visible();
        let Some(index) = visible.iter().position(|item| item.action == action) else {
            return false;
        };
        self.palette.selected = index;
        true
    }

    /// 命令面板是否打开。
    pub fn palette_open(&self) -> bool {
        self.palette.open
    }

    /// 命令面板里的「Muxterm Info」：打开浮层并从 Core 读构建信息。
    ///
    /// 对照 macOS `AboutWindow.swift`。信息每次打开都重读：日志开关是启动期
    /// 决定的，缓存旧值会让用户以为日志没开。
    pub fn open_info(&mut self, cx: &mut Context<Self>) {
        self.info = Some(crate::frontend::utils::corebridge::ClientBuildInfo::load());
        self.info_status = None;
        cx.notify();
    }

    pub fn close_info(&mut self, cx: &mut Context<Self>) {
        self.info_status = None;
        if self.info.take().is_some() {
            cx.notify();
        }
    }

    pub fn info(&self) -> Option<&crate::frontend::utils::corebridge::ClientBuildInfo> {
        self.info.as_ref()
    }

    /// 测试注入：无头环境也能验证浮层文案。
    pub fn set_info_for_tests(
        &mut self,
        info: crate::frontend::utils::corebridge::ClientBuildInfo,
    ) {
        self.info = Some(info);
        self.info_status = None;
    }

    /// 复制日志位置：日志开没开都要能复制（没开就复制目录）。
    pub fn copy_log_path(&mut self, cx: &mut Context<Self>) {
        let Some(info) = self.info.as_ref() else {
            return;
        };
        let (value, label) = crate::frontend::gpui::chrome::info::log_copy_target(info);
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(value));
        self.info_status = Some(format!("{label}… copied"));
        cx.notify();
    }

    pub fn copy_commit(&mut self, cx: &mut Context<Self>) {
        let Some(commit) = self.info.as_ref().map(|info| info.commit.clone()) else {
            return;
        };
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(commit));
        self.info_status = Some("Commit copied".to_string());
        cx.notify();
    }

    /// Info 浮层底部的状态文案（复制反馈）。
    pub fn info_status(&self) -> Option<&str> {
        self.info_status.as_deref()
    }

    /// 命令面板查询串（测试用）。
    pub fn palette_query_for_tests(&self) -> String {
        self.palette.query()
    }

    /// 命令面板当前选中下标（过滤后列表内）。
    pub fn palette_selected(&self) -> usize {
        self.palette.selected
    }

    pub fn open_palette(&mut self, cx: &mut Context<Self>) {
        self.palette.show();
        cx.notify();
    }

    pub fn close_palette(&mut self, cx: &mut Context<Self>) {
        self.palette.dismiss();
        cx.notify();
    }

    pub fn palette_push(&mut self, text: &str, cx: &mut Context<Self>) {
        let visible = self.palette_visible().len();
        for ch in text.chars() {
            self.palette.push_char(ch, visible);
        }
        // 过滤结果变少时选中项也要跟着夹回来。
        let visible = self.palette_visible().len();
        self.palette.move_selection(0, visible);
        cx.notify();
    }

    pub fn palette_backspace(&mut self, cx: &mut Context<Self>) {
        let visible = self.palette_visible().len();
        self.palette.backspace(visible);
        cx.notify();
    }

    pub fn move_palette_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let visible = self.palette_visible().len();
        self.palette.move_selection(delta, visible);
        cx.notify();
    }

    /// 执行选中命令。不可执行项只关面板，不编造副作用。
    pub fn run_selected_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.palette_visible().get(self.palette.selected).cloned() else {
            self.close_palette(cx);
            return;
        };
        self.run_palette_action(item.action, window, cx);
    }

    /// 面板行点击与回车共用一条执行路径。
    ///
    /// `Languages` 是唯一不关面板的动作：它把目录换成语言子菜单（Esc 回目录）。
    pub fn run_palette_action(
        &mut self,
        action: crate::frontend::gpui::chrome::palette_view::PaletteAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::frontend::gpui::chrome::palette_view::PaletteAction;
        if matches!(action, PaletteAction::Languages) {
            self.open_language_menu(cx);
            return;
        }
        self.close_palette(cx);
        match action {
            PaletteAction::KeyAction(action) => {
                self.run_action(action, window, cx);
            }
            PaletteAction::TabBarTop => self.set_tab_bar_position(
                crate::frontend::gpui::chrome::tab_bar_view::TabBarPosition::Top,
                cx,
            ),
            PaletteAction::TabBarBottom => self.set_tab_bar_position(
                crate::frontend::gpui::chrome::tab_bar_view::TabBarPosition::Bottom,
                cx,
            ),
            PaletteAction::Info => self.open_info(cx),
            PaletteAction::OpenLocal => self.open_local_sessions(cx),
            PaletteAction::OpenSsh => self.open_ssh_hosts(cx),
            PaletteAction::RenameTab => self.open_rename_tab(cx),
            PaletteAction::RenameWorkspace => self.open_rename_workspace(cx),
            PaletteAction::MoveTabLeft => self.move_active_tab(-1, cx),
            PaletteAction::MoveTabRight => self.move_active_tab(1, cx),
            PaletteAction::SetLanguage(language) => self.set_language(language, cx),
            PaletteAction::ToggleTheme => self.toggle_theme(cx),
            PaletteAction::ToggleStatusBarMode => self.toggle_status_bar_mode(cx),
            PaletteAction::Languages => unreachable!("Languages 已在上面处理"),
        }
    }

    /// tab 栏位置：改状态并写回 Core 配置（`[ui] tab_bar_position`）。
    pub fn set_tab_bar_position(
        &mut self,
        position: crate::frontend::gpui::chrome::tab_bar_view::TabBarPosition,
        cx: &mut Context<Self>,
    ) {
        self.tab_bar_position = position;
        self.commands.push(ClientCommand::ConfigApply {
            path: "ui.tab_bar_position".to_string(),
            value: serde_json::Value::String(
                match position {
                    crate::frontend::gpui::chrome::tab_bar_view::TabBarPosition::Top => "top",
                    crate::frontend::gpui::chrome::tab_bar_view::TabBarPosition::Bottom => "bottom",
                }
                .to_string(),
            ),
        });
        cx.notify();
    }

    /// Cmd +/-/0：字号缩放只改前端主题，不写配置文件（Swift 同样只改内存）。
    pub fn zoom_font(
        &mut self,
        action: crate::frontend::gpui::model::shortcuts::KeyAction,
        cx: &mut Context<Self>,
    ) {
        use crate::frontend::gpui::model::shortcuts::KeyAction;
        self.theme = match action {
            KeyAction::IncreaseFontSize => self.theme.zoomed(1),
            KeyAction::DecreaseFontSize => self.theme.zoomed(-1),
            _ => {
                let mut theme = self.theme.clone();
                theme.font_size = self.configured_font_size;
                theme
            }
        };
        self.cell = (self.theme.cell_width(), self.theme.cell_height());
        cx.notify();
    }

    /// pane 的 cols/rows。
    ///
    /// 优先取 Surface 的实时网格：Core 的拓扑快照要等下一个事件才回来，
    /// 状态栏如果只读 store，窗口一变大就一直显示打开时的 80x24
    ///（用户报的「状态栏不更新」就是这个）。GC 快照只做兜底。
    pub fn pane_grid_size(&self, pane_id: u32) -> Option<(u16, u16)> {
        let workspace = self.visible.as_deref()?;
        if let Some(surface) = self.surface(workspace, pane_id) {
            let grid = surface.grid();
            return Some((grid.cols() as u16, grid.rows() as u16));
        }
        let view = self.store.workspace(workspace)?;
        view.panes
            .values()
            .flatten()
            .find(|pane| pane.id == pane_id)
            .map(|pane| (pane.cols, pane.rows))
    }

    /// 聚焦 pane 的 cols/rows（`split_auto` 判断方向用）。
    pub fn focused_pane_grid_size(&self) -> Option<(u16, u16)> {
        self.pane_grid_size(self.focused_pane?)
    }

    /// 状态栏右侧文案：`pane <id> · <cols>x<rows>`。
    pub fn pane_geometry_label(&self) -> String {
        let pane_id = self.focused_pane.unwrap_or(0);
        let geometry = self
            .pane_grid_size(pane_id)
            .map(|(cols, rows)| format!("{cols}x{rows}"))
            .unwrap_or_else(|| String::from("-"));
        format!("pane {pane_id} · {geometry}")
    }

    /// tab 栏投影：活动指示来自 attention，数量后缀来自 pane 数。
    pub fn tab_items(&self) -> Vec<crate::frontend::gpui::chrome::tab_bar_view::TabBarItem> {
        if let Some(row) = self.aggregate_tab_items() {
            return row;
        }
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

    /// 打开目标配置窗口；`None` 表示新建。
    pub fn open_target_editor(&mut self, config: Option<&TargetConfig>, cx: &mut Context<Self>) {
        self.target_editor = Some(match config {
            Some(config) => {
                crate::frontend::gpui::model::target_editor::TargetEditor::editing(config)
            }
            None => crate::frontend::gpui::model::target_editor::TargetEditor::open(),
        });
        self.target_editor_error = None;
        cx.notify();
    }

    pub fn close_target_editor(&mut self, cx: &mut Context<Self>) {
        if self.target_editor.is_none() {
            return;
        }
        self.target_editor = None;
        self.target_editor_error = None;
        cx.notify();
    }

    /// 点某个字段让它获得焦点。
    pub fn set_target_editor_focus(
        &mut self,
        focus: crate::frontend::gpui::model::target_editor::TargetEditorFocus,
        cx: &mut Context<Self>,
    ) {
        self.target_editor_focus = focus;
        cx.notify();
    }

    pub fn target_editor(
        &self,
    ) -> Option<&crate::frontend::gpui::model::target_editor::TargetEditor> {
        self.target_editor.as_ref()
    }

    /// 改草稿；返回值的借用在下一次 `cx.notify` 前结束。
    pub fn update_target_editor(
        &mut self,
        edit: impl FnOnce(&mut crate::frontend::gpui::model::target_editor::TargetEditor),
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.target_editor.as_mut() {
            edit(editor);
            cx.notify();
        }
    }

    /// 打开用的 ClientTarget：本地 tmux 没有显式 socket 时用本进程的 `-L`。
    ///
    /// 否则用 `-L` 启动的隔离 server 上新建 workspace 会跑到默认 server，
    /// 测试也会误碰用户会话。
    pub(crate) fn client_target_with_app_socket(
        &self,
        config: &TargetConfig,
    ) -> crate::frontend::utils::corebridge::ClientTarget {
        let mut target = client_target_for(config);
        if target.socket.is_none() && target.transport == "local" && target.runtime == "tmux" {
            target.socket = self.socket.clone();
        }
        target
    }

    /// 保存：配置通过 `ClientCommand::OpenTarget` 交给 Catalog 打开。
    pub fn save_target_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.target_editor.as_ref() else {
            return;
        };
        let config = match editor.save() {
            Ok(config) => config,
            Err(error) => {
                self.target_editor_error = Some(error.message().to_string());
                cx.notify();
                return;
            }
        };
        self.open_pending = true;
        self.open_started = Some(std::time::Instant::now());
        self.commands.push(ClientCommand::OpenTarget {
            target: self.client_target_with_app_socket(&config),
            intent: crate::frontend::utils::corebridge::ClientOpenIntent::CreateIfMissing,
        });
        self.target_editor = None;
        self.target_editor_error = None;
        cx.notify();
    }

    /// Cmd-,：打开设置。manifest 与当前值从 Core 快照来（无头测试直接注入）。
    pub fn open_settings(
        &mut self,
        manifest: &serde_json::Value,
        values: &serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        self.settings_manifest =
            crate::frontend::gpui::model::settings::SettingsManifest::parse(manifest);
        self.settings_draft = crate::frontend::gpui::model::settings::SettingsDraft::new(values);
        let first = self
            .settings_manifest
            .categories()
            .first()
            .map(|category| category.id.clone());
        self.settings_draft.set_selected(first);
        // 与 Swift 一致：打开时键盘在搜索框，⭣ 才落到字段。
        self.settings_focus = SettingsFocus::Search;
        self.settings_edit = None;
        self.settings_discard_pending = false;
        self.settings_open = true;
        cx.notify();
    }

    /// 关闭设置；有未提交修改时由调用方决定是否 `settings_discard`。
    pub fn close_settings(&mut self, cx: &mut Context<Self>) {
        if !self.settings_open {
            return;
        }
        self.settings_open = false;
        self.settings_focus = SettingsFocus::Field(0);
        self.settings_edit = None;
        self.settings_discard_pending = false;
        cx.notify();
    }

    /// 侧栏可见分类（受搜索词影响）。
    pub fn settings_categories(
        &self,
    ) -> Vec<crate::frontend::gpui::model::settings::SettingsCategory> {
        self.settings_manifest
            .visible_categories(self.settings_draft.query())
    }

    /// 选中的分类。
    pub fn settings_selected(
        &self,
    ) -> Option<crate::frontend::gpui::model::settings::SettingsCategory> {
        let selected = self.settings_draft.selected()?.to_string();
        self.settings_categories()
            .into_iter()
            .find(|category| category.id == selected)
            .or_else(|| {
                self.settings_manifest
                    .categories()
                    .into_iter()
                    .find(|category| category.id == selected)
            })
    }

    /// 有未提交修改。
    pub fn settings_dirty(&self) -> bool {
        self.settings_draft.is_dirty()
    }

    /// 草稿里某个路径的当前值。
    pub fn settings_value(&self, path: &str) -> Option<serde_json::Value> {
        self.settings_draft.get(path).cloned()
    }

    /// 当前搜索词。
    pub fn settings_query(&self) -> &str {
        self.settings_draft.query()
    }

    pub fn settings_select(&mut self, id: &str, cx: &mut Context<Self>) {
        self.settings_draft.set_selected(Some(id.to_string()));
        cx.notify();
    }

    pub fn settings_set_query(&mut self, query: impl Into<String>, cx: &mut Context<Self>) {
        self.settings_draft.set_query(query);
        // 过滤后当前分类不可见时，切到第一条可见分类；草稿内容不动。
        let selected_visible = self
            .settings_draft
            .selected()
            .map(|selected| {
                self.settings_categories()
                    .iter()
                    .any(|category| category.id == selected)
            })
            .unwrap_or(false);
        if !selected_visible {
            let first = self
                .settings_categories()
                .first()
                .map(|category| category.id.clone());
            self.settings_draft.set_selected(first);
        }
        cx.notify();
    }

    /// 改一个字段（草稿态，不写 Core）。
    pub fn settings_set(&mut self, path: &str, value: serde_json::Value, cx: &mut Context<Self>) {
        self.settings_draft.set(path, value);
        cx.notify();
    }

    /// 丢弃修改，回到打开设置时的值。
    pub fn settings_discard(&mut self, cx: &mut Context<Self>) {
        self.settings_edit = None;
        self.settings_discard_pending = false;
        self.settings_draft.revert();
        cx.notify();
    }

    /// 键盘落点（搜索框 / 第几个字段）。
    pub fn settings_focus(&self) -> SettingsFocus {
        self.settings_focus
    }

    /// 正在编辑的字段与缓冲；渲染时优先显示缓冲。
    pub fn settings_edit(&self) -> Option<&SettingsEdit> {
        self.settings_edit.as_ref()
    }

    /// 有未保存修改、正在等用户确认放弃。
    pub fn settings_discard_pending(&self) -> bool {
        self.settings_discard_pending
    }

    /// 当前分类的字段列表。
    pub fn settings_fields(&self) -> Vec<crate::frontend::gpui::model::settings::SettingsField> {
        self.settings_selected()
            .map(|category| category.fields)
            .unwrap_or_default()
    }

    pub fn settings_focused_field(
        &self,
    ) -> Option<crate::frontend::gpui::model::settings::SettingsField> {
        match self.settings_focus {
            SettingsFocus::Field(index) => self.settings_fields().into_iter().nth(index),
            SettingsFocus::Search => None,
        }
    }

    /// 切换分类并把键盘落点放回字段列表。
    fn settings_step_category(&mut self, delta: i32, cx: &mut Context<Self>) {
        let categories = self.settings_categories();
        if categories.is_empty() {
            return;
        }
        let current = self
            .settings_draft
            .selected()
            .and_then(|selected| {
                categories
                    .iter()
                    .position(|category| category.id == selected)
            })
            .unwrap_or(0) as i32;
        let count = categories.len() as i32;
        let next = ((current + delta) % count + count) % count;
        let id = categories[next as usize].id.clone();
        self.settings_draft.set_selected(Some(id));
        self.settings_edit = None;
        self.settings_focus = if self.settings_fields().is_empty() {
            SettingsFocus::Search
        } else {
            SettingsFocus::Field(0)
        };
        cx.notify();
    }

    /// 在字段之间移动键盘落点。
    fn settings_step_field(&mut self, delta: i32, cx: &mut Context<Self>) {
        let fields = self.settings_fields();
        if fields.is_empty() {
            self.settings_focus = SettingsFocus::Search;
            cx.notify();
            return;
        }
        let next = match self.settings_focus {
            SettingsFocus::Search => 0,
            SettingsFocus::Field(index) => {
                let count = fields.len() as i32;
                (((index as i32 + delta) % count + count) % count) as usize
            }
        };
        self.settings_focus = SettingsFocus::Field(next.min(fields.len() - 1));
        self.settings_edit = None;
        cx.notify();
    }

    /// 点击某个字段：把键盘落点移过去。
    pub fn settings_focus_field(&mut self, index: usize, cx: &mut Context<Self>) {
        self.settings_focus = SettingsFocus::Field(index);
        self.settings_edit = None;
        cx.notify();
    }

    pub fn settings_focus_search(&mut self, cx: &mut Context<Self>) {
        self.settings_focus = SettingsFocus::Search;
        self.settings_edit = None;
        cx.notify();
    }

    /// 点控件：把落点移过去再按方向改一格（开关翻转、枚举循环）。
    pub fn settings_cycle_field(&mut self, index: usize, delta: i32, cx: &mut Context<Self>) {
        self.settings_focus_field(index, cx);
        self.settings_adjust_field(delta, cx);
    }

    /// 左右键 / 空格：开关取反、枚举循环、数字按方向加减。
    fn settings_adjust_field(&mut self, delta: i32, cx: &mut Context<Self>) {
        use crate::frontend::gpui::model::settings::ControlKind;
        let Some(field) = self.settings_focused_field() else {
            return;
        };
        let current = self.settings_draft.get(&field.path).cloned();
        let value = match &field.control {
            ControlKind::Switch => {
                let on = current.and_then(|value| value.as_bool()).unwrap_or(false);
                serde_json::Value::Bool(!on)
            }
            ControlKind::Select | ControlKind::ThemePicker => {
                let current_text = current.as_ref().and_then(serde_json::Value::as_str);
                let next = if delta < 0 {
                    // 反向循环：取「当前项的前一项」。
                    let mut target: Option<String> = None;
                    for option in &field.options {
                        target = Some(option.clone());
                        if Some(option.as_str()) == current_text {
                            break;
                        }
                    }
                    target
                } else {
                    field.next_option(current_text)
                };
                match next {
                    Some(next) => serde_json::Value::String(next),
                    None => return,
                }
            }
            ControlKind::Number => {
                let number = current
                    .as_ref()
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(0.0);
                let step = if delta < 0 { -1.0 } else { 1.0 };
                let next = (number + step).max(0.0);
                serde_json::json!(next)
            }
            _ => return,
        };
        self.settings_set(&field.path, value, cx);
    }

    /// 键盘输入：搜索框追加查询词；文本字段进去编辑缓冲。
    fn settings_type(&mut self, text: &str, cx: &mut Context<Self>) {
        match self.settings_focus {
            SettingsFocus::Search => {
                let query = format!("{}{}", self.settings_draft.query(), text);
                self.settings_set_query(query, cx);
            }
            SettingsFocus::Field(index) => {
                let Some(field) = self.settings_fields().into_iter().nth(index) else {
                    return;
                };
                if !field.control.is_text_like() {
                    return;
                }
                let mut buffer = self
                    .settings_edit
                    .as_ref()
                    .map(|edit| edit.buffer.clone())
                    .unwrap_or_else(|| {
                        crate::frontend::gpui::model::settings::display_value(
                            self.settings_value(&field.path).as_ref(),
                        )
                    });
                buffer.push_str(text);
                self.settings_edit = Some(SettingsEdit {
                    path: field.path.clone(),
                    buffer: buffer.clone(),
                });
                self.settings_commit_buffer(&field, &buffer, cx);
            }
        }
    }

    /// 退格：查询词或编辑缓冲各删一个字符。
    fn settings_backspace(&mut self, cx: &mut Context<Self>) {
        match self.settings_focus {
            SettingsFocus::Search => {
                let mut query = self.settings_draft.query().to_string();
                query.pop();
                self.settings_set_query(query, cx);
            }
            SettingsFocus::Field(index) => {
                let Some(field) = self.settings_fields().into_iter().nth(index) else {
                    return;
                };
                if !field.control.is_text_like() {
                    return;
                }
                let mut buffer = self
                    .settings_edit
                    .as_ref()
                    .map(|edit| edit.buffer.clone())
                    .unwrap_or_else(|| {
                        crate::frontend::gpui::model::settings::display_value(
                            self.settings_value(&field.path).as_ref(),
                        )
                    });
                buffer.pop();
                self.settings_edit = Some(SettingsEdit {
                    path: field.path.clone(),
                    buffer: buffer.clone(),
                });
                self.settings_commit_buffer(&field, &buffer, cx);
            }
        }
    }

    /// 解析编辑缓冲：数字解析失败就保留原值（缓冲继续留在界面上）。
    fn settings_commit_buffer(
        &mut self,
        field: &crate::frontend::gpui::model::settings::SettingsField,
        buffer: &str,
        cx: &mut Context<Self>,
    ) {
        use crate::frontend::gpui::model::settings::ControlKind;
        let value = match &field.control {
            ControlKind::Number => match buffer.trim().parse::<f64>() {
                Ok(number) => serde_json::json!(number),
                Err(_) => return,
            },
            ControlKind::Multiline => serde_json::Value::Array(
                buffer
                    .split('\n')
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(|line| serde_json::Value::String(line.to_string()))
                    .collect(),
            ),
            ControlKind::FontFallback => serde_json::Value::Array(
                buffer
                    .split(',')
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(|item| serde_json::Value::String(item.to_string()))
                    .collect(),
            ),
            _ => serde_json::Value::String(buffer.to_string()),
        };
        self.settings_set(&field.path, value, cx);
    }

    /// 结束编辑：把缓冲提交进草稿。
    fn settings_finish_edit(&mut self) {
        self.settings_edit = None;
    }

    /// 设置浮层的 Esc / 取消：有改动先确认，否则直接关。
    pub fn settings_cancel(&mut self, cx: &mut Context<Self>) {
        if self.settings_draft.is_dirty() && !self.settings_discard_pending {
            self.settings_discard_pending = true;
            cx.notify();
            return;
        }
        self.settings_confirm_discard(cx);
    }

    /// 确认放弃修改并关闭。
    pub fn settings_confirm_discard(&mut self, cx: &mut Context<Self>) {
        self.settings_discard(cx);
        self.close_settings(cx);
    }

    /// 继续编辑：退回未保存修改。
    pub fn settings_keep_editing(&mut self, cx: &mut Context<Self>) {
        self.settings_discard_pending = false;
        cx.notify();
    }

    /// Apply 并关闭；没有改动时只关闭。
    pub fn settings_apply_and_close(&mut self, cx: &mut Context<Self>) {
        self.settings_finish_edit();
        if self.settings_draft.is_dirty() {
            self.settings_apply(cx);
        }
        self.settings_discard_pending = false;
        self.close_settings(cx);
    }

    /// 设置浮层独占键盘：Esc 取消、Enter 应用、Tab 换分类、上下换字段、
    /// 左右/空格改控件；其余（包括可打印字符）都留在浮层里，绝不漏进终端。
    fn route_settings_key(&mut self, keystroke: &gpui::Keystroke, cx: &mut Context<Self>) {
        let plain = !keystroke.modifiers.platform
            && !keystroke.modifiers.control
            && !keystroke.modifiers.alt;
        if self.settings_discard_pending {
            match keystroke.key.as_str() {
                "escape" => self.settings_keep_editing(cx),
                "enter" => self.settings_confirm_discard(cx),
                _ => {}
            }
            return;
        }
        match keystroke.key.as_str() {
            "escape" => self.settings_cancel(cx),
            "enter" => self.settings_apply_and_close(cx),
            "tab" => {
                let delta = if keystroke.modifiers.shift { -1 } else { 1 };
                self.settings_step_category(delta, cx);
            }
            "up" => self.settings_step_field(-1, cx),
            "down" => self.settings_step_field(1, cx),
            "left" => self.settings_adjust_field(-1, cx),
            "right" => self.settings_adjust_field(1, cx),
            "space" if plain => {
                if self.settings_focus == SettingsFocus::Search {
                    self.settings_type(" ", cx);
                } else {
                    self.settings_adjust_field(1, cx);
                }
            }
            "backspace" => self.settings_backspace(cx),
            _ => {
                if plain {
                    if let Some(text) = keystroke
                        .key_char
                        .as_deref()
                        .filter(|text| !text.is_empty() && text.chars().all(|ch| !ch.is_control()))
                    {
                        self.settings_type(text, cx);
                    }
                }
            }
        }
    }

    /// Apply：每个改动一条 ConfigApply，再以当前值为新 baseline。
    pub fn settings_apply(&mut self, cx: &mut Context<Self>) -> usize {
        let patches = self.settings_draft.patches_for(&self.settings_manifest);
        let count = patches.len();
        for (path, value) in patches {
            self.commands
                .push(ClientCommand::ConfigApply { path, value });
        }
        self.settings_draft.rebase();
        cx.notify();
        count
    }

    /// 输入框打开时绑定的目标：当前 workspace + 聚焦 pane。
    pub fn composer_target(&self) -> Option<ComposerTarget> {
        let workspace_id = self.visible.clone()?;
        let pane_id = self.focused_pane?;
        let view = self.store.workspace(&workspace_id)?;
        let workspace = view.workspace.as_ref();
        let canonical = workspace
            .and_then(|workspace| workspace.resolved_target.as_ref())
            .as_ref()
            .and_then(|target| target.get("canonical"));
        let transport = canonical
            .and_then(|canonical| canonical.get("transport"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("local");
        let (kind, host) = if transport == "ssh" {
            let alias = canonical
                .map(crate::frontend::gpui::model::target::canonical_target)
                .unwrap_or_default();
            (ComposerTargetKind::Ssh, alias.to_string())
        } else {
            (ComposerTargetKind::Local, String::new())
        };
        Some(ComposerTarget {
            kind,
            host,
            workspace_name: workspace
                .map(|workspace| workspace.name.clone())
                .unwrap_or_default(),
            workspace_id: Some(workspace_id),
            pane_id: Some(pane_id),
        })
    }

    /// Cmd-I：打开输入框。已有草稿直接接上，不覆盖用户写了一半的内容。
    pub fn open_composer(&mut self, cx: &mut Context<Self>) {
        let target = self.composer_target();
        let page = self.composer.activate(target);
        self.composer_status = None;
        let key = page.key.clone();
        self.composer_selection = page.selection;
        self.composer_key = Some(key);
        self.composer_open = true;
        cx.notify();
    }

    /// Esc：收起窗口，草稿留着。
    pub fn close_composer(&mut self, cx: &mut Context<Self>) {
        if !self.composer_open {
            return;
        }
        self.composer_open = false;
        cx.notify();
    }

    /// 当前页快照。
    pub fn composer_page(
        &self,
    ) -> Option<&crate::frontend::gpui::model::composer::ComposerDraftPage> {
        self.composer.active_page()
    }

    /// 把文本追加到当前页末尾（IME 提交与普通按键共用）。
    pub fn composer_insert(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(page) = self.composer.active_page().cloned() else {
            return;
        };
        let mut next = page.text.clone();
        next.push_str(text);
        let selection = crate::frontend::gpui::model::composer::TextRange::new(
            crate::frontend::gpui::model::composer::utf16_len(&next),
            0,
        );
        self.composer.update(next, selection, &page.key);
        self.composer_selection = selection;
        cx.notify();
    }

    /// 退格：删掉最后一个字符（不跨越 UTF-8 边界）。
    pub fn composer_backspace(&mut self, cx: &mut Context<Self>) {
        let Some(page) = self.composer.active_page().cloned() else {
            return;
        };
        let mut next = page.text.clone();
        next.pop();
        let selection = crate::frontend::gpui::model::composer::TextRange::new(
            crate::frontend::gpui::model::composer::utf16_len(&next),
            0,
        );
        self.composer.update(next, selection, &page.key);
        self.composer_selection = selection;
        cx.notify();
    }

    /// 在光标处插入图片路径（粘贴图片时用），返回新光标。
    pub fn composer_insert_image_path(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(page) = self.composer.active_page().cloned() else {
            return;
        };
        let insertion = crate::frontend::gpui::model::composer::inserting_image_path(
            path,
            &page.text,
            self.composer_selection,
        );
        let selection = crate::frontend::gpui::model::composer::TextRange::new(insertion.caret, 0);
        self.composer.update(insertion.text, selection, &page.key);
        self.composer_selection = selection;
        cx.notify();
    }

    /// Cmd-Enter：把当前页原文写进目标 pane，再清空这一页。
    ///
    /// 空内容、没有目标都不发送，只在状态行说明原因，和 Swift 一致。
    pub fn send_composer(&mut self, cx: &mut Context<Self>) {
        let Some(page) = self.composer.active_page().cloned() else {
            return;
        };
        if crate::frontend::gpui::model::composer::ComposerDraft::new(page.text.clone())
            .payload()
            .is_none()
        {
            self.composer_status = Some("nothing to send".to_string());
            cx.notify();
            return;
        }
        let Some(target) = page.target.clone() else {
            self.composer_status = Some("no target".to_string());
            cx.notify();
            return;
        };
        let Some(pane_id) = target.pane_id else {
            self.composer_status = Some("no target".to_string());
            cx.notify();
            return;
        };
        let draft = crate::frontend::gpui::model::composer::ComposerDraft::new(page.text.clone());
        let Some(text) = draft.payload() else {
            return;
        };
        // 发送就是普通输入：和手打字走同一条 `ClientCommand::Input` 通道。
        self.commands.push(ClientCommand::Input {
            workspace_id: target.workspace_id.clone(),
            pane_id,
            data: text.as_bytes().to_vec(),
            quiet: false,
        });
        self.composer.clear(&page.key);
        self.composer_selection = crate::frontend::gpui::model::composer::TextRange::default();
        self.composer_status = Some("sent".to_string());
        cx.notify();
    }

    /// tab 栏按 visible 下标切页。
    pub fn composer_select_visible(&mut self, index: usize, cx: &mut Context<Self>) {
        if !self.composer.select_visible(index) {
            return;
        }
        if let Some(page) = self.composer.active_page() {
            self.composer_selection = page.selection;
            self.composer_key = Some(page.key.clone());
        }
        self.composer_status = None;
        cx.notify();
    }

    /// 输入框 tab 栏要画的页。
    pub fn composer_visible_pages(
        &self,
    ) -> Vec<crate::frontend::gpui::model::composer::ComposerDraftPage> {
        self.composer.visible_pages().into_iter().cloned().collect()
    }

    /// 指定 workspace 里聚焦 pane 的画面（无头验收用）。
    pub fn pane_screen_of(&self, workspace_id: &str) -> Option<String> {
        let view = self.store.workspace(workspace_id)?;
        let pane = view
            .active_tab_id()
            .map(|tab| view.panes_for_tab(tab))
            .and_then(|panes| {
                panes
                    .iter()
                    .find(|pane| pane.is_active)
                    .or_else(|| panes.first())
            })
            .map(|pane| pane.id)?;
        let surface = self.surfaces.get(&(workspace_id.to_string(), pane))?;
        Some(surface.grid().screen_lines().join("\n"))
    }

    /// 某个 pane 当前可见画面（无头验收用；不含滚动区）。
    pub fn pane_screen(&self, pane_id: u32) -> Option<String> {
        let workspace = self.visible.as_deref()?;
        let surface = self.surfaces.get(&(workspace.to_string(), pane_id))?;
        Some(surface.grid().screen_lines().join("\n"))
    }

    /// 面板是否打开（Esc 关闭后按键回到终端）。
    pub fn panel_open(&self) -> bool {
        self.panel.open
    }

    /// 面板当前 tab。
    pub fn panel_tab(&self) -> crate::frontend::gpui::model::panel::PanelTab {
        self.panel.model.tab
    }

    /// 面板当前行。
    pub fn panel_rows(&self) -> Vec<crate::frontend::gpui::chrome::panel_view::PanelRow> {
        self.projected_panel_rows()
    }

    /// 面板查询串。
    pub fn panel_query(&self) -> String {
        self.panel
            .actions
            .as_ref()
            .map(|menu| menu.query.clone())
            .unwrap_or_else(|| self.panel.model.query())
    }

    /// 面板当前选中的行（渲染高亮用）。
    pub fn panel_selected(&self) -> usize {
        self.panel.selected
    }

    /// 上下移动选中行。
    pub fn move_panel_selection(&mut self, delta: i32, cx: &mut Context<Self>) {
        if let Some(menu) = self.panel.actions.as_mut() {
            let len = menu.rows().len();
            menu.selected =
                (menu.selected as i32 + delta).clamp(0, len.saturating_sub(1) as i32) as usize;
        } else {
            let len = self.panel_rows().len();
            self.panel.move_selection(delta, len);
            self.panel_scroll.scroll_to_item(self.panel.selected);
        }
        cx.notify();
    }

    /// 面板查询串编辑。
    pub fn panel_push_query(&mut self, text: &str, cx: &mut Context<Self>) {
        self.edit_panel_query(Some(text), cx);
    }

    pub fn panel_backspace(&mut self, cx: &mut Context<Self>) {
        self.edit_panel_query(None, cx);
    }

    /// Enter：已打开的 workspace 直接切过去；现有 runtime 交给 Catalog attach。
    pub fn activate_panel_row(&mut self, cx: &mut Context<Self>) {
        if self.panel.actions.is_some() {
            self.run_panel_action(cx);
            return;
        }
        if let Some(row) = self.panel_rows().get(self.panel.selected).cloned() {
            self.activate_workspace_row(row, cx);
        }
    }

    /// 拉一次现有 runtime（Core discovery）。
    ///
    /// 面板打开时按需跑：这是跨进程查询，不能放在渲染路径上。
    pub fn refresh_existing(&mut self, cx: &mut Context<Self>) {
        self.refresh_usage(cx);
        self.discover_picker_root(cx);
    }

    /// 读 Core 的 attach 频率（面板排序）。同步调用：Core 只回内存快照。
    pub fn refresh_usage(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.pump.as_ref().map(|pump| pump.client()) else {
            return;
        };
        match client.workspace_usage() {
            Ok(entries) => {
                self.panel.usage =
                    crate::frontend::gpui::model::usage::UsageEntry::collect(&entries);
                cx.notify();
            }
            Err(error) => {
                tracing::warn!(target = "muxterm::gpui", "读 attach 频率失败: {error}");
            }
        }
    }

    /// 无头测试注入 Core 的 attach 频率。
    #[doc(hidden)]
    pub fn set_usage_for_tests(
        &mut self,
        entries: Vec<crate::frontend::gpui::model::usage::UsageEntry>,
    ) {
        self.panel.usage = entries;
    }

    /// 无头测试注入 discovery 结果。
    #[doc(hidden)]
    pub fn set_existing_for_tests(
        &mut self,
        existing: Vec<crate::frontend::gpui::model::existing::ExistingRuntime>,
    ) {
        self.existing = existing;
        self.existing_loaded = true;
    }

    /// discovery 是否已经跑完（面板显示空态用）。
    pub fn existing_loaded(&self) -> bool {
        self.existing_loaded
    }

    /// 打开面板并切到指定 tab；已打开时只切内容，不做 toggle。
    pub fn show_panel(
        &mut self,
        tab: crate::frontend::gpui::model::panel::PanelTab,
        cx: &mut Context<Self>,
    ) {
        let opening = !self.panel.open;
        self.panel.show(tab);
        if opening {
            self.panel
                .picker
                .navigate(crate::frontend::gpui::model::workspace_picker::PickerStage::Root);
            self.panel.model.input.clear();
            self.panel.actions = None;
        }
        if tab == crate::frontend::gpui::model::panel::PanelTab::Workspaces {
            self.refresh_existing(cx);
        }
        if tab == crate::frontend::gpui::model::panel::PanelTab::Search {
            self.refresh_search();
        }
        self.reset_panel_selection();
        cx.notify();
    }

    /// 跑一次 Core 索引搜索并更新快照。
    ///
    /// Core 的 `search_all` 是同步 FFI（本地索引，无网络），所以这里直接调，
    /// 不需要像 Swift 那样排队 + requestID 去丢过期结果。空查询清空快照，
    /// 避免 Search tab 一打开就列出整个库。
    pub fn refresh_search(&mut self) -> bool {
        use crate::frontend::gpui::model::search::{SearchHit, SearchSnapshot};
        let query = self.panel.model.query();
        if query.trim().is_empty() {
            // 空查询不去打扰 Core，也不清掉上一轮的结果：打开 Search tab 时
            // 先看见上次搜到的东西，比一片空白更有用。
            return false;
        }
        // 没有 Core（无头测试 / 泵还没起来）时保留已有快照：注入的命中不该
        // 因为一次按键就被清空，实机上也只是暂时沿用上一次结果。
        let Some(client) = self.pump.as_ref().map(|pump| pump.client()) else {
            return false;
        };
        let hits = client.search_all(query.trim()).ok();
        let snapshot = SearchSnapshot {
            hits: hits
                .unwrap_or_default()
                .into_iter()
                .map(|hit| SearchHit {
                    workspace_id: hit.workspace_id,
                    tab_id: hit.tab_id,
                    pane_id: hit.pane_id,
                    seq: hit.seq,
                    line: hit.line,
                })
                .collect(),
        };
        let changed = self.search_snapshot.as_ref() != Some(&snapshot);
        self.search_snapshot = Some(snapshot);
        changed
    }

    /// 关闭面板。
    pub fn close_panel(&mut self, cx: &mut Context<Self>) {
        if !self.panel.open {
            return;
        }
        self.panel.close();
        self.panel.actions = None;
        self.panel
            .picker
            .navigate(crate::frontend::gpui::model::workspace_picker::PickerStage::Root);
        cx.notify();
    }

    /// Tab / Shift+Tab 循环面板 tab。
    pub fn cycle_panel_tab(&mut self, back: bool, cx: &mut Context<Self>) {
        self.panel.cycle(back);
        cx.notify();
    }

    /// 无头测试 / 未来 FFI 快照：注入搜索命中。
    #[doc(hidden)]
    pub fn set_search_snapshot(
        &mut self,
        snapshot: Option<crate::frontend::gpui::model::search::SearchSnapshot>,
    ) {
        self.search_snapshot = snapshot;
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
        // IME 提交要落到**当前真正吃文字的地方**：面板 / 命令面板 / 设置 / 改名。
        // 以前只判断 `panel.open`，于是命令面板与设置里提交的中文被当成终端输入。
        if self.panel.open {
            self.panel_push_query(text, cx);
            return;
        }
        if self.palette.open {
            self.palette_push(text, cx);
            return;
        }
        if self.settings_open {
            self.settings_type(text, cx);
            return;
        }
        if self.prompt.is_some() {
            self.prompt_push(text, cx);
            return;
        }
        if self.composer_open {
            self.composer_insert(text, cx);
            return;
        }
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
        self.note_input_sent();
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
        self.note_input_sent();
        cx.notify();
    }

    /// 更新 IME 组合文本（空字符串表示组合结束）。
    /// 输入法提交后的收尾：组合态必须清空，否则下一次按键会被当成继续组合。
    fn finish_ime_commit(&mut self, cx: &mut Context<Self>) {
        if !self.marked_text.is_empty() {
            self.marked_text.clear();
            cx.notify();
        }
    }

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
        let (cell_width, cell_height) = self.cell;
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

    /// 窗口级字符网格上报（tmux `refresh-client -C`）。
    ///
    /// 只报 pane 尺寸不够：控制 runtime 会一直等 `ResizeClient` 才发出
    /// attach 首屏，于是窗口里有 pane 却永远空白（Core 会跳过 80x24 的
    /// 占位尺寸）。
    pub fn apply_client_size(&mut self, cols: u16, rows: u16) {
        let Some(workspace) = self.visible.clone() else {
            return;
        };
        let cols = cols.max(2);
        let rows = rows.max(1);
        if self.last_client_size.as_ref() == Some(&(workspace.clone(), cols, rows)) {
            return;
        }
        self.last_client_size = Some((workspace.clone(), cols, rows));
        self.commands.push(ClientCommand::Resize {
            workspace_id: Some(workspace),
            pane_id: None,
            cols,
            rows,
        });
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
        let (_, cell_height) = self.cell;
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
                        // 选区是视口坐标：滚走以后高亮会盖到别的内容上。
                        self.selection = None;
                        self.dragging_selection = false;
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
            self.note_input_sent();
            cx.notify();
        }
    }

    pub fn select_workspace(&mut self, workspace: String, cx: &mut Context<Self>) {
        // 固定聚合槽不是 Core workspace：点它只能切到聚合投影。
        if let Some(kind) = aggregate_kind_for_identity(&workspace) {
            self.select_aggregate(kind, cx);
            return;
        }
        self.aggregate = None;
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

    /// 含这个 pane 的 tab id（对照 Swift `cachedTabID(containingPane:)`）。
    pub fn tab_containing_pane(&self, workspace_id: &str, pane_id: u32) -> Option<u32> {
        let view = self.store.workspace(workspace_id)?;
        view.panes.iter().find_map(|(tab_id, panes)| {
            panes
                .iter()
                .any(|pane| pane.id == pane_id)
                .then_some(*tab_id)
        })
    }

    /// 跳到某个 pane 的**完整身份**：workspace → tab → pane。
    ///
    /// 对照 Swift `MainWindow.jumpToPane(tabId:paneId:...)`：
    /// 1. 切 workspace；2. 按 pane 反查 tab 并 `SwitchTab`；3. 显式 `SwitchPane`
    ///    （即使焦点已经在这个 pane 上也要发：刚从别的 tab 过来时 Core 的活动
    ///    pane 可能还是旧 tab 的）。
    pub fn reveal_pane(&mut self, workspace_id: &str, pane_id: u32, cx: &mut Context<Self>) {
        self.select_workspace(workspace_id.to_string(), cx);
        if let Some(tab_id) = self.tab_containing_pane(workspace_id, pane_id) {
            if self.active_tab_id() != Some(tab_id) {
                self.select_tab(tab_id, cx);
            }
        }
        self.focused_pane = Some(pane_id);
        self.pending_focus = Some(pane_id);
        self.commands.push(ClientCommand::Task {
            workspace_id: Some(workspace_id.to_string()),
            task: crate::frontend::utils::corebridge::ClientTask::SwitchPane { pane_id },
        });
        cx.notify();
    }

    pub fn select_tab(&mut self, tab_id: u32, cx: &mut Context<Self>) {
        if self.select_aggregate_tab(tab_id, cx) {
            return;
        }
        self.commands.push(ClientCommand::Task {
            workspace_id: self.visible.clone(),
            task: crate::frontend::utils::corebridge::ClientTask::SwitchTab { tab_id },
        });
        cx.notify();
    }

    /// 关当前 workspace 的某个 tab（tab 栏 ✕ 与命令面板共用）。
    pub fn close_tab(&mut self, tab_id: u32, cx: &mut Context<Self>) {
        self.commands.push(ClientCommand::Task {
            workspace_id: self.visible.clone(),
            task: crate::frontend::utils::corebridge::ClientTask::CloseTab { tab_id },
        });
        cx.notify();
    }

    /// `Cmd-[` / `Cmd-]`：在当前 tab 的布局叶子之间循环。
    ///
    /// 对照 Swift `MainWindow.movePane(offset:)`：
    /// - 导航范围只取当前 tab（布局叶子优先，zoom 时回到快照顺序）；
    /// - 结果是显式 `SwitchPane(pane)`，不是让 Core 自己猜 `active_pane`；
    /// - zoom 中切 pane 会先退出 zoom，再对新 pane 重新全屏。
    pub fn move_pane_focus(&mut self, offset: i32, cx: &mut Context<Self>) -> bool {
        use crate::frontend::gpui::model::pane_navigation;
        use crate::frontend::utils::corebridge::ClientTask;
        let Some(workspace) = self.visible.clone() else {
            return false;
        };
        let Some(view) = self.store.workspace(&workspace) else {
            return false;
        };
        let Some(tab_id) = view.active_tab_id() else {
            return false;
        };
        let panes = view.panes_for_tab(tab_id);
        let pane_ids: Vec<u32> = panes.iter().map(|pane| pane.id).collect();
        let active = self
            .focused_pane
            .filter(|id| pane_ids.contains(id))
            .or_else(|| panes.iter().find(|pane| pane.is_active).map(|pane| pane.id));
        let Some(active) = active else {
            return false;
        };
        let layout_leaves = view
            .layouts
            .get(&tab_id)
            .map(pane_navigation::layout_leaf_ids);
        let navigation = pane_navigation::navigation_pane_ids(layout_leaves.as_deref(), &pane_ids);
        let Some(target) = pane_navigation::target(&navigation, active, offset) else {
            return false;
        };
        if target == active {
            return false;
        }
        let was_zoomed = layout_leaves
            .as_deref()
            .and_then(|leaves| pane_navigation::zoomed_pane_id(leaves, &pane_ids))
            .is_some();
        self.commands.push(ClientCommand::Task {
            workspace_id: Some(workspace.clone()),
            task: ClientTask::SwitchPane { pane_id: target },
        });
        if was_zoomed {
            // tmux 选别的 pane 会自动退出 zoom；重新对目标 pane 全屏。
            self.commands.push(ClientCommand::Task {
                workspace_id: Some(workspace),
                task: ClientTask::TogglePaneFullscreen { pane_id: target },
            });
        }
        cx.notify();
        true
    }

    pub fn focus_pane(&mut self, pane_id: u32, cx: &mut Context<Self>) {
        if self.focused_pane == Some(pane_id) {
            return;
        }
        // 选区只属于一个 pane。
        self.selection = None;
        self.dragging_selection = false;
        self.focused_pane = Some(pane_id);
        self.pending_focus = Some(pane_id);
        self.commands.push(ClientCommand::Task {
            workspace_id: self.visible.clone(),
            task: crate::frontend::utils::corebridge::ClientTask::SwitchPane { pane_id },
        });
        cx.notify();
    }

    /// 键 → 终端字节。Cmd 组合留给 chrome。
    pub fn handle_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        let chord = crate::frontend::gpui::model::shortcuts::KeyChord {
            command: keystroke.modifiers.platform,
            shift: keystroke.modifiers.shift,
            option: keystroke.modifiers.alt,
            control: keystroke.modifiers.control,
            key: keystroke.key.clone(),
        };
        // 目标配置窗口打开时独占按键：Esc 取消、Tab 换字段、Enter 保存，
        // 无修饰键的字符写进当前字段。
        if self.target_editor.is_some() && !keystroke.modifiers.platform {
            match keystroke.key.as_str() {
                "escape" => {
                    self.close_target_editor(cx);
                    return;
                }
                "enter" => {
                    self.save_target_editor(cx);
                    return;
                }
                "tab" => {
                    let ssh_visible = self
                        .target_editor
                        .as_ref()
                        .is_some_and(|editor| editor.ssh_field_visible());
                    self.target_editor_focus = if keystroke.modifiers.shift {
                        self.target_editor_focus.previous(ssh_visible)
                    } else {
                        self.target_editor_focus.next(ssh_visible)
                    };
                    cx.notify();
                    return;
                }
                "backspace" => {
                    let focus = self.target_editor_focus;
                    self.update_target_editor(|editor| editor.backspace(focus), cx);
                    return;
                }
                _ => {}
            }
            if let Some(text) = keystroke.key_char.as_deref() {
                if !text.is_empty() && text.chars().all(|ch| !ch.is_control()) {
                    let focus = self.target_editor_focus;
                    self.update_target_editor(|editor| editor.append(focus, text), cx);
                    return;
                }
            }
        }

        // 重命名浮层独占按键：Esc 取消、回车提交、退格删字，
        // 其余可打印字符进输入框（与其他浮层同样的规则，绝不漏进终端）。
        if self.prompt.is_some() {
            if keystroke.modifiers.platform {
                if keystroke.key == "v" {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| {
                        item.entries().iter().find_map(|entry| match entry {
                            gpui::ClipboardEntry::String(text) => Some(text.text().to_string()),
                            gpui::ClipboardEntry::Image(_) => None,
                        })
                    }) {
                        self.prompt_push(&text, cx);
                    }
                }
                cx.stop_propagation();
                return;
            }
            match keystroke.key.as_str() {
                "escape" => self.cancel_prompt(cx),
                "enter" => self.submit_prompt(cx),
                "backspace" => self.prompt_backspace(cx),
                _ => {
                    if let Some(text) = keystroke.key_char.as_deref() {
                        if !text.is_empty() && text.chars().all(|ch| !ch.is_control()) {
                            self.prompt_push(text, cx);
                        }
                    }
                }
            }
            cx.stop_propagation();
            return;
        }

        // pane `⋯` 菜单打开时独占单击：上下选择、回车执行、Esc 关闭。
        // 其它键先关菜单再继续 —— 菜单不是模态窗口，用户可以直接接着敲终端。
        if self.pane_menu.is_some() {
            match keystroke.key.as_str() {
                "escape" => {
                    self.close_pane_menu(cx);
                    cx.stop_propagation();
                    return;
                }
                "enter" => {
                    let action = self
                        .pane_menu
                        .as_ref()
                        .and_then(|menu| menu.selected_action());
                    let pane_id = self.pane_menu.as_ref().map(|menu| menu.pane_id);
                    if let (Some(action), Some(pane_id)) = (action, pane_id) {
                        self.run_pane_title_action(pane_id, action, window, cx);
                    }
                    cx.stop_propagation();
                    return;
                }
                "up" => {
                    self.move_pane_menu(-1, cx);
                    cx.stop_propagation();
                    return;
                }
                "down" => {
                    self.move_pane_menu(1, cx);
                    cx.stop_propagation();
                    return;
                }
                _ => self.close_pane_menu(cx),
            }
        }

        // Muxterm Info 浮层：Esc 关闭，其余无修饰键不落到终端。
        if self.info.is_some() && !keystroke.modifiers.platform {
            if keystroke.key == "escape" {
                self.close_info(cx);
            }
            cx.stop_propagation();
            return;
        }

        // 设置浮层独占按键：Esc 取消、Enter 应用、Tab 换分类、上下换字段。
        // 之前这里没有分支，Esc 和所有可打印键都会漏到终端 —— 设置关不掉。
        if self.settings_open {
            if keystroke.modifiers.platform {
                // Cmd-, / Cmd-Q 等继续走动作表；其余 Cmd 组合留在浮层里。
                if let Some(action) =
                    crate::frontend::gpui::model::shortcuts::builtin_action(&chord)
                {
                    if self.run_action(action, window, cx) {
                        cx.stop_propagation();
                        return;
                    }
                }
                cx.stop_propagation();
                return;
            }
            self.route_settings_key(keystroke, cx);
            cx.stop_propagation();
            return;
        }

        // 命令面板打开时独占按键：Esc 关闭、上下键移动、回车执行，
        // 其余可打印字符进查询串（与 macOS `NSSearchField` 行为一致）。
        if self.palette.open {
            let plain = !keystroke.modifiers.platform && !keystroke.modifiers.control;
            let control_only = keystroke.modifiers.control
                && !keystroke.modifiers.platform
                && !keystroke.modifiers.alt;
            match keystroke.key.as_str() {
                "escape" => {
                    // 二级列表先退回命令目录，再按一次才关面板。
                    if !self.palette_menu_back(cx) {
                        self.close_palette(cx);
                    }
                    cx.stop_propagation();
                    return;
                }
                "enter" => {
                    self.run_selected_palette(window, cx);
                    return;
                }
                "up" => {
                    self.move_palette_selection(-1, cx);
                    return;
                }
                "down" => {
                    self.move_palette_selection(1, cx);
                    return;
                }
                // `Alt`/`Ctrl`+Backspace 按词删；放在裸 `backspace` 之前，
                // 否则会被它先吃掉（unreachable pattern）。
                "backspace" if keystroke.modifiers.alt || keystroke.modifiers.control => {
                    self.palette.input.delete_word_backward();
                    self.palette.selected = 0;
                    cx.notify();
                    return;
                }
                "backspace" => {
                    self.palette_backspace(cx);
                    return;
                }
                // 查询框是真正的单行编辑器：光标、选区、行首行尾、向前删除。
                "left" | "right" | "home" | "end" | "delete" => {
                    let input = &mut self.palette.input;
                    match keystroke.key.as_str() {
                        "delete" => input.delete_forward(),
                        "home" => input.move_to(false, keystroke.modifiers.shift),
                        "end" => input.move_to(true, keystroke.modifiers.shift),
                        "left" if keystroke.modifiers.alt => {
                            input.move_word(false, keystroke.modifiers.shift)
                        }
                        "right" if keystroke.modifiers.alt => {
                            input.move_word(true, keystroke.modifiers.shift)
                        }
                        "left" => input.move_cursor(-1, keystroke.modifiers.shift),
                        _ => input.move_cursor(1, keystroke.modifiers.shift),
                    }
                    cx.notify();
                    return;
                }
                "a" if keystroke.modifiers.platform => {
                    self.palette.input.select_all();
                    cx.notify();
                    return;
                }
                "c" | "x" if keystroke.modifiers.platform => {
                    if let Some(text) = self.palette.input.selected_text() {
                        if keystroke.key == "x" {
                            self.palette.input.backspace();
                        }
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                        cx.notify();
                    }
                    return;
                }
                "v" if keystroke.modifiers.platform => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| {
                        item.entries().iter().find_map(|entry| match entry {
                            gpui::ClipboardEntry::String(text) => Some(text.text().to_string()),
                            gpui::ClipboardEntry::Image(_) => None,
                        })
                    }) {
                        self.palette_push(&text, cx);
                    }
                    return;
                }
                "p" | "n" if control_only => {
                    // Emacs 风格：Ctrl-P / Ctrl-N 与上下键同一条选择路径。
                    self.move_palette_selection(if keystroke.key == "n" { 1 } else { -1 }, cx);
                    return;
                }
                _ => {}
            }
            if plain {
                if let Some(text) = keystroke.key_char.as_deref() {
                    if !text.is_empty() && text.chars().all(|ch| !ch.is_control()) {
                        self.palette_push(text, cx);
                        // 消费掉：否则同一个按键还会经 IME 的 `replace_text_in_range`
                        // 再提交一遍（查询串变成 "aabbcc"）。
                        cx.stop_propagation();
                        return;
                    }
                }
            }
            // 面板打开时 Cmd 组合继续走全局动作，其余键不再漏进终端。
            if !keystroke.modifiers.platform {
                return;
            }
        }

        if self.panel.open {
            self.route_panel_key(keystroke, cx);
            cx.stop_propagation();
            return;
        }

        // 输入框打开时先吃掉自己的键：Cmd-Enter 发送、Esc 收起、Tab 切页，
        // 无修饰键的字符写进草稿。其它 Cmd 组合仍然按全局动作执行。
        if self.composer_open {
            let plain = !keystroke.modifiers.platform && !keystroke.modifiers.control;
            if keystroke.modifiers.platform && keystroke.key == "enter" {
                self.send_composer(cx);
                return;
            }
            if plain {
                match keystroke.key.as_str() {
                    "escape" => {
                        self.close_composer(cx);
                        return;
                    }
                    "tab" => {
                        let visible = self.composer.visible_pages().len();
                        if visible > 1 {
                            let current = self.composer.active_visible_index().unwrap_or(0);
                            let next = if keystroke.modifiers.shift {
                                (current + visible - 1) % visible
                            } else {
                                (current + 1) % visible
                            };
                            self.composer_select_visible(next, cx);
                        }
                        return;
                    }
                    "backspace" => {
                        self.composer_backspace(cx);
                        return;
                    }
                    "enter" => {
                        self.composer_insert("\n", cx);
                        return;
                    }
                    _ => {}
                }
                if let Some(text) = keystroke.key_char.as_deref() {
                    if !text.is_empty() && text.chars().all(|ch| !ch.is_control()) {
                        self.composer_insert(text, cx);
                        return;
                    }
                }
            }
        }

        if let Some(action) = crate::frontend::gpui::model::shortcuts::builtin_action(&chord) {
            if self.run_action(action, window, cx) {
                // **必须消费事件**：macOS 会先拿同一条 chord 问视图（performKeyEquivalent），
                // 再拿它去匹配原生菜单。不消费就等于按一次执行两遍——Cmd-D 分两个窗格、
                // Cmd-, 打开设置后又马上被菜单那条 `OpenSettings` 当成取消关掉。
                self.key_consumed = true;
                cx.stop_propagation();
                cx.notify();
                return;
            }
        }
        if keystroke.modifiers.platform {
            // Cmd-V：图片优先（与 Swift `MuxTerminalView.paste(from:)` 一致）。
            // 以前这里只读文本，剪贴板里是截图时这次粘贴什么都不做 —— 用户看到
            // 的就是「复制图片粘贴没反应」，而 Core 侧一条日志都没有。
            if keystroke.key == "v" && !self.paste_clipboard_image(cx) {
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
            // 同上：带 Command 的按键绝不能同时交给原生菜单再执行一遍。
            self.key_consumed = true;
            cx.stop_propagation();
            cx.notify();
            return;
        }

        // 面板打开时独占按键：面板有查询串与选中行，任何键都不该漏进终端。
        if self.panel.open {
            match keystroke.key.as_str() {
                "escape" => {
                    self.close_panel(cx);
                    cx.notify();
                    return;
                }
                "tab" => {
                    self.cycle_panel_tab(keystroke.modifiers.shift, cx);
                    cx.notify();
                    return;
                }
                "enter" => {
                    self.activate_panel_row(cx);
                    cx.notify();
                    return;
                }
                "up" => {
                    self.move_panel_selection(-1, cx);
                    return;
                }
                "down" => {
                    self.move_panel_selection(1, cx);
                    return;
                }
                "backspace" => {
                    self.panel_backspace(cx);
                    return;
                }
                _ => {}
            }
            // 可打印字符进查询串（Ctrl/Alt 组合留给快捷键，不进文本）。
            if !keystroke.modifiers.control && !keystroke.modifiers.alt {
                let text = keystroke
                    .key_char
                    .clone()
                    .filter(|text| !text.is_empty())
                    .or_else(|| {
                        (keystroke.key.chars().count() == 1
                            && !keystroke.modifiers.platform
                            && keystroke.key != "space")
                            .then(|| keystroke.key.clone())
                    });
                if let Some(text) = text {
                    self.panel_push_query(&text, cx);
                }
            }
            cx.stop_propagation();
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
        // 无修饰键的可打印文本先让给平台输入法：中文/日文组词必须先看到
        // 首字母 `n`、`i`。文本会经 `insertText` → `replace_text_in_range`
        // → `commit_text` 回到这里；直接编码 + 消费会让 macOS 的
        // `inputContext` 收不到这次 keyDown，输入法永远起不来。
        // 对照 macOS `MuxTerminalView.keyDown` 的 `interpretKeyEvents`。
        if crate::frontend::gpui::terminal::keymap::defers_to_ime(&press) {
            self.ime_deferred = true;
            return;
        }
        // IME 占位符按键：一个字节都不发，也不交给平台（否则远端会显示 <ffff>）。
        use crate::frontend::gpui::model::ime::{key_dispatch, KeyDispatch};
        if press
            .key_char
            .as_deref()
            .is_some_and(|text| key_dispatch(Some(text)) == KeyDispatch::Ignore)
        {
            cx.stop_propagation();
            return;
        }
        if let Some(bytes) = encode(&press, app_cursor) {
            let forwarded = !bytes.is_empty();
            self.commands.push(ClientCommand::Input {
                workspace_id: Some(workspace),
                pane_id,
                data: bytes,
                quiet: false,
            });
            self.note_input_sent();
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
        // 先提交再清组合态：`commit_text` 里要判断当前有没有组合文本。
        self.commit_text(text, cx);
        self.finish_ime_commit(cx);
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

/// 固定聚合槽的身份 → 聚合投影。真实 workspace id 返回 `None`。
fn aggregate_kind_for_identity(
    identity: &str,
) -> Option<crate::frontend::gpui::chrome::panel_view::AggregateKind> {
    use crate::frontend::gpui::chrome::panel_view::AggregateKind;
    use crate::frontend::gpui::model::aggregate::{AGENTS_IDENTITY, SHELLS_IDENTITY};
    match identity {
        SHELLS_IDENTITY => Some(AggregateKind::Shells),
        AGENTS_IDENTITY => Some(AggregateKind::Agents),
        _ => None,
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
            .map(crate::frontend::gpui::model::target::canonical_target)
            .filter(|alias| !alias.is_empty())
            .unwrap_or("ssh");
        return TargetTransport::Ssh {
            name: alias.to_string(),
        }
        .label()
        .to_string();
    }
    TargetTransport::Local.label().to_string()
}

/// 把编辑器里的目标配置翻译成 Core 的 target DTO。
fn client_target_for(config: &TargetConfig) -> crate::frontend::utils::corebridge::ClientTarget {
    use crate::frontend::gpui::model::target::TargetTransport;
    use crate::frontend::utils::corebridge::ClientTarget;
    let (transport, target) = match &config.transport {
        TargetTransport::Local => ("local".to_string(), None),
        TargetTransport::Ssh { name } => ("ssh".to_string(), Some(name.clone())),
    };
    ClientTarget {
        name: config.name.clone(),
        runtime: config.runtime.label().to_lowercase(),
        transport,
        target,
        path: config.path.clone(),
        session: config.session.clone(),
        socket: config.socket.clone(),
        workspace_id: config.workspace_id.clone(),
    }
}

/// Core 配置快照 → `[ui]` 段。
pub(crate) fn ui_config_from_snapshot(
    snapshot: &crate::frontend::utils::corebridge::ClientConfigSnapshot,
) -> crate::frontend::utils::corebridge::ClientUiConfig {
    serde_json::from_value::<crate::frontend::utils::corebridge::ClientConfig>(
        snapshot.values.clone(),
    )
    .map(|config| config.ui)
    .unwrap_or_default()
}

/// 窗口标题文字。
///
/// `[ui] show_title_bar = false` 时返回空串：macOS 上就是「没有窗口 title」，
/// 只剩 traffic lights —— 与 Swift 版默认观感一致。打开时按
/// `workspace — Muxterm` 显示，方便在 Mission Control 里辨认窗口。
pub fn window_title_text(show_title_bar: bool, workspace: &str) -> String {
    if !show_title_bar {
        return String::new();
    }
    let workspace = workspace.trim();
    if workspace.is_empty() {
        "Muxterm".to_string()
    } else {
        format!("{workspace} — Muxterm")
    }
}

/// Core 配置快照 → 主题。
///
/// `values` 解出 `[font] family/size`；`resolved_theme` 是 Core 解析好的终端
/// 调色板（含 `theme.name = "system"` 的明暗解析）。两者缺一就退回浅色默认。
pub(crate) fn theme_from_snapshot(
    snapshot: &crate::frontend::utils::corebridge::ClientConfigSnapshot,
) -> Theme {
    let font = serde_json::from_value::<crate::frontend::utils::corebridge::ClientConfig>(
        snapshot.values.clone(),
    )
    .map(|config| config.font)
    .unwrap_or_else(|_| crate::frontend::utils::corebridge::ClientFontConfig {
        family: crate::frontend::gpui::theme::DEFAULT_FONT_FAMILY.to_string(),
        size: crate::frontend::gpui::theme::DEFAULT_FONT_SIZE,
        fallback: Vec::new(),
    });
    Theme::from_config(snapshot.resolved_theme.as_ref(), &font)
}

/// 从 `TextSystem` 量出真实的等宽格子尺寸。
///
/// 宽 = 字符 advance × 字距；高 = (ascent + descent) × 行距。字体不存在时
/// gpui 会退回 fallback 字体栈，量出来的仍然是它实际绘制的那一份。
/// 空闲时把轮询间隔按倍数退避，上限 [`IDLE_PUMP_INTERVAL`]。
///
/// 有事件就回到 [`PUMP_INTERVAL`]：打字 / 输出时不牺牲延迟；空闲时不空转
/// （每次轮询都是一次 FFI + JSON 解析，125Hz 空转会明显吃 CPU）。
/// 输入之后的快轮询窗口：按键回显必须立刻看得见。
pub const INPUT_URGENT_WINDOW: Duration = Duration::from_millis(400);

/// 这一拍该等多久。
///
/// 有变化 → `PUMP_INTERVAL`；刚有输入 → 也保持 `PUMP_INTERVAL`（否则空闲退避
/// 到 64ms，敲一个字要等两三帧才看到回显）；都没有 → 指数退避。
pub fn next_pump_delay(current: Duration, dirty: bool, input_pending: bool) -> Duration {
    if dirty || input_pending {
        return PUMP_INTERVAL;
    }
    (current * 2).min(IDLE_PUMP_INTERVAL)
}

pub fn pump_delay(current: Duration, dirty: bool) -> Duration {
    if dirty {
        return PUMP_INTERVAL;
    }
    (current * 2).min(IDLE_PUMP_INTERVAL)
}

fn measure_cell_size(theme: &Theme, text_system: &gpui::WindowTextSystem) -> (f32, f32) {
    use gpui::px;
    let font = gpui::font(theme.font_family.clone());
    let font_id = text_system.resolve_font(&font);
    let size = px(theme.font_size);
    let width = text_system
        .advance(font_id, size, 'M')
        .map(|advance| f32::from(advance.width))
        .unwrap_or_else(|_| theme.cell_width() / theme.column_spacing)
        * theme.column_spacing;
    let height = {
        let ascent = f32::from(text_system.ascent(font_id, size));
        let descent = f32::from(text_system.descent(font_id, size));
        // gpui 的 descent 是「基线到字形底部」的带符号值（通常为负），
        // 直接相加会把行高压成 0.7 倍，整行字被裁掉一半。
        let line = ascent + descent.abs();
        if line > 0.0 {
            line
        } else {
            theme.cell_height() / theme.line_spacing
        }
    } * theme.line_spacing;
    (width.max(1.0), height.max(1.0))
}

/// `[ui] tab_bar_position` → tab 栏位置。缺省跟随 macOS：底部。
pub(crate) fn tab_bar_position_from_snapshot(
    snapshot: &crate::frontend::utils::corebridge::ClientConfigSnapshot,
) -> crate::frontend::gpui::chrome::tab_bar_view::TabBarPosition {
    use crate::frontend::gpui::chrome::tab_bar_view::TabBarPosition;
    snapshot
        .values
        .get("ui")
        .and_then(|ui| ui.get("tab_bar_position"))
        .and_then(serde_json::Value::as_str)
        .map(|value| match value.trim().to_ascii_lowercase().as_str() {
            "top" => TabBarPosition::Top,
            _ => TabBarPosition::Bottom,
        })
        .unwrap_or(TabBarPosition::Bottom)
}

/// 现有 runtime：默认 server + 当前隔离 socket，按 session/socket 去重。
///
/// 对照 `ConnectionDiscovery.listExistingConnections`：先取 catalog 的
/// attachable session，再把 `-L` 那个 server 上的 session 合进来。
fn discover_existing_rows(
    socket: Option<&str>,
) -> anyhow::Result<Vec<crate::frontend::gpui::model::existing::ExistingRuntime>> {
    use crate::frontend::gpui::model::existing::ExistingRuntime;
    let mut rows = ExistingRuntime::collect(
        &crate::frontend::utils::corebridge::CoreBridge::discover_existing("tmux", None, None)?,
    );
    if let Some(socket) = socket {
        let sessions = crate::frontend::utils::corebridge::CoreBridge::discover_tmux_sessions(
            "local",
            None,
            Some(socket),
        )?;
        for session in sessions {
            rows.push(ExistingRuntime::from_tmux_session(&session, Some(socket)));
        }
    }
    let mut seen = std::collections::HashSet::new();
    rows.retain(|row| {
        seen.insert((
            row.runtime.clone(),
            row.session.clone(),
            row.socket.clone().unwrap_or_default(),
        ))
    });
    Ok(rows)
}

#[cfg(test)]
mod pump_tests {
    use super::*;

    /// 刚写过输入的那几百毫秒必须保持快轮询：否则回显要等空闲退避的 64ms。
    #[test]
    fn input_keeps_the_pump_fast_until_the_echo_lands() {
        let mut delay = IDLE_PUMP_INTERVAL;
        assert_eq!(
            next_pump_delay(delay, false, true),
            PUMP_INTERVAL,
            "有输入就要立刻回到快轮询"
        );
        // 没有输入也没有变化：继续按空闲上限。
        delay = IDLE_PUMP_INTERVAL;
        assert_eq!(next_pump_delay(delay, false, false), IDLE_PUMP_INTERVAL);
    }

    /// 空闲时退避、有事件时立刻回到最快节奏。
    #[test]
    fn pump_delay_backs_off_when_idle_and_resets_on_activity() {
        assert_eq!(pump_delay(PUMP_INTERVAL, true), PUMP_INTERVAL);
        let mut delay = PUMP_INTERVAL;
        for _ in 0..8 {
            delay = pump_delay(delay, false);
        }
        assert_eq!(delay, IDLE_PUMP_INTERVAL, "空闲不能超过上限");
        assert_eq!(pump_delay(IDLE_PUMP_INTERVAL, false), IDLE_PUMP_INTERVAL);
        assert_eq!(pump_delay(IDLE_PUMP_INTERVAL, true), PUMP_INTERVAL);
    }
}
