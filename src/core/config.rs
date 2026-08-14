//! 配置与主题加载（Alacritty 风格）。
//!
//! 顶层 `~/.config/muxterm/config.toml`：
//! - `[font]` family / size
//! - `[theme]` name
//! - `[statusbar]` mode（tmux / theme）
//! - `[pool]` max_slots（warm 连接上限）
//! - `[tmux]` auto_mouse / default_session
//! - `[ssh]` host / port / user / key_path
//! - `[scrollback]` lines
//! - `[ui]` tab 栏位置/高度、标题栏
//! - `[pane]` 默认程序与工作目录
//! - `[behavior]` 最后 pane 退出 / 异常退出策略
//! - `[[keybindings]]` key/mods/action 数组
//!
//! QuickConnect 项目列表在同目录的 `quickconnect.toml`（见
//! [`crate::core::quickconnect`]），不写进 `config.toml`。
//!
//! 主题：`configs/themes/<name>.toml` 或 `~/.config/muxterm/themes/<name>.toml`，
//! 定义 ANSI 16 色 + 背景/前景/光标。解析逻辑是纯函数，附单元测试。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ============================================================================
// 顶层配置
// ============================================================================

/// 顶层配置（Alacritty 风格）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    #[serde(default)]
    pub font: FontConfig,
    #[serde(default)]
    pub theme: ThemeConfig,
    #[serde(default)]
    pub statusbar: StatusbarConfig,
    #[serde(default)]
    pub pool: PoolConfig,
    #[serde(default)]
    pub tmux: TmuxConfig,
    #[serde(default)]
    pub ssh: SshFileConfig,
    #[serde(default)]
    pub scrollback: ScrollbackConfig,
    #[serde(default)]
    pub attention: AttentionConfig,
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub pane: PaneConfig,
    #[serde(default)]
    pub behavior: BehaviorConfig,
    #[serde(default)]
    pub keybindings: Vec<KeyBinding>,
}

/// `[font]`。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FontConfig {
    #[serde(default = "default_font_family")]
    pub family: String,
    #[serde(default = "default_font_size")]
    pub size: f32,
}
fn default_font_family() -> String {
    "Monospace".into()
}
fn default_font_size() -> f32 {
    12.0
}
impl Default for FontConfig {
    fn default() -> Self {
        FontConfig {
            family: default_font_family(),
            size: default_font_size(),
        }
    }
}

/// `[theme]`。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThemeConfig {
    #[serde(default = "default_theme")]
    pub name: String,
}
fn default_theme() -> String {
    "light".into()
}
impl Default for ThemeConfig {
    fn default() -> Self {
        ThemeConfig {
            name: default_theme(),
        }
    }
}

/// `[statusbar]`：muxterm 状态栏渲染模式。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatusbarConfig {
    /// `tmux`（默认）= 有 tmux 就跟 tmux 的 status 配置与颜色一致；
    /// `theme` = 只用 muxterm 主题黑/白渲染，忽略 tmux 配色。
    #[serde(default = "default_statusbar_mode")]
    pub mode: String,
}
fn default_statusbar_mode() -> String {
    "tmux".into()
}
impl Default for StatusbarConfig {
    fn default() -> Self {
        StatusbarConfig {
            mode: default_statusbar_mode(),
        }
    }
}

/// `[pool]`：QuickConnect warm connection pool 上限。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PoolConfig {
    /// 同时保留的 warm 连接数（同时决定 Recent 显示条数上限）。
    #[serde(default = "default_pool_max_slots")]
    pub max_slots: u32,
}
fn default_pool_max_slots() -> u32 {
    5
}
impl Default for PoolConfig {
    fn default() -> Self {
        PoolConfig {
            max_slots: default_pool_max_slots(),
        }
    }
}

/// `[tmux]`。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TmuxConfig {
    /// attach 后自动 `set -g mouse on`。
    #[serde(default = "default_auto_mouse")]
    pub auto_mouse: bool,
    /// 启动时自动 attach 的 session 名（空=不自动 attach）。
    #[serde(default)]
    pub default_session: String,
    /// tmux socket 名（`tmux -L`）；空则用默认 socket。
    /// CLI `-L/--socket` 会覆盖此字段。
    #[serde(default)]
    pub socket: String,
}
fn default_auto_mouse() -> bool {
    true
}
impl Default for TmuxConfig {
    fn default() -> Self {
        TmuxConfig {
            auto_mouse: true,
            default_session: String::new(),
            socket: String::new(),
        }
    }
}

impl TmuxConfig {
    /// 非空 socket → `["-L", name]`，供本地 tmux 子进程使用。
    pub fn socket_args(&self) -> Vec<String> {
        let sock = self.socket.trim();
        if sock.is_empty() {
            Vec::new()
        } else {
            vec!["-L".into(), sock.to_string()]
        }
    }
}

/// `[ssh]`：远程 tmux -CC 默认连接参数。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SshFileConfig {
    #[serde(default)]
    pub host: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    #[serde(default)]
    pub user: String,
    /// 私钥路径；空则尝试 ssh-agent。
    #[serde(default)]
    pub key_path: String,
}
fn default_ssh_port() -> u16 {
    22
}
impl Default for SshFileConfig {
    fn default() -> Self {
        SshFileConfig {
            host: String::new(),
            port: default_ssh_port(),
            user: String::new(),
            key_path: String::new(),
        }
    }
}

impl SshFileConfig {
    /// 是否已配置可用 host。
    pub fn is_configured(&self) -> bool {
        !self.host.trim().is_empty()
    }

    /// 转为运行时 [`crate::core::config::SshConfig`]。
    pub fn to_ssh_config(&self) -> crate::core::config::SshConfig {
        let user = if self.user.trim().is_empty() {
            std::env::var("USER").unwrap_or_else(|_| "root".into())
        } else {
            self.user.clone()
        };
        crate::core::config::SshConfig::from_file_fields(
            self.host.clone(),
            self.port,
            user,
            self.key_path.clone(),
        )
    }
}

/// `[scrollback]`。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScrollbackConfig {
    #[serde(default = "default_scrollback")]
    pub lines: u32,
}
fn default_scrollback() -> u32 {
    10000
}
impl Default for ScrollbackConfig {
    fn default() -> Self {
        ScrollbackConfig {
            lines: default_scrollback(),
        }
    }
}

/// `[attention]`：阶段 B 注意力聚合配置。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AttentionConfig {
    #[serde(default = "default_attention_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub blocked_regex: Vec<String>,
    #[serde(default = "default_attention_debounce_ms")]
    pub debounce_ms: u64,
}
fn default_attention_enabled() -> bool {
    true
}
fn default_attention_debounce_ms() -> u64 {
    100
}
impl Default for AttentionConfig {
    fn default() -> Self {
        AttentionConfig {
            enabled: default_attention_enabled(),
            blocked_regex: Vec::new(),
            debounce_ms: default_attention_debounce_ms(),
        }
    }
}

/// `[ui]`：极简布局相关。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UiConfig {
    /// tab 栏位置：`"bottom"`（默认，像 tmux）或 `"top"`。
    #[serde(default = "default_tab_bar_position")]
    pub tab_bar_position: String,
    /// tab 栏高度（像素）。
    #[serde(default = "default_tab_bar_height")]
    pub tab_bar_height: u32,
    /// 是否显示窗口标题文字（程序名 / tab 标题）。
    #[serde(default = "default_true")]
    pub show_title_bar: bool,
    /// 无边框模式（预留，GTK 侧按能力启用）。
    #[serde(default)]
    pub borderless: bool,
}
fn default_tab_bar_position() -> String {
    "bottom".into()
}
fn default_tab_bar_height() -> u32 {
    24
}
fn default_true() -> bool {
    true
}
impl Default for UiConfig {
    fn default() -> Self {
        UiConfig {
            tab_bar_position: default_tab_bar_position(),
            tab_bar_height: default_tab_bar_height(),
            show_title_bar: true,
            borderless: false,
        }
    }
}

impl UiConfig {
    /// tab 栏是否放在底部。
    pub fn tab_bar_at_bottom(&self) -> bool {
        !self.tab_bar_position.eq_ignore_ascii_case("top")
    }
}

/// `[pane]`：本地 pane 默认程序与工作目录。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PaneConfig {
    /// 默认启动命令（支持 `$SHELL`）。
    #[serde(default = "default_pane_command")]
    pub default_command: String,
    /// 初始工作目录（支持 `$HOME`）。
    #[serde(default = "default_pane_workdir")]
    pub workdir: String,
}
fn default_pane_command() -> String {
    "$SHELL".into()
}
fn default_pane_workdir() -> String {
    "$HOME".into()
}
impl Default for PaneConfig {
    fn default() -> Self {
        PaneConfig {
            default_command: default_pane_command(),
            workdir: default_pane_workdir(),
        }
    }
}

/// 最后 pane/tab 全部退出后的行为。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OnLastPaneExit {
    /// 关闭窗口（默认）。
    #[default]
    CloseWindow,
    /// 保留空窗口，等用户手动关或新建 tab。
    KeepEmpty,
    /// 旧逻辑：再开一个空 shell（已废弃，解析时仍接受）。
    NewShell,
}

/// 程序异常退出（非 0）时的附加行为。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OnProgramExitAbnormal {
    /// 关 pane，并在状态栏提示（默认）。
    #[default]
    Notify,
    /// 关 pane，不提示。
    Close,
    /// 不关 pane（保留终端输出，便于查看错误）。
    Keep,
}

/// `[behavior]`。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BehaviorConfig {
    #[serde(default)]
    pub on_last_pane_exit: OnLastPaneExit,
    #[serde(default)]
    pub on_program_exit_abnormal: OnProgramExitAbnormal,
}
impl Default for BehaviorConfig {
    fn default() -> Self {
        BehaviorConfig {
            on_last_pane_exit: OnLastPaneExit::CloseWindow,
            on_program_exit_abnormal: OnProgramExitAbnormal::Notify,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            font: FontConfig::default(),
            theme: ThemeConfig::default(),
            statusbar: StatusbarConfig::default(),
            pool: PoolConfig::default(),
            tmux: TmuxConfig::default(),
            ssh: SshFileConfig::default(),
            scrollback: ScrollbackConfig::default(),
            attention: AttentionConfig::default(),
            ui: UiConfig::default(),
            pane: PaneConfig::default(),
            behavior: BehaviorConfig::default(),
            keybindings: default_keybindings(),
        }
    }
}

/// 展开配置里的简单环境变量占位（`$SHELL` / `$HOME`）。
pub fn expand_config_value(raw: &str) -> String {
    let t = raw.trim();
    if t == "$SHELL" {
        return std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    }
    if t == "$HOME" {
        return std::env::var("HOME").unwrap_or_else(|_| "/".into());
    }
    if let Some(rest) = t.strip_prefix('$') {
        if let Ok(v) = std::env::var(rest) {
            return v;
        }
    }
    // QuickConnect/配置文件里的 `~` / `~/...`：展开成用户主目录，否则
    // tmux new-session -c / local shell cwd 会拿到字面 `~` 报 ENOENT。
    if t == "~" {
        return std::env::var("HOME").unwrap_or_else(|_| "/".into());
    }
    if let Some(rest) = t.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
        return format!("{}/{}", home.trim_end_matches('/'), rest);
    }
    t.to_string()
}

/// 从 `default_command` 解析 argv（空白分割；无空白则单元素）。
pub fn parse_command_argv(command: &str) -> Vec<String> {
    let expanded = expand_config_value(command);
    let parts: Vec<String> = expanded.split_whitespace().map(|s| s.to_string()).collect();
    if parts.is_empty() {
        vec![expand_config_value("$SHELL")]
    } else {
        parts
    }
}

/// macOS GUI applications start with a minimal environment. Launch the user's
/// default zsh/bash as a login shell so /etc/zprofile and ~/.zprofile establish
/// the same Homebrew PATH as Terminal.app. Explicit command arguments are kept.
pub fn prepare_pane_argv_for_platform(mut argv: Vec<String>, is_macos: bool) -> Vec<String> {
    if !is_macos || argv.len() != 1 {
        return argv;
    }
    let shell = program_basename(&argv[0]);
    if matches!(shell.as_str(), "zsh" | "bash") {
        argv.push("-l".into());
    }
    argv
}

/// argv[0] 的 basename，用作 pane 默认显示名。
pub fn program_basename(argv0: &str) -> String {
    Path::new(argv0)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(argv0)
        .to_string()
}

/// 解码 waitpid 风格的 status → 退出码（信号终止为 128+sig）。
pub fn decode_wait_status(status: i32) -> i32 {
    // WIFEXITED: (status & 0x7f) == 0
    if status & 0x7f == 0 {
        (status >> 8) & 0xff
    } else if ((status & 0x7f) + 1) as i8 >> 1 > 0 {
        // WIFSIGNALED
        128 + (status & 0x7f)
    } else {
        status
    }
}

/// 用户配置目录：`~/.config/muxterm`（或 `$XDG_CONFIG_HOME/muxterm`）。
pub fn user_config_dir() -> Option<PathBuf> {
    dirs_config()
}

impl Config {
    /// 用户配置文件路径：`~/.config/muxterm/config.toml`。
    pub fn user_config_path() -> Option<PathBuf> {
        user_config_dir().map(|d| d.join("config.toml"))
    }

    /// 加载：优先用户配置，缺失走默认。
    pub fn load() -> Result<Self> {
        match Self::user_config_path() {
            Some(p) if p.exists() => {
                let raw = std::fs::read_to_string(&p)
                    .with_context(|| format!("读取配置文件失败: {}", p.display()))?;
                parse_config_toml(&raw).with_context(|| format!("解析配置失败: {}", p.display()))
            }
            _ => {
                tracing::info!(target = "muxterm::config", "无用户配置，使用默认值");
                Ok(Config::default())
            }
        }
    }
}

/// 从 TOML 解析配置（纯函数）。用户未提供的字段走默认（serde default）。
pub fn parse_config_toml(raw: &str) -> Result<Config> {
    let mut cfg: Config = toml::from_str(raw).context("配置 TOML 反序列化失败")?;
    // 若用户没提供 keybindings（空数组也是「显式提供空」），补默认。
    // 注意：toml 里没写 keybindings 时 serde 用 Vec::default()（空），
    // 这里检测：如果用户没写任何 [[keybindings]]，就用默认全套。
    if cfg.keybindings.is_empty() && raw.lines().all(|l| !l.contains("[[keybindings]]")) {
        cfg.keybindings = default_keybindings();
    }
    Ok(cfg)
}

// ============================================================================
// 快捷键
// ============================================================================

/// 一条快捷键绑定。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeyBinding {
    pub key: String,
    #[serde(default)]
    pub mods: Vec<String>,
    pub action: String,
}

/// 快捷键动作（解析后用于匹配）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    NewWindow,
    NewTab,
    NewPane,
    NewPaneVertical,
    SwitchTab1,
    SwitchTab2,
    SwitchTab3,
    SwitchTab4,
    SwitchTab5,
    SwitchTab6,
    SwitchTab7,
    SwitchTab8,
    SwitchTab9,
    SwitchTabLast,
    SwitchPanePrev,
    SwitchPaneNext,
    Search,
    CommandPalette,
    QuickConnect,
    Quit,
    IncreaseFontSize,
    DecreaseFontSize,
    ResetFontSize,
    TogglePaneFullscreen,
    Copy,
    Paste,
    /// 未知动作（保留原始字符串，匹配时忽略）。
    Unknown,
}

impl Action {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s {
            "new_window" => Action::NewWindow,
            "new_tab" => Action::NewTab,
            "new_pane" => Action::NewPane,
            "new_pane_vertical" => Action::NewPaneVertical,
            "switch_tab_1" => Action::SwitchTab1,
            "switch_tab_2" => Action::SwitchTab2,
            "switch_tab_3" => Action::SwitchTab3,
            "switch_tab_4" => Action::SwitchTab4,
            "switch_tab_5" => Action::SwitchTab5,
            "switch_tab_6" => Action::SwitchTab6,
            "switch_tab_7" => Action::SwitchTab7,
            "switch_tab_8" => Action::SwitchTab8,
            "switch_tab_9" => Action::SwitchTab9,
            "switch_tab_last" => Action::SwitchTabLast,
            "switch_pane_prev" => Action::SwitchPanePrev,
            "switch_pane_next" => Action::SwitchPaneNext,
            "search" => Action::Search,
            "command_palette" => Action::CommandPalette,
            "quick_connect" => Action::QuickConnect,
            "quit" => Action::Quit,
            "increase_font_size" => Action::IncreaseFontSize,
            "decrease_font_size" => Action::DecreaseFontSize,
            "reset_font_size" => Action::ResetFontSize,
            "toggle_pane_fullscreen" => Action::TogglePaneFullscreen,
            "copy" => Action::Copy,
            "paste" => Action::Paste,
            _ => Action::Unknown,
        }
    }
}

/// 默认快捷键（Alt+N/T/D/Shift+D/1-9/0/[ ]/R/P）。
pub fn default_keybindings() -> Vec<KeyBinding> {
    vec![
        kb("n", &["alt"], "new_window"),
        kb("t", &["alt"], "new_tab"),
        // 水平 / 竖直分割：与 TUI（Alt+S / Alt+V）对齐；保留 Alt+D 兼容 ARCHITECTURE
        kb("s", &["alt"], "new_pane"),
        kb("v", &["alt"], "new_pane_vertical"),
        kb("d", &["alt"], "new_pane"),
        kb("d", &["alt", "shift"], "new_pane_vertical"),
        kb("1", &["alt"], "switch_tab_1"),
        kb("2", &["alt"], "switch_tab_2"),
        kb("3", &["alt"], "switch_tab_3"),
        kb("4", &["alt"], "switch_tab_4"),
        kb("5", &["alt"], "switch_tab_5"),
        kb("6", &["alt"], "switch_tab_6"),
        kb("7", &["alt"], "switch_tab_7"),
        kb("8", &["alt"], "switch_tab_8"),
        kb("9", &["alt"], "switch_tab_9"),
        kb("0", &["alt"], "switch_tab_last"),
        kb("[", &["alt"], "switch_pane_prev"),
        kb("]", &["alt"], "switch_pane_next"),
        kb("r", &["alt"], "search"),
        // Linux Alt+P / macOS Cmd+P：QuickConnect。不绑 Ctrl+P，留给终端「上一个」。
        kb("p", &["alt"], "quick_connect"),
        // Linux Alt+Shift+P / macOS Cmd+Shift+P：命令面板
        kb("p", &["alt", "shift"], "command_palette"),
        kb("q", &["alt"], "quick_connect"),
        kb("q", &["control"], "quit"),
        kb("plus", &["control"], "increase_font_size"),
        kb("minus", &["control"], "decrease_font_size"),
        kb("0", &["control"], "reset_font_size"),
        kb("return", &["control"], "toggle_pane_fullscreen"),
        kb("c", &["control", "shift"], "copy"),
        kb("v", &["control", "shift"], "paste"),
    ]
}

fn kb(key: &str, mods: &[&str], action: &str) -> KeyBinding {
    KeyBinding {
        key: key.into(),
        mods: mods.iter().map(|s| s.to_string()).collect(),
        action: action.into(),
    }
}

/// 平台无关修饰键（自建 bitflags，不依赖任何 GUI 库）。
///
/// GTK / AppKit / WinUI 在各自 platform 层把原生修饰键转成此类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Modifiers(pub u8);

impl Modifiers {
    pub const NONE: Modifiers = Modifiers(0);
    pub const CONTROL: Modifiers = Modifiers(0b0001);
    pub const SHIFT: Modifiers = Modifiers(0b0010);
    pub const ALT: Modifiers = Modifiers(0b0100);
    pub const SUPER: Modifiers = Modifiers(0b1000);

    /// 是否包含全部 `other` 标志位。
    pub fn contains(self, other: Modifiers) -> bool {
        self.0 & other.0 == other.0
    }

    /// 并入标志位。
    pub fn insert(&mut self, other: Modifiers) {
        self.0 |= other.0;
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// 修饰键集合（规范化小写，排序存以便 Hash/Eq）。
///
/// 用于 keybinding 查找表；与 [`Modifiers`] 可互转。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct ModSet(pub Vec<String>);

impl ModSet {
    /// 从平台无关 [`Modifiers`] 构造。
    pub fn from_modifiers(mods: Modifiers) -> Self {
        let mut v = Vec::new();
        if mods.contains(Modifiers::CONTROL) {
            v.push("control".into());
        }
        if mods.contains(Modifiers::SHIFT) {
            v.push("shift".into());
        }
        if mods.contains(Modifiers::ALT) {
            v.push("alt".into());
        }
        if mods.contains(Modifiers::SUPER) {
            v.push("super".into());
        }
        v.sort();
        ModSet(v)
    }

    pub fn from_binding(mods: &[String]) -> Self {
        let mut v: Vec<String> = mods.iter().map(|m| m.to_lowercase()).collect();
        v.sort();
        v.dedup();
        ModSet(v)
    }
}

// ============================================================================
// 主题
// ============================================================================

/// 主题：ANSI 16 色 + 背景/前景/光标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub name: String,
    pub background: Rgb,
    pub foreground: Rgb,
    pub cursor: Rgb,
    pub colors: [Rgb; 16],
}

/// sRGB 颜色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn to_u32(self) -> u32 {
        ((self.0 as u32) << 16) | ((self.1 as u32) << 8) | (self.2 as u32)
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ThemeFile {
    name: String,
    background: String,
    foreground: String,
    cursor: String,
    color0: String,
    color1: String,
    color2: String,
    color3: String,
    color4: String,
    color5: String,
    color6: String,
    color7: String,
    color8: String,
    color9: String,
    color10: String,
    color11: String,
    color12: String,
    color13: String,
    color14: String,
    color15: String,
}

impl Theme {
    pub fn load(name: &str) -> Result<Self> {
        if let Some(dir) = dirs_themes() {
            let p = dir.join(format!("{name}.toml"));
            if p.exists() {
                let raw = std::fs::read_to_string(&p)
                    .with_context(|| format!("读取主题文件失败: {}", p.display()))?;
                return parse_theme_toml(&raw);
            }
        }
        let builtin = Path::new("configs/themes").join(format!("{name}.toml"));
        if builtin.exists() {
            let raw = std::fs::read_to_string(&builtin)
                .with_context(|| format!("读取内置主题失败: {}", builtin.display()))?;
            return parse_theme_toml(&raw);
        }
        if let Some(raw) = Self::embedded(name) {
            return parse_theme_toml(raw);
        }
        anyhow::bail!("找不到主题: {name}")
    }

    /// 编译期嵌入的 light/dark，不依赖 CWD / 安装前缀。
    pub fn embedded(name: &str) -> Option<&'static str> {
        match name.trim().to_ascii_lowercase().as_str() {
            "light" => Some(include_str!("../../configs/themes/light.toml")),
            "dark" => Some(include_str!("../../configs/themes/dark.toml")),
            _ => None,
        }
    }

    /// 主题切换目标：dark ↔ light（大小写不敏感）。未知名当作 light 侧。
    pub fn toggle_target(current: &str) -> &'static str {
        if current.trim().eq_ignore_ascii_case("dark") {
            "light"
        } else {
            "dark"
        }
    }
}

pub fn parse_theme_toml(raw: &str) -> Result<Theme> {
    let f: ThemeFile = toml::from_str(raw).context("主题 TOML 反序列化失败")?;
    let colors = [
        parse_hex(&f.color0)?,
        parse_hex(&f.color1)?,
        parse_hex(&f.color2)?,
        parse_hex(&f.color3)?,
        parse_hex(&f.color4)?,
        parse_hex(&f.color5)?,
        parse_hex(&f.color6)?,
        parse_hex(&f.color7)?,
        parse_hex(&f.color8)?,
        parse_hex(&f.color9)?,
        parse_hex(&f.color10)?,
        parse_hex(&f.color11)?,
        parse_hex(&f.color12)?,
        parse_hex(&f.color13)?,
        parse_hex(&f.color14)?,
        parse_hex(&f.color15)?,
    ];
    Ok(Theme {
        name: f.name,
        background: parse_hex(&f.background)?,
        foreground: parse_hex(&f.foreground)?,
        cursor: parse_hex(&f.cursor)?,
        colors,
    })
}

pub fn parse_hex(s: &str) -> Result<Rgb> {
    // 兼容 `#rrggbb` 与 `rrggbb` 两种写法：FFI 上报 `refresh-client -r` 时
    // 传的是不带 `#` 的 hex，此前强制要求 `#` 导致上报静默失败。
    let s = s.trim().strip_prefix('#').unwrap_or(s.trim());
    if s.len() != 6 {
        anyhow::bail!("颜色应为 6 位十六进制: {s}");
    }
    let r = u8::from_str_radix(&s[0..2], 16)?;
    let g = u8::from_str_radix(&s[2..4], 16)?;
    let b = u8::from_str_radix(&s[4..6], 16)?;
    Ok(Rgb(r, g, b))
}

fn dirs_config() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|d| d.join("muxterm"))
}

fn dirs_themes() -> Option<PathBuf> {
    dirs_config().map(|d| d.join("themes"))
}

// Re-export SSH config types (defined in runtime::tmux::ssh_client)
#[allow(unused_imports)]
pub use crate::core::runtime::tmux::ssh_client::{
    parse_ssh_connect_line, parse_ssh_target, SshAuth, SshConfig, SshError,
};

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    const CONFIG_SAMPLE: &str = r##"
[font]
family = "JetBrains Mono"
size = 13.0

[theme]
name = "dark"

[tmux]
auto_mouse = true
default_session = ""

[ssh]
host = "example.com"
port = 2222
user = "alice"
key_path = "/home/alice/.ssh/id_ed25519"

[scrollback]
lines = 5000

[[keybindings]]
key = "n"
mods = ["alt"]
action = "new_window"

[[keybindings]]
key = "t"
mods = ["alt"]
action = "new_tab"
"##;

    const THEME_SAMPLE: &str = r##"
name = "Dark"
background = "#1e1e2e"
foreground = "#cdd6f4"
cursor = "#f5e0dc"
color0  = "#45475a"
color1  = "#f38ba8"
color2  = "#a6e3a1"
color3  = "#f9e2af"
color4  = "#89b4fa"
color5  = "#f5c2e7"
color6  = "#94e2d5"
color7  = "#bac2de"
color8  = "#585b70"
color9  = "#f38ba8"
color10 = "#a6e3a1"
color11 = "#f9e2af"
color12 = "#89b4fa"
color13 = "#f5c2e7"
color14 = "#94e2d5"
color15 = "#a6adc8"
"##;

    #[test]
    fn parse_config_full() {
        let c = parse_config_toml(CONFIG_SAMPLE).unwrap();
        assert_eq!(c.font.family, "JetBrains Mono");
        assert_eq!(c.font.size, 13.0);
        assert_eq!(c.theme.name, "dark"); // 样例文件显式指定 dark
        assert!(c.tmux.auto_mouse);
        assert_eq!(c.ssh.host, "example.com");
        assert_eq!(c.ssh.port, 2222);
        assert_eq!(c.ssh.user, "alice");
        assert!(c.ssh.key_path.ends_with("id_ed25519"));
        assert_eq!(c.scrollback.lines, 5000);
        // 用户只写了 2 条 keybindings，应保留用户的（不补默认）
        assert_eq!(c.keybindings.len(), 2);
        assert_eq!(c.keybindings[0].action, "new_window");
    }

    #[test]
    fn parse_config_empty_uses_defaults() {
        let c = parse_config_toml("").unwrap();
        assert_eq!(c.font.family, "Monospace");
        assert_eq!(c.font.size, 12.0);
        assert_eq!(c.theme.name, "light"); // 默认浅色
        assert!(c.tmux.auto_mouse);
        assert_eq!(c.scrollback.lines, 10000);
        // 空 keybindings → 补默认全套
        assert_eq!(c.keybindings.len(), default_keybindings().len());
    }

    #[test]
    fn user_config_dir_contains_config_toml() {
        match (user_config_dir(), Config::user_config_path()) {
            (Some(dir), Some(file)) => {
                assert_eq!(file, dir.join("config.toml"));
                assert!(dir.ends_with("muxterm"));
            }
            (None, None) => {}
            other => panic!("user_config_dir 与 user_config_path 应同时有或同时无: {other:?}"),
        }
    }

    #[test]
    fn parse_config_partial() {
        let c = parse_config_toml("[font]\nsize = 20.0\n").unwrap();
        assert_eq!(c.font.size, 20.0);
        assert_eq!(c.font.family, "Monospace"); // 默认
    }

    #[test]
    fn parse_config_tmux_section() {
        let c =
            parse_config_toml("[tmux]\nauto_mouse = false\ndefault_session = \"main\"\n").unwrap();
        assert!(!c.tmux.auto_mouse);
        assert_eq!(c.tmux.default_session, "main");
        assert_eq!(c.tmux.socket, "");
    }

    /// `[tmux].socket`（socket 配置）反序列化 + `socket_args()`。
    #[test]
    fn tmux_socket_config_deserialize() {
        let c = parse_config_toml("[tmux]\nsocket = \"muxterm-dev\"\n").unwrap();
        assert_eq!(c.tmux.socket, "muxterm-dev");
        assert_eq!(
            c.tmux.socket_args(),
            vec!["-L".to_string(), "muxterm-dev".to_string()]
        );
    }

    /// `[tmux].socket` 序列化往返（serde TOML）。
    #[test]
    fn tmux_socket_config_serialize_roundtrip() {
        let original = TmuxConfig {
            auto_mouse: true,
            default_session: "main".into(),
            socket: "iso".into(),
        };
        let encoded = toml::to_string(&original).expect("serialize TmuxConfig");
        assert!(
            encoded.contains("socket") && encoded.contains("iso"),
            "encoded={encoded}"
        );
        let decoded: TmuxConfig = toml::from_str(&encoded).expect("deserialize TmuxConfig");
        assert_eq!(decoded, original);
        assert_eq!(
            decoded.socket_args(),
            vec!["-L".to_string(), "iso".to_string()]
        );
    }

    #[test]
    fn tmux_socket_args_empty_and_trim() {
        assert!(TmuxConfig::default().socket_args().is_empty());
        let spaced = TmuxConfig {
            socket: "  mux  ".into(),
            ..Default::default()
        };
        assert_eq!(
            spaced.socket_args(),
            vec!["-L".to_string(), "mux".to_string()]
        );
        let blank = TmuxConfig {
            socket: "   ".into(),
            ..Default::default()
        };
        assert!(blank.socket_args().is_empty());
    }

    #[test]
    fn parse_config_ssh_section() {
        let c = parse_config_toml(
            r#"[ssh]
host = "box"
port = 2200
user = "bob"
key_path = "~/.ssh/id_rsa"
"#,
        )
        .unwrap();
        assert!(c.ssh.is_configured());
        assert_eq!(c.ssh.port, 2200);
        let runtime = c.ssh.to_ssh_config();
        assert_eq!(runtime.host, "box");
        assert_eq!(runtime.user, "bob");
        assert_eq!(runtime.port, 2200);
    }

    #[test]
    fn parse_config_ssh_defaults_when_absent() {
        let c = parse_config_toml("").unwrap();
        assert!(!c.ssh.is_configured());
        assert_eq!(c.ssh.port, 22);
    }

    #[test]
    fn parse_theme_full() {
        let t = parse_theme_toml(THEME_SAMPLE).unwrap();
        assert_eq!(t.name, "Dark");
        assert_eq!(t.background, parse_hex("#1e1e2e").unwrap());
        assert_eq!(t.foreground, parse_hex("#cdd6f4").unwrap());
        assert_eq!(t.colors.len(), 16);
        assert_eq!(t.colors[15], parse_hex("#a6adc8").unwrap());
    }

    #[test]
    fn parse_hex_valid_and_invalid() {
        assert_eq!(parse_hex("#000000").unwrap(), Rgb(0, 0, 0));
        assert_eq!(parse_hex("#ffffff").unwrap(), Rgb(255, 255, 255));
        // FFI 上报颜色时可能不带 `#`，两种写法都接受。
        assert_eq!(parse_hex("000000").unwrap(), Rgb(0, 0, 0));
        assert_eq!(parse_hex("cdd6f4").unwrap(), Rgb(0xcd, 0xd6, 0xf4));
        assert!(parse_hex("#fff").is_err());
        assert!(parse_hex("#zz0000").is_err());
    }

    #[test]
    fn default_keybindings_complete() {
        let kb = default_keybindings();
        // 应包含所有核心动作
        let actions: HashSet<_> = kb.iter().map(|k| k.action.as_str()).collect();
        for a in [
            "new_window",
            "new_tab",
            "new_pane",
            "new_pane_vertical",
            "switch_tab_1",
            "switch_tab_last",
            "switch_pane_prev",
            "switch_pane_next",
            "search",
            "command_palette",
            "quick_connect",
            "quit",
            "increase_font_size",
            "toggle_pane_fullscreen",
            "copy",
            "paste",
        ] {
            assert!(actions.contains(a), "缺少默认动作 {a}");
        }
        // 9 个数字 tab
        let n: Vec<_> = kb
            .iter()
            .filter(|k| k.action.starts_with("switch_tab_"))
            .map(|k| k.key.clone())
            .collect();
        assert!(n.contains(&"1".into()) && n.contains(&"9".into()));
    }

    #[test]
    fn action_from_str_known() {
        assert_eq!(Action::from_str("new_window"), Action::NewWindow);
        assert_eq!(
            Action::from_str("new_pane_vertical"),
            Action::NewPaneVertical
        );
        assert_eq!(Action::from_str("switch_tab_last"), Action::SwitchTabLast);
        assert_eq!(Action::from_str("quick_connect"), Action::QuickConnect);
        assert_eq!(Action::from_str("quit"), Action::Quit);
        assert_eq!(Action::from_str("copy"), Action::Copy);
        assert_eq!(Action::from_str("paste"), Action::Paste);
        assert_eq!(
            Action::from_str("increase_font_size"),
            Action::IncreaseFontSize
        );
        assert_eq!(
            Action::from_str("toggle_pane_fullscreen"),
            Action::TogglePaneFullscreen
        );
        assert_eq!(Action::from_str("nonsense"), Action::Unknown);
    }

    #[test]
    fn modset_from_binding_normalizes_case() {
        let ms = ModSet::from_binding(&["Alt".into(), "SHIFT".into()]);
        assert!(ms.0.contains(&"alt".to_string()));
        assert!(ms.0.contains(&"shift".to_string()));
    }

    #[test]
    fn modifiers_bitflags_and_modset_roundtrip() {
        let mut m = Modifiers::NONE;
        m.insert(Modifiers::ALT);
        m.insert(Modifiers::CONTROL);
        assert!(m.contains(Modifiers::ALT));
        assert!(m.contains(Modifiers::CONTROL));
        assert!(!m.contains(Modifiers::SHIFT));
        let ms = ModSet::from_modifiers(m);
        assert!(ms.0.contains(&"alt".to_string()));
        assert!(ms.0.contains(&"control".to_string()));
        assert_eq!(ms.0.len(), 2);
    }

    #[test]
    fn builtin_themes_parse() {
        let dark = std::fs::read_to_string("configs/themes/dark.toml").unwrap();
        let t = parse_theme_toml(&dark).unwrap();
        assert_eq!(t.name, "Dark");
        assert_eq!(t.colors.len(), 16);

        let light = std::fs::read_to_string("configs/themes/light.toml").unwrap();
        let t = parse_theme_toml(&light).unwrap();
        assert_eq!(t.name, "Light");
        assert_eq!(t.colors.len(), 16);
    }

    #[test]
    fn embedded_themes_load_without_cwd_files() {
        let light = parse_theme_toml(Theme::embedded("light").expect("light")).unwrap();
        let dark = parse_theme_toml(Theme::embedded("DARK").expect("dark")).unwrap();
        assert_eq!(light.background, parse_hex("#eff1f5").unwrap());
        assert_eq!(dark.background, parse_hex("#1e1e2e").unwrap());
        let loaded = Theme::load("light").unwrap();
        assert_eq!(loaded.background, light.background);
    }

    #[test]
    fn toggle_target_is_case_insensitive() {
        assert_eq!(Theme::toggle_target("dark"), "light");
        assert_eq!(Theme::toggle_target("Dark"), "light");
        assert_eq!(Theme::toggle_target("light"), "dark");
        assert_eq!(Theme::toggle_target("Light"), "dark");
        assert_eq!(Theme::toggle_target("fallback"), "dark");
    }

    #[test]
    fn default_keybindings_alt_p_is_quick_connect() {
        let kb = default_keybindings();
        let find = |key: &str, mods: &[&str], action: &str| {
            kb.iter().any(|b| {
                b.key == key
                    && ModSet::from_binding(&b.mods)
                        == ModSet::from_binding(
                            &mods.iter().map(|m| m.to_string()).collect::<Vec<_>>(),
                        )
                    && b.action == action
            })
        };
        let bound = |key: &str, mods: &[&str]| {
            kb.iter().any(|b| {
                b.key == key
                    && ModSet::from_binding(&b.mods)
                        == ModSet::from_binding(
                            &mods.iter().map(|m| m.to_string()).collect::<Vec<_>>(),
                        )
            })
        };
        assert!(find("p", &["alt"], "quick_connect"));
        assert!(find("p", &["alt", "shift"], "command_palette"));
        // Ctrl+P 必须透传给终端（readline 上一个）
        assert!(!bound("p", &["control"]));
        assert!(!bound("p", &["control", "shift"]));
        assert!(!bound("p", &["super"]));
        assert!(!bound("p", &["super", "shift"]));
        assert!(find("c", &["control", "shift"], "copy"));
        assert!(find("v", &["control", "shift"], "paste"));
        // Ctrl+C / Ctrl+V 必须透传（SIGINT / 程序自己的粘贴）
        assert!(!bound("c", &["control"]));
        assert!(!bound("v", &["control"]));
    }

    #[test]
    fn modset_alt_shift_matches_from_modifiers() {
        let binding = ModSet::from_binding(&["alt".into(), "shift".into()]);
        let mut m = Modifiers::NONE;
        m.insert(Modifiers::ALT);
        m.insert(Modifiers::SHIFT);
        assert_eq!(ModSet::from_modifiers(m), binding);
    }

    #[test]
    fn parse_ui_pane_behavior_sections() {
        let raw = r##"
[ui]
tab_bar_position = "top"
tab_bar_height = 28
show_title_bar = false
borderless = true

[pane]
default_command = "/bin/zsh"
workdir = "/tmp"

[behavior]
on_last_pane_exit = "keep_empty"
on_program_exit_abnormal = "close"
"##;
        let c = parse_config_toml(raw).unwrap();
        assert_eq!(c.ui.tab_bar_position, "top");
        assert_eq!(c.ui.tab_bar_height, 28);
        assert!(!c.ui.show_title_bar);
        assert!(c.ui.borderless);
        assert!(!c.ui.tab_bar_at_bottom());
        assert_eq!(c.pane.default_command, "/bin/zsh");
        assert_eq!(c.pane.workdir, "/tmp");
        assert_eq!(c.behavior.on_last_pane_exit, OnLastPaneExit::KeepEmpty);
        assert_eq!(
            c.behavior.on_program_exit_abnormal,
            OnProgramExitAbnormal::Close
        );
    }

    #[test]
    fn parse_ui_pane_behavior_defaults() {
        let c = parse_config_toml("").unwrap();
        assert_eq!(c.ui.tab_bar_position, "bottom");
        assert_eq!(c.ui.tab_bar_height, 24);
        assert!(c.ui.show_title_bar);
        assert!(!c.ui.borderless);
        assert!(c.ui.tab_bar_at_bottom());
        assert_eq!(c.pane.default_command, "$SHELL");
        assert_eq!(c.pane.workdir, "$HOME");
        assert_eq!(c.behavior.on_last_pane_exit, OnLastPaneExit::CloseWindow);
        assert_eq!(
            c.behavior.on_program_exit_abnormal,
            OnProgramExitAbnormal::Notify
        );
    }

    #[test]
    fn expand_and_parse_command() {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        assert_eq!(expand_config_value("$SHELL"), shell);
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
        assert_eq!(expand_config_value("$HOME"), home);
        assert_eq!(expand_config_value("~"), home);
        assert_eq!(
            expand_config_value("~/Developer/muxterm"),
            format!("{}/Developer/muxterm", home.trim_end_matches('/'))
        );
        assert_eq!(expand_config_value("/bin/bash"), "/bin/bash");
        assert_eq!(
            parse_command_argv("/usr/bin/python3 script.py"),
            vec!["/usr/bin/python3".to_string(), "script.py".to_string()]
        );
        assert_eq!(program_basename("/usr/bin/bash"), "bash");
        assert_eq!(program_basename("vim"), "vim");
    }

    #[test]
    fn decode_wait_status_exited_and_signaled() {
        // exit 0 → status 0
        assert_eq!(decode_wait_status(0), 0);
        // exit 1 → ((1) << 8)
        assert_eq!(decode_wait_status(1 << 8), 1);
        // exit 42
        assert_eq!(decode_wait_status(42 << 8), 42);
        // SIGTERM (15) → 128+15
        assert_eq!(decode_wait_status(15), 128 + 15);
    }

    /// 对应：无配置文件时应走默认值（load 路径缺失）。
    #[test]
    fn test_config_default_snapshot() {
        let c = Config::default();
        assert_eq!(c.font.family, "Monospace");
        assert_eq!(c.font.size, 12.0);
        assert_eq!(c.theme.name, "light");
        assert!(c.tmux.auto_mouse);
        assert_eq!(c.scrollback.lines, 10000);
        assert!(c.attention.enabled);
        assert_eq!(c.attention.debounce_ms, 100);
        assert!(c.attention.blocked_regex.is_empty());
        assert_eq!(c.behavior.on_last_pane_exit, OnLastPaneExit::CloseWindow);
        assert_eq!(c.pane.default_command, "$SHELL");
        assert_eq!(c.keybindings, default_keybindings());
    }

    /// 对应：自定义字体/主题/快捷键覆盖默认。
    #[test]
    fn test_config_custom_overrides_font_theme_keys() {
        let raw = r##"
[font]
family = "Fira Code"
size = 14.5
[theme]
name = "dark"
[[keybindings]]
key = "n"
mods = ["alt"]
action = "new_tab"
"##;
        let c = parse_config_toml(raw).unwrap();
        assert_eq!(c.font.family, "Fira Code");
        assert_eq!(c.font.size, 14.5);
        assert_eq!(c.theme.name, "dark");
        assert_eq!(c.keybindings.len(), 1);
        assert_eq!(c.keybindings[0].action, "new_tab");
    }

    /// 对应：残缺合法 TOML 缺失字段回落默认（不是整份失败）。
    #[test]
    fn test_config_partial_toml_falls_back_fields() {
        let c = parse_config_toml("[scrollback]\nlines = 123\n").unwrap();
        assert_eq!(c.scrollback.lines, 123);

        let c = parse_config_toml(
            "[attention]\nenabled = false\ndebounce_ms = 0\nblocked_regex = [\"ask\", \"confirm?\"]\n",
        )
        .unwrap();
        assert!(!c.attention.enabled);
        assert_eq!(c.attention.debounce_ms, 0);
        assert_eq!(c.attention.blocked_regex, vec!["ask", "confirm?"]);
        assert_eq!(c.font.family, "Monospace");
        assert_eq!(c.theme.name, "light");
        assert!(!c.keybindings.is_empty()); // 未写 [[keybindings]] → 补默认
    }

    /// 对应：非法 TOML 语法应报错，不能 silently 吞掉。
    #[test]
    fn test_config_invalid_toml_errors() {
        assert!(parse_config_toml("[[[not valid").is_err());
        assert!(parse_config_toml("font = ???").is_err());
    }

    /// 对应：用户写了 [[keybindings]] 后只保留用户条目，不混入默认全套。
    #[test]
    fn test_config_user_keybindings_replace_defaults() {
        let raw = "[[keybindings]]\nkey = \"x\"\nmods = []\naction = \"search\"\n";
        let c = parse_config_toml(raw).unwrap();
        assert_eq!(c.keybindings.len(), 1);
        assert_eq!(c.keybindings[0].key, "x");
        assert_eq!(c.keybindings[0].action, "search");
    }

    #[test]
    fn test_config_rgb_to_u32_packing() {
        assert_eq!(Rgb(0x12, 0x34, 0x56).to_u32(), 0x0012_3456);
        assert_eq!(Rgb(0xff, 0x00, 0xaa).to_u32(), 0x00ff_00aa);
    }

    #[test]
    fn test_config_tab_bar_position_helpers() {
        let mut ui = UiConfig {
            tab_bar_position: "bottom".into(),
            ..Default::default()
        };
        assert!(ui.tab_bar_at_bottom());
        ui.tab_bar_position = "TOP".into();
        assert!(!ui.tab_bar_at_bottom());
        ui.tab_bar_position = "Bottom".into();
        assert!(ui.tab_bar_at_bottom());
    }

    #[test]
    fn test_config_program_basename_paths() {
        // 对应：标题栏/tab 名从路径抽 basename
        assert_eq!(program_basename("/usr/bin/bash"), "bash");
        assert_eq!(program_basename("/usr/local/bin/opencode"), "opencode");
        assert_eq!(program_basename("python3"), "python3");
        assert_eq!(program_basename(""), "");
    }

    #[test]
    fn macos_default_shell_uses_login_mode() {
        assert_eq!(
            prepare_pane_argv_for_platform(vec!["/bin/zsh".into()], true),
            vec!["/bin/zsh".to_string(), "-l".to_string()]
        );
    }

    #[test]
    fn explicit_shell_arguments_are_preserved() {
        assert_eq!(
            prepare_pane_argv_for_platform(vec!["/bin/zsh".into(), "-f".into()], true),
            vec!["/bin/zsh".to_string(), "-f".to_string()]
        );
    }

    #[test]
    fn test_config_parse_command_argv_empty_uses_shell() {
        let shell = expand_config_value("$SHELL");
        assert_eq!(parse_command_argv(""), vec![shell.clone()]);
        assert_eq!(parse_command_argv("   "), vec![shell]);
    }

    #[test]
    fn test_config_behavior_new_shell_deprecated_variant() {
        let c = parse_config_toml("[behavior]\non_last_pane_exit = \"new_shell\"\n").unwrap();
        assert_eq!(c.behavior.on_last_pane_exit, OnLastPaneExit::NewShell);
    }
}
