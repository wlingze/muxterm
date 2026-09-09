//! FFI handle construction and ownership C ABI functions.

use std::ffi::{c_char, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;

use crate::activity::ActivityState;
use crate::config::SettingsService;
use crate::logging::{init_logging, LoggingConfig};
use crate::projects::{ProjectStore, ProjectsService};
use crate::protocol::terminal::emulate::DEFAULT_SCROLLBACK_LINES;
use crate::runtime::{DaemonRuntime, ShellRuntime, TmuxRuntime};
use crate::workspace::template::WorkspaceTemplate;
use muxterm_protocol::WorkspaceId;

use super::super::callbacks::FfiCallbacks;
use super::support::{cstr_opt, MuxtermHandle};

fn open_settings_service() -> SettingsService {
    match SettingsService::default_user() {
        Ok(mut service) => {
            if let Err(error) = service.migrate_legacy_quickconnect() {
                tracing::warn!(
                    target = "muxterm::config",
                    "QuickConnect 迁移未完成: {error}"
                );
            }
            if let Err(error) = service.migrate_legacy_linux_preferences() {
                tracing::warn!(
                    target = "muxterm::config",
                    "Linux preferences 迁移未完成: {error}"
                );
            }
            service
        }
        Err(error) => {
            tracing::warn!(
                target = "muxterm::config",
                "配置不可用，使用内存默认值: {error}"
            );
            let path = crate::config::Config::user_config_path()
                .unwrap_or_else(|| std::path::PathBuf::from("config.toml"));
            SettingsService::in_memory_default(path)
        }
    }
}

/// Create an empty Catalog handle.
#[no_mangle]
pub extern "C" fn muxterm_catalog_new() -> *mut MuxtermHandle {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(rt) = new_ffi_runtime() else {
            return ptr::null_mut();
        };
        boxed_handle(crate::catalog::Catalog::with_builtins(), rt)
    }))
    .unwrap_or(ptr::null_mut())
}

/// Create a legacy handle and open one workspace.
#[no_mangle]
pub extern "C" fn muxterm_new(
    runtime_type: *const c_char,
    socket: *const c_char,
    session: *const c_char,
) -> *mut MuxtermHandle {
    legacy_new_handle(
        runtime_type,
        socket,
        session,
        ptr::null(),
        ptr::null(),
        None,
    )
}

/// Create and connect a legacy handle in one step.
#[no_mangle]
pub extern "C" fn muxterm_new_connect(
    runtime_type: *const c_char,
    socket: *const c_char,
    session: *const c_char,
    ssh_alias: *const c_char,
    start_directory: *const c_char,
) -> *mut MuxtermHandle {
    legacy_new_handle(
        runtime_type,
        socket,
        session,
        ssh_alias,
        start_directory,
        None,
    )
}

/// Create and connect a legacy handle with an initial client size.
#[no_mangle]
pub extern "C" fn muxterm_new_connect_sized(
    runtime_type: *const c_char,
    socket: *const c_char,
    session: *const c_char,
    ssh_alias: *const c_char,
    start_directory: *const c_char,
    cols: u16,
    rows: u16,
) -> *mut MuxtermHandle {
    legacy_new_handle(
        runtime_type,
        socket,
        session,
        ssh_alias,
        start_directory,
        (cols >= 2 && rows >= 1).then_some((cols, rows)),
    )
}

fn legacy_new_handle(
    runtime_type: *const c_char,
    socket: *const c_char,
    session: *const c_char,
    ssh_alias: *const c_char,
    start_directory: *const c_char,
    client_size: Option<(u16, u16)>,
) -> *mut MuxtermHandle {
    let kind = cstr_opt(runtime_type)
        .unwrap_or_else(|| "local".into())
        .to_ascii_lowercase();
    let sock = cstr_opt(socket);
    let sess = cstr_opt(session);
    let alias = cstr_opt(ssh_alias);
    let start_dir = cstr_opt(start_directory);

    let Some(rt) = new_ffi_runtime() else {
        return ptr::null_mut();
    };
    let mut catalog = crate::catalog::Catalog::with_builtins();

    let (id, name, runtime, scrollback_lines) =
        match legacy_runtime_spec(&kind, sock, sess, alias, start_dir, client_size) {
            Some(spec) => spec,
            None => return ptr::null_mut(),
        };
    let fut =
        catalog
            .pool_mut()
            .open_with_scrollback(id.clone(), name, scrollback_lines, move |_| Ok(runtime));
    if rt.block_on(fut).is_err() {
        return ptr::null_mut();
    }

    boxed_handle(catalog, rt)
}

fn new_ffi_runtime() -> Option<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .ok()
}

fn boxed_handle(
    mut catalog: crate::catalog::Catalog,
    rt: tokio::runtime::Runtime,
) -> *mut MuxtermHandle {
    let attention_config = crate::config::Config::load()
        .map(|c| c.attention)
        .unwrap_or_default();
    let settings = open_settings_service();
    let templates = settings
        .document()
        .templates
        .iter()
        .cloned()
        .map(WorkspaceTemplate::try_from)
        .collect::<anyhow::Result<Vec<_>>>()
        .and_then(|templates| catalog.set_templates(templates));
    if let Err(error) = templates {
        tracing::warn!(
            target = "muxterm::config",
            "WorkspaceTemplate 加载失败，使用空注册表: {error}"
        );
    }
    let projects = match ProjectStore::from_settings(&settings) {
        Ok(store) => ProjectsService::new(store),
        Err(error) => {
            tracing::warn!(
                target = "muxterm::config",
                "Project 加载失败，使用内存空集合: {error}"
            );
            ProjectsService::in_memory()
        }
    };
    // The FFI handle is the product composition root. Move the live pool out
    // of Catalog so all runtimes and target connections have one Core owner
    // in production.
    let pool = catalog.take_pool();
    let connections = catalog.take_connections();
    Box::into_raw(Box::new(MuxtermHandle {
        catalog,
        connections,
        pool,
        projects,
        rt,
        callbacks: FfiCallbacks::default(),
        activity: ActivityState::new(attention_config),
        settings,
        event_data: Vec::new(),
        event_names: Vec::new(),
        tab_names: Vec::new(),
        layout_nodes: Vec::new(),
        deferred_events: std::collections::VecDeque::new(),
        workspace_ids: Vec::new(),
    }))
}

/// Legacy runtime specification used by deprecated constructors.
fn legacy_runtime_spec(
    kind: &str,
    sock: Option<String>,
    sess: Option<String>,
    alias: Option<String>,
    start_dir: Option<String>,
    client_size: Option<(u16, u16)>,
) -> Option<(
    WorkspaceId,
    String,
    std::boxed::Box<dyn crate::runtime::Runtime>,
    usize,
)> {
    let scrollback_lines = configured_scrollback_lines();
    let runtime: std::boxed::Box<dyn crate::runtime::Runtime> = match kind {
        "tmux" => {
            let sock_ref = sock.as_deref();
            let mut tmux = if let Some(name) = sess.as_deref() {
                TmuxRuntime::new_with_attach(sock_ref, name)
            } else if let Some(dir) = start_dir.as_deref() {
                TmuxRuntime::new_with_cwd(sock_ref, Some(dir))
            } else {
                TmuxRuntime::new(sock_ref)
            };
            tmux.set_scrollback_lines(scrollback_lines as u32);
            if let Some((cols, rows)) = client_size {
                tmux.set_client_size(cols, rows);
            }
            std::boxed::Box::new(tmux)
        }
        "ssh" | "tmux-ssh" => {
            let (alias_name, sock_owned) =
                TmuxRuntime::ssh_alias_and_tmux_socket(sock.as_deref(), alias.as_deref())?;
            let sock_ref = sock_owned.as_deref();
            let mut tmux = if let Some(name) = sess.as_deref() {
                TmuxRuntime::new_ssh_attach(&alias_name, sock_ref, name)
            } else {
                TmuxRuntime::new_ssh(&alias_name, sock_ref)
            };
            tmux.set_scrollback_lines(scrollback_lines as u32);
            if let Some((cols, rows)) = client_size {
                tmux.set_client_size(cols, rows);
            }
            std::boxed::Box::new(tmux)
        }
        "daemon" => {
            let name = sess.clone().unwrap_or_else(|| "default".into());
            let path = sock
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| muxterm_protocol::daemon::default_socket_path(&name));
            std::boxed::Box::new(DaemonRuntime::new(path, name))
        }
        _ => std::boxed::Box::new(ShellRuntime::new(
            "$SHELL",
            start_dir.as_deref().unwrap_or(""),
        )),
    };
    let transport = if matches!(kind, "ssh" | "tmux-ssh") {
        "ssh"
    } else {
        "local"
    };
    let runtime_kind = if matches!(kind, "daemon") {
        "daemon"
    } else if matches!(kind, "ssh" | "tmux-ssh") || kind == "tmux" {
        "tmux"
    } else {
        "shell"
    };
    let session = sess.unwrap_or_default();
    let id = WorkspaceId::new(transport, alias.as_deref(), &session, runtime_kind, "");
    let name = if session.is_empty() {
        "muxterm".to_string()
    } else {
        session.clone()
    };
    Some((id, name, runtime, scrollback_lines))
}

/// Read the configured scrollback limit for compatibility constructors.
pub(crate) fn configured_scrollback_lines() -> usize {
    crate::config::Config::load()
        .map(|config| config.scrollback.lines.max(1) as usize)
        .unwrap_or(DEFAULT_SCROLLBACK_LINES)
}

/// Free a handle and shut down its workspaces.
///
/// # Safety
/// `h` came from a constructor and is freed at most once.
#[no_mangle]
pub unsafe extern "C" fn muxterm_free(h: *mut MuxtermHandle) {
    if h.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut handle = Box::from_raw(h);
        handle.pool_mut().shutdown_all();
    }));
}

/// 初始化核心日志（macOS .app 由 Swift 在创建 CoreBridge 前调用）。
///
/// `level` 取 `trace` / `debug` / `info` / `warn` / `error`；`log_file` 为
/// `NULL` 时写 stderr。重复调用（AlreadyInitialized）视为成功，不会 panic。
/// 返回 0=ok，-1=err。
#[no_mangle]
pub extern "C" fn muxterm_init_logging(log_file: *const c_char, level: *const c_char) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        let level = cstr_opt(level).unwrap_or_else(|| "info".into());
        let file = cstr_opt(log_file).map(std::path::PathBuf::from);
        match init_logging(LoggingConfig { level, file }) {
            Ok(()) => 0,
            Err(_) => -1,
        }
    }))
    .unwrap_or(-1)
}

/// 释放 discovery API 返回的 JSON 字符串。
///
/// # Safety
/// `value` 必须是本库返回且尚未释放的指针。
#[no_mangle]
pub unsafe extern "C" fn muxterm_free_string(value: *mut c_char) {
    if !value.is_null() {
        drop(CString::from_raw(value));
    }
}
