//! 配置与主题加载（Alacritty 风格）。
//!
//! 顶层 `~/.config/muxterm/config.toml`：
//! - `[font]` family / size
//! - `[theme]` name
//! - `[tmux]` auto_mouse / default_session
//! - `[scrollback]` lines
//! - `[[keybindings]]` key/mods/action 数组
//!
//! 主题：`configs/themes/<name>.toml` 或 `~/.config/muxterm/themes/<name>.toml`，
//! 定义 ANSI 16 色 + 背景/前景/光标。解析逻辑是纯函数，附单元测试。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
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
    pub tmux: TmuxConfig,
    #[serde(default)]
    pub scrollback: ScrollbackConfig,
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
    "dark".into()
}
impl Default for ThemeConfig {
    fn default() -> Self {
        ThemeConfig {
            name: default_theme(),
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
}
fn default_auto_mouse() -> bool {
    true
}
impl Default for TmuxConfig {
    fn default() -> Self {
        TmuxConfig {
            auto_mouse: true,
            default_session: String::new(),
        }
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

impl Default for Config {
    fn default() -> Self {
        Config {
            font: FontConfig::default(),
            theme: ThemeConfig::default(),
            tmux: TmuxConfig::default(),
            scrollback: ScrollbackConfig::default(),
            keybindings: default_keybindings(),
        }
    }
}

impl Config {
    /// 用户配置文件路径：`~/.config/muxterm/config.toml`。
    pub fn user_config_path() -> Option<PathBuf> {
        dirs_config().map(|d| d.join("config.toml"))
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
    /// 未知动作（保留原始字符串，匹配时忽略）。
    Unknown,
}

impl Action {
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
            _ => Action::Unknown,
        }
    }
}

/// 默认快捷键（Alt+N/T/D/Shift+D/1-9/0/[ ]/R/P）。
pub fn default_keybindings() -> Vec<KeyBinding> {
    vec![
        kb("n", &["alt"], "new_window"),
        kb("t", &["alt"], "new_tab"),
        kb("d", &["alt"], "new_pane"),
        kb("D", &["alt", "shift"], "new_pane_vertical"),
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
        kb("p", &["alt"], "command_palette"),
    ]
}

fn kb(key: &str, mods: &[&str], action: &str) -> KeyBinding {
    KeyBinding {
        key: key.into(),
        mods: mods.iter().map(|s| s.to_string()).collect(),
        action: action.into(),
    }
}

/// 修饰键集合（规范化小写）。
/// 修饰键集合（规范化小写，排序存以便 Hash/Eq）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct ModSet(pub Vec<String>);

impl ModSet {
    pub fn from_gdk(mods: gtk4::gdk::ModifierType) -> Self {
        let mut s = HashSet::new();
        use gtk4::gdk::ModifierType as M;
        if mods.contains(M::CONTROL_MASK) {
            s.insert("control".into());
        }
        if mods.contains(M::SHIFT_MASK) {
            s.insert("shift".into());
        }
        if mods.contains(M::ALT_MASK) {
            s.insert("alt".into());
        }
        if mods.contains(M::SUPER_MASK) {
            s.insert("super".into());
        }
        let mut v: Vec<String> = s.into_iter().collect();
        v.sort();
        v.dedup();
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
        anyhow::bail!("找不到主题: {name}")
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
    let s = s
        .trim()
        .strip_prefix('#')
        .ok_or_else(|| anyhow::anyhow!("颜色缺少 # 前缀: {s}"))?;
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

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG_SAMPLE: &str = r##"
[font]
family = "JetBrains Mono"
size = 13.0

[theme]
name = "dark"

[tmux]
auto_mouse = true
default_session = ""

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
        assert_eq!(c.theme.name, "dark");
        assert!(c.tmux.auto_mouse);
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
        assert_eq!(c.theme.name, "dark");
        assert!(c.tmux.auto_mouse);
        assert_eq!(c.scrollback.lines, 10000);
        // 空 keybindings → 补默认全套
        assert_eq!(c.keybindings.len(), default_keybindings().len());
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
        assert!(parse_hex("000000").is_err());
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
        assert_eq!(Action::from_str("nonsense"), Action::Unknown);
    }

    #[test]
    fn modset_from_binding_normalizes_case() {
        let ms = ModSet::from_binding(&["Alt".into(), "SHIFT".into()]);
        assert!(ms.0.contains(&"alt".to_string()));
        assert!(ms.0.contains(&"shift".to_string()));
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
}
