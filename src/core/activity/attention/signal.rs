//! 注意力信号：OSC 133 / BEL / 通知类 OSC 的语义化事件。

use super::state::PaneStatus;

/// 信号来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttentionSource {
    /// BEL（0x07）。
    Bel,
    /// OSC 9、合法 OSC 99、`OSC 777;notify` 或
    /// `OSC 1337;RequestAttention=...` 通知类。
    OscNotify,
    /// OSC 133 FTCS 序列。
    Osc133,
}

/// 一条注意力事件（LINUX-PLAN §0.4）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttentionSignal {
    /// OSC 133 C：命令开始出输出 → Working。
    CommandStart,
    /// OSC 133 D：命令结束，可选退出码 → Done。
    CommandDone { exit_code: Option<u8> },
    /// BEL 或通知类 OSC：需要关注 → Blocked。
    AttentionRequest { source: AttentionSource },
    /// Runtime 提供的权威 pane 状态（例如结构化 agent lifecycle）。
    ///
    /// `initial=true` 只建立 attach / authority handoff 的 bootstrap 现状，
    /// 不产生一条新的通知；后续结构化状态优先于同 pane 字节流里的
    /// OSC/BEL 猜测。
    AuthoritativeStatus { status: PaneStatus, initial: bool },
    /// Runtime 表示该 pane 已不再有权威结构化状态，恢复字节信号判断。
    ClearAuthoritativeStatus,
}

/// `pwd` / `ls` / 空回车这类瞬间结束的交互，不应进 Commands 列表。
pub fn is_transient_shell_command(command: &str) -> bool {
    let token = command
        .trim()
        .trim_start_matches(|ch: char| ch == '\'' || ch == '"')
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_start_matches('-')
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("");
    if token.is_empty() {
        return true;
    }
    matches!(
        token.to_ascii_lowercase().as_str(),
        "ls" | "pwd"
            | "cd"
            | "echo"
            | "true"
            | "false"
            | "clear"
            | "cls"
            | "date"
            | "whoami"
            | "hostname"
            | "type"
            | "which"
            | "dirs"
            | "pushd"
            | "popd"
            | ":"
            | "umask"
            | "alias"
            | "printf"
            | "test"
            | "["
            | "help"
            | "history"
    )
}

#[cfg(test)]
mod tests {
    use super::is_transient_shell_command;

    #[test]
    fn transient_commands_include_pwd_ls_and_empty_enter() {
        assert!(is_transient_shell_command(""));
        assert!(is_transient_shell_command("   "));
        assert!(is_transient_shell_command("pwd"));
        assert!(is_transient_shell_command("ls -la"));
        assert!(is_transient_shell_command("/bin/ls"));
        assert!(!is_transient_shell_command("cargo test"));
        assert!(!is_transient_shell_command("go"));
        assert!(!is_transient_shell_command("htop"));
    }
}
