use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

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
        let name = Self::resolve_name(name);
        if let Some(dir) = super::dirs_themes() {
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
        if let Some(raw) = Self::embedded(&name) {
            return parse_theme_toml(raw);
        }
        anyhow::bail!("找不到主题: {name}")
    }

    /// 编译期嵌入的 light/dark，不依赖 CWD / 安装前缀。
    pub fn embedded(name: &str) -> Option<&'static str> {
        match name.trim().to_ascii_lowercase().as_str() {
            "light" => Some(include_str!("../../../configs/themes/light.toml")),
            "dark" => Some(include_str!("../../../configs/themes/dark.toml")),
            "white" => Some(include_str!("../../../configs/themes/white.toml")),
            "black" => Some(include_str!("../../../configs/themes/black.toml")),
            _ => None,
        }
    }

    /// Resolve the portable `system` name without coupling core to GTK/AppKit.
    /// Platform launchers can set `MUXTERM_THEME=black|white` when they know
    /// the native appearance; Linux also honours the conventional GTK_THEME.
    pub fn resolve_name(name: &str) -> String {
        let normalized = name.trim().to_ascii_lowercase();
        if normalized != "system" {
            return normalized;
        }
        if let Ok(value) = std::env::var("MUXTERM_THEME") {
            let value = value.trim().to_ascii_lowercase();
            if matches!(value.as_str(), "black" | "dark") {
                return "black".into();
            }
            if matches!(value.as_str(), "white" | "light") {
                return "white".into();
            }
        }
        if let Ok(value) = std::env::var("GTK_THEME") {
            if value.to_ascii_lowercase().contains("dark") {
                return "black".into();
            }
        }
        // White is the deterministic fallback when no platform appearance is
        // available (headless CLI, first launch, or a minimal environment).
        "white".into()
    }

    /// 主题切换目标：dark ↔ light（大小写不敏感）。未知名当作 light 侧。
    pub fn toggle_target(current: &str) -> &'static str {
        match current.trim().to_ascii_lowercase().as_str() {
            "black" => "white",
            "white" => "black",
            "dark" => "light",
            "light" => "dark",
            _ => "dark",
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
