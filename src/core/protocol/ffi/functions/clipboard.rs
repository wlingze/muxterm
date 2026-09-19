//! 图片粘贴 C ABI：PNG 字节在 start 返回前复制，后台不借用 handle。
use super::support::{
    cstr_opt, json_error, json_string, parse_workspace_id, resolve_c_io_pane, MuxtermHandle,
};
use std::ffi::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// # Safety
/// handle 必须独占有效；workspace 是 C 字符串，png 指向 len 个可读字节。
#[no_mangle]
pub unsafe extern "C" fn muxterm_image_paste_start_json(
    h: *mut MuxtermHandle,
    workspace: *const c_char,
    pane: u32,
    png: *const u8,
    len: usize,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null()
            || workspace.is_null()
            || png.is_null()
            || len > crate::clipboard::MAX_IMAGE_BYTES
        {
            return json_error("invalid image paste arguments or image exceeds 20 MiB");
        }
        let Some(workspace) = cstr_opt(workspace) else {
            return json_error("invalid workspace");
        };
        let id = parse_workspace_id(&workspace);
        let handle = &mut *h;
        let Some(ws) = handle.pool.get(&id) else {
            return json_error("workspace is closed");
        };
        let Some(pane) = resolve_c_io_pane(pane, ws) else {
            return json_error("pane is closed");
        };
        match handle.start_image_paste(id, pane, std::slice::from_raw_parts(png, len).to_vec()) {
            Ok(()) => json_string(serde_json::json!({"ok":true})),
            Err(error) => json_error(error),
        }
    }))
    .unwrap_or_else(|_| json_error("image paste start panic"))
}

/// # Safety
/// handle 必须独占有效。与其它 poll 一样仅由 frontend event owner 调用。
#[no_mangle]
pub unsafe extern "C" fn muxterm_image_paste_poll_json(h: *mut MuxtermHandle) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return json_error("handle is null");
        }
        match (&mut *h).poll_image_paste() {
            Ok(path) => {
                json_string(serde_json::json!({"ok":true,"pending":path.is_none(),"path":path}))
            }
            Err(error) => json_error(error),
        }
    }))
    .unwrap_or_else(|_| json_error("image paste poll panic"))
}
