//! 设置窗口的 schema 驱动模型。
//!
//! 移植自 macOS `App/SettingsWindow.swift` 的 manifest 消费部分：侧栏分类来自
//! Core 的 `manifest.groups`，每个字段的控件类型、生效方式、标题都由 manifest
//! 决定，前端不写死一份配置树。
//!
//! 动态文案：Swift 用 `MuxtermI18n.trDynamic(key, fallback:)`；Rust 的 catalog 是
//! 固定 `TextKey` 枚举，查不到就退回同一套英文 fallback（与本表一致）。

use serde_json::Value;

/// 字段控件类型，来自 manifest 的 `control`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlKind {
    FontPicker,
    FontFallback,
    Number,
    Text,
    Multiline,
    Switch,
    Select,
    ThemePicker,
    Directory,
    ProjectEditor,
    ShortcutEditor,
    /// manifest 里出现但前端还不认识的控件：退化成文本框，不丢字段。
    Other(String),
}

impl ControlKind {
    pub fn parse(raw: &str) -> Self {
        match raw {
            "font_picker" => Self::FontPicker,
            "font_fallback" => Self::FontFallback,
            "number" => Self::Number,
            "text" => Self::Text,
            "multiline" => Self::Multiline,
            "switch" => Self::Switch,
            "select" => Self::Select,
            "theme_picker" => Self::ThemePicker,
            "directory" | "file" => Self::Directory,
            "project_editor" => Self::ProjectEditor,
            "shortcut_editor" => Self::ShortcutEditor,
            other => Self::Other(other.to_string()),
        }
    }

    /// 能不能直接点选（true 时渲染成一组选项）。
    pub fn is_switch(&self) -> bool {
        matches!(self, Self::Switch)
    }

    /// 枚举型控件：点击/左右键在 `options` 里循环。
    pub fn is_select(&self) -> bool {
        matches!(self, Self::Select | Self::ThemePicker)
    }

    /// 文本型控件：键盘直接改值。
    pub fn is_text_like(&self) -> bool {
        matches!(
            self,
            Self::FontPicker
                | Self::FontFallback
                | Self::Number
                | Self::Text
                | Self::Multiline
                | Self::Directory
        )
    }
}

/// 字段什么时候生效，来自 manifest 的 `apply`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyMode {
    Immediate,
    NextWorkspace,
    MacOs,
    Save,
}

impl ApplyMode {
    pub fn parse(raw: &str) -> Self {
        match raw {
            "immediate" => Self::Immediate,
            "next_workspace" => Self::NextWorkspace,
            "macos" => Self::MacOs,
            _ => Self::Save,
        }
    }

    /// 行尾的小标签。
    pub fn label(self) -> &'static str {
        match self {
            Self::Immediate => "ON APPLY",
            Self::NextWorkspace => "NEXT WORKSPACE",
            Self::MacOs => "macOS",
            Self::Save => "ON SAVE",
        }
    }
}

/// manifest 里的一个字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsField {
    /// 带前导 `/` 的路径，与 manifest 完全一致。
    pub path: String,
    pub control: ControlKind,
    pub apply: ApplyMode,
    pub title_key: String,
    pub options: Vec<String>,
}

impl SettingsField {
    pub fn title(&self) -> String {
        field_title(&self.path, &self.title_key)
    }

    pub fn description(&self) -> String {
        field_description(&self.path, &self.title_key)
    }

    /// 枚举控件的下一个取值：越界回绕，没有选项时返回 `None`。
    pub fn next_option(&self, current: Option<&str>) -> Option<String> {
        let options = &self.options;
        if options.is_empty() {
            return None;
        }
        let index = current
            .and_then(|current| options.iter().position(|option| option == current))
            .map(|index| index + 1)
            .unwrap_or(0);
        options.get(index % options.len()).cloned()
    }

    /// 枚举控件显示当前值的文案（`system` → `System`，主题等走同一套）。
    pub fn option_text(&self, value: Option<&str>) -> String {
        value
            .map(|value| option_label(&self.path, value))
            .unwrap_or_default()
    }
}

/// manifest 里的一个分类（`groups[]`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsGroup {
    pub id: String,
    pub title_key: String,
    pub fields: Vec<SettingsField>,
}

/// 侧栏要画的一条。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsCategory {
    pub id: String,
    pub title: String,
    pub hint: String,
    pub description: String,
    /// SF Symbol 名；gpui 不画图标，但保留它作为 parity 证据。
    pub icon: &'static str,
    pub section: &'static str,
    pub fields: Vec<SettingsField>,
}

/// 解析后的 manifest。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SettingsManifest {
    pub groups: Vec<SettingsGroup>,
}

impl SettingsManifest {
    /// 从 Core 返回的 manifest JSON 解析；结构不对的字段直接跳过，不让整个窗口失败。
    pub fn parse(manifest: &Value) -> Self {
        let mut groups = Vec::new();
        for group in manifest
            .get("groups")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let Some(id) = group.get("id").and_then(Value::as_str) else {
                continue;
            };
            let title_key = group
                .get("title_key")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let mut fields = Vec::new();
            for field in group
                .get("fields")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default()
            {
                if let Some(platforms) = field.get("platforms").and_then(Value::as_array) {
                    let platform = if cfg!(target_os = "macos") {
                        "macos"
                    } else if cfg!(target_os = "windows") {
                        "windows"
                    } else {
                        "linux"
                    };
                    if !platforms
                        .iter()
                        .any(|entry| entry.as_str() == Some(platform))
                    {
                        continue;
                    }
                }
                let Some(path) = field.get("path").and_then(Value::as_str) else {
                    continue;
                };
                let options = field
                    .get("options")
                    .and_then(Value::as_array)
                    .map(|options| {
                        options
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                fields.push(SettingsField {
                    path: path.to_string(),
                    control: ControlKind::parse(
                        field
                            .get("control")
                            .and_then(Value::as_str)
                            .unwrap_or("text"),
                    ),
                    apply: ApplyMode::parse(
                        field.get("apply").and_then(Value::as_str).unwrap_or("save"),
                    ),
                    title_key: field
                        .get("title_key")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    options,
                });
            }
            groups.push(SettingsGroup {
                id: id.to_string(),
                title_key: title_key.to_string(),
                fields,
            });
        }
        Self { groups }
    }

    pub fn categories(&self) -> Vec<SettingsCategory> {
        self.groups
            .iter()
            .map(|group| SettingsCategory {
                id: group.id.clone(),
                title: category_title(&group.id, &group.title_key),
                hint: category_hint(&group.id).to_string(),
                description: category_description(&group.id).to_string(),
                icon: category_icon(&group.id),
                section: section_title(&group.id),
                fields: group.fields.clone(),
            })
            .collect()
    }

    /// 侧栏 + 搜索过滤后的分类；query 为空时全部返回。
    pub fn visible_categories(&self, query: &str) -> Vec<SettingsCategory> {
        self.categories()
            .into_iter()
            .filter(|category| matches_search(category, query))
            .collect()
    }
}

/// 分类标题：优先 manifest 的 `title_key` 尾段，人化成 Title Case。
pub fn category_title(id: &str, title_key: &str) -> String {
    let tail = title_key.rsplit('.').next().unwrap_or_default();
    let source = if tail.is_empty() { id } else { tail };
    let fallback = match id {
        "ui" => "Interface".to_string(),
        "ssh" => "SSH".to_string(),
        _ => humanize(source),
    };
    localized(
        &format!("settings_{}", if id == "ui" { "interface" } else { id }),
        &fallback,
    )
}

/// Core manifest 的动态标题复用同一份 typed i18n catalog。
pub fn localized(key: &str, fallback: &str) -> String {
    use crate::frontend::utils::i18n::{tr, TextKey};
    TextKey::ALL
        .iter()
        .find(|entry| entry.as_str() == key)
        .map(|key| tr(*key))
        .unwrap_or_else(|| fallback.to_string())
}

/// `font.line_spacing` → `Font Line Spacing`。
pub fn humanize(raw: &str) -> String {
    raw.replace(['.', '/', '_', '-'], " ")
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// 字段标题：manifest 没给 key 时退回路径尾段。
pub fn field_title(path: &str, title_key: &str) -> String {
    let fallback = match path {
        "/font/family" => "Font family",
        "/font/size" => "Font size",
        "/font/line_spacing" => "Line height",
        "/font/column_spacing" => "Character width",
        "/font/fallback" => "Fallback fonts",
        "/theme/name" => "Theme",
        "/theme/light" => "Light appearance",
        "/theme/dark" => "Dark appearance",
        "/scrollback/lines" => "Scrollback lines",
        "/attention/enabled" => "Workspace attention",
        "/attention/blocked_regex" => "Blocked output patterns",
        "/attention/debounce_ms" => "Notification delay",
        "/ui/tab_bar_position" => "Tab bar position",
        "/ui/tab_bar_style" => "Tab style",
        "/ui/show_title_bar" => "Show title bar",
        "/ui/language" => "Language",
        "/tmux/auto_mouse" => "Mouse forwarding",
        "/tmux/default_session" => "Default session",
        "/tmux/socket" => "tmux socket",
        "/pool/max_slots" => "Workspace reminder limit",
        "/pane/default_command" => "Default shell command",
        "/pane/workdir" => "Initial directory",
        "/ssh/host" => "Default SSH host",
        "/ssh/user" => "SSH user",
        "/ssh/port" => "SSH port",
        "/ssh/key_path" => "SSH private key",
        "/shortcuts/preset" => "Configuration labels",
        "/shortcuts/primary_key" => "Primary modifier",
        "/shortcuts/overrides" => "Custom shortcuts",
        "/projects" => "Projects",
        "/debug/enabled" => "Debug logging",
        "/debug/log_dir" => "Log directory",
        _ => "",
    };
    if !fallback.is_empty() {
        return localized(&title_key.replace('.', "_"), fallback);
    }
    if !title_key.is_empty() {
        return humanize(title_key.rsplit('.').next().unwrap_or_default());
    }
    humanize(path.rsplit('/').next().unwrap_or(path))
}

/// 字段说明：只有少数路径有专文，其余留空（Swift 同样返回空串）。
pub fn field_description(path: &str, _title_key: &str) -> String {
    match path {
        "/font/family" => "Search a monospace font, then set size, line height, and character width. 100 is the font's natural spacing.".to_string(),
        "/font/size" => "Adjust the terminal scale without changing your display settings.".to_string(),
        "/font/line_spacing" => "Vertical spacing as a percent. 100 keeps the font's natural line height.".to_string(),
        "/font/column_spacing" => "Character width as a percent. 100 keeps the font's natural advance.".to_string(),
        "/scrollback/lines" => "How much history each pane keeps above the visible screen.".to_string(),
        "/attention/blocked_regex" => "Extra patterns that count as blocked output, one per line.".to_string(),
        "/ui/tab_bar_position" => "Put the tab strip above or below the panes.".to_string(),
        "/ui/tab_bar_style" => "How tabs share the available width.".to_string(),
        "/ui/show_title_bar" => "Show the native window title. Turn it off for a bare title bar that only keeps the traffic lights, like the macOS build.".to_string(),
        "/ui/borderless" => "Use client side decorations instead of the system title bar.".to_string(),
        "/ui/language" => "Language for menus, panels and this window.".to_string(),
        "/ui/tab_bar_height" => "Height of the tab strip in pixels.".to_string(),
        "/statusbar/mode" => "Show tmux's status line or the theme's own bar.".to_string(),
        "/ssh/host" => "Default host for new SSH workspaces.".to_string(),
        "/ssh/port" => "Port used when connecting over SSH.".to_string(),
        "/ssh/user" => "User name used when connecting over SSH.".to_string(),
        "/ssh/key_path" => "Private key handed to ssh; empty uses your agent.".to_string(),
        "/behavior/on_last_pane_exit" => "What happens when the last pane in a tab exits.".to_string(),
        "/behavior/on_program_exit_abnormal" => "What happens when a pane's program exits with an error.".to_string(),
        "/platform/macos/option_as_alt" => "Type Option as Alt/Esc instead of accented characters.".to_string(),
        "/platform/linux/client_side_decorations" => "Draw window buttons inside the app.".to_string(),
        "/update/auto_check" => "Check for a newer release once at startup.".to_string(),
        "/projects" => "Reusable workspace launch profiles shared by Open. Changes are saved with Apply.".to_string(),
        "/shortcuts/preset" => "Only changes the labels used in configuration. Every default keeps the Colemak hand position in both layouts.".to_string(),
        "/shortcuts/primary_key" => "Modifier used for configured shortcuts.".to_string(),
        _ => String::new(),
    }
}

/// 选项文案。只有这些组合有专文，其余按 `humanize` 显示原始值。
/// 字段值的一行文本；数组与对象按 JSON 展开，避免吞掉内容。
pub fn display_value(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "—".to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Bool(flag)) => flag.to_string(),
        Some(Value::Number(number)) => number.to_string(),
        Some(other) => other.to_string(),
    }
}

pub fn option_label(path: &str, value: &str) -> String {
    let label = match (path, value) {
        ("/theme/name", "system") => "Follow system",
        ("/theme/name", "black") | ("/theme/dark", "black") | ("/theme/light", "black") => "Black",
        ("/theme/name", "white") | ("/theme/dark", "white") | ("/theme/light", "white") => "White",
        ("/statusbar/mode", "tmux") => "Use tmux colors",
        ("/statusbar/mode", "theme") => "Use theme colors",
        ("/ui/tab_bar_position", "top") => "Top",
        ("/ui/tab_bar_position", "bottom") => "Bottom",
        ("/ui/tab_bar_style", "equal_width") => "Fill equally",
        ("/ui/tab_bar_style", "fixed") => "Fixed width (compact)",
        ("/ui/tab_bar_style", "compact") => "Fit title",
        ("/behavior/on_last_pane_exit", "close_window") => "Close the window",
        ("/behavior/on_last_pane_exit", "keep_empty") => "Keep an empty window",
        ("/behavior/on_last_pane_exit", "new_shell") => "Open a new shell",
        ("/behavior/on_program_exit_abnormal", "notify") => "Keep and notify",
        ("/behavior/on_program_exit_abnormal", "close") => "Close the pane",
        ("/behavior/on_program_exit_abnormal", "keep") => "Keep the pane",
        ("/shortcuts/primary_key", "auto") => "Automatic",
        ("/shortcuts/primary_key", "alt") => "Alt",
        ("/shortcuts/primary_key", "command") => "Command",
        ("/shortcuts/primary_key", "control") => "Control",
        ("/shortcuts/primary_key", "super") => "Super",
        _ => return humanize(value),
    };
    label.to_string()
}

pub fn category_hint(id: &str) -> &'static str {
    match id {
        "appearance" => "Fonts & colors",
        "runtime" => "Workspaces",
        "quick_panel" => "Recent attaches",
        "attention" => "Agent signals",
        "ui" => "Window chrome",
        "ssh" => "Remote access",
        "behavior" => "Exit rules",
        "platform" => "Desktop specific",
        "projects" => "Launch profiles",
        "shortcuts" => "Keyboard",
        "debug" => "Launch log",
        _ => "General",
    }
}

pub fn category_description(id: &str) -> &'static str {
    match id {
        "appearance" => "Tune the terminal you look at all day: type, scale, and color.",
        "debug" => "Turn on a debug log and choose where it is written.",
        "runtime" => "Set defaults for new workspaces, panes, and terminal history.",
        "quick_panel" => "Choose how far back recent workspace usage affects Quick Panel order.",
        "attention" => "Decide when Muxterm should surface work that needs your attention.",
        "ui" => "Shape the surrounding window chrome and tab bar.",
        "ssh" => "Defaults used when opening remote workspaces over SSH.",
        "behavior" => "Choose what Muxterm does when panes or commands exit.",
        "platform" => "Options specific to the desktop platform you are running on.",
        "projects" => "Save the workspaces you return to most often.",
        "shortcuts" => "Choose a keyboard preset and customize individual actions.",
        _ => "Configure this part of Muxterm.",
    }
}

pub fn category_icon(id: &str) -> &'static str {
    match id {
        "appearance" => "textformat",
        "runtime" => "terminal",
        "quick_panel" => "clock.arrow.circlepath",
        "attention" => "bell",
        "ui" => "macwindow",
        "ssh" => "network",
        "behavior" => "arrow.turn.down.right",
        "platform" => "desktopcomputer",
        "update" => "arrow.down.circle",
        "projects" => "folder",
        "shortcuts" => "keyboard",
        "debug" => "ladybug",
        _ => "gearshape",
    }
}

pub fn section_title(id: &str) -> &'static str {
    match id {
        "appearance" => "Terminal",
        "runtime" => "Workspace defaults",
        "quick_panel" => "Quick Panel ranking",
        "attention" => "Attention",
        "ui" => "Interface",
        "ssh" => "SSH defaults",
        "behavior" => "Exit behavior",
        "platform" => "Platform",
        "projects" => "Workspace profiles",
        "shortcuts" => "Keyboard shortcuts",
        "debug" => "Debug log",
        _ => "Settings",
    }
}

/// 搜索：分类标题/提示/说明，或任一字段标题命中都算。
pub fn matches_search(category: &SettingsCategory, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    if category.title.to_lowercase().contains(&query)
        || category.hint.to_lowercase().contains(&query)
        || category.description.to_lowercase().contains(&query)
    {
        return true;
    }
    category
        .fields
        .iter()
        .any(|field| field.title().to_lowercase().contains(&query))
}

/// 按 `/a/b` 路径取嵌套值。
pub fn value_at<'a>(values: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = values;
    for segment in path.split('/').filter(|segment| !segment.is_empty()) {
        current = current.get(segment)?;
    }
    Some(current)
}

/// 按 `/a/b` 路径写入嵌套值；中间层不存在时补对象。
pub fn set_at(values: &mut Value, path: &str, next: Value) -> bool {
    let segments: Vec<&str> = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    let Some((last, parents)) = segments.split_last() else {
        return false;
    };
    let mut current = values;
    for segment in parents {
        if !current.is_object() {
            return false;
        }
        let object = current.as_object_mut().expect("刚检查过");
        current = object
            .entry((*segment).to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
    }
    let Some(object) = current.as_object_mut() else {
        return false;
    };
    object.insert((*last).to_string(), next);
    true
}

/// 设置窗口的草稿：一份值副本 + 选择状态。
///
/// Apply 只提交与 baseline 不同的路径，所以「打开设置、什么都没改、按 Apply」
/// 不会有副作用（对照 `testApplyWithDefaultManifestDoesNotRejectDraft`）。
#[derive(Debug, Clone, Default)]
pub struct SettingsDraft {
    baseline: Value,
    values: Value,
    selected: Option<String>,
    query: String,
}

impl SettingsDraft {
    pub fn new(baseline: &Value) -> Self {
        Self {
            baseline: baseline.clone(),
            values: baseline.clone(),
            selected: None,
            query: String::new(),
        }
    }

    pub fn values(&self) -> &Value {
        &self.values
    }

    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub fn set_selected(&mut self, id: Option<String>) {
        self.selected = id;
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// 搜索只影响侧栏可见性，不动草稿内容，也不改当前分类。
    pub fn set_query(&mut self, query: impl Into<String>) {
        self.query = query.into();
    }

    /// 在草稿里改一个字段。
    pub fn set(&mut self, path: &str, value: Value) -> bool {
        set_at(&mut self.values, path, value)
    }

    pub fn get(&self, path: &str) -> Option<&Value> {
        value_at(&self.values, path)
    }

    pub fn is_dirty(&self) -> bool {
        !self.patches().is_empty()
    }

    /// 与 baseline 不同的字段，按 manifest 顺序返回。
    pub fn patches_for(&self, manifest: &SettingsManifest) -> Vec<(String, Value)> {
        let mut patches = Vec::new();
        for group in &manifest.groups {
            for field in &group.fields {
                let next = self.get(&field.path);
                let previous = value_at(&self.baseline, &field.path);
                if let Some(next) = next {
                    if previous != Some(next) {
                        patches.push((field.path.clone(), next.clone()));
                    }
                }
            }
        }
        patches
    }

    pub fn patches(&self) -> Vec<(String, Value)> {
        let mut patches = Vec::new();
        collect_patches(&self.baseline, &self.values, String::new(), &mut patches);
        patches
    }

    /// 丢弃修改，保留选择与搜索词。
    pub fn revert(&mut self) {
        self.values = self.baseline.clone();
    }

    /// Core 提交成功后，把当前值当作新的 baseline。
    pub fn rebase(&mut self) {
        self.baseline = self.values.clone();
    }
}

fn collect_patches(
    previous: &Value,
    next: &Value,
    path: String,
    patches: &mut Vec<(String, Value)>,
) {
    match (previous, next) {
        (Value::Object(previous_map), Value::Object(next_map)) => {
            for (key, next_value) in next_map {
                let child = if path.is_empty() {
                    format!("/{key}")
                } else {
                    format!("{path}/{key}")
                };
                match previous_map.get(key) {
                    Some(previous_value) => {
                        collect_patches(previous_value, next_value, child, patches)
                    }
                    None => patches.push((child, next_value.clone())),
                }
            }
        }
        _ => {
            if previous != next && !path.is_empty() {
                patches.push((path, next.clone()));
            }
        }
    }
}

/// 侧栏固定宽度（Swift 里是 220pt）。
pub const SIDEBAR_WIDTH: f32 = 220.0;

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> SettingsManifest {
        SettingsManifest::parse(&crate::config::ConfigDocument::manifest_json())
    }

    /// 对照 `SettingsWindowE2ETests.testManifestCategoriesDriveSidebarAndSelection`。
    #[test]
    fn manifest_categories_drive_the_sidebar() {
        let manifest = manifest();
        let ids: Vec<String> = manifest
            .categories()
            .into_iter()
            .map(|category| category.id)
            .collect();
        assert!(!ids.is_empty(), "manifest 必须给出分类");
        assert!(ids.iter().any(|id| id == "appearance"));
        let appearance = manifest
            .categories()
            .into_iter()
            .find(|category| category.id == "appearance")
            .expect("appearance");
        assert_eq!(appearance.title, "Appearance");
        assert_eq!(appearance.section, "Terminal");
        assert_eq!(appearance.icon, "textformat");
        assert_eq!(appearance.fields[0].path, "/font/family");
        assert_eq!(appearance.fields[0].control, ControlKind::FontPicker);
        assert_eq!(appearance.fields[1].apply, ApplyMode::Immediate);
    }

    /// 对照 `SettingsWindowE2ETests.testCategoryTitleHumanizesManifestKey`。
    #[test]
    fn category_title_humanizes_the_manifest_key() {
        assert_eq!(
            category_title("quick_panel", "settings.quick_panel"),
            "Quick Panel"
        );
        assert_eq!(category_title("debug", ""), "Debug");
        assert_eq!(
            category_title("x", "settings.font.line_spacing"),
            "Line Spacing"
        );
        assert_eq!(
            field_title("/font/line_spacing", ""),
            "Line height",
            "已知字段采用 Swift 的可读标题，不显示 schema 标识符"
        );
    }

    /// 对照 `SettingsWindowE2ETests.testSearchSwitchesCategoryWithoutDiscardingDraft`。
    #[test]
    fn search_keeps_the_draft_and_the_selection() {
        let manifest = manifest();
        let mut draft = SettingsDraft::new(&serde_json::json!({ "font": { "size": 13.0 } }));
        draft.set_selected(Some("appearance".into()));
        draft.set("/font/size", serde_json::json!(15.0));

        draft.set_query("ssh");
        let visible = manifest.visible_categories(draft.query());
        assert!(visible.iter().any(|category| category.id == "ssh"));
        assert!(!visible.iter().any(|category| category.id == "appearance"));
        assert_eq!(draft.selected(), Some("appearance"), "搜索不动选择");
        assert_eq!(draft.get("/font/size"), Some(&serde_json::json!(15.0)));

        draft.set_query("");
        assert_eq!(
            manifest.visible_categories("").len(),
            manifest.categories().len()
        );
    }

    /// 搜索命中字段标题也算。
    #[test]
    fn search_matches_field_titles() {
        let manifest = manifest();
        let visible = manifest.visible_categories("scrollback");
        assert!(visible.iter().any(|category| category.id == "runtime"));
        assert!(!visible.iter().any(|category| category.id == "appearance"));
    }

    /// 对照 `SettingsWindowE2ETests.testApplyWithDefaultManifestDoesNotRejectDraft`。
    #[test]
    fn applying_a_fresh_draft_produces_no_patches() {
        let manifest = manifest();
        let baseline = serde_json::json!({
            "font": { "family": "Menlo", "size": 13.0 },
            "ui": { "tab_bar_position": "top" }
        });
        let draft = SettingsDraft::new(&baseline);
        assert!(!draft.is_dirty());
        assert!(draft.patches_for(&manifest).is_empty());
        assert!(draft.patches().is_empty());
    }

    /// 改过的字段才进 patch，且路径与 manifest 一致。
    #[test]
    fn only_changed_fields_become_patches() {
        let manifest = manifest();
        let baseline = serde_json::json!({ "ui": { "tab_bar_position": "top" } });
        let mut draft = SettingsDraft::new(&baseline);
        draft.set("/ui/tab_bar_position", serde_json::json!("bottom"));
        draft.set("/font/size", serde_json::json!(15.0));

        let patches = draft.patches_for(&manifest);
        assert_eq!(
            patches,
            vec![
                ("/font/size".to_string(), serde_json::json!(15.0)),
                (
                    "/ui/tab_bar_position".to_string(),
                    serde_json::json!("bottom")
                ),
            ]
        );
        assert!(draft.is_dirty());

        draft.revert();
        assert!(!draft.is_dirty());
        assert_eq!(
            draft.get("/ui/tab_bar_position"),
            Some(&serde_json::json!("top"))
        );
    }

    /// manifest 之外的改动 `patches()` 也报，但不会混进 `patches_for`。
    #[test]
    fn patches_outside_the_manifest_are_still_reported() {
        let manifest = manifest();
        let mut draft = SettingsDraft::new(&serde_json::json!({}));
        draft.set("/custom/knob", serde_json::json!("on"));
        assert!(
            draft.patches_for(&manifest).is_empty(),
            "manifest 里没有 /custom/knob"
        );
        // baseline 里没有这棵子树时，整棵子树作为一次 add 上报。
        assert_eq!(
            draft.patches(),
            vec![("/custom".to_string(), serde_json::json!({ "knob": "on" }))]
        );
    }

    /// 选项文案表。
    #[test]
    fn option_labels_cover_the_choice_fields() {
        assert_eq!(option_label("/ui/tab_bar_position", "top"), "Top");
        assert_eq!(option_label("/ui/tab_bar_position", "bottom"), "Bottom");
        assert_eq!(
            option_label("/ui/tab_bar_style", "equal_width"),
            "Fill equally"
        );
        assert_eq!(option_label("/theme/name", "system"), "Follow system");
        assert_eq!(
            option_label("/behavior/on_last_pane_exit", "keep_empty"),
            "Keep an empty window"
        );
        assert_eq!(
            option_label("/behavior/on_program_exit_abnormal", "notify"),
            "Keep and notify"
        );
        assert_eq!(option_label("/shortcuts/primary_key", "super"), "Super");
        assert_eq!(option_label("/unknown/path", "some_value"), "Some Value");
    }

    /// 生效标签。
    #[test]
    fn apply_labels_match_the_manifest_modes() {
        assert_eq!(ApplyMode::parse("immediate").label(), "ON APPLY");
        assert_eq!(ApplyMode::parse("next_workspace").label(), "NEXT WORKSPACE");
        assert_eq!(ApplyMode::parse("macos").label(), "macOS");
        assert_eq!(ApplyMode::parse("anything-else").label(), "ON SAVE");
    }

    /// 分类的说明与提示。
    #[test]
    fn category_copy_covers_the_manifest_groups() {
        let manifest = manifest();
        for group in &manifest.groups {
            let category = manifest
                .categories()
                .into_iter()
                .find(|category| category.id == group.id)
                .expect("category");
            assert!(!category.title.is_empty(), "{} 缺标题", group.id);
            assert!(!category.hint.is_empty(), "{} 缺提示", group.id);
            assert!(!category.description.is_empty(), "{} 缺说明", group.id);
            assert!(
                !category.icon.is_empty(),
                "{} 缺图标（parity 证据）",
                group.id
            );
        }
    }

    /// 嵌套值读写。
    #[test]
    fn value_lookup_and_write_walk_the_object() {
        let mut values = serde_json::json!({ "a": { "b": 1 } });
        assert_eq!(value_at(&values, "/a/b"), Some(&serde_json::json!(1)));
        assert_eq!(value_at(&values, "/a/missing"), None);
        assert!(set_at(&mut values, "/a/c/d", serde_json::json!("x")));
        assert_eq!(
            value_at(&values, "/a/c/d"),
            Some(&serde_json::json!("x")),
            "中间层要自动补"
        );
    }

    /// 未知控件退化成 `Other`，字段不丢。
    #[test]
    fn unknown_controls_keep_the_field() {
        let manifest = SettingsManifest::parse(&serde_json::json!({
            "groups": [{
                "id": "future",
                "title_key": "settings.future",
                "fields": [
                    {"path": "/future/knob", "control": "orb", "apply": "save", "title_key": ""}
                ]
            }]
        }));
        assert_eq!(
            manifest.groups[0].fields[0].control,
            ControlKind::Other("orb".into())
        );
        assert_eq!(manifest.groups[0].fields[0].apply, ApplyMode::Save);
    }

    /// 缺字段的 manifest 不 panic。
    #[test]
    fn malformed_manifest_is_skipped_not_panicked() {
        let manifest = SettingsManifest::parse(&serde_json::json!({ "groups": "nope" }));
        assert!(manifest.groups.is_empty());
        let manifest = SettingsManifest::parse(&serde_json::json!({
            "groups": [{"id": "a"}, {"title_key": "no.id"}, {"id": "b", "fields": [{"control": "text"}]}]
        }));
        assert_eq!(manifest.groups.len(), 2);
        assert_eq!(manifest.groups[0].id, "a");
        assert!(manifest.groups[0].fields.is_empty());
        assert_eq!(manifest.groups[1].id, "b");
        assert!(
            manifest.groups[1].fields.is_empty(),
            "没有 path 的字段直接跳过"
        );
    }

    /// 侧栏宽度是固定 220pt（对照 `testSidebarKeepsFixedWidthWhenWindowResizes`）。
    #[test]
    fn sidebar_width_is_fixed() {
        assert_eq!(SIDEBAR_WIDTH, 220.0);
    }
}
