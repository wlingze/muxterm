//! 终端字体设置与缩放（纯逻辑）。
//!
//! `[font] family/size` 来自统一 config.toml；运行期 Ctrl+/-/0 缩放字号
//! 通过 Core SettingsService 写回 config.toml。旧 `preferences.toml` 由
//! Core 迁移读取，platform 不再读写该文件。

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
    fn font_zoom_steps_by_one_and_clamps() {
        assert_eq!(FontSettings::zoomed(12.5, 1), 13.5);
        assert_eq!(FontSettings::zoomed(12.5, -1), 11.5);
        assert_eq!(FontSettings::clamp_size(8.0), MIN_FONT_SIZE);
        assert_eq!(FontSettings::clamp_size(99.0), MAX_FONT_SIZE);
    }
}
