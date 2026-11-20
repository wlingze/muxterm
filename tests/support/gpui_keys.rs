//! 无头测试里的快捷键写法：统一用 `cmd-…` 表示「平台命令键」。
//!
//! 生产代码把命令键归一化到 `[shortcuts] primary_key`：macOS 是 Command，
//! Linux / Windows 缺省是 Alt（Ctrl 必须留给终端的控制字节）。所以同一个
//! 行为测试在 Linux/Windows 上要发 `alt-…`，在 macOS 上仍然是 `cmd-…`。
//!
//! 用 `keys("cmd-shift-t")` 包一层，测试就能三平台共用一份断言。

use muxterm::frontend::gpui::platform::{primary_modifier, PrimaryModifier};

/// 当前平台的命令键修饰位（`Modifiers::platform` 在 Linux 上是 Ctrl）。
pub fn modifiers() -> gpui::Modifiers {
    match primary_modifier() {
        PrimaryModifier::Command => gpui::Modifiers {
            platform: true,
            ..Default::default()
        },
        PrimaryModifier::Control => gpui::Modifiers {
            control: true,
            ..Default::default()
        },
        // Alt / Super：缺省命令键是 Alt。
        _ => gpui::Modifiers {
            alt: true,
            ..Default::default()
        },
    }
}

/// gpui keystroke 语法里的命令键名。
pub fn primary_token() -> &'static str {
    match primary_modifier() {
        PrimaryModifier::Command => "cmd",
        PrimaryModifier::Control => "ctrl",
        PrimaryModifier::Alt => "alt",
        // Super（Win 键）在 gpui 的 keystroke 语法里没有独立写法，测试沿用 alt。
        PrimaryModifier::Super => "alt",
    }
}

/// 把每个和弦里的 `cmd` 换成当前平台的命令键；多和弦串按空格逐段处理。
pub fn keys(chords: &str) -> String {
    let token = primary_token();
    chords
        .split(' ')
        .map(|chord| {
            let mut parts: Vec<&str> = chord.split('-').collect();
            if let Some(first) = parts.first_mut() {
                if *first == "cmd" {
                    *first = token;
                }
            }
            parts.join("-")
        })
        .collect::<Vec<_>>()
        .join(" ")
}
