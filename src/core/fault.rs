//! 进程级 fault 兜底：panic hook + catch_unwind 报告。
//!
//! glib 的 `extern "C"` trampoline 不能 unwind，Rust panic 穿过去会 abort。
//! 所有会进 C 回调的热路径（16ms poll、feed_reply_state 等）先
//! `catch_unwind`，Err 走 [`report`]；没被接住的 panic 由 [`install_hook`]
//! 打进 tracing（`--log-file` 可见），hook 本身不能再 panic。

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Mutex;

static LAST_FAULT: Mutex<Option<String>> = Mutex::new(None);
static HOOK_INSTALLED: std::sync::Once = std::sync::Once::new();

/// 安装全局 panic hook：payload + backtrace 进 tracing（target `muxterm::fault`）。
///
/// 在 `init_logging` 成功之后调用一次。hook 内禁止 panic。
pub fn install_hook() {
    HOOK_INSTALLED.call_once(|| {
        let default = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let message = panic_message(info.payload());
            let backtrace = std::backtrace::Backtrace::force_capture();
            tracing::error!(
                target: "muxterm::fault",
                message = %message,
                backtrace = %backtrace,
                "panic captured by muxterm fault hook"
            );
            // 记录最近一条，测试可读；不 panic。
            if let Ok(mut last) = LAST_FAULT.lock() {
                *last = Some(message);
            }
            default(info);
        }));
    });
}

/// 从 panic payload 提取第一行消息（String / &str / 其它 → 类型名）。
fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        format!("panic payload: {:?}", payload.type_id())
    }
}

/// catch_unwind 的 Err 走这里：记录最近一条 + tracing::error。
///
/// 调用方负责 UI 兜底（弹窗等）；这里只保证日志与测试可读。
pub fn report(where_: &str, payload: Box<dyn Any + Send>) {
    let message = panic_message(&*payload);
    let backtrace = std::backtrace::Backtrace::force_capture();
    tracing::error!(
        target: "muxterm::fault",
        where_ = %where_,
        message = %message,
        backtrace = %backtrace,
        "fault reported"
    );
    if let Ok(mut last) = LAST_FAULT.lock() {
        *last = Some(message);
    }
}

/// 最近一条 fault 消息（测试用）。
pub fn last_message() -> Option<String> {
    LAST_FAULT.lock().ok().and_then(|l| l.clone())
}

/// 清空最近一条（测试隔离用）。
pub fn clear_last_message() {
    if let Ok(mut last) = LAST_FAULT.lock() {
        *last = None;
    }
}

/// 便捷：`catch_unwind` + `report`，返回 `Option<T>`。
pub fn run<T>(where_: &str, f: impl FnOnce() -> T) -> Option<T> {
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => Some(value),
        Err(payload) => {
            report(where_, payload);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// W19d：catch_unwind 里 panic，report 后 last_message 含 token，进程不结束。
    #[test]
    fn fault_report_captures_message_without_aborting() {
        clear_last_message();
        let result = run("test", || {
            panic!("W19_FAULT_TOKEN");
        });
        assert!(result.is_none(), "panic 应被接住");
        let last = last_message().expect("report 应记录最近一条");
        assert!(
            last.contains("W19_FAULT_TOKEN"),
            "last_message 必须含 token: {last}"
        );
    }

    #[test]
    fn run_returns_value_on_success() {
        let value = run("test", || 42);
        assert_eq!(value, Some(42));
    }
}
