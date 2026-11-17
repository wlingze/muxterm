//! 命令面板里非 `KeyAction` 的那些命令，以及它们需要的小状态机。
//!
//! 覆盖：
//! - `Local` / `SSH`：直接把快速面板导航到会话列表 / 机器列表；
//! - `Rename Tab…` / `Rename Workspace…`：输入浮层 → Core `rename_*`；
//! - `Move Tab Left/Right`：Core `move_tab`；
//! - `Language`：语言子菜单 + `i18n::set_language` + 写回 `[ui] language`；
//! - `Theme`：`theme.name` 在浅/深之间切换；
//! - `Status Bar Mode`：`statusbar.mode` 切换，tmux 模式显示 tmux 自己的左右段。
//!
//! 纯逻辑（主题切换方向、tab 重排目标、配置值解析）都在这里做成可单测的函数。

use gpui::Context;

use crate::frontend::command_queue::ClientCommand;
use crate::frontend::gpui::chrome::palette_view::PaletteItem;
use crate::frontend::gpui::model::panel::PanelTab;
use crate::frontend::gpui::model::status_bar::StatusBarSnapshot;
use crate::frontend::gpui::model::target::TargetTransport;
use crate::frontend::gpui::state::GpuiApp;
use crate::frontend::utils::corebridge::{ClientTask, CoreBridge};
use crate::frontend::utils::i18n::{self, Language, TextKey};

/// 命令面板的二级列表（Esc 回到目录，而不是关面板）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteMenu {
    Languages,
}

/// 重命名浮层要改谁。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptTarget {
    RenameTab { tab_id: u32 },
    RenameWorkspace,
}

/// 重命名输入浮层。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptState {
    pub target: PromptTarget,
    pub title: String,
    pub value: String,
    pub error: Option<String>,
}

/// 状态栏来源：tmux 段或前端主题。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusBarMode {
    #[default]
    Tmux,
    Theme,
}

impl StatusBarMode {
    pub fn as_config(self) -> &'static str {
        match self {
            Self::Tmux => "tmux",
            Self::Theme => "theme",
        }
    }

    /// 配置里只有 `tmux` / `theme` 两个值；其它拼写按默认 tmux 处理。
    pub fn from_config(value: &str) -> Self {
        if value.trim().eq_ignore_ascii_case("theme") {
            Self::Theme
        } else {
            Self::Tmux
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Self::Tmux => Self::Theme,
            Self::Theme => Self::Tmux,
        }
    }
}

/// 主题切换：暗色就用浅色主题，反之用深色。
///
/// 配置里 `theme.name` 既可能是 `system` 也可能是具体主题名；这里只回答
/// 「下一该写哪个固定主题」，不动 `theme.light` / `theme.dark` 的映射。
pub fn next_theme_name(is_dark: bool) -> &'static str {
    if is_dark {
        "white"
    } else {
        "black"
    }
}

/// 主题背景是否偏暗（相对亮度 < 0.5）。
pub fn theme_is_dark(bg: gpui::Rgba) -> bool {
    let luminance = 0.2126 * bg.r + 0.7152 * bg.g + 0.0722 * bg.b;
    luminance < 0.5
}

/// 当前 tab 往左/右挪的目标。`delta` 为 -1 / +1。
///
/// 返回 `(from, target, before)`：Core 的语义是「把 from 插到 target 前/后」。
pub fn move_tab_target(tabs: &[u32], active: u32, delta: isize) -> Option<(u32, u32, bool)> {
    let index = tabs.iter().position(|id| *id == active)?;
    if delta < 0 {
        let previous = index.checked_sub(1)?;
        Some((active, tabs[previous], true))
    } else {
        let next = index.checked_add(1).filter(|next| *next < tabs.len())?;
        Some((active, tabs[next], false))
    }
}

/// 配置里的 `[ui] language` 值 → `Language`。
pub fn language_from_config(value: &str) -> Language {
    match value.trim().to_ascii_lowercase().as_str() {
        "en" | "en-us" | "en-gb" => Language::English,
        "zh" | "zh-cn" | "zh-hans" => Language::SimplifiedChinese,
        _ => Language::System,
    }
}

impl GpuiApp {
    pub fn palette_menu(&self) -> Option<PaletteMenu> {
        self.palette_menu
    }

    /// 语言子菜单的条目（当前语言带 `· Current` 标记）。
    pub fn language_menu_items(&self) -> Vec<PaletteItem> {
        crate::frontend::gpui::chrome::palette_view::language_items(self.language)
    }

    pub fn language(&self) -> Language {
        self.language
    }

    /// `Local`：面板直接进本机 session 列表。
    pub fn open_local_sessions(&mut self, cx: &mut Context<Self>) {
        self.show_panel(PanelTab::Workspaces, cx);
        self.navigate_picker_host(TargetTransport::Local, cx);
    }

    /// `SSH`：面板直接进机器列表。
    pub fn open_ssh_hosts(&mut self, cx: &mut Context<Self>) {
        self.show_panel(PanelTab::Workspaces, cx);
        self.begin_panel_open(cx);
    }

    pub fn prompt(&self) -> Option<&PromptState> {
        self.prompt.as_ref()
    }

    pub fn open_rename_tab(&mut self, cx: &mut Context<Self>) {
        let Some(workspace) = self.visible.clone() else {
            return;
        };
        let Some(tab_id) = self
            .store
            .workspace(&workspace)
            .and_then(|view| view.active_tab_id())
        else {
            return;
        };
        let name = self
            .tabs()
            .into_iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| tab.name)
            .unwrap_or_default();
        self.prompt = Some(PromptState {
            target: PromptTarget::RenameTab { tab_id },
            title: i18n::tr(TextKey::RenameTab),
            value: name,
            error: None,
        });
        cx.notify();
    }

    pub fn open_rename_workspace(&mut self, cx: &mut Context<Self>) {
        let Some(workspace) = self.visible.clone() else {
            return;
        };
        let name = self.workspace_name(&workspace);
        self.prompt = Some(PromptState {
            target: PromptTarget::RenameWorkspace,
            title: i18n::tr(TextKey::RenameWorkspace),
            value: name,
            error: None,
        });
        cx.notify();
    }

    pub fn cancel_prompt(&mut self, cx: &mut Context<Self>) {
        if self.prompt.take().is_some() {
            cx.notify();
        }
    }

    pub fn prompt_push(&mut self, text: &str, cx: &mut Context<Self>) {
        if let Some(prompt) = self.prompt.as_mut() {
            for ch in text.chars().filter(|ch| !ch.is_control()) {
                prompt.value.push(ch);
            }
            prompt.error = None;
            cx.notify();
        }
    }

    pub fn prompt_backspace(&mut self, cx: &mut Context<Self>) {
        if let Some(prompt) = self.prompt.as_mut() {
            prompt.value.pop();
            prompt.error = None;
            cx.notify();
        }
    }

    /// 回车提交：名字为空就留在浮层里报错，不把空名字发给 Core。
    pub fn submit_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.prompt.clone() else {
            return;
        };
        let name = prompt.value.trim().to_string();
        if name.is_empty() || name.chars().any(char::is_control) {
            if let Some(prompt) = self.prompt.as_mut() {
                prompt.error = Some("Name cannot be empty".into());
            }
            cx.notify();
            return;
        }
        let Some(workspace) = self.visible.clone() else {
            self.prompt = None;
            return;
        };
        match prompt.target {
            PromptTarget::RenameTab { tab_id } => {
                self.commands.push(ClientCommand::RenameTab {
                    workspace_id: workspace,
                    tab_id,
                    name,
                });
            }
            PromptTarget::RenameWorkspace => {
                self.commands.push(ClientCommand::RenameWorkspace {
                    workspace_id: workspace,
                    name,
                });
            }
        }
        self.prompt = None;
        cx.notify();
    }

    /// `Move Tab Left/Right`：算出目标后交给 Core 重排。
    pub fn move_active_tab(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(workspace) = self.visible.clone() else {
            return;
        };
        let tabs: Vec<u32> = self.tabs().into_iter().map(|tab| tab.id).collect();
        let Some(active) = self
            .store
            .workspace(&workspace)
            .and_then(|view| view.active_tab_id())
        else {
            return;
        };
        if let Some((from, target, before)) = move_tab_target(&tabs, active, delta) {
            self.commands.push(ClientCommand::MoveTab {
                workspace_id: workspace,
                from,
                target,
                before,
            });
            cx.notify();
        }
    }

    /// `Theme`：浅↔深切换并写回 `[theme] name`，随后从 Core 快照重读。
    pub fn toggle_theme(&mut self, cx: &mut Context<Self>) {
        let next = next_theme_name(theme_is_dark(self.theme.bg));
        self.commands.push(ClientCommand::ConfigApply {
            path: "theme.name".into(),
            value: serde_json::Value::String(next.into()),
        });
        self.config_reload_pending = true;
        cx.notify();
    }

    pub fn status_bar_mode(&self) -> StatusBarMode {
        self.status_bar_mode
    }

    pub fn status_snapshot(&self) -> Option<&StatusBarSnapshot> {
        self.status_snapshot.as_ref()
    }

    /// `Status Bar Mode`：tmux ↔ theme，写回 `[statusbar] mode`。
    pub fn toggle_status_bar_mode(&mut self, cx: &mut Context<Self>) {
        let next = self.status_bar_mode.toggled();
        self.status_bar_mode = next;
        self.commands.push(ClientCommand::ConfigApply {
            path: "statusbar.mode".into(),
            value: serde_json::Value::String(next.as_config().into()),
        });
        if matches!(next, StatusBarMode::Tmux) {
            self.refresh_status_snapshot();
        }
        cx.notify();
    }

    /// `Language`：切到语言子菜单。
    pub fn open_language_menu(&mut self, cx: &mut Context<Self>) {
        self.palette_menu = Some(PaletteMenu::Languages);
        self.palette.input.clear();
        self.palette.selected = 0;
        cx.notify();
    }

    /// 语言子菜单 Esc：回到命令目录，不关面板。
    pub fn palette_menu_back(&mut self, cx: &mut Context<Self>) -> bool {
        if self.palette_menu.take().is_none() {
            return false;
        }
        self.palette.input.clear();
        self.palette.selected = 0;
        cx.notify();
        true
    }

    /// 选一种界面语言：立刻生效并写回 `[ui] language`。
    pub fn set_language(&mut self, language: Language, cx: &mut Context<Self>) {
        self.language = language;
        i18n::set_language(language);
        self.commands.push(ClientCommand::ConfigApply {
            path: "ui.language".into(),
            value: serde_json::Value::String(language.tag().into()),
        });
        self.config_reload_pending = true;
        self.palette_menu = None;
        self.palette.input.clear();
        self.palette.selected = 0;
        cx.notify();
    }

    /// ConfigApply 之后的配置重读：主题、语言、状态栏模式都从 Core 快照来。
    pub(super) fn apply_pending_config(&mut self) -> bool {
        if !self.config_reload_pending {
            return false;
        }
        self.config_reload_pending = false;
        let Some(pump) = self.pump.as_ref() else {
            return false;
        };
        let Ok(snapshot) = pump.client().config_describe() else {
            return false;
        };
        self.reload_from_snapshot(&snapshot);
        true
    }

    fn reload_from_snapshot(
        &mut self,
        snapshot: &crate::frontend::utils::corebridge::ClientConfigSnapshot,
    ) {
        self.apply_shortcut_values(&snapshot.values);
        self.theme = super::theme_from_snapshot(snapshot);
        self.configured_font_size = self.theme.font_size;
        self.cell = (self.theme.cell_width(), self.theme.cell_height());
        self.language = language_from_config(
            snapshot.values["ui"]["language"]
                .as_str()
                .unwrap_or("system"),
        );
        i18n::set_language(self.language);
        // 窗口标题开关同样来自 Core：关掉后下一帧就把标题写成空串。
        self.set_show_title_bar(super::ui_config_from_snapshot(snapshot).show_title_bar);
        self.tab_bar_style = super::tab_bar_style_from_snapshot(snapshot);
        self.status_bar_mode = StatusBarMode::from_config(
            snapshot.values["statusbar"]["mode"]
                .as_str()
                .unwrap_or("tmux"),
        );
        self.refresh_status_snapshot();
    }

    /// 测试注入：无头环境没有 Core，直接给一份 tmux status 快照。
    pub fn set_status_snapshot_for_tests(&mut self, snapshot: StatusBarSnapshot) {
        self.status_snapshot = Some(snapshot);
        self.status_snapshot_at = Some(std::time::Instant::now());
    }

    pub fn set_status_bar_mode_for_tests(&mut self, mode: StatusBarMode) {
        self.status_bar_mode = mode;
    }

    /// 事件泵里按 2 秒节流刷新 tmux 状态段。
    pub(super) fn refresh_status_snapshot_if_due(&mut self) -> bool {
        if !matches!(self.status_bar_mode, StatusBarMode::Tmux) {
            return false;
        }
        let due = self
            .status_snapshot_at
            .is_none_or(|at| at.elapsed() >= std::time::Duration::from_secs(2));
        if !due {
            return false;
        }
        let before = self.status_snapshot.clone();
        self.refresh_status_snapshot();
        // 只有真的变了才要求重绘：tmux status 每 2 秒都查一次，
        // 内容没变时不能白重绘整窗。
        self.status_snapshot != before
    }

    /// 读一次可见 workspace 的 tmux status 快照（`statusbar.mode = tmux` 才用）。
    pub(super) fn refresh_status_snapshot(&mut self) {
        let Some(workspace) = self.visible.clone() else {
            return;
        };
        let Some(target) = self.workspace_target(&workspace) else {
            self.status_snapshot = None;
            return;
        };
        let runtime = target.runtime.label();
        let alias = match &target.transport {
            TargetTransport::Local => None,
            TargetTransport::Ssh { name } => Some(name.as_str()),
        };
        let session = target.session.clone().unwrap_or_default();
        let value =
            CoreBridge::status_snapshot_json(runtime, alias, target.socket.as_deref(), &session);
        self.status_snapshot = value
            .ok()
            .and_then(|value| serde_json::to_vec(&value).ok())
            .and_then(|bytes| StatusBarSnapshot::decode(&bytes))
            .filter(|snapshot| snapshot.enabled);
        self.status_snapshot_at = Some(std::time::Instant::now());
    }

    /// 状态栏的 tmux 左右段（mode=tmux 且有快照时才显示）。
    pub fn tmux_status_segments(&self) -> Option<(String, String)> {
        if !matches!(self.status_bar_mode, StatusBarMode::Tmux) {
            return None;
        }
        let snapshot = self.status_snapshot.as_ref()?;
        if snapshot.left.trim().is_empty() && snapshot.right.trim().is_empty() {
            return None;
        }
        Some((snapshot.left.clone(), snapshot.right.clone()))
    }
}

/// 语言子菜单选中后执行；供 `run_palette_action` 复用。
impl GpuiApp {
    pub(crate) fn queue_task(&mut self, task: ClientTask) {
        self.commands.push(ClientCommand::Task {
            workspace_id: self.visible.clone(),
            task,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 左移/右移的边界：挪到两端就不再发命令。
    #[test]
    fn move_tab_target_walks_neighbours_and_stops_at_the_edges() {
        let tabs = [1, 2, 3];
        assert_eq!(move_tab_target(&tabs, 2, -1), Some((2, 1, true)));
        assert_eq!(move_tab_target(&tabs, 3, 1), None, "最右的 tab 不能再右移");
        assert_eq!(move_tab_target(&tabs, 1, -1), None, "最左的 tab 不能再左移");
        assert_eq!(move_tab_target(&tabs, 2, 1), Some((2, 3, false)));
        assert_eq!(move_tab_target(&tabs, 9, 1), None, "不在列表里的 tab 不动");
        assert_eq!(move_tab_target(&[], 1, 1), None);
    }

    /// 主题方向：暗色切浅色，浅色切深色。
    #[test]
    fn next_theme_flips_between_white_and_black() {
        assert_eq!(next_theme_name(true), "white");
        assert_eq!(next_theme_name(false), "black");
        assert!(theme_is_dark(gpui::rgb(0x111111)));
        assert!(!theme_is_dark(gpui::rgb(0xffffff)));
    }

    /// 状态栏模式只有两个取值，未知拼写按默认 tmux。
    #[test]
    fn status_bar_mode_round_trips_through_config() {
        assert_eq!(StatusBarMode::from_config("theme"), StatusBarMode::Theme);
        assert_eq!(StatusBarMode::from_config("TMUX"), StatusBarMode::Tmux);
        assert_eq!(StatusBarMode::from_config("nonsense"), StatusBarMode::Tmux);
        assert_eq!(StatusBarMode::Tmux.as_config(), "tmux");
        assert_eq!(StatusBarMode::Theme.toggled().as_config(), "tmux");
    }

    /// `[ui] language` 的取值解析。
    #[test]
    fn language_parses_the_config_value() {
        assert_eq!(language_from_config("zh-CN"), Language::SimplifiedChinese);
        assert_eq!(language_from_config("zh"), Language::SimplifiedChinese);
        assert_eq!(language_from_config("en"), Language::English);
        assert_eq!(language_from_config("system"), Language::System);
        assert_eq!(language_from_config(""), Language::System);
    }
}
