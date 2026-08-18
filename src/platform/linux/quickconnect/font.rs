//! 终端字体设置与缩放（纯逻辑）。
//!
//! `[font] family/size` 来自统一 config.toml；运行期 Ctrl+/-/0 缩放字号
//! 通过 Core SettingsService 写回 config.toml。`Preferences` 类型只保留
//! 旧 `preferences.toml` 的解析形状，供 Core 迁移读取，不再作为运行期来源。

use std::path::PathBuf;

/// 终端字体设置。
#[derive(Debug, Clone, PartialEq)]
pub struct FontSettings {
    pub family: String,
    pub size: f32,
    pub fallback: Vec<String>,
}

impl Default for FontSettings {
    fn default() -> Self {
        FontSettings {
            family: "JetBrains Mono".into(),
            size: 12.0,
            fallback: vec!["Noto Sans Mono".into(), "monospace".into()],
        }
    }
}

pub const MIN_FONT_SIZE: f32 = 9.0;
pub const MAX_FONT_SIZE: f32 = 36.0;
pub const ZOOM_STEP: f32 = 1.0;

impl FontSettings {
    pub fn clamp_size(size: f32) -> f32 {
        size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
    }

    /// 从当前字号按方向缩放（+1 增大 / -1 减小），并夹在合法区间。
    pub fn zoomed(size: f32, direction: i32) -> f32 {
        Self::clamp_size(size + direction as f32 * ZOOM_STEP)
    }
}

/// 运行期偏好（主题 / status bar 模式 / 字号），覆盖 config.toml。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Preferences {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statusbar_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f32>,
}

/// 用户偏好文件路径：`~/.config/muxterm/preferences.toml`。
pub fn user_preferences_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("muxterm").join("preferences.toml"))
}

impl Preferences {
    pub fn load() -> Self {
        let Some(path) = user_preferences_path() else {
            return Preferences::default();
        };
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Preferences::default();
        };
        toml::from_str(&raw).unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = user_preferences_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let Ok(raw) = toml::to_string_pretty(self) else {
            return;
        };
        let _ = std::fs::write(&path, raw);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_zoom_clamped() {
        assert_eq!(FontSettings::zoomed(12.0, 1), 13.0);
        assert_eq!(FontSettings::zoomed(36.0, 1), 36.0);
        assert_eq!(FontSettings::zoomed(9.0, -1), 9.0);
    }

    #[test]
    fn preferences_roundtrip_toml() {
        let p = Preferences {
            theme: Some("dark".into()),
            statusbar_mode: Some("theme".into()),
            font_size: Some(14.0),
        };
        let raw = toml::to_string_pretty(&p).unwrap();
        let back: Preferences = toml::from_str(&raw).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn preferences_default_omits_optional_fields() {
        let p = Preferences::default();
        let raw = toml::to_string_pretty(&p).unwrap();
        assert!(!raw.contains("theme"), "{raw}");
        assert!(!raw.contains("statusbar_mode"), "{raw}");
        assert!(!raw.contains("font_size"), "{raw}");
        let back: Preferences = toml::from_str(&raw).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn font_zoom_steps_by_one_and_clamps() {
        assert_eq!(FontSettings::zoomed(12.5, 1), 13.5);
        assert_eq!(FontSettings::zoomed(12.5, -1), 11.5);
        assert_eq!(FontSettings::clamp_size(8.0), MIN_FONT_SIZE);
        assert_eq!(FontSettings::clamp_size(99.0), MAX_FONT_SIZE);
    }
}
