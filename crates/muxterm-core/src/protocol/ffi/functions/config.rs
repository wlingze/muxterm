//! Core-owned configuration transaction C ABI functions.

use std::ffi::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::config::{Rgb, Theme};
use crate::config_service::action_catalog::resolve_effective_keybindings;
use crate::config_service::{ConfigEvent, JsonPatchOperation, SettingsService};

use super::super::api::{cstr_opt, json_string, MuxtermHandle};

fn config_json_error(error: impl std::fmt::Display) -> *mut c_char {
    json_string(serde_json::json!({
        "ok": false,
        "error": {
            "code": "config_error",
            "message": error.to_string(),
            "path": serde_json::Value::Null,
            "suggestion": "检查 config schema、JSON Pointer 和字段类型",
        },
    }))
}

fn rgb_json(color: Rgb) -> serde_json::Value {
    serde_json::json!([color.0, color.1, color.2])
}

fn resolved_theme_json(values: &serde_json::Value) -> Option<serde_json::Value> {
    let theme = values.get("theme")?.as_object()?;
    let configured = theme.get("name")?.as_str()?.trim();
    let name = if configured.eq_ignore_ascii_case("system") {
        let resolved = Theme::resolve_name("system");
        if resolved == "black" {
            theme.get("dark")?.as_str()?.trim()
        } else {
            theme.get("light")?.as_str()?.trim()
        }
    } else {
        configured
    };
    let theme = Theme::load(name).ok()?;
    Some(serde_json::json!({
        "name": name,
        "background": rgb_json(theme.background),
        "foreground": rgb_json(theme.foreground),
        "cursor": rgb_json(theme.cursor),
        "colors": theme.colors.iter().copied().map(rgb_json).collect::<Vec<_>>(),
    }))
}

/// Return the resolved configuration, defaults, JSON Schema and UI Manifest.
/// The returned string is released with `muxterm_free_string`.
///
/// # Safety
/// `h` must be a live handle returned by `muxterm_new` or `muxterm_new_connect`.
#[no_mangle]
pub unsafe extern "C" fn muxterm_config_describe_json(h: *mut MuxtermHandle) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return config_json_error("handle 为空");
        }
        let snapshot = (&*h).settings.snapshot();
        let mut data = serde_json::to_value(snapshot).unwrap_or_else(|_| serde_json::json!({}));
        let resolved_theme =
            resolved_theme_json(data.get("values").unwrap_or(&serde_json::Value::Null))
                .unwrap_or(serde_json::Value::Null);
        let effective_keybindings = serde_json::to_value(resolve_effective_keybindings(
            &(&*h).settings.document().shortcuts,
        ))
        .unwrap_or_else(|_| serde_json::Value::Array(Vec::new()));
        if let Some(data) = data.as_object_mut() {
            data.insert(
                "path".into(),
                serde_json::json!((&*h).settings.path().to_string_lossy()),
            );
            data.insert("resolved_theme".into(), resolved_theme);
            data.insert("effective_keybindings".into(), effective_keybindings);
        }
        json_string(serde_json::json!({
            "ok": true,
            "data": data,
            "warnings": [],
        }))
    }))
    .unwrap_or_else(|_| config_json_error("config describe panic"))
}

/// Validate the default configuration or one explicit file without opening a
/// product handle.
#[no_mangle]
pub extern "C" fn muxterm_config_validate_json(path: *const c_char) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let service = match cstr_opt(path) {
            Some(path) => SettingsService::open(path),
            None => SettingsService::default_user(),
        };
        match service {
            Ok(service) => match service.document().validate() {
                Ok(()) => json_string(serde_json::json!({
                    "ok": true,
                    "data": {
                        "valid": true,
                        "path": service.path().to_string_lossy(),
                    },
                    "warnings": [],
                })),
                Err(error) => config_json_error(error),
            },
            Err(error) => config_json_error(error),
        }
    }))
    .unwrap_or_else(|_| config_json_error("config validate panic"))
}

/// Begin a Core-owned draft transaction.
///
/// # Safety
/// `h` must be a live handle returned by `muxterm_new` or `muxterm_new_connect`.
#[no_mangle]
pub unsafe extern "C" fn muxterm_config_begin_json(h: *mut MuxtermHandle) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return config_json_error("handle 为空");
        }
        let transaction = (&mut *h).settings.begin();
        json_string(serde_json::json!({
            "ok": true,
            "data": {"transaction": transaction},
        }))
    }))
    .unwrap_or_else(|_| config_json_error("config begin panic"))
}

/// Apply an RFC 6902-style add/replace/remove patch to a draft transaction.
///
/// # Safety
/// `h`, `transaction`, and `patch` must be valid pointers; the strings must be
/// NUL-terminated UTF-8 and the handle must remain alive for this call.
#[no_mangle]
pub unsafe extern "C" fn muxterm_config_patch_json(
    h: *mut MuxtermHandle,
    transaction: *const c_char,
    patch: *const c_char,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return config_json_error("handle 为空");
        }
        let Some(transaction) = cstr_opt(transaction) else {
            return config_json_error("transaction 为空");
        };
        let Some(patch) = cstr_opt(patch) else {
            return config_json_error("patch 为空");
        };
        let operations: Vec<JsonPatchOperation> = match serde_json::from_str(&patch) {
            Ok(value) => value,
            Err(error) => return config_json_error(format!("patch JSON 无效: {error}")),
        };
        match (&mut *h).settings.patch(&transaction, &operations) {
            Ok(result) => {
                json_string(serde_json::json!({"ok": true, "data": result, "warnings": []}))
            }
            Err(error) => config_json_error(error),
        }
    }))
    .unwrap_or_else(|_| config_json_error("config patch panic"))
}

/// Commit a draft transaction after validation and optimistic merge.
///
/// # Safety
/// `h` must be live and `transaction` must point to a NUL-terminated UTF-8
/// transaction ID created by `muxterm_config_begin_json`.
#[no_mangle]
pub unsafe extern "C" fn muxterm_config_commit_json(
    h: *mut MuxtermHandle,
    transaction: *const c_char,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return config_json_error("handle 为空");
        }
        let Some(transaction) = cstr_opt(transaction) else {
            return config_json_error("transaction 为空");
        };
        match (&mut *h).settings.commit(&transaction) {
            Ok(revision) => json_string(
                serde_json::json!({"ok": true, "data": {"revision": revision}, "warnings": []}),
            ),
            Err(error) => config_json_error(error),
        }
    }))
    .unwrap_or_else(|_| config_json_error("config commit panic"))
}

/// Cancel a draft transaction and roll back previews.
///
/// # Safety
/// `h` must be live and `transaction` must point to a NUL-terminated UTF-8
/// transaction ID created by `muxterm_config_begin_json`.
#[no_mangle]
pub unsafe extern "C" fn muxterm_config_cancel_json(
    h: *mut MuxtermHandle,
    transaction: *const c_char,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return config_json_error("handle 为空");
        }
        let Some(transaction) = cstr_opt(transaction) else {
            return config_json_error("transaction 为空");
        };
        match (&mut *h).settings.cancel(&transaction) {
            Ok(()) => json_string(serde_json::json!({"ok": true, "data": {}, "warnings": []})),
            Err(error) => config_json_error(error),
        }
    }))
    .unwrap_or_else(|_| config_json_error("config cancel panic"))
}

/// Reload configuration from disk and return the new revision.
///
/// # Safety
/// `h` must be a live handle returned by `muxterm_new` or `muxterm_new_connect`.
#[no_mangle]
pub unsafe extern "C" fn muxterm_config_reload_json(h: *mut MuxtermHandle) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return config_json_error("handle 为空");
        }
        match (&mut *h).settings.reload() {
            Ok(revision) => json_string(
                serde_json::json!({"ok": true, "data": {"revision": revision}, "warnings": []}),
            ),
            Err(error) => config_json_error(error),
        }
    }))
    .unwrap_or_else(|_| config_json_error("config reload panic"))
}

/// Drain configuration preview/commit/reload events.
///
/// # Safety
/// `h` must be a live handle returned by `muxterm_new` or `muxterm_new_connect`.
#[no_mangle]
pub unsafe extern "C" fn muxterm_config_events_json(h: *mut MuxtermHandle) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return config_json_error("handle 为空");
        }
        let events: Vec<ConfigEvent> = (&mut *h).settings.drain_events();
        json_string(serde_json::json!({
            "ok": true,
            "data": {"events": events},
            "warnings": [],
        }))
    }))
    .unwrap_or_else(|_| config_json_error("config events panic"))
}
