//! 桌面平台差异表：默认字体、菜单呈现、命令键、窗口装饰。
//!
//! 这里是 gpui 前端唯一按 `target_os` 分叉的策略层。**只放纯函数**：
//! 输入是 OS / 配置 / `Modifiers`，输出是策略值，方便在任意平台跑测试
//! （`DesktopOs` 是参数，不是 `cfg!`）。
//!
//! 三平台的实际差异（2026-11-19 核对）：
//!
//! | 维度 | macOS | Linux | Windows |
//! | --- | --- | --- | --- |
//! | 命令键 | Command | 配置 `[shortcuts] primary_key`，`auto` = Alt | 同 Linux（`auto` = Alt） |
//! | 菜单 | 原生 NSMenu | 自绘菜单条 | 自绘菜单条 |
//! | 默认等宽 | Menlo | DejaVu Sans Mono | Cascadia Mono |
//! | 窗口装饰 | 透明标题栏 + traffic lights | 服务端装饰（Wayland 允许客户端） | 原生标题栏 |
//! | 高 DPI | `NSHighResolutionCapable` | 由合成器提供 | manifest DPI aware（gpui） |

use gpui::Modifiers;

/// 运行期桌面平台。`current()` 是唯一允许读 `cfg!(target_os)` 的地方。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopOs {
    MacOs,
    Linux,
    Windows,
}

impl DesktopOs {
    /// 本进程所在平台。
    pub const fn current() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self::MacOs
        }
        #[cfg(target_os = "windows")]
        {
            Self::Windows
        }
        #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
        {
            Self::Linux
        }
    }

    /// 配置文件与 CI 里的平台标识（与 `[settings] platforms = [...]` 一致）。
    pub const fn id(self) -> &'static str {
        match self {
            Self::MacOs => "macos",
            Self::Linux => "linux",
            Self::Windows => "windows",
        }
    }

    /// 缺省等宽字体：必须是该平台自带的等宽字体，否则字宽漂移会错列。
    ///
    /// 注意 Core 的配置缺省是 `JetBrains Mono`，前端启动时会把
    /// `assets/fonts/JetBrainsMono-Regular.ttf` 注册进 `TextSystem`，所以
    /// 真正生效的是内置字体；这里的值只用于配置缺失/首帧。
    pub const fn default_monospace_family(self) -> &'static str {
        match self {
            Self::MacOs => "Menlo",
            Self::Linux => "DejaVu Sans Mono",
            Self::Windows => "Cascadia Mono",
        }
    }

    /// 字体回退栈：先内置 JetBrains Mono（跨平台一致），再各平台自带等宽。
    pub const fn monospace_fallbacks(self) -> &'static [&'static str] {
        match self {
            Self::MacOs => &["JetBrains Mono", "Menlo", "Monaco", "Noto Sans Symbols 2"],
            Self::Linux => &[
                "JetBrains Mono",
                "DejaVu Sans Mono",
                "Liberation Mono",
                "Noto Sans Symbols 2",
            ],
            Self::Windows => &[
                "JetBrains Mono",
                "Cascadia Mono",
                "Consolas",
                "Noto Sans Symbols 2",
            ],
        }
    }

    /// 有原生应用菜单栏吗？只有 macOS 有；Linux / Windows 用自绘菜单条。
    pub const fn uses_native_menu_bar(self) -> bool {
        matches!(self, Self::MacOs)
    }

    /// 窗口标题栏是否隐藏（macOS 只留 traffic lights；其它平台交给系统）。
    pub const fn hides_title_text(self, show_title_bar: bool) -> bool {
        // macOS：`[ui] show_title_bar = false` 时标题留空、只留三个按钮。
        // 其它平台标题栏由窗口管理器画，空标题会变成一条没有信息的空条，
        // 所以即使配置成 false 也保留 workspace 名。
        matches!(self, Self::MacOs) && !show_title_bar
    }
}

/// 主快捷键修饰键。语义与 Core `config/action_catalog::primary_modifier`
/// 完全一致：`auto` 在 macOS 上是 Command，其它平台是 Alt。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimaryModifier {
    Command,
    Control,
    Super,
    Alt,
}

impl PrimaryModifier {
    /// 解析配置值；`auto` / 空 / 未知值返回 `None`（由调用方按平台取默认）。
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "command" => Some(Self::Command),
            "control" => Some(Self::Control),
            "super" => Some(Self::Super),
            "alt" => Some(Self::Alt),
            _ => None,
        }
    }

    /// `auto` 的平台默认：macOS = Command，Linux / Windows = Alt。
    pub const fn default_for(os: DesktopOs) -> Self {
        match os {
            DesktopOs::MacOs => Self::Command,
            DesktopOs::Linux | DesktopOs::Windows => Self::Alt,
        }
    }

    /// 配置文件里写回去的值。
    pub const fn config_value(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::Control => "control",
            Self::Super => "super",
            Self::Alt => "alt",
        }
    }

    /// 菜单 / 设置页里的显示名。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Command => "⌘",
            Self::Control => "Ctrl",
            Self::Super => "Super",
            Self::Alt => "Alt",
        }
    }

    /// 这个修饰键在输入事件里按下了吗？
    pub fn is_held(self, modifiers: &Modifiers) -> bool {
        match self {
            // gpui 的 `platform` 字段 = macOS Command / Windows Win / Linux Super。
            Self::Command | Self::Super => modifiers.platform,
            Self::Control => modifiers.control,
            Self::Alt => modifiers.alt,
        }
    }
}

/// 归一化后的和弦修饰键：`command` 表示「主快捷键键被按下」。
///
/// 前端其余代码只认这一套（`KeyChord` / `model::shortcuts` 都是 command 语义），
/// 平台差异全部收在这里。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChordModifiers {
    pub command: bool,
    pub shift: bool,
    pub option: bool,
    pub control: bool,
}

/// 把 gpui 的原生修饰键翻译成「命令键」语义。
///
/// macOS：Command = `platform`，Option = `alt`，Control 照旧（保留终端 Ctrl 语义）。
/// 其它平台：命令键来自 `[shortcuts] primary_key`（`auto` = Alt），
/// 被当作命令键消费的那一位不再同时算作 option/control，避免
/// `Alt+Shift+T` 既被当成 `Cmd+Shift+T` 又被当成 `Alt+Shift+T`。
pub fn chord_modifiers(
    os: DesktopOs,
    primary: PrimaryModifier,
    modifiers: &Modifiers,
) -> ChordModifiers {
    match os {
        DesktopOs::MacOs => ChordModifiers {
            command: modifiers.platform,
            shift: modifiers.shift,
            option: modifiers.alt,
            control: modifiers.control,
        },
        DesktopOs::Linux | DesktopOs::Windows => {
            let command = primary.is_held(modifiers);
            ChordModifiers {
                command,
                shift: modifiers.shift,
                option: modifiers.alt && primary != PrimaryModifier::Alt,
                // 命令键就是 Control 时，Control 不再重复出现在和弦里。
                control: modifiers.control && primary != PrimaryModifier::Control,
            }
        }
    }
}

/// 主快捷键键的显示名（macOS 用 ⌘，其它平台用配置的名字）。
pub fn primary_label(os: DesktopOs, primary: PrimaryModifier) -> &'static str {
    match (os, primary) {
        (DesktopOs::MacOs, PrimaryModifier::Command) => "⌘",
        (_, modifier) => modifier.label(),
    }
}

/// 快捷键提示的分隔风格：macOS 连写（`⌘⇧T`），其它平台加号（`Ctrl+Shift+T`）。
pub const fn uses_compact_shortcut_labels(os: DesktopOs) -> bool {
    matches!(os, DesktopOs::MacOs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modifiers(control: bool, alt: bool, shift: bool, platform: bool) -> Modifiers {
        Modifiers {
            control,
            alt,
            shift,
            platform,
            function: false,
        }
    }

    #[test]
    fn macos_keeps_command_control_split() {
        let chord = chord_modifiers(
            DesktopOs::MacOs,
            PrimaryModifier::Command,
            &modifiers(false, false, false, true),
        );
        assert!(chord.command, "macOS Command 是命令键");
        assert!(!chord.control, "Command 不能算成 Control");

        // macOS 上普通 Ctrl 仍是终端控制键，不是命令键。
        let ctrl = chord_modifiers(
            DesktopOs::MacOs,
            PrimaryModifier::Command,
            &modifiers(true, false, false, false),
        );
        assert!(!ctrl.command);
        assert!(ctrl.control);
    }

    #[test]
    fn linux_and_windows_use_the_configured_primary_modifier() {
        for os in [DesktopOs::Linux, DesktopOs::Windows] {
            // auto = Alt
            let alt = chord_modifiers(
                os,
                PrimaryModifier::Alt,
                &modifiers(false, true, false, false),
            );
            assert!(alt.command, "{os:?}: Alt 是缺省命令键");
            assert!(!alt.option, "命令键不能同时算作 Option");

            // 终端 Ctrl 组合不受影响：Ctrl-C 仍然是控制字节。
            let ctrl_c = chord_modifiers(
                os,
                PrimaryModifier::Alt,
                &modifiers(true, false, false, false),
            );
            assert!(!ctrl_c.command);
            assert!(ctrl_c.control);

            // 显式配成 control 时，Ctrl 变成命令键。
            let primary_ctrl = chord_modifiers(
                os,
                PrimaryModifier::Control,
                &modifiers(true, false, false, false),
            );
            assert!(primary_ctrl.command);
            assert!(!primary_ctrl.control);
        }
    }

    #[test]
    fn shift_survives_the_translation() {
        let chord = chord_modifiers(
            DesktopOs::Linux,
            PrimaryModifier::Alt,
            &modifiers(false, true, true, false),
        );
        assert!(chord.command);
        assert!(chord.shift, "Alt+Shift+T 必须能区分 Alt+T");
    }

    #[test]
    fn auto_primary_key_defaults_per_platform() {
        assert_eq!(
            PrimaryModifier::default_for(DesktopOs::MacOs),
            PrimaryModifier::Command
        );
        assert_eq!(
            PrimaryModifier::default_for(DesktopOs::Linux),
            PrimaryModifier::Alt
        );
        assert_eq!(
            PrimaryModifier::default_for(DesktopOs::Windows),
            PrimaryModifier::Alt
        );
        assert_eq!(PrimaryModifier::parse("auto"), None);
        assert_eq!(
            PrimaryModifier::parse(" CONTROL "),
            Some(PrimaryModifier::Control)
        );
    }

    #[test]
    fn native_menu_bar_is_macos_only() {
        assert!(DesktopOs::MacOs.uses_native_menu_bar());
        assert!(!DesktopOs::Linux.uses_native_menu_bar());
        assert!(!DesktopOs::Windows.uses_native_menu_bar());
    }

    #[test]
    fn every_platform_has_a_monospace_default_and_fallbacks() {
        for os in [DesktopOs::MacOs, DesktopOs::Linux, DesktopOs::Windows] {
            assert!(!os.default_monospace_family().is_empty());
            assert!(
                os.monospace_fallbacks().contains(&"JetBrains Mono"),
                "{os:?}: 内置字体必须在回退栈里"
            );
        }
    }

    #[test]
    fn only_macos_hides_the_title_text() {
        assert!(DesktopOs::MacOs.hides_title_text(false));
        assert!(!DesktopOs::MacOs.hides_title_text(true));
        assert!(!DesktopOs::Linux.hides_title_text(false));
        assert!(!DesktopOs::Windows.hides_title_text(false));
    }

    #[test]
    fn shortcut_labels_follow_the_platform_style() {
        assert!(uses_compact_shortcut_labels(DesktopOs::MacOs));
        assert!(!uses_compact_shortcut_labels(DesktopOs::Linux));
        assert_eq!(primary_label(DesktopOs::Linux, PrimaryModifier::Alt), "Alt");
        assert_eq!(
            primary_label(DesktopOs::MacOs, PrimaryModifier::Command),
            "⌘"
        );
    }
}
