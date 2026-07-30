//! 回调注册（可选，替代纯轮询）。

use super::api::MuxtermHandle;
use super::types::CStateChange;

/// pane 增量输出回调。
pub type OnOutputFn = extern "C" fn(pane_id: u32, data: *const u8, len: usize);

/// 通用状态变更回调。
pub type OnStateChangeFn = extern "C" fn(event: *const CStateChange);

/// 注册在 [`MuxtermHandle`] 上的回调。
#[derive(Default, Clone, Copy)]
pub struct FfiCallbacks {
    pub on_output: Option<OnOutputFn>,
    pub on_state_change: Option<OnStateChangeFn>,
}

/// 设置回调。传 `None` 清除对应回调。
///
/// # Safety
/// `h` 必须是 `muxterm_new` 返回且未 `muxterm_free` 的指针。
#[no_mangle]
pub unsafe extern "C" fn muxterm_set_callbacks(
    h: *mut MuxtermHandle,
    output: Option<OnOutputFn>,
    state_change: Option<OnStateChangeFn>,
) {
    if h.is_null() {
        return;
    }
    let handle = &mut *h;
    handle.callbacks.on_output = output;
    handle.callbacks.on_state_change = state_change;
}
