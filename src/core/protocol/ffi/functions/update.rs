//! 客户端自更新 C ABI。
//!
//! 前端只调用三个动作：读状态、检查、安装。真正的网络/校验/落盘都在 Core，
//! 前端不接触 HTTP 与安装目录。

use std::ffi::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};

use super::support::{json_error, json_string, MuxtermHandle};

/// 读取当前更新状态（供前端首帧渲染提醒与按钮）。
///
/// # Safety
/// `h` 是有效的 handle。
#[no_mangle]
pub unsafe extern "C" fn muxterm_update_status_json(h: *mut MuxtermHandle) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return json_error("handle 为空");
        }
        let handle = &mut *h;
        handle.updater.poll();
        handle.updater.maybe_auto_check();
        json_string(serde_json::json!({
            "ok": true,
            "status": handle.updater.status_json(),
        }))
    }))
    .unwrap_or_else(|_| json_error("update status panic"))
}

/// 开始检查新版本；结果是异步的，通过 `muxterm_update_take_events_json`
/// 与 `muxterm_update_status_json` 观察。
///
/// # Safety
/// `h` 是有效的 handle。
#[no_mangle]
pub unsafe extern "C" fn muxterm_update_check_json(h: *mut MuxtermHandle) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return json_error("handle 为空");
        }
        let handle = &mut *h;
        let started = handle.updater.start_check();
        json_string(serde_json::json!({
            "ok": true,
            "started": started,
            "status": handle.updater.status_json(),
        }))
    }))
    .unwrap_or_else(|_| json_error("update check panic"))
}

/// 一键更新：下载已发现的新版本，校验后安装。
///
/// # Safety
/// `h` 是有效的 handle。
#[no_mangle]
pub unsafe extern "C" fn muxterm_update_install_json(h: *mut MuxtermHandle) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return json_error("handle 为空");
        }
        let handle = &mut *h;
        let started = handle.updater.start_install();
        json_string(serde_json::json!({
            "ok": true,
            "started": started,
            "status": handle.updater.status_json(),
        }))
    }))
    .unwrap_or_else(|_| json_error("update install panic"))
}

/// 取走更新状态变更事件（与其它 lane 一样是 drain 语义）。
///
/// # Safety
/// `h` 是有效的 handle。
#[no_mangle]
pub unsafe extern "C" fn muxterm_update_take_events_json(h: *mut MuxtermHandle) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return json_error("handle 为空");
        }
        let handle = &mut *h;
        let mut events = handle.updater.take_events_json();
        if let Some(object) = events.as_object_mut() {
            object.insert("ok".into(), serde_json::json!(true));
        }
        json_string(events)
    }))
    .unwrap_or_else(|_| json_error("update events panic"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;
    use std::ptr;

    #[test]
    fn null_handle_reports_an_error_instead_of_crashing() {
        unsafe {
            for function in [
                muxterm_update_status_json,
                muxterm_update_check_json,
                muxterm_update_install_json,
                muxterm_update_take_events_json,
            ] {
                let pointer = function(ptr::null_mut());
                assert!(!pointer.is_null());
                let text = CStr::from_ptr(pointer).to_str().unwrap();
                let value: serde_json::Value = serde_json::from_str(text).unwrap();
                assert_eq!(value["ok"], false);
                crate::protocol::ffi::functions::handle::muxterm_free_string(pointer);
            }
        }
    }

    #[test]
    fn exported_symbols_are_declared_in_the_c_header() {
        let header = include_str!("../../../../frontend/macos/CoreBridge/include/muxterm.h");
        for symbol in [
            "muxterm_update_status_json",
            "muxterm_update_check_json",
            "muxterm_update_install_json",
            "muxterm_update_take_events_json",
        ] {
            assert!(
                header.contains(symbol),
                "muxterm.h 必须声明 {symbol}，否则 Swift/Windows 前端无法调用"
            );
        }
    }
}
