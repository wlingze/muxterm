//! Runtime 为新终端子进程准备环境，不能依赖 GUI 的启动方式。

use crate::transport::{ChannelRequest, TargetConnection};

pub(super) fn installed_utf8_locale(connection: &dyn TargetConnection) -> Option<String> {
    let output = connection
        .exec_command(ChannelRequest::Exec {
            argv: vec!["locale".into(), "-a".into()],
            cwd: None,
            env: Vec::new(),
            pty: None,
        })
        .ok()?;
    if output.status != 0 {
        return None;
    }
    choose_utf8_locale(&String::from_utf8_lossy(&output.stdout))
}

pub(super) fn choose_utf8_locale(output: &str) -> Option<String> {
    let locales: Vec<&str> = output.lines().map(str::trim).collect();
    ["C.UTF-8", "C.utf8", "en_US.UTF-8", "en_US.utf8"]
        .into_iter()
        .find_map(|preferred| {
            locales
                .iter()
                .find(|available| available.eq_ignore_ascii_case(preferred))
                .map(|available| (*available).to_string())
        })
        .or_else(|| {
            locales
                .into_iter()
                .find(|locale| is_utf8(locale))
                .map(str::to_string)
        })
}

fn is_utf8(value: &str) -> bool {
    value
        .trim()
        .to_ascii_lowercase()
        .replace('-', "")
        .ends_with("utf8")
}

pub(super) fn terminal_metadata() -> Vec<(String, String)> {
    vec![
        ("TERM".into(), "xterm-256color".into()),
        ("COLORTERM".into(), "truecolor".into()),
    ]
}

/// 保留目标已有的 UTF-8 locale；桌面环境缺失或非 UTF-8 时选用目标实际安装的值。
/// 失败不缓存，下一次建 pane 可以重新探测。
pub(super) fn shell_environment(
    connection: &dyn TargetConnection,
) -> Option<Vec<(String, String)>> {
    let output = connection
        .exec_command(ChannelRequest::Exec {
            argv: vec!["locale".into(), "charmap".into()],
            cwd: None,
            env: Vec::new(),
            pty: None,
        })
        .ok()?;
    let mut env = terminal_metadata();
    if output.status != 0 || !is_utf8(&String::from_utf8_lossy(&output.stdout)) {
        let locale = installed_utf8_locale(connection)?;
        // LC_ALL 优先于 LC_CTYPE，必须一起覆盖失效的 ASCII 环境。
        for key in ["LANG", "LC_CTYPE", "LC_ALL"] {
            env.push((key.into(), locale.clone()));
        }
    }
    Some(env)
}
