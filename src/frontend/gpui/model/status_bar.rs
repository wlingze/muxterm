//! status bar 的纯模型：目标参数、模式、宽度预算、颜色与内联样式解析。
//!
//! 移植自 `chrome/StatusBarModel.swift`（547 行）。所有函数都是纯逻辑，
//! 不碰 AppKit，所以能整块搬过来逐条对照 Swift 测试。

use serde_json::Value;

use super::sidebar::Indicator;

/// status 快照查询的目标参数。
///
/// SSH 连接的 alias 必须放在 `ssh_alias`：以前它被误存进 `socket`，导致
/// 查询时执行本地 `tmux -L <alias>` 而不是 `ssh <alias> tmux ...`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusQueryTarget {
    pub socket: Option<String>,
    pub ssh_alias: Option<String>,
}

impl StatusQueryTarget {
    /// 中文注释：ssh 后端把 alias 从 socket 里择出来。
    pub fn resolve(backend_type: &str, socket: Option<String>, ssh_alias: Option<String>) -> Self {
        if backend_type.to_lowercase() == "ssh" {
            // 旧调用把 alias 放在 socket；新接口显式传 ssh_alias 时，socket
            // 就是真正的远端 tmux `-L`，必须保留给 status / discovery 查询。
            if ssh_alias.is_some() {
                return Self { socket, ssh_alias };
            }
            return Self {
                socket: None,
                ssh_alias: socket,
            };
        }
        Self { socket, ssh_alias }
    }
}

/// status bar 模式。
///
/// - `tmux`：完全采用 tmux 的 status 配置与颜色（默认）；
/// - `theme`：只用 muxterm 主题的黑 / 白默认色，忽略 tmux 的彩色样式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusBarMode {
    Tmux,
    Theme,
}

impl StatusBarMode {
    /// 从 config.toml 的 `[statusbar] mode = "tmux"|"theme"` 解析；
    /// 兼容旧名 `color_mode = "gui"`，缺省 / 未知回退 tmux。
    pub fn from_toml(toml: Option<&str>) -> Self {
        let Some(toml) = toml else {
            return StatusBarMode::Tmux;
        };
        let mut in_status_bar = false;
        for raw_line in toml.split('\n') {
            let line = raw_line.trim();
            if line.starts_with('[') && line.ends_with(']') {
                in_status_bar = line == "[statusbar]";
                continue;
            }
            if !in_status_bar {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            if key != "mode" && key != "color_mode" {
                continue;
            }
            let value = value.trim().trim_matches('"').to_lowercase();
            return match value.as_str() {
                "theme" | "muxterm" | "muxterm_theme" | "gui" => StatusBarMode::Theme,
                _ => StatusBarMode::Tmux,
            };
        }
        StatusBarMode::Tmux
    }
}

/// status bar 横向空间预算。
///
/// tmux 自己按 `status-left-length` / `status-right-length` 截断左右段，
/// 窗口列表始终拥有剩余空间。原生条用比例预算实现同一语义。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StatusBarWidthBudget {
    pub left_max: f32,
    pub right_max: f32,
    pub window_min: f32,
}

/// left / right 段最多占整条 bar 的比例（各 36%，合 72%）。
pub const SIDE_MAX_FRACTION: f32 = 0.36;
/// 窗口列表最少占整条 bar 的比例（28%）。
pub const WINDOW_MIN_FRACTION: f32 = 0.28;

/// 按整条 bar 宽度计算左右段封顶与窗口列表最小宽度。
pub fn width_budget(total_width: f32) -> StatusBarWidthBudget {
    let side = (total_width * SIDE_MAX_FRACTION).max(0.0);
    let window = (total_width * WINDOW_MIN_FRACTION).max(0.0);
    StatusBarWidthBudget {
        left_max: side,
        right_max: side,
        window_min: window,
    }
}

/// 单个 tab 按钮固定宽度（超出省略号）。
pub const FIXED_TAB_WIDTH: f32 = 104.0;
/// status-right 最少可见宽度。
pub const STATUS_RIGHT_MIN_WIDTH: f32 = 64.0;
/// 状态点 18 + Workspace 24 + 铃铛 24 + 加号 28 + 间距。
pub const CHROME_WIDTH: f32 = 136.0;

/// 放不下的 tab 数量（>0 必须滚动 / 溢出，不得压缩 right + chrome）。
pub fn overflow_count(tab_count: usize, bar_width: f32, left_width: f32) -> usize {
    let reserved = STATUS_RIGHT_MIN_WIDTH + CHROME_WIDTH + left_width.max(0.0) + 16.0;
    let available = (bar_width - reserved).max(0.0);
    let fit = (available / FIXED_TAB_WIDTH).floor() as isize;
    let fit = fit.max(0) as usize;
    tab_count.saturating_sub(fit)
}

/// 提醒位：状态栏铃铛。颜色取最高优先级状态（done > blocked > working > idle），
/// `count` 是**该颜色**的条目数，不是全部注意力条目的总数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusBarAttention {
    pub count: usize,
    pub indicator: Option<Indicator>,
}

impl StatusBarAttention {
    /// 中文注释：计数为 0 时没有颜色。
    pub fn new(count: isize, indicator: Option<Indicator>) -> Self {
        let count = count.max(0) as usize;
        Self {
            count,
            indicator: if count > 0 { indicator } else { None },
        }
    }

    /// 从一组状态里选最高优先级那个，并数它出现了几次。
    pub fn from_indicators(indicators: &[Indicator]) -> Self {
        let dominant = indicators.iter().copied().min_by_key(|i| i.activity_rank());
        match dominant {
            Some(color) => Self {
                count: indicators.iter().filter(|i| **i == color).count(),
                indicator: Some(color),
            },
            None => Self {
                count: 0,
                indicator: None,
            },
        }
    }

    pub fn is_active(self) -> bool {
        self.count > 0
    }
}

/// 只有 >= 1 才在铃铛上显示数字。
pub fn shows_badge_count(count: usize) -> bool {
    count >= 1
}

/// 状态栏体现的 Workspace 类型。Shells / Agents 是前端固定聚合槽，
/// 用单独外观避免它们看起来像普通 tmux / Herdr Workspace。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusBarPresentation {
    Workspace,
    Shells,
    Agents,
    Opening,
}

/// GUI tab 标题（序号 + 名字），不用 tmux `#[fg=…]` 格式串。
pub fn tab_title(index: u32, name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        format!("{index}")
    } else {
        format!("{index}  {trimmed}")
    }
}

/// status bar 里的一个窗口条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusBarWindow {
    pub window_id: u32,
    pub index: u32,
    pub name: String,
    pub flags: String,
    /// 前端驱动的高亮标记：切 tab 时本地更新，不等 tmux 慢查询。
    pub current: bool,
    pub text: String,
}

/// muxterm status bar 快照（对应 Core `StatusSnapshot` 的 JSON）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatusBarSnapshot {
    pub enabled: bool,
    pub position: String,
    pub justify: String,
    pub interval: u64,
    /// 订阅推送会原地更新（`refresh-client -B` → `%subscription-changed`）。
    pub left: String,
    pub right: String,
    pub left_length: i64,
    pub right_length: i64,
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

impl StatusBarSnapshot {
    /// 解析外层响应 `{ok, error, status}`；`ok != true` 或无 status 时返回 `None`。
    pub fn decode(data: &[u8]) -> Option<Self> {
        let json: Value = serde_json::from_slice(data).ok()?;
        if json.get("ok").and_then(Value::as_bool) != Some(true) {
            return None;
        }
        let status = json.get("status")?;
        let text = |key: &str| {
            status
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let windows = status
            .get("windows")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(decode_window).collect())
            .unwrap_or_default();
        Some(Self {
            enabled: status
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            position: text("position"),
            justify: text("justify"),
            interval: status.get("interval").and_then(Value::as_u64).unwrap_or(0),
            left: text("left"),
            right: text("right"),
            left_length: status
                .get("left_length")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            right_length: status
                .get("right_length")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            status_style: text("status_style"),
            left_style: text("left_style"),
            right_style: text("right_style"),
            separator: text("separator"),
            window_format: text("window_format"),
            window_current_format: text("window_current_format"),
            window_style: text("window_style"),
            window_current_style: text("window_current_style"),
            windows,
            error: status
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }

    /// 解析 status 文案时按 tmux 的真实 window_index 排序。
    ///
    /// 这只服务于 status 数据本身；Tab UI 的顺序和编号来自 Core snapshot，
    /// 不应调用此方法重建第二份窗口列表。
    pub fn windows_by_index(&self) -> Vec<StatusBarWindow> {
        let mut windows = self.windows.clone();
        windows.sort_by_key(|w| (w.index, w.window_id));
        windows
    }

    /// 按 tmux window_index 查找稳定 window_id。
    pub fn window_id_for_index(&self, index: u32) -> Option<u32> {
        self.windows
            .iter()
            .find(|w| w.index == index)
            .map(|w| w.window_id)
    }

    /// 前端驱动高亮：把 current 标记移到指定窗口。
    /// 切 tab 时立即调用，不等 tmux 快照查询。
    pub fn updating_current_window(&self, window_id: u32) -> Self {
        let mut copy = self.clone();
        for window in &mut copy.windows {
            window.current = window.window_id == window_id;
        }
        copy
    }

    /// 前端驱动关闭：移除指定窗口条目。
    pub fn removing_window(&self, window_id: u32) -> Self {
        let mut copy = self.clone();
        copy.windows.retain(|w| w.window_id != window_id);
        copy
    }
}

fn decode_window(value: &Value) -> Option<StatusBarWindow> {
    Some(StatusBarWindow {
        window_id: value.get("window_id")?.as_u64()? as u32,
        index: value.get("index").and_then(Value::as_u64).unwrap_or(0) as u32,
        name: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        flags: value
            .get("flags")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        current: value
            .get("current")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        text: value
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    })
}

/// sRGB 颜色（0…1）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StatusBarColor {
    pub red: f32,
    pub green: f32,
    pub blue: f32,
}

impl StatusBarColor {
    pub const fn new(red: f32, green: f32, blue: f32) -> Self {
        Self { red, green, blue }
    }

    /// 中文注释：gpui 用 u8 通道，这里是唯一的转换点。
    pub fn to_rgba_u8(self) -> (u8, u8, u8) {
        let channel = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        (channel(self.red), channel(self.green), channel(self.blue))
    }
}

/// 一段文本的 status bar 样式。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StatusBarTextStyle {
    pub fg: Option<StatusBarColor>,
    pub bg: Option<StatusBarColor>,
    pub bold: bool,
    pub reverse: bool,
}

/// 解析后的文本片段：文字 + 样式。
#[derive(Debug, Clone, PartialEq)]
pub struct StatusBarStyledSegment {
    pub text: String,
    pub style: StatusBarTextStyle,
}

/// 解析一个 style 字符串（如 `bg=green,fg=black,bold`）。
pub fn parse_style(style: &str) -> StatusBarTextStyle {
    let mut result = StatusBarTextStyle::default();
    for part in style.split(',') {
        let token = part.trim();
        if token.is_empty() {
            continue;
        }
        match token {
            "default" | "none" => {
                result = StatusBarTextStyle::default();
                continue;
            }
            "bold" => {
                result.bold = true;
                continue;
            }
            "nobold" => {
                result.bold = false;
                continue;
            }
            "reverse" => {
                result.reverse = true;
                continue;
            }
            "noreverse" => {
                result.reverse = false;
                continue;
            }
            _ => {}
        }
        if let Some((key, value)) = token.split_once('=') {
            match key.to_lowercase().as_str() {
                "fg" => result.fg = color(value),
                "bg" => result.bg = color(value),
                _ => {}
            }
        }
    }
    result
}

/// 把 `#[...]` 指令作用到当前样式。
pub fn apply_directive(
    directive: &str,
    style: StatusBarTextStyle,
    base: StatusBarTextStyle,
) -> StatusBarTextStyle {
    let mut result = style;
    for part in directive.split(',') {
        let token = part.trim();
        if token.is_empty() {
            continue;
        }
        match token {
            "default" => {
                result = base;
                continue;
            }
            // v1：忽略 push / pop，不维护栈。
            "push-default" | "pop-default" => continue,
            "bold" => {
                result.bold = true;
                continue;
            }
            "nobold" => {
                result.bold = false;
                continue;
            }
            "reverse" => {
                result.reverse = true;
                continue;
            }
            "noreverse" => {
                result.reverse = false;
                continue;
            }
            _ => {}
        }
        if let Some((key, value)) = token.split_once('=') {
            // align / range / list / norange / nolist 等仅布局，忽略。
            match key.to_lowercase().as_str() {
                "fg" => result.fg = color(value),
                "bg" => result.bg = color(value),
                _ => {}
            }
        }
    }
    result
}

/// 解析内联样式文本（status-left / right / window 格式），返回带样式的片段。
pub fn parse_inline(text: &str, base: StatusBarTextStyle) -> Vec<StatusBarStyledSegment> {
    let chars: Vec<char> = text.chars().collect();
    let mut segments = Vec::new();
    let mut current = base;
    let mut plain = String::new();
    let mut index = 0;

    while index < chars.len() {
        if chars[index] == '#' && index + 1 < chars.len() && chars[index + 1] == '[' {
            if let Some(offset) = chars[index + 2..].iter().position(|c| *c == ']') {
                let end = index + 2 + offset;
                if !plain.is_empty() {
                    segments.push(StatusBarStyledSegment {
                        text: std::mem::take(&mut plain),
                        style: current,
                    });
                }
                let directive: String = chars[index + 2..end].iter().collect();
                current = apply_directive(&directive, current, base);
                index = end + 1;
                continue;
            }
        }
        plain.push(chars[index]);
        index += 1;
    }
    if !plain.is_empty() {
        segments.push(StatusBarStyledSegment {
            text: plain,
            style: current,
        });
    }
    segments
}

/// 颜色名 / 十六进制 / xterm 索引 → sRGB。
pub fn color(name: &str) -> Option<StatusBarColor> {
    let value = name.trim().to_lowercase();
    if value == "default" {
        return None;
    }
    if let Some(hex) = value.strip_prefix('#') {
        return hex_color(hex);
    }
    // muxterm 主题色是不带 # 的 `rrggbb`。
    if value.len() == 6 && u32::from_str_radix(&value, 16).is_ok() {
        return hex_color(&value);
    }
    if let Some(index) = value.strip_prefix("colour") {
        if let Ok(n) = index.parse::<i64>() {
            return xterm256(n);
        }
    }
    named_color(&value)
}

/// xterm 256 色板。
pub fn xterm256(n: i64) -> Option<StatusBarColor> {
    if !(0..=255).contains(&n) {
        return None;
    }
    if n < 16 {
        const BASE: [(u8, u8, u8); 16] = [
            (0, 0, 0),
            (205, 49, 49),
            (13, 188, 121),
            (229, 229, 16),
            (36, 114, 200),
            (188, 63, 188),
            (17, 168, 205),
            (229, 229, 229),
            (102, 102, 102),
            (241, 76, 76),
            (35, 209, 139),
            (245, 245, 67),
            (59, 142, 234),
            (214, 112, 214),
            (41, 184, 219),
            (255, 255, 255),
        ];
        let (r, g, b) = BASE[n as usize];
        return Some(StatusBarColor::new(
            r as f32 / 255.0,
            g as f32 / 255.0,
            b as f32 / 255.0,
        ));
    }
    if n < 232 {
        let m = n - 16;
        let level = |x: i64| {
            if x == 0 {
                0.0
            } else {
                (40 * x + 55) as f32 / 255.0
            }
        };
        return Some(StatusBarColor::new(
            level((m / 36) % 6),
            level((m / 6) % 6),
            level(m % 6),
        ));
    }
    let gray = (8 + (n - 232) * 10) as f32 / 255.0;
    Some(StatusBarColor::new(gray, gray, gray))
}

fn named_color(name: &str) -> Option<StatusBarColor> {
    let rgb = |r: u8, g: u8, b: u8| {
        Some(StatusBarColor::new(
            r as f32 / 255.0,
            g as f32 / 255.0,
            b as f32 / 255.0,
        ))
    };
    match name {
        "black" => rgb(0, 0, 0),
        "red" => rgb(205, 49, 49),
        "green" => rgb(13, 188, 121),
        "yellow" => rgb(229, 229, 16),
        "blue" => rgb(36, 114, 200),
        "magenta" => rgb(188, 63, 188),
        "cyan" => rgb(17, 168, 205),
        "white" => rgb(229, 229, 229),
        "grey" | "gray" | "brightblack" => rgb(102, 102, 102),
        "brightred" => rgb(241, 76, 76),
        "brightgreen" => rgb(35, 209, 139),
        "brightyellow" => rgb(245, 245, 67),
        "brightblue" => rgb(59, 142, 234),
        "brightmagenta" => rgb(214, 112, 214),
        "brightcyan" => rgb(41, 184, 219),
        "brightwhite" => rgb(255, 255, 255),
        _ => None,
    }
}

fn hex_color(hex: &str) -> Option<StatusBarColor> {
    let expanded: String = if hex.len() == 3 {
        hex.chars().flat_map(|c| [c, c]).collect()
    } else {
        hex.to_string()
    };
    if expanded.len() != 6 {
        return None;
    }
    let value = u32::from_str_radix(&expanded, 16).ok()?;
    Some(StatusBarColor::new(
        ((value >> 16) & 0xff) as f32 / 255.0,
        ((value >> 8) & 0xff) as f32 / 255.0,
        (value & 0xff) as f32 / 255.0,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }

    /// 对照 `StatusBarColorTests.testNamedAndXtermColors`。
    #[test]
    fn named_and_xterm_colors() {
        let red = color("red").expect("red 应可解析");
        assert!(approx(red.red, 205.0 / 255.0));
        assert!(approx(red.green, 49.0 / 255.0));
        assert!(approx(red.blue, 49.0 / 255.0));

        let bright_white = color("brightwhite").expect("brightwhite 应可解析");
        assert_eq!(bright_white, StatusBarColor::new(1.0, 1.0, 1.0));

        let black = color("colour0").expect("colour0 应可解析");
        assert_eq!(black, StatusBarColor::new(0.0, 0.0, 0.0));

        // xterm 灰度：colour232=8, colour255=238。
        let gray = color("colour255").expect("colour255 应可解析");
        assert!(approx(gray.red, 238.0 / 255.0));

        let white = color("colour231").expect("colour231 应可解析");
        assert_eq!(white, StatusBarColor::new(1.0, 1.0, 1.0));

        let short = color("#abc").expect("#abc 应可解析");
        assert!(approx(short.red, 170.0 / 255.0));
        assert!(approx(short.green, 187.0 / 255.0));
        assert!(approx(short.blue, 204.0 / 255.0));

        let bare = color("cdd6f4").expect("裸 rrggbb 应可解析");
        assert!(approx(bare.red, 205.0 / 255.0));
        assert!(approx(bare.green, 214.0 / 255.0));
        assert!(approx(bare.blue, 244.0 / 255.0));

        assert_eq!(color("default"), None);
        assert_eq!(color("bogus"), None);
    }

    /// 对照 `testXtermCubeAndGray`。
    #[test]
    fn xterm_cube_and_gray() {
        assert_eq!(xterm256(16), Some(StatusBarColor::new(0.0, 0.0, 0.0)));
        assert_eq!(xterm256(231), Some(StatusBarColor::new(1.0, 1.0, 1.0)));
        assert_eq!(
            xterm256(232),
            Some(StatusBarColor::new(8.0 / 255.0, 8.0 / 255.0, 8.0 / 255.0))
        );
        assert_eq!(xterm256(256), None);
    }

    /// 对照 `StatusBarModeTests.testDefaultsToTmux`。
    #[test]
    fn mode_defaults_to_tmux() {
        assert_eq!(StatusBarMode::from_toml(None), StatusBarMode::Tmux);
        assert_eq!(
            StatusBarMode::from_toml(Some("[font]\nsize = 18")),
            StatusBarMode::Tmux
        );
    }

    /// 对照 `testParsesThemeMode`。
    #[test]
    fn mode_parses_theme() {
        assert_eq!(
            StatusBarMode::from_toml(Some("[statusbar]\nmode = \"theme\"")),
            StatusBarMode::Theme
        );
    }

    /// 对照 `testParsesLegacyGuiModeAsTheme`。
    #[test]
    fn mode_parses_legacy_gui_as_theme() {
        assert_eq!(
            StatusBarMode::from_toml(Some("[statusbar]\ncolor_mode = \"gui\"")),
            StatusBarMode::Theme
        );
    }

    /// 对照 `StatusQueryTargetTests.testLocalKeepsSocket`。
    #[test]
    fn query_target_local_keeps_socket() {
        let target = StatusQueryTarget::resolve("tmux", Some("my-socket".to_string()), None);
        assert_eq!(target.socket.as_deref(), Some("my-socket"));
        assert_eq!(target.ssh_alias, None);
    }

    /// 对照 `testSshMovesAliasOutOfSocket`。
    #[test]
    fn query_target_ssh_moves_alias_out_of_socket() {
        let target = StatusQueryTarget::resolve("ssh", Some("ryzen".to_string()), None);
        assert_eq!(target.socket, None);
        assert_eq!(target.ssh_alias.as_deref(), Some("ryzen"));
    }

    /// 对照 `testSshWithExplicitAliasPreservesRemoteSocket`。
    #[test]
    fn query_target_ssh_with_explicit_alias_preserves_remote_socket() {
        let target = StatusQueryTarget::resolve(
            "ssh",
            Some("muxterm-test-remote".to_string()),
            Some("ryzen".to_string()),
        );
        assert_eq!(target.socket.as_deref(), Some("muxterm-test-remote"));
        assert_eq!(target.ssh_alias.as_deref(), Some("ryzen"));
    }

    /// 对照 `StatusBarStyleParserTests.testParseStyleString`。
    #[test]
    fn parse_style_string() {
        let style = parse_style("bg=green,fg=black,bold");
        assert_eq!(style.fg, color("black"));
        assert_eq!(style.bg, color("green"));
        assert!(style.bold);
        assert!(!style.reverse);
    }

    /// 对照 `testParseStyleDefault`。
    #[test]
    fn parse_style_default() {
        assert_eq!(parse_style("default"), StatusBarTextStyle::default());
        assert_eq!(parse_style(""), StatusBarTextStyle::default());
    }

    /// 对照 `testParseInlineSegments`。
    #[test]
    fn parse_inline_segments() {
        let segments = parse_inline(
            "#[fg=colour233,bg=colour241,bold] 13/08 #[fg=colour233,bg=colour245,nobold] 13:50 ",
            StatusBarTextStyle::default(),
        );
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].text, " 13/08 ");
        assert_eq!(segments[0].style.fg, color("colour233"));
        assert_eq!(segments[0].style.bg, color("colour241"));
        assert!(segments[0].style.bold);
        assert_eq!(segments[1].style.fg, color("colour233"));
        assert_eq!(segments[1].style.bg, color("colour245"));
        assert!(!segments[1].style.bold);
    }

    /// 对照 `testParseInlineDefaultResetsToBase`：`#[default]` 回到传入的 base。
    #[test]
    fn parse_inline_default_resets_to_base() {
        let base = StatusBarTextStyle {
            fg: color("white"),
            bg: color("black"),
            bold: true,
            reverse: false,
        };
        let segments = parse_inline("A#[default]B#[fg=red]C", base);
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].style, base);
        assert_eq!(segments[1].style, base);
        assert_eq!(segments[2].style.fg, color("red"));
        // 未覆盖的属性（bg）保持 base。
        assert_eq!(segments[2].style.bg, base.bg);
    }

    /// 对照 `StatusBarSnapshotDecodingTests.testDecodeSnapshotJSON`。
    #[test]
    fn decode_snapshot_json() {
        let json = r##"
        {
          "ok": true,
          "status": {
            "enabled": true,
            "position": "bottom",
            "justify": "left",
            "interval": 15,
            "left": " foo ",
            "right": "#[fg=colour233,bg=colour241,bold] 13/08 ",
            "left_length": 20,
            "right_length": 50,
            "status_style": "bg=green,fg=black",
            "left_style": "default",
            "right_style": "default",
            "separator": " ",
            "window_format": " #I#[fg=colour237]:#[fg=colour250]#W#[fg=colour244]#F ",
            "window_current_format": " #I#[fg=colour250]:#[fg=colour255]#W#[fg=colour50]#F ",
            "window_style": "default",
            "window_current_style": "default",
            "windows": [
              { "window_id": 0, "index": 1, "name": "sleep", "flags": "*", "current": true, "text": " 1#[fg=colour237]:#[fg=colour250]sleep#[fg=colour244]* " }
            ],
            "error": null
          }
        }
        "##;
        let snapshot = StatusBarSnapshot::decode(json.as_bytes()).expect("应当能解析");
        assert!(snapshot.enabled);
        assert_eq!(snapshot.position, "bottom");
        assert_eq!(snapshot.windows.len(), 1);
        assert_eq!(snapshot.windows[0].window_id, 0);
        assert_eq!(snapshot.windows[0].index, 1);
        assert!(snapshot.windows[0].current);
        let segments = parse_inline(&snapshot.right, StatusBarTextStyle::default());
        assert!(!segments.is_empty());
    }

    /// 对照 `StatusBarLayoutPolicyTests.testBudgetFitsInsideBar`。
    #[test]
    fn budget_fits_inside_bar() {
        let total = 960.0_f32;
        let budget = width_budget(total);
        assert!(approx(budget.left_max, total * SIDE_MAX_FRACTION));
        assert!(approx(budget.right_max, total * SIDE_MAX_FRACTION));
        assert!(approx(budget.window_min, total * WINDOW_MIN_FRACTION));
        // 36% + 36% + 28% = 100%，左右段与窗口列表不会互相挤没。
        assert!(budget.left_max + budget.right_max + budget.window_min <= total);
    }

    /// 对照 `testBudgetFitsNarrowWindow`：窄窗口（最小 480pt）下窗口列表仍可见。
    #[test]
    fn budget_fits_narrow_window() {
        let total = 480.0_f32;
        let budget = width_budget(total);
        assert!(budget.window_min > 100.0);
        assert!(budget.left_max + budget.right_max + budget.window_min <= total);
    }

    /// 对照 `testBudgetClampsNonPositiveWidth`。
    #[test]
    fn budget_clamps_non_positive_width() {
        let budget = width_budget(0.0);
        assert_eq!(budget.left_max, 0.0);
        assert_eq!(budget.right_max, 0.0);
        assert_eq!(budget.window_min, 0.0);
    }

    /// 对照 `testManyTabsOverflowInsteadOfCrushingRight`。
    #[test]
    fn many_tabs_overflow_instead_of_crushing_right() {
        let overflow = overflow_count(20, 720.0, 80.0);
        assert!(overflow > 0, "20 个 tab 在 720pt 下必须溢出");
    }

    /// 对照 `testFewTabsFitWithoutOverflow`。
    #[test]
    fn few_tabs_fit_without_overflow() {
        assert_eq!(overflow_count(2, 960.0, 40.0), 0);
    }

    /// 对照 `testReservedChromeAndRightStayPositive`。
    #[test]
    fn reserved_chrome_and_right_stay_positive() {
        assert!(STATUS_RIGHT_MIN_WIDTH > 0.0);
        assert!(CHROME_WIDTH > 0.0);
        assert!(FIXED_TAB_WIDTH > 0.0);
    }

    /// 对照 `StatusBarAttentionTests.testZeroCountIsInactive`。
    #[test]
    fn zero_count_is_inactive() {
        assert!(!StatusBarAttention::new(0, None).is_active());
    }

    /// 对照 `testPositiveCountIsActive`。
    #[test]
    fn positive_count_is_active() {
        assert!(StatusBarAttention::new(1, Some(Indicator::Blocked)).is_active());
        assert!(StatusBarAttention::new(3, Some(Indicator::Blocked)).is_active());
    }

    /// 对照 `testNegativeCountClampsToZero`。
    #[test]
    fn negative_count_clamps_to_zero() {
        let attention = StatusBarAttention::new(-5, Some(Indicator::Done));
        assert_eq!(attention.count, 0);
        assert!(!attention.is_active());
        assert_eq!(attention.indicator, None);
    }

    /// 对照 `testDominantActivityUsesSidebarPriority`。
    #[test]
    fn dominant_activity_uses_sidebar_priority() {
        let mixed = StatusBarAttention::from_indicators(&[
            Indicator::Working,
            Indicator::Blocked,
            Indicator::Done,
            Indicator::Idle,
        ]);
        assert_eq!(mixed.indicator, Some(Indicator::Done));
        let weaker = StatusBarAttention::from_indicators(&[Indicator::Working, Indicator::Idle]);
        assert_eq!(weaker.indicator, Some(Indicator::Working));
    }

    /// 对照 `testCountMatchesDominantColorNotTotal`：铃铛数字必须是该颜色的
    /// 条目数，不是全部注意力条目总数。
    #[test]
    fn count_matches_dominant_color_not_total() {
        let mixed = StatusBarAttention::from_indicators(&[
            Indicator::Working,
            Indicator::Blocked,
            Indicator::Blocked,
            Indicator::Blocked,
            Indicator::Done,
            Indicator::Idle,
        ]);
        assert_eq!(mixed.indicator, Some(Indicator::Done));
        assert_eq!(mixed.count, 1, "铃铛是完成色时数字必须是完成数，不是 6");

        let blocked = StatusBarAttention::from_indicators(&[
            Indicator::Blocked,
            Indicator::Blocked,
            Indicator::Working,
            Indicator::Working,
            Indicator::Working,
        ]);
        assert_eq!(blocked.indicator, Some(Indicator::Blocked));
        assert_eq!(blocked.count, 2, "铃铛是阻塞色时数字必须是阻塞数，不是 5");

        let working = StatusBarAttention::from_indicators(&[
            Indicator::Working,
            Indicator::Working,
            Indicator::Working,
        ]);
        assert_eq!(working.indicator, Some(Indicator::Working));
        assert_eq!(working.count, 3);
        assert!(shows_badge_count(working.count));
        assert!(shows_badge_count(1));
        assert!(!shows_badge_count(0));
    }

    /// 对照 `StatusBarTabTitleTests.testIndexAndName`。
    #[test]
    fn tab_title_index_and_name() {
        assert_eq!(tab_title(1, "zsh"), "1  zsh");
        assert_eq!(tab_title(2, "  "), "2");
        assert_eq!(tab_title(3, ""), "3");
        assert!(!tab_title(1, "sleep").contains("#["));
    }

    /// 对照 `StatusBarFrontendSyncTests.testWindowIndexLookupUsesStableWindowID`。
    #[test]
    fn window_index_lookup_uses_stable_window_id() {
        let snapshot = StatusBarSnapshot {
            windows: vec![
                StatusBarWindow {
                    window_id: 47,
                    index: 7,
                    name: "seven".to_string(),
                    flags: String::new(),
                    current: false,
                    text: String::new(),
                },
                StatusBarWindow {
                    window_id: 25,
                    index: 6,
                    name: "six".to_string(),
                    flags: String::new(),
                    current: true,
                    text: String::new(),
                },
                StatusBarWindow {
                    window_id: 10,
                    index: 1,
                    name: "one".to_string(),
                    flags: String::new(),
                    current: false,
                    text: String::new(),
                },
            ],
            ..StatusBarSnapshot::default()
        };
        assert_eq!(snapshot.window_id_for_index(7), Some(47));
        assert_eq!(snapshot.window_id_for_index(6), Some(25));
        assert_eq!(snapshot.window_id_for_index(9), None);
        assert_eq!(
            snapshot
                .windows_by_index()
                .iter()
                .map(|w| w.index)
                .collect::<Vec<_>>(),
            vec![1, 6, 7],
            "status 文案按 tmux window_index 排序"
        );
    }

    /// 对照 `testDecodedWindowsAreMutable`：前端驱动高亮不等 tmux 慢查询。
    #[test]
    fn decoded_windows_are_mutable() {
        let snapshot = StatusBarSnapshot {
            windows: vec![
                StatusBarWindow {
                    window_id: 1,
                    index: 1,
                    name: "one".to_string(),
                    flags: String::new(),
                    current: true,
                    text: String::new(),
                },
                StatusBarWindow {
                    window_id: 2,
                    index: 2,
                    name: "two".to_string(),
                    flags: String::new(),
                    current: false,
                    text: String::new(),
                },
            ],
            ..StatusBarSnapshot::default()
        };
        let updated = snapshot.updating_current_window(2);
        assert!(!updated.windows[0].current);
        assert!(updated.windows[1].current);
        assert!(snapshot.windows[0].current, "原快照不得被就地改掉");
    }

    /// 对照 `testRemovingWindowDropsTabEntry`。
    #[test]
    fn removing_window_drops_tab_entry() {
        let snapshot = StatusBarSnapshot {
            windows: vec![
                StatusBarWindow {
                    window_id: 1,
                    index: 1,
                    name: "one".to_string(),
                    flags: String::new(),
                    current: false,
                    text: String::new(),
                },
                StatusBarWindow {
                    window_id: 2,
                    index: 2,
                    name: "two".to_string(),
                    flags: String::new(),
                    current: true,
                    text: String::new(),
                },
            ],
            ..StatusBarSnapshot::default()
        };
        let after = snapshot.removing_window(1);
        assert_eq!(after.windows.len(), 1);
        assert_eq!(after.windows[0].window_id, 2);
    }

    /// 未闭合的 `#[` 必须退回普通文本，不能被吞掉。
    #[test]
    fn unterminated_directive_falls_back_to_plain_text() {
        let segments = parse_inline("a#[fg=red", StatusBarTextStyle::default());
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "a#[fg=red");
    }

    /// `ok:false` 的响应解不出快照，避免把错误当空 status 画出来。
    #[test]
    fn decode_rejects_unsuccessful_response() {
        assert!(StatusBarSnapshot::decode(br#"{"ok": false, "error": "boom"}"#).is_none());
        assert!(StatusBarSnapshot::decode(b"not json").is_none());
    }
}
