//! FFI handle construction and ownership C ABI functions.

use std::ffi::{c_char, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;

use crate::activity::ActivityState;
use crate::config::SettingsService;
use crate::logging::{init_logging, LoggingConfig};
use crate::muxterm::Muxterm;
use crate::projects::{ProjectStore, ProjectsService};
use crate::protocol::task::Task;
use crate::protocol::terminal::emulate::DEFAULT_SCROLLBACK_LINES;
use crate::protocol::WorkspaceId;
use crate::runtime::shell::daemon_runtime::DaemonRuntime;
use crate::transport::registry::ConnectionRegistry;
use crate::workspace::pool::WorkspacePool;
use crate::workspace::spec::WorkspaceSpec;
use crate::workspace::template::{TemplateRegistry, WorkspaceTemplate};

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
        boxed_handle(
            crate::catalog::Catalog::with_builtins(),
            WorkspacePool::default(),
            rt,
            ConnectionRegistry::new(),
        )
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
    let catalog = crate::catalog::Catalog::with_builtins();
    let legacy = match legacy_runtime_spec(&kind, sock, sess, alias, start_dir, client_size) {
        Some(spec) => spec,
        None => return ptr::null_mut(),
    };
    let mut connections = ConnectionRegistry::new();
    let runtime = match legacy.runtime {
        LegacyRuntime::Provider(spec) => match Muxterm::new_runtime_parts(
            catalog.runtime_registry().as_ref(),
            catalog.transport_registry().as_ref(),
            &mut connections,
            &spec,
        ) {
            Ok(runtime) => runtime,
            Err(error) => {
                tracing::warn!(
                    target = "muxterm::ffi",
                    "legacy provider open failed: {error:#}"
                );
                return ptr::null_mut();
            }
        },
        LegacyRuntime::Daemon(runtime) => runtime,
    };
    let mut pool = WorkspacePool::default();
    let fut = pool.open_with_scrollback(
        legacy.id.clone(),
        legacy.name,
        legacy.scrollback_lines,
        move |_| Ok(runtime),
    );
    let Ok(workspace) = rt.block_on(fut) else {
        return ptr::null_mut();
    };
    if let Some((cols, rows)) = legacy.client_size {
        let _ = workspace.execute(Task::ResizeClient { cols, rows });
    }

    boxed_handle(catalog, pool, rt, connections)
}

fn new_ffi_runtime() -> Option<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .ok()
}

fn boxed_handle(
    catalog: crate::catalog::Catalog,
    pool: WorkspacePool,
    rt: tokio::runtime::Runtime,
    connections: ConnectionRegistry,
) -> *mut MuxtermHandle {
    let runtime_registry = catalog.runtime_registry();
    let transport_registry = catalog.transport_registry();
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
        .and_then(TemplateRegistry::new);
    let templates = match templates {
        Ok(templates) => templates,
        Err(error) => {
            tracing::warn!(
                target = "muxterm::config",
                "WorkspaceTemplate 加载失败，使用空注册表: {error}"
            );
            TemplateRegistry::default()
        }
    };
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
    Box::into_raw(Box::new(MuxtermHandle {
        catalog,
        runtime_registry,
        transport_registry,
        connections,
        templates,
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
        deferred_batches: std::collections::VecDeque::new(),
        deferred_activity_events: std::collections::VecDeque::new(),
        workspace_ids: Vec::new(),
    }))
}

enum LegacyRuntime {
    Provider(Box<WorkspaceSpec>),
    Daemon(Box<dyn crate::runtime::Runtime>),
}

struct LegacyRuntimeConfig {
    id: WorkspaceId,
    name: String,
    runtime: LegacyRuntime,
    scrollback_lines: usize,
    client_size: Option<(u16, u16)>,
}

/// Legacy runtime specification used by deprecated constructors.
///
/// tmux and shell go through the same provider registry as product opens.
/// The daemon variant remains a direct adapter because it is a host-side
/// compatibility endpoint, not a selectable RuntimeProvider.
fn legacy_runtime_spec(
    kind: &str,
    sock: Option<String>,
    sess: Option<String>,
    alias: Option<String>,
    start_dir: Option<String>,
    client_size: Option<(u16, u16)>,
) -> Option<LegacyRuntimeConfig> {
    let scrollback_lines = configured_scrollback_lines();
    let session = sess.unwrap_or_default();
    let legacy_name = if session.is_empty() {
        "muxterm".to_string()
    } else {
        session.clone()
    };

    let (id, runtime) = match kind {
        "tmux" => {
            let spec = WorkspaceSpec {
                transport: "local".into(),
                alias: None,
                session: session.clone(),
                runtime: "tmux".into(),
                path: start_dir.unwrap_or_default(),
                socket: sock,
                create: false,
                scrollback_lines: scrollback_lines as u32,
                provenance: None,
                template: None,
            };
            (
                WorkspaceId::new("local", alias.as_deref(), &session, "tmux", ""),
                LegacyRuntime::Provider(Box::new(spec)),
            )
        }
        "ssh" | "tmux-ssh" => {
            let (alias_name, socket) =
                crate::runtime::tmux::provider::TmuxDriver::legacy_ssh_alias_and_tmux_socket(
                    sock.as_deref(),
                    alias.as_deref(),
                )?;
            let spec = WorkspaceSpec {
                transport: "ssh".into(),
                alias: Some(alias_name),
                session: session.clone(),
                runtime: "tmux".into(),
                path: String::new(),
                socket,
                create: false,
                scrollback_lines: scrollback_lines as u32,
                provenance: None,
                template: None,
            };
            (
                WorkspaceId::new("ssh", alias.as_deref(), &session, "tmux", ""),
                LegacyRuntime::Provider(Box::new(spec)),
            )
        }
        "daemon" => {
            let daemon_name = if session.is_empty() {
                "default".to_string()
            } else {
                session.clone()
            };
            let path = sock
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| crate::protocol::daemon::default_socket_path(&daemon_name));
            (
                WorkspaceId::new("local", alias.as_deref(), &session, "daemon", ""),
                LegacyRuntime::Daemon(Box::new(DaemonRuntime::new(path, daemon_name))),
            )
        }
        _ => {
            let spec = WorkspaceSpec {
                transport: "local".into(),
                alias: None,
                session: String::new(),
                runtime: "shell".into(),
                path: start_dir.unwrap_or_default(),
                socket: None,
                create: false,
                scrollback_lines: scrollback_lines as u32,
                provenance: None,
                template: None,
            };
            (
                WorkspaceId::new("local", alias.as_deref(), "", "shell", ""),
                LegacyRuntime::Provider(Box::new(spec)),
            )
        }
    };

    Some(LegacyRuntimeConfig {
        id,
        name: legacy_name,
        runtime,
        scrollback_lines,
        client_size,
    })
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
