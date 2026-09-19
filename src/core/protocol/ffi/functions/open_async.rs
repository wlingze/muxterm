//! 非阻塞产品打开：后台只拥有待打开 Workspace，现有池与 FFI 缓冲仍归主线程。
use std::ffi::{c_char, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use crate::catalog::ResolvedTarget;
use crate::muxterm::Muxterm;
use crate::protocol::candidate::{CandidateRef, OpenRequest};
use crate::transport::ConnectionRegistry;
use crate::workspace::Workspace;

use super::support::{json_error, json_string, MuxtermHandle};

enum Opened {
    Existing(Box<ResolvedTarget>),
    New(Box<Workspace>),
}

pub(crate) struct PendingOpen {
    receiver: Receiver<(ConnectionRegistry, anyhow::Result<Opened>)>,
    request: OpenRequest,
}

/// Start one asynchronous open without moving or borrowing the live pool.
/// # Safety
/// `h` is a live exclusively accessed handle; `request` is a valid C string.
#[no_mangle]
pub unsafe extern "C" fn muxterm_open_start_json(
    h: *mut MuxtermHandle,
    request: *const c_char,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() || request.is_null() {
            return json_error("handle or request is null");
        }
        let handle = &mut *h;
        if handle.pending_open.is_some() {
            return json_error("a workspace open is already pending");
        }
        let request: OpenRequest = match CStr::from_ptr(request)
            .to_str()
            .ok()
            .and_then(|text| serde_json::from_str(text).ok())
        {
            Some(request) => request,
            None => return json_error("invalid open request"),
        };
        let runtimes = handle.runtime_registry.clone();
        let transports = handle.transport_registry.clone();
        let catalog =
            crate::catalog::Catalog::from_registries(runtimes.clone(), transports.clone());
        let mut connections = handle.connections.clone();
        let projects = handle.projects.list_projects().to_vec();
        let recent: Vec<_> = handle
            .pool
            .list()
            .into_iter()
            .filter_map(|workspace| workspace.resolved_target().cloned())
            .collect();
        let templates = handle.templates.clone();
        let executor = handle.rt.handle().clone();
        let worker_request = request.clone();
        let (sender, receiver) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("muxterm-open".into())
            .spawn(move || {
                let _runtime_context = executor.enter();
                let result = catch_unwind(AssertUnwindSafe(|| -> anyhow::Result<Opened> {
                    let resolved = catalog.resolve_open_request_with_recent(
                        &mut connections,
                        &worker_request,
                        &projects,
                        &recent,
                    )?;
                    if recent
                        .iter()
                        .any(|old| old.workspace_id() == resolved.workspace_id())
                    {
                        return Ok(Opened::Existing(Box::new(resolved)));
                    }
                    let spec = &resolved.spec;
                    let runtime =
                        Muxterm::new_runtime_parts(&runtimes, &transports, &mut connections, spec)?;
                    let mut workspace = Workspace::new_with_scrollback(
                        spec.id(),
                        spec.name(),
                        runtime,
                        spec.scrollback_lines as usize,
                    );
                    executor.block_on(workspace.connect())?;
                    if spec.create {
                        if let Some(template) = spec
                            .template
                            .as_ref()
                            .and_then(|name| templates.get(name))
                            .cloned()
                        {
                            workspace.start_template(template)?;
                        }
                    }
                    workspace.set_resolved_target(resolved);
                    Ok(Opened::New(Box::new(workspace)))
                }))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("workspace open panicked")));
                let _ = sender.send((connections, result));
            });
        if let Err(error) = spawned {
            return json_error(error);
        }
        handle.pending_open = Some(PendingOpen { receiver, request });
        json_string(serde_json::json!({"ok": true}))
    }))
    .unwrap_or_else(|_| json_error("open start panic"))
}

/// Poll and adopt the completed workspace on the FFI owner thread.
/// `activate` is decided at completion so navigation during loading is preserved.
/// # Safety
/// `h` is a live exclusively accessed handle.
#[no_mangle]
pub unsafe extern "C" fn muxterm_open_poll_json(
    h: *mut MuxtermHandle,
    activate: bool,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() { return json_error("handle is null"); }
        let handle = &mut *h;
        let Some(pending) = handle.pending_open.as_ref() else {
            return json_error("no workspace open pending");
        };
        let received = match pending.receiver.try_recv() {
            Ok(received) => received,
            Err(TryRecvError::Empty) => return json_string(serde_json::json!({"ok": true, "pending": true})),
            Err(TryRecvError::Disconnected) => {
                handle.pending_open = None;
                return json_error("workspace open worker disconnected");
            }
        };
        let pending = handle.pending_open.take().expect("pending open");
        handle.connections.merge(received.0);
        let opened = match received.1 {
            Ok(opened) => opened,
            Err(error) => return json_error(format!("{error:#}")),
        };
        let resolved = match &opened {
            Opened::Existing(resolved) => resolved.as_ref(),
            Opened::New(workspace) => workspace.resolved_target().expect("resolved workspace"),
        };
        let id = resolved.workspace_id();
        if let Some(existing) = handle.pool.get(&id) {
            if existing.resolved_target().map(|old| &old.spec) != Some(&resolved.spec) {
                return json_error("workspace identity conflicts with existing specification");
            }
        } else {
            let Opened::New(workspace) = opened else {
                return json_error("workspace was closed while opening; retry to open it again");
            };
            let previous = handle.pool.active_id().cloned();
            handle.pool.insert_connected(*workspace);
            if !activate {
                if let Some(previous) = previous { handle.pool.activate(&previous); }
            }
        }
        if activate { handle.pool.activate(&id); }
        if let CandidateRef::Worktree { project_id, worktree_id } = pending.request.candidate {
            if let Some(worktree) = handle.projects.store_mut()
                .get_mut(&crate::projects::ProjectId::from(project_id.as_str()))
                .and_then(|project| project.worktree_mut(&crate::projects::WorktreeId::from(worktree_id.as_str()))) {
                worktree.open_workspace = Some(id.clone());
            }
        }
        let workspace = handle.pool.get(&id).expect("adopted workspace");
        json_string(serde_json::json!({"ok": true, "pending": false,
            "id": id.as_str(), "name": workspace.name(),
            "resolved_target": workspace.resolved_target().map(super::catalog::resolved_target_json),
        }))
    })).unwrap_or_else(|_| json_error("open completion panic"))
}
