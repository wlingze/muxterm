//! Pane 注意力状态机（LINUX-PLAN §8.1 C2.2）。
//!
//! 转移表（未列出的格子 = 保持原状态）：
//! - Unknown/Idle/Working/Done/Blocked × CommandStart/CommandDone/
//!   AttentionRequest/UserInput/BecameVisible/RegexMatch/RegexClear/OutputActivity
//! - Blocked + UserInput → Idle（输入才算处理，看见不算）
//! - Done + BecameVisible → Idle；Blocked + BecameVisible → 仍 Blocked
//! - AttentionRequest 幂等；清早了会自愈（Idle + AttentionRequest → Blocked）

/// Pane 注意力状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneStatus {
    Unknown,
    Working,
    Done,
    Blocked,
    Idle,
}

/// 状态机事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneEvent {
    CommandStart,
    CommandDone { exit_code: Option<u8> },
    AttentionRequest,
    UserInput,
    BecameVisible,
    RegexMatch,
    RegexClear,
    OutputActivity,
}

/// 完整转移表：`transition(state, event)`。
///
/// 行 = 当前状态，列 = 事件；None 表示保持原状态。
pub fn transition(state: PaneStatus, event: PaneEvent) -> PaneStatus {
    use PaneEvent::*;
    use PaneStatus::*;
    match (state, event) {
        (Unknown, CommandStart) => Working,
        (Unknown, CommandDone { .. }) => Done,
        (Unknown, AttentionRequest) => Blocked,
        (Unknown, RegexMatch) => Blocked,
        (Idle, CommandStart) => Working,
        (Idle, CommandDone { .. }) => Done,
        (Idle, AttentionRequest) => Blocked,
        (Idle, RegexMatch) => Blocked,
        (Working, CommandStart) => Working,
        (Working, CommandDone { .. }) => Done,
        (Working, AttentionRequest) => Blocked,
        (Working, RegexMatch) => Blocked,
        (Done, CommandStart) => Working,
        (Done, CommandDone { .. }) => Done,
        (Done, AttentionRequest) => Blocked,
        (Done, BecameVisible) => Idle,
        (Done, RegexMatch) => Blocked,
        (Blocked, CommandStart) => Working,
        (Blocked, CommandDone { .. }) => Done,
        (Blocked, AttentionRequest) => Blocked,
        (Blocked, UserInput) => Idle,
        (Blocked, RegexMatch) => Blocked,
        (Blocked, RegexClear) => Idle,
        (Blocked, OutputActivity) => Working,
        _ => state,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 表驱动穷举：5 状态 × 8 事件全断言（含保持原状态）。
    #[test]
    fn transition_table_exhaustive() {
        use PaneEvent::*;
        use PaneStatus::*;
        let events = [
            CommandStart,
            CommandDone { exit_code: None },
            CommandDone { exit_code: Some(0) },
            AttentionRequest,
            UserInput,
            BecameVisible,
            RegexMatch,
            RegexClear,
            OutputActivity,
        ];
        for state in [Unknown, Idle, Working, Done, Blocked] {
            for event in events {
                let got = transition(state, event);
                let want = match (state, event) {
                    (Unknown, CommandStart) => Working,
                    (Unknown, CommandDone { .. }) => Done,
                    (Unknown, AttentionRequest) => Blocked,
                    (Unknown, RegexMatch) => Blocked,
                    (Idle, CommandStart) => Working,
                    (Idle, CommandDone { .. }) => Done,
                    (Idle, AttentionRequest) => Blocked,
                    (Idle, RegexMatch) => Blocked,
                    (Working, CommandStart) => Working,
                    (Working, CommandDone { .. }) => Done,
                    (Working, AttentionRequest) => Blocked,
                    (Working, RegexMatch) => Blocked,
                    (Done, CommandStart) => Working,
                    (Done, CommandDone { .. }) => Done,
                    (Done, AttentionRequest) => Blocked,
                    (Done, BecameVisible) => Idle,
                    (Done, RegexMatch) => Blocked,
                    (Blocked, CommandStart) => Working,
                    (Blocked, CommandDone { .. }) => Done,
                    (Blocked, AttentionRequest) => Blocked,
                    (Blocked, UserInput) => Idle,
                    (Blocked, RegexMatch) => Blocked,
                    (Blocked, RegexClear) => Idle,
                    (Blocked, OutputActivity) => Working,
                    _ => state,
                };
                assert_eq!(got, want, "state={state:?} event={event:?}");
            }
        }
    }

    #[test]
    fn blocked_survives_became_visible() {
        assert_eq!(
            transition(PaneStatus::Blocked, PaneEvent::BecameVisible),
            PaneStatus::Blocked
        );
    }

    #[test]
    fn done_clears_on_visible() {
        assert_eq!(
            transition(PaneStatus::Done, PaneEvent::BecameVisible),
            PaneStatus::Idle
        );
    }

    #[test]
    fn blocked_clears_on_user_input() {
        assert_eq!(
            transition(PaneStatus::Blocked, PaneEvent::UserInput),
            PaneStatus::Idle
        );
    }

    #[test]
    fn re_lights_after_clear() {
        assert_eq!(
            transition(PaneStatus::Idle, PaneEvent::AttentionRequest),
            PaneStatus::Blocked
        );
        assert_eq!(
            transition(PaneStatus::Idle, PaneEvent::RegexMatch),
            PaneStatus::Blocked
        );
    }
}
