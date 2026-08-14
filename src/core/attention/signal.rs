//! 注意力信号：OSC 133 / BEL / 通知类 OSC 的语义化事件。

/// 信号来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttentionSource {
    /// BEL（0x07）。
    Bel,
    /// OSC 9 / 99 / 777 / 1337 通知类。
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
}
