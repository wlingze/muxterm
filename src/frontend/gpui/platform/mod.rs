//! 平台相关的接线。纯逻辑一律放在 `model/`，这里只做「gpui ↔ 操作系统」。
//!
//! 分层：
//! - [`desktop`]：OS 策略表（默认字体、命令键、菜单呈现、窗口装饰）。纯函数。
//! - [`clipboard_image`]：剪贴板图片 → PNG（各平台取到的原始格式不同）。
//! - `macos_menu` / `macos_keys`：macOS 原生菜单栏与物理键位监视器，
//!   仅 macOS 编译。
//! - 本文件：按 `target_os` 分派的稳定 API，其余模块只调这里。

pub mod clipboard_image;
pub mod desktop;

pub use desktop::{DesktopOs, PrimaryModifier};
use gpui::{Keystroke, Modifiers};

// objc 的 `msg_send!` / `class!` 宏内部会引用 `cargo-clippy` 等 cfg，
// 在 2021 edition 下触发 `unexpected_cfgs`。这是依赖宏的噪音，与本 crate 无关。
#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
pub mod macos_menu;

#[cfg(target_os = "macos")]
pub use macos_menu::{install_menu, refresh_menu};

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
pub mod macos_keys;

/// 当前平台。
pub const fn os() -> DesktopOs {
    DesktopOs::current()
}

/// 窗口标题栏配置。
///
/// macOS 侧 `titleVisibility` 是隐藏的：gpui 这里同样不画标题，只留 traffic
/// lights。Linux / Windows 的标题栏由窗口管理器画，传 `None` 让系统决定
/// （Wayland 的 xdg-decoration 走服务端装饰，GNOME 才会给关闭按钮）。
pub fn titlebar_options() -> Option<gpui::TitlebarOptions> {
    match os() {
        DesktopOs::MacOs => Some(gpui::TitlebarOptions {
            title: Some(" ".into()),
            appears_transparent: false,
            traffic_light_position: None,
        }),
        DesktopOs::Linux | DesktopOs::Windows => None,
    }
}

/// 当前平台有原生应用菜单栏吗（macOS 是，Linux / Windows 用自绘菜单条）。
pub const fn uses_native_menu_bar() -> bool {
    os().uses_native_menu_bar()
}

/// 运行期生效的命令键。默认值由平台决定（macOS = Command，其它 = Alt），
/// 配置 `[shortcuts] primary_key` 生效后覆盖。
static PRIMARY: std::sync::OnceLock<std::sync::Mutex<PrimaryModifier>> = std::sync::OnceLock::new();

fn primary_slot() -> &'static std::sync::Mutex<PrimaryModifier> {
    PRIMARY.get_or_init(|| std::sync::Mutex::new(PrimaryModifier::default_for(os())))
}

/// 写入配置里的 `[shortcuts] primary_key`（`auto` 回到平台默认）。
///
/// 由 `GpuiApp::apply_shortcut_values` 在每次配置快照落地时调用；失败
/// （未知值 / 锁中毒）保持原值，不 panic。
pub fn set_primary_key(value: &str) {
    let next = PrimaryModifier::parse(value).unwrap_or_else(|| PrimaryModifier::default_for(os()));
    if let Ok(mut slot) = primary_slot().lock() {
        *slot = next;
    }
}

/// 当前生效的命令键。
pub fn primary_modifier() -> PrimaryModifier {
    primary_slot()
        .lock()
        .map(|slot| *slot)
        .unwrap_or_else(|poisoned| *poisoned.into_inner())
}

/// 命令键的显示名（macOS `⌘`，Linux/Windows `Alt` / `Ctrl` …）。
pub fn primary_label() -> &'static str {
    desktop::primary_label(os(), primary_modifier())
}

/// 这一次输入事件里「命令键」按下了吗？
///
/// 鼠标链接点击、子视图（命令面板 / 设置窗口）的快捷键都走这里，
/// 不再各自读 `modifiers.platform`（Linux 上那是 Super，Windows 上是 Win）。
pub fn command_held(modifiers: &Modifiers) -> bool {
    desktop::chord_modifiers(os(), primary_modifier(), modifiers).command
}

/// 把 gpui 的原生按键归一化成「命令键」语义的 keystroke。
///
/// macOS 原样返回（Command = `platform`）。Linux / Windows 把配置的命令键
/// 映射到 `platform` 字段，其余字段清掉被消费的那一位——这样 `handle_key`
/// 里既有的 `modifiers.platform` 判定无需改动就能跨平台。
pub fn normalize_keystroke(keystroke: &Keystroke) -> Keystroke {
    let chord = desktop::chord_modifiers(os(), primary_modifier(), &keystroke.modifiers);
    let mut normalized = keystroke.clone();
    normalized.modifiers.platform = chord.command;
    normalized.modifiers.shift = chord.shift;
    normalized.modifiers.alt = chord.option;
    normalized.modifiers.control = chord.control;
    normalized
}

/// 没命中动作表的命令键组合要不要交回终端？
///
/// macOS 上 Command 是纯界面键，任何 Cmd 组合都不进终端；
/// Linux / Windows 上命令键常常同时是终端的 Meta 键（缺省 Alt），
/// 不交回去就会吃掉 readline 的 Alt-F / Alt-B。
pub fn unbound_command_reaches_terminal() -> bool {
    !matches!(os(), DesktopOs::MacOs)
}

/// 菜单 / 设置页里的快捷键提示：macOS 连写（`⌘⇧T`），其它平台加号（`Alt+Shift+T`）。
///
/// `parts` 是已经过滤好、排好序的修饰键显示名。
pub fn format_shortcut(parts: &[&str], key: &str) -> String {
    if desktop::uses_compact_shortcut_labels(os()) {
        let mut out = String::new();
        for label in parts {
            out.push_str(label);
        }
        out.push_str(key);
        out
    } else if parts.is_empty() {
        key.to_string()
    } else {
        format!("{}+{key}", parts.join("+"))
    }
}

/// 和弦的显示文本：修饰键顺序与键名都按平台。
///
/// 平台作为参数，测试可以在任意机器上验证三套写法。
pub fn chord_label_for(
    os: DesktopOs,
    primary: PrimaryModifier,
    command: bool,
    shift: bool,
    option: bool,
    control: bool,
    key: &str,
) -> String {
    let compact = desktop::uses_compact_shortcut_labels(os);
    let key_text = match key {
        "\r" | "enter" => {
            if compact {
                "↩".to_string()
            } else {
                "Enter".to_string()
            }
        }
        "escape" => {
            if compact {
                "⎋".to_string()
            } else {
                "Esc".to_string()
            }
        }
        "left" => "←".to_string(),
        "right" => "→".to_string(),
        "up" => "↑".to_string(),
        "down" => "↓".to_string(),
        " " => "Space".to_string(),
        other => other.to_uppercase(),
    };
    // 命令键就是 Alt 时，`option` 再写一次会变成 "Alt+Alt+I"；这类和弦在
    // Linux / Windows 上本来就没有物理含义，去重后更诚实。
    let mut labels: Vec<&str> = Vec::new();
    let candidates: [(&str, bool); 4] = if compact {
        [("⌃", control), ("⌥", option), ("⇧", shift), ("⌘", command)]
    } else {
        [
            (desktop::primary_label(os, primary), command),
            ("Shift", shift),
            ("Alt", option && primary != PrimaryModifier::Alt),
            ("Ctrl", control),
        ]
    };
    for (label, on) in candidates {
        if on && !labels.contains(&label) {
            labels.push(label);
        }
    }
    if compact {
        let mut out = String::new();
        for label in labels {
            out.push_str(label);
        }
        out.push_str(&key_text);
        out
    } else if labels.is_empty() {
        key_text
    } else {
        format!("{}+{key_text}", labels.join("+"))
    }
}

/// 当前平台 + 当前配置下的和弦显示文本。
pub fn chord_label(command: bool, shift: bool, option: bool, control: bool, key: &str) -> String {
    chord_label_for(
        os(),
        primary_modifier(),
        command,
        shift,
        option,
        control,
        key,
    )
}

/// 安装物理键位记录器（macOS）。其它平台是空操作。
#[cfg(target_os = "macos")]
pub fn install_physical_key_tracker() {
    macos_keys::install();
}

/// 非 macOS：没有需要安装的东西。
#[cfg(not(target_os = "macos"))]
pub fn install_physical_key_tracker() {}

/// 当前事件对应的 Colemak 锚点（不是 US QWERTY 标签）。
#[cfg(target_os = "macos")]
pub fn physical_key_for_event(expected_layout_key: &str) -> Option<&'static str> {
    macos_keys::physical_key_for_event(expected_layout_key)
}

/// 非 macOS 没有 AppKit 键码，回落到布局字符。
#[cfg(not(target_os = "macos"))]
pub fn physical_key_for_event(_expected_layout_key: &str) -> Option<&'static str> {
    None
}

/// 原生快捷键边界与菜单提示使用同一配置。
pub fn configure_shortcuts(
    map: &crate::frontend::gpui::model::shortcut_editor::ShortcutMap,
    modal: bool,
    recording: bool,
    qwerty_labels: bool,
) {
    #[cfg(target_os = "macos")]
    {
        macos_keys::configure(map, modal, recording);
        macos_menu::refresh_shortcut_keys(map, qwerty_labels);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (map, modal, recording, qwerty_labels);
}

/// 非 macOS 没有原生菜单，刷新是空操作（自绘菜单条直接读 `GpuiApp` 的状态）。
#[cfg(not(target_os = "macos"))]
pub fn refresh_menu(_state: crate::frontend::gpui::model::menu::MenuState) -> bool {
    false
}

/// 安装窗口菜单栏，返回菜单动作的接收端。
///
/// 只有 macOS 有原生菜单栏；Linux / Windows 的菜单是窗口内自绘的
/// （`chrome/menu_bar.rs`），不需要接收端。
#[cfg(not(target_os = "macos"))]
pub fn install_menu() -> Option<
    futures::channel::mpsc::UnboundedReceiver<
        crate::frontend::gpui::model::shortcuts::ShortcutCommand,
    >,
> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_modifier_defaults_to_platform_choice() {
        // 进程启动时还没读到配置，命令键就是平台默认。
        // 只断言**默认值**，不写全局：并行测试下改全局会让别的用例看到中间值。
        assert_eq!(
            PrimaryModifier::default_for(DesktopOs::current()),
            if cfg!(target_os = "macos") {
                PrimaryModifier::Command
            } else {
                PrimaryModifier::Alt
            }
        );
        // `auto` 解析成平台默认；显式值原样生效（纯函数，可并行）。
        assert_eq!(PrimaryModifier::parse("auto"), None);
        assert_eq!(
            PrimaryModifier::parse("control"),
            Some(PrimaryModifier::Control)
        );
        assert_eq!(PrimaryModifier::parse("alt"), Some(PrimaryModifier::Alt));
        assert_eq!(PrimaryModifier::parse("hyper"), None);
    }

    #[test]
    fn normalized_keystroke_keeps_platform_command_semantics() {
        let mut keystroke = Keystroke::parse("t").unwrap();
        keystroke.modifiers = Modifiers {
            control: false,
            alt: cfg!(not(target_os = "macos")),
            shift: false,
            platform: cfg!(target_os = "macos"),
            function: false,
        };
        let normalized = normalize_keystroke(&keystroke);
        assert!(normalized.modifiers.platform, "命令键要落到 platform 字段");
        assert!(!normalized.modifiers.alt, "被消费的 Alt 不能留在 alt 里");
    }

    #[test]
    fn configured_primary_key_can_be_written_and_read_back() {
        // 这里唯一会写全局的用例：写完立刻恢复平台默认，配合 test.sh 的
        // `--test-threads=1` 使用（CI 与本地门禁都这样跑）。
        set_primary_key("control");
        assert_eq!(primary_modifier(), PrimaryModifier::Control);
        set_primary_key("auto");
        assert_eq!(
            primary_modifier(),
            PrimaryModifier::default_for(DesktopOs::current())
        );
    }

    #[test]
    fn command_held_follows_the_platform_not_super() {
        let ctrl = Modifiers {
            control: true,
            alt: false,
            shift: false,
            platform: false,
            function: false,
        };
        let super_key = Modifiers {
            control: false,
            alt: false,
            shift: false,
            platform: true,
            function: false,
        };
        if cfg!(target_os = "macos") {
            assert!(!command_held(&ctrl), "macOS 上 Ctrl 不是命令键");
            assert!(command_held(&super_key));
        } else {
            assert!(!command_held(&ctrl), "Linux/Windows 缺省命令键是 Alt");
            assert!(!command_held(&super_key), "Super/Win 不是缺省命令键");
        }
    }

    #[test]
    fn unbound_command_only_reaches_the_terminal_off_macos() {
        assert_eq!(
            unbound_command_reaches_terminal(),
            !cfg!(target_os = "macos")
        );
    }

    #[test]
    fn menu_bar_kind_matches_the_platform() {
        assert_eq!(uses_native_menu_bar(), cfg!(target_os = "macos"));
    }

    #[test]
    fn shortcut_formatting_matches_platform_style() {
        if cfg!(target_os = "macos") {
            assert_eq!(format_shortcut(&["⇧", "⌘"], "T"), "⇧⌘T");
        } else {
            let label = format_shortcut(&[primary_label(), "Shift"], "T");
            assert!(label.ends_with("+Shift+T"), "实际: {label}");
            assert!(label.contains(primary_label()), "实际: {label}");
        }
        assert_eq!(format_shortcut(&[], "T"), "T");
    }

    #[test]
    fn chord_labels_follow_the_platform_notation() {
        let label = chord_label(true, false, false, false, "d");
        if cfg!(target_os = "macos") {
            assert_eq!(label, "⌘D");
        } else {
            assert_eq!(label, format!("{}+D", primary_label()));
        }
        // 键名也跟着平台：macOS 用符号，其它平台用文字。
        let enter = chord_label(false, false, false, false, "enter");
        if cfg!(target_os = "macos") {
            assert_eq!(enter, "↩");
        } else {
            assert_eq!(enter, "Enter");
        }
    }
}
