//! status bar 快照模型与样式解析（纯逻辑，与 macOS StatusBarModel 一致）。
//!
//! 快照 JSON 由 core `muxterm_status_snapshot_json` 产生；样式解析支持
//! 颜色名 / `colourN` / `#rgb` / `#rrggbb` / `rrggbb` 与 bold/reverse 属性，
//! 以及 `#[...]` 内联指令（align/range/list 等布局指令忽略）。

use serde::Deserialize;

/// status bar 模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusBarMode {
    /// 连接 tmux 时完全采用 tmux 的 status 配置与颜色（默认）。
    Tmux,
    /// 只用 muxterm 主题黑/白默认色，忽略 tmux 彩色样式。
    Theme,
}

impl StatusBarMode {
    /// 从 config.toml `[statusbar] mode` 解析；兼容旧名 `color_mode = "gui"`。
    pub fn from_toml(toml: Option<&str>) -> StatusBarMode {
        let value = toml
            .unwrap_or("")
            .trim()
            .trim_matches('"')
            .to_ascii_lowercase();
        match value.as_str() {
            "theme" | "muxterm" | "muxterm_theme" | "gui" => StatusBarMode::Theme,
            _ => StatusBarMode::Tmux,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            StatusBarMode::Tmux => "tmux",
            StatusBarMode::Theme => "theme",
        }
    }
}

/// tmux status bar 快照（对应 Rust `StatusSnapshot` 的 JSON）。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct StatusBarSnapshot {
    pub enabled: bool,
    pub position: String,
    pub justify: String,
    pub interval: u64,
    pub left: String,
    pub right: String,
    pub left_length: usize,
    pub right_length: usize,
    pub status_style: String,
    pub left_style: String,
    pub right_style: String,
    pub separator: String,
    pub window_format: String,
    pub window_current_format: String,
    pub window_style: String,
    pub window_current_style: String,
    pub windows: Vec<StatusBarWindow>,
    pub error: Option<String>,
}

/// status bar 里的一个窗口条目。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct StatusBarWindow {
    pub window_id: u32,
    pub index: u32,
    pub name: String,
    pub flags: String,
    pub current: bool,
    pub text: String,
}

/// sRGB 颜色（0…1）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StatusBarColor {
    pub red: f64,
    pub green: f64,
    pub blue: f64,
}

/// 一段文本的 status bar 样式。
#[derive(Debug, Clone, PartialEq)]
pub struct StatusBarTextStyle {
    pub fg: Option<StatusBarColor>,
    pub bg: Option<StatusBarColor>,
    pub bold: bool,
    pub reverse: bool,
}

impl Default for StatusBarTextStyle {
    fn default() -> Self {
        StatusBarTextStyle {
            fg: None,
            bg: None,
            bold: false,
            reverse: false,
        }
    }
}

/// 解析后的文本片段：文字 + 样式。
#[derive(Debug, Clone, PartialEq)]
pub struct StatusBarStyledSegment {
    pub text: String,
    pub style: StatusBarTextStyle,
}

/// status bar 样式解析。
pub enum StatusBarStyleParser {}

impl StatusBarStyleParser {
    /// 解析 style 字符串（如 `bg=green,fg=black,bold`）。
    pub fn parse(style: &str) -> StatusBarTextStyle {
        let mut result = StatusBarTextStyle::default();
        for part in style.split(',') {
            let token = part.trim();
            if token.is_empty() {
                continue;
            }
            if token == "default" || token == "none" {
                result = StatusBarTextStyle::default();
                continue;
            }
            if token == "bold" {
                result.bold = true;
                continue;
            }
            if token == "nobold" {
                result.bold = false;
                continue;
            }
            if token == "reverse" {
                result.reverse = true;
                continue;
            }
            if token == "noreverse" {
                result.reverse = false;
                continue;
            }
            if let Some(eq) = token.find('=') {
                let key = token[..eq].to_ascii_lowercase();
                let value = token[eq + 1..].trim().to_string();
                if key == "fg" {
                    result.fg = Self::color(&value);
                } else if key == "bg" {
                    result.bg = Self::color(&value);
                }
            }
        }
        result
    }

    /// 解析内联样式文本，返回带样式的片段。
    pub fn parse_inline(text: &str, base: StatusBarTextStyle) -> Vec<StatusBarStyledSegment> {
        let mut segments = Vec::new();
        let mut current = base.clone();
        let mut plain = String::new();

        let flush = |plain: &mut String, current: &StatusBarTextStyle, segments: &mut Vec<StatusBarStyledSegment>| {
            if !plain.is_empty() {
                segments.push(StatusBarStyledSegment {
                    text: std::mem::take(plain),
                    style: current.clone(),
                });
            }
        };

        let bytes: Vec<char> = text.chars().collect();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == '#' && i + 1 < bytes.len() && bytes[i + 1] == '[' {
                if let Some(end) = text[i + 2..].find(']') {
                    flush(&mut plain, &current, &mut segments);
                    let directive = &text[i + 2..i + 2 + end];
                    current = Self::apply(directive, &current, &base);
                    i += 2 + end + 1;
                    continue;
                }
            }
            plain.push(bytes[i]);
            i += 1;
        }
        flush(&mut plain, &current, &mut segments);
        segments
    }

    /// 把 `#[...]` 指令作用到当前样式。
    pub fn apply(directive: &str, style: &StatusBarTextStyle, base: &StatusBarTextStyle) -> StatusBarTextStyle {
        let mut result = style.clone();
        for part in directive.split(',') {
            let token = part.trim();
            if token.is_empty() {
                continue;
            }
            if token == "default" {
                result = base.clone();
                continue;
            }
            if token == "push-default" || token == "pop-default" {
                // v1：忽略 push/pop，不维护栈
                continue;
            }
            if token == "bold" {
                result.bold = true;
                continue;
            }
            if token == "nobold" {
                result.bold = false;
                continue;
            }
            if token == "reverse" {
                result.reverse = true;
                continue;
            }
            if token == "noreverse" {
                result.reverse = false;
                continue;
            }
            if let Some(eq) = token.find('=') {
                let key = token[..eq].to_ascii_lowercase();
                let value = token[eq + 1..].trim().to_string();
                if key == "fg" {
                    result.fg = Self::color(&value);
                } else if key == "bg" {
                    result.bg = Self::color(&value);
                }
                // align/range/list/norange/nolist 等仅布局，忽略
            }
        }
        result
    }

    /// 颜色名 → sRGB。
    pub fn color(name: &str) -> Option<StatusBarColor> {
        let v = name.trim().to_ascii_lowercase();
        if v == "default" {
            return None;
        }
        if let Some(hex) = v.strip_prefix('#') {
            return hex_color(hex);
        }
        // muxterm 主题色是不带 # 的 `rrggbb`
        if v.len() == 6 && u32::from_str_radix(&v, 16).is_ok() {
            return hex_color(&v);
        }
        if let Some(n) = v.strip_prefix("colour") {
            if let Ok(n) = n.parse::<i32>() {
                return Self::xterm256(n);
            }
        }
        named_color(&v)
    }

    /// xterm 256 色板。
    pub fn xterm256(n: i32) -> Option<StatusBarColor> {
        if !(0..=255).contains(&n) {
            return None;
        }
        if n < 16 {
            const BASE: [(f64, f64, f64); 16] = [
                (0.0, 0.0, 0.0),
                (205.0, 49.0, 49.0),
                (13.0, 188.0, 121.0),
                (229.0, 229.0, 16.0),
                (36.0, 114.0, 200.0),
                (188.0, 63.0, 188.0),
                (17.0, 168.0, 205.0),
                (229.0, 229.0, 229.0),
                (102.0, 102.0, 102.0),
                (241.0, 76.0, 76.0),
                (35.0, 209.0, 139.0),
                (245.0, 245.0, 67.0),
                (59.0, 142.0, 234.0),
                (214.0, 112.0, 214.0),
                (41.0, 184.0, 219.0),
                (255.0, 255.0, 255.0),
            ];
            let (r, g, b) = BASE[n as usize];
            return Some(StatusBarColor {
                red: r / 255.0,
                green: g / 255.0,
                blue: b / 255.0,
            });
        }
        if n < 232 {
            let m = n - 16;
            let r = (m / 36) % 6;
            let g = (m / 6) % 6;
            let b = m % 6;
            let level = |x: i32| if x == 0 { 0.0 } else { (40 * x + 55) as f64 / 255.0 };
            return Some(StatusBarColor {
                red: level(r),
                green: level(g),
                blue: level(b),
            });
        }
        let gray = (8 + (n - 232) * 10) as f64 / 255.0;
        Some(StatusBarColor {
            red: gray,
            green: gray,
            blue: gray,
        })
    }

    /// 合并 base style 与 section override（section 缺省字段继承 base）。
    pub fn merged(base: &StatusBarTextStyle, override_style: &str) -> StatusBarTextStyle {
        let style = Self::parse(override_style);
        StatusBarTextStyle {
            fg: style.fg.or(base.fg),
            bg: style.bg.or(base.bg),
            bold: style.bold || base.bold,
            reverse: style.reverse || base.reverse,
        }
    }
}

fn named_color(name: &str) -> Option<StatusBarColor> {
    let c = |r: f64, g: f64, b: f64| StatusBarColor {
        red: r / 255.0,
        green: g / 255.0,
        blue: b / 255.0,
    };
    match name {
        "black" => Some(c(0.0, 0.0, 0.0)),
        "red" => Some(c(205.0, 49.0, 49.0)),
        "green" => Some(c(13.0, 188.0, 121.0)),
        "yellow" => Some(c(229.0, 229.0, 16.0)),
        "blue" => Some(c(36.0, 114.0, 200.0)),
        "magenta" => Some(c(188.0, 63.0, 188.0)),
        "cyan" => Some(c(17.0, 168.0, 205.0)),
        "white" => Some(c(229.0, 229.0, 229.0)),
        "grey" | "gray" | "brightblack" => Some(c(102.0, 102.0, 102.0)),
        "brightred" => Some(c(241.0, 76.0, 76.0)),
        "brightgreen" => Some(c(35.0, 209.0, 139.0)),
        "brightyellow" => Some(c(245.0, 245.0, 67.0)),
        "brightblue" => Some(c(59.0, 142.0, 234.0)),
        "brightmagenta" => Some(c(214.0, 112.0, 214.0)),
        "brightcyan" => Some(c(41.0, 184.0, 219.0)),
        "brightwhite" => Some(StatusBarColor {
            red: 1.0,
            green: 1.0,
            blue: 1.0,
        }),
        _ => None,
    }
}

fn hex_color(hex: &str) -> Option<StatusBarColor> {
    let mut h = hex.to_string();
    if h.len() == 3 {
        h = h.chars().map(|c| format!("{c}{c}")).collect();
    }
    if h.len() != 6 {
        return None;
    }
    let value = u32::from_str_radix(&h, 16).ok()?;
    Some(StatusBarColor {
        red: ((value >> 16) & 0xff) as f64 / 255.0,
        green: ((value >> 8) & 0xff) as f64 / 255.0,
        blue: (value & 0xff) as f64 / 255.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_from_toml() {
        assert_eq!(StatusBarMode::from_toml(Some("tmux")), StatusBarMode::Tmux);
        assert_eq!(StatusBarMode::from_toml(Some("theme")), StatusBarMode::Theme);
        assert_eq!(StatusBarMode::from_toml(Some("gui")), StatusBarMode::Theme);
        assert_eq!(StatusBarMode::from_toml(None), StatusBarMode::Tmux);
    }

    #[test]
    fn parse_style_pairs() {
        let s = StatusBarStyleParser::parse("bg=green,fg=black,bold");
        assert_eq!(s.bold, true);
        assert_eq!(s.bg, StatusBarStyleParser::color("green"));
        assert_eq!(s.fg, StatusBarStyleParser::color("black"));
    }

    #[test]
    fn inline_directives_split_segments() {
        let segments = StatusBarStyleParser::parse_inline(
            "#[fg=red]hi #[bg=blue]there",
            StatusBarTextStyle::default(),
        );
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].text, "hi ");
        assert_eq!(segments[0].style.fg, StatusBarStyleParser::color("red"));
        assert_eq!(segments[1].style.bg, StatusBarStyleParser::color("blue"));
    }

    #[test]
    fn xterm256_cube_and_gray() {
        let c = StatusBarStyleParser::xterm256(16).unwrap();
        assert_eq!(c.red, 0.0);
        let g = StatusBarStyleParser::xterm256(232).unwrap();
        assert!((g.red - 8.0 / 255.0).abs() < 1e-9);
    }

    #[test]
    fn merged_overrides_only_present_fields() {
        let base = StatusBarTextStyle {
            fg: StatusBarStyleParser::color("white"),
            bg: None,
            bold: false,
            reverse: false,
        };
        let merged = StatusBarStyleParser::merged(&base, "bg=blue");
        assert_eq!(merged.fg, base.fg);
        assert_eq!(merged.bg, StatusBarStyleParser::color("blue"));
    }

    #[test]
    fn snapshot_deserializes_from_core_json() {
        let json = r##"{
            "enabled": true,
            "position": "bottom",
            "justify": "centre",
            "interval": 15,
            "left": "muxterm",
            "right": "#[fg=red]%H:%M",
            "left_length": 20,
            "right_length": 50,
            "status_style": "bg=colour234",
            "left_style": "default",
            "right_style": "default",
            "separator": " ",
            "window_format": "#I:#W",
            "window_current_format": "#[reverse]#I:#W",
            "window_style": "default",
            "window_current_style": "bg=blue",
            "windows": [{"window_id": 0, "index": 1, "name": "bash", "flags": "*", "current": true, "text": "1:bash"}],
            "error": null
        }"##;
        let snapshot: StatusBarSnapshot = serde_json::from_str(json).unwrap();
        assert!(snapshot.enabled);
        assert_eq!(snapshot.windows.len(), 1);
        assert!(snapshot.windows[0].current);
    }
}
