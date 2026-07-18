//! 配置与主题加载。
//!
//! - `Config`：从 `~/.config/muxterm/config.toml` 读取（缺失走默认值）。
//! - `Theme`：从 `configs/themes/<name>.toml` 或用户主题目录读取，定义 ANSI 16 色
//!   + 背景/前景/光标颜色。
//!
//! 解析逻辑是纯函数（`parse_config_toml` / `parse_theme_toml`），便于单元测试。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ============================================================================
// Config
// ============================================================================

/// 顶层配置。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    #[serde(default)]
    pub terminal: TerminalConfig,
    #[serde(default)]
    pub tmux: TmuxConfig,
}

/// `[terminal]` 段。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalConfig {
    #[serde(default = "default_font_family")]
    pub font_family: String,
    #[serde(default = "default_font_size")]
    pub font_size: u32,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_scrollback")]
    pub scrollback_lines: u32,
}

/// `[tmux]` 段。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TmuxConfig {
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub session_name: String,
}

fn default_font_family() -> String {
    "Monospace".into()
}
fn default_font_size() -> u32 {
    12
}
fn default_theme() -> String {
    "dark".into()
}
fn default_scrollback() -> u32 {
    10000
}
fn default_mode() -> String {
    "local".into()
}

impl Default for Config {
    fn default() -> Self {
        Config {
            terminal: TerminalConfig::default(),
            tmux: TmuxConfig::default(),
        }
    }
}

impl Default for TerminalConfig {
    fn default() -> Self {
        TerminalConfig {
            font_family: default_font_family(),
            font_size: default_font_size(),
            theme: default_theme(),
            scrollback_lines: default_scrollback(),
        }
    }
}

impl Default for TmuxConfig {
    fn default() -> Self {
        TmuxConfig {
            mode: default_mode(),
            session_name: String::new(),
        }
    }
}

impl Config {
    /// 返回用户配置文件路径：`~/.config/muxterm/config.toml`。
    pub fn user_config_path() -> Option<PathBuf> {
        dirs_config().map(|d| d.join("config.toml"))
    }

    /// 加载配置：优先用户配置，缺失则返回默认。
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

/// 从 TOML 文本解析配置（纯函数，便于测试）。
pub fn parse_config_toml(raw: &str) -> Result<Config> {
    let cfg: Config = toml::from_str(raw).context("配置 TOML 反序列化失败")?;
    Ok(cfg)
}

// ============================================================================
// Theme
// ============================================================================

/// 主题：ANSI 16 色 + 背景/前景/光标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub name: String,
    /// 背景色（RGB）。
    pub background: Rgb,
    /// 前景色（RGB）。
    pub foreground: Rgb,
    /// 光标色（RGB）。
    pub cursor: Rgb,
    /// ANSI 0..=15。
    pub colors: [Rgb; 16],
}

/// sRGB 颜色（0..=255 每通道）。
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
    /// 从主题目录与主题名加载，优先用户主题，再回退内置目录。
    pub fn load(name: &str) -> Result<Self> {
        // 用户主题目录：~/.config/muxterm/themes/<name>.toml
        if let Some(dir) = dirs_themes() {
            let p = dir.join(format!("{name}.toml"));
            if p.exists() {
                let raw = std::fs::read_to_string(&p)
                    .with_context(|| format!("读取主题文件失败: {}", p.display()))?;
                return parse_theme_toml(&raw)
                    .with_context(|| format!("解析主题失败: {}", p.display()));
            }
        }
        // 内置主题目录（repo 内 configs/themes）—— 运行时按相对路径找
        let builtin = Path::new("configs/themes").join(format!("{name}.toml"));
        if builtin.exists() {
            let raw = std::fs::read_to_string(&builtin)
                .with_context(|| format!("读取内置主题失败: {}", builtin.display()))?;
            return parse_theme_toml(&raw)
                .with_context(|| format!("解析内置主题失败: {}", builtin.display()));
        }
        anyhow::bail!("找不到主题: {name}（既不在用户主题目录，也不在 configs/themes/）")
    }
}

/// 解析主题 TOML（纯函数）。
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

/// 解析 `#rrggbb` 为 `Rgb`。
pub fn parse_hex(s: &str) -> Result<Rgb> {
    let s = s
        .trim()
        .strip_prefix('#')
        .ok_or_else(|| anyhow::anyhow!("颜色缺少 # 前缀: {s}"))?;
    if s.len() != 6 {
        anyhow::bail!("颜色应为 6 位十六进制: {s}");
    }
    let r =
        u8::from_str_radix(&s[0..2], 16).with_context(|| format!("红色通道非法: {}", &s[0..2]))?;
    let g =
        u8::from_str_radix(&s[2..4], 16).with_context(|| format!("绿色通道非法: {}", &s[2..4]))?;
    let b =
        u8::from_str_radix(&s[4..6], 16).with_context(|| format!("蓝色通道非法: {}", &s[4..6]))?;
    Ok(Rgb(r, g, b))
}

// ============================================================================
// 目录辅助（不依赖外部 dirs crate）
// ============================================================================

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

    const CONFIG_SAMPLE: &str = r#"
[terminal]
font_family = "JetBrains Mono"
font_size = 14
theme = "dark"
scrollback_lines = 5000

[tmux]
mode = "local"
session_name = "main"
"#;

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
        assert_eq!(c.terminal.font_family, "JetBrains Mono");
        assert_eq!(c.terminal.font_size, 14);
        assert_eq!(c.terminal.theme, "dark");
        assert_eq!(c.terminal.scrollback_lines, 5000);
        assert_eq!(c.tmux.mode, "local");
        assert_eq!(c.tmux.session_name, "main");
    }

    #[test]
    fn parse_config_empty_uses_defaults() {
        let c = parse_config_toml("").unwrap();
        assert_eq!(c.terminal.font_family, "Monospace");
        assert_eq!(c.terminal.font_size, 12);
        assert_eq!(c.terminal.theme, "dark");
        assert_eq!(c.terminal.scrollback_lines, 10000);
        assert_eq!(c.tmux.mode, "local");
        assert!(c.tmux.session_name.is_empty());
    }

    #[test]
    fn parse_config_partial_section() {
        let c = parse_config_toml("[terminal]\nfont_size = 20\n").unwrap();
        assert_eq!(c.terminal.font_size, 20);
        // 缺失字段走默认
        assert_eq!(c.terminal.font_family, "Monospace");
        assert_eq!(c.terminal.theme, "dark");
    }

    #[test]
    fn parse_theme_full() {
        let t = parse_theme_toml(THEME_SAMPLE).unwrap();
        assert_eq!(t.name, "Dark");
        assert_eq!(t.background, parse_hex("#1e1e2e").unwrap());
        assert_eq!(t.foreground, parse_hex("#cdd6f4").unwrap());
        assert_eq!(t.cursor, parse_hex("#f5e0dc").unwrap());
        assert_eq!(t.colors[0], parse_hex("#45475a").unwrap());
        assert_eq!(t.colors[15], parse_hex("#a6adc8").unwrap());
        assert_eq!(t.colors.len(), 16);
    }

    #[test]
    fn parse_hex_valid() {
        assert_eq!(parse_hex("#000000").unwrap(), Rgb(0, 0, 0));
        assert_eq!(parse_hex("#ffffff").unwrap(), Rgb(255, 255, 255));
        assert_eq!(parse_hex("#1e1e2e").unwrap(), Rgb(0x1e, 0x1e, 0x2e));
    }

    #[test]
    fn parse_hex_invalid_prefix() {
        assert!(parse_hex("000000").is_err());
    }

    #[test]
    fn parse_hex_invalid_length() {
        assert!(parse_hex("#fff").is_err());
        assert!(parse_hex("#fffffff").is_err());
    }

    #[test]
    fn parse_hex_invalid_digit() {
        assert!(parse_hex("#zz0000").is_err());
    }

    #[test]
    fn rgb_to_u32() {
        assert_eq!(Rgb(0, 0, 0).to_u32(), 0x000000);
        assert_eq!(Rgb(0xff, 0x00, 0x00).to_u32(), 0xff0000);
        assert_eq!(Rgb(0x1e, 0x1e, 0x2e).to_u32(), 0x1e1e2e);
    }

    #[test]
    fn theme_missing_color_errors() {
        let raw = r##"
name = "Bad"
background = "#000000"
foreground = "#ffffff"
cursor = "#ffffff"
color0 = "#000000"
"##;
        assert!(parse_theme_toml(raw).is_err());
    }

    #[test]
    fn builtin_themes_parse() {
        // 从仓库根目录加载内置主题，确认示例文件格式正确
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
