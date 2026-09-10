//! Catalog candidate discovery and product-level open requests.

use std::ffi::{c_char, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::catalog::{OpenRequest, ResolveIntent};
use crate::muxterm::Muxterm;
use crate::protocol::candidate::CandidateRef;

use super::support::{
    cstr_opt, json_error, json_open_error, json_resolve_error, json_string, parse_workspace_id,
    MuxtermHandle,
};

/// List the unified Project/Worktree/Existing/Recent candidates.
///
/// Existing rows are refreshed from all registered transport targets before
/// the four sources are aggregated. `recent_limit` controls how many recent
/// workspaces are appended to the result.
///
/// # Safety
/// `h` is a valid handle and has not been freed.
#[no_mangle]
pub unsafe extern "C" fn muxterm_candidates_json(
    h: *mut MuxtermHandle,
    recent_limit: u32,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return json_error("handle 为空");
        }
        let handle = &mut *h;
        let existing = match handle
            .catalog
            .discover_sessions(&mut handle.connections, "all", "")
        {
            Ok(rows) => rows,
            Err(error) => return json_error(error),
        };
        let candidates = handle.catalog.candidates_with_pool(
            handle.projects.list_projects(),
            &existing,
            recent_limit as usize,
            handle.pool(),
        );
        json_string(serde_json::json!({
            "ok": true,
            "candidates": candidates,
        }))
    }))
    .unwrap_or_else(|_| json_error("candidates list panic"))
}

/// Open a product-level [`OpenRequest`] through the Catalog resolver.
///
/// The frontend sends Candidate identity and intent only; it never constructs
/// a WorkspaceSpec. The returned object contains the opened Workspace id and
/// the Core-owned resolved descriptor for display/debugging.
///
/// # Safety
/// `h` is a valid handle and has not been freed; `request` is a NUL-terminated
/// UTF-8 JSON string.
#[no_mangle]
pub unsafe extern "C" fn muxterm_open_json(
    h: *mut MuxtermHandle,
    request: *const c_char,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() || request.is_null() {
            return json_error("handle 或 request 为空");
        }
        let Ok(request) = CStr::from_ptr(request).to_str() else {
            return json_error("request 不是合法 UTF-8");
        };
        let request = match serde_json::from_str::<OpenRequest>(request) {
            Ok(request) => request,
            Err(error) => return json_error(format!("OpenRequest JSON 解析失败: {error}")),
        };
        let handle = &mut *h;
        let previous_active = handle.pool().active_id().cloned();
        let resolved = match handle.resolve_open_request(&request) {
            Ok(resolved) => resolved,
            Err(error) => return json_resolve_error(&error),
        };
        let workspace_id = resolved.workspace_id();
        let result = {
            let (rt, runtime_registry, transport_registry, connections, pool) = (
                &handle.rt,
                &handle.runtime_registry,
                &handle.transport_registry,
                &mut handle.connections,
                &mut handle.pool,
            );
            rt.block_on(Muxterm::open_resolved_parts(
                runtime_registry,
                transport_registry,
                connections,
                &handle.templates,
                pool,
                resolved,
            ))
        };
        let (name, resolved_target) = match result {
            Ok(workspace) => (
                workspace.name().to_string(),
                workspace.resolved_target().map(resolved_target_json),
            ),
            Err(error) => return json_open_error(&error),
        };

        if !request.activate {
            if let Some(previous_active) = previous_active {
                handle.pool_mut().activate(&previous_active);
            }
        }
        if let CandidateRef::Worktree {
            project_id,
            worktree_id,
        } = &request.candidate
        {
            let project_id = crate::projects::ProjectId::from(project_id.as_str());
            let worktree_id = crate::projects::WorktreeId::from(worktree_id.as_str());
            if let Some(worktree) = handle
                .projects_mut()
                .store_mut()
                .get_mut(&project_id)
                .and_then(|project| project.worktree_mut(&worktree_id))
            {
                worktree.open_workspace = Some(workspace_id.clone());
            }
        }
        json_string(serde_json::json!({
            "ok": true,
            "id": workspace_id.as_str(),
            "name": name,
            "resolved_target": resolved_target,
        }))
    }))
    .unwrap_or_else(|_| json_error("open request panic"))
}

/// JSON target → TargetConfig compatibility open path.
///
/// Product callers should prefer `muxterm_open_json`; this entry remains for
/// migration clients that have not yet converted TargetConfig to OpenRequest.
///
/// # Safety
/// `h` is valid and not freed; `target` and `intent` are NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_open_target_json(
    h: *mut MuxtermHandle,
    target: *const c_char,
    intent: *const c_char,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() || target.is_null() {
            return json_error("handle 或 target 为空");
        }
        let Ok(target) = CStr::from_ptr(target).to_str() else {
            return json_error("target 不是合法 UTF-8");
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(target) else {
            return json_error("target JSON 解析失败");
        };
        let Some(config) = target_config_from_json(&value) else {
            return json_error("target JSON 缺必要字段（name/runtime/transport）");
        };
        let intent = match cstr_opt(intent).as_deref() {
            Some("create_if_missing") => ResolveIntent::CreateIfMissing,
            _ => ResolveIntent::AttachOnly,
        };
        let handle = &mut *h;
        let result = {
            let (rt, catalog, runtime_registry, transport_registry, connections, pool) = (
                &handle.rt,
                &mut handle.catalog,
                &handle.runtime_registry,
                &handle.transport_registry,
                &mut handle.connections,
                &mut handle.pool,
            );
            let resolved = match catalog.resolve_target(connections, &config, intent) {
                Ok(resolved) => resolved,
                Err(error) => return json_resolve_error(&error),
            };
            rt.block_on(Muxterm::open_resolved_parts(
                runtime_registry,
                transport_registry,
                connections,
                &handle.templates,
                pool,
                resolved,
            ))
        };
        match result {
            Ok(workspace) => json_string(serde_json::json!({
                "ok": true,
                "id": workspace.id().as_str(),
                "name": workspace.name(),
                "resolved_target": workspace.resolved_target().map(resolved_target_json),
            })),
            Err(err) => json_open_error(&err),
        }
    }))
    .unwrap_or_else(|_| json_error("workspace_open_target_json panic"))
}

/// Create a native worktree from a live workspace and open its resulting
/// workspace through the Core-owned product pool.
///
/// The frontend sends only product fields. Runtime construction, Herdr's
/// `worktree.create`, and pool insertion remain inside Core.
///
/// # Safety
/// All pointers are either null or NUL-terminated UTF-8 strings and `h` is a
/// live handle returned by a constructor.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_worktree_create_json(
    h: *mut MuxtermHandle,
    source_workspace_id: *const c_char,
    branch: *const c_char,
    path: *const c_char,
    base: *const c_char,
    label: *const c_char,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return json_error("handle 为空");
        }
        let Some(source_workspace_id) = cstr_opt(source_workspace_id) else {
            return json_error("source workspace id 为空");
        };
        let Some(branch) = cstr_opt(branch).filter(|value| !value.trim().is_empty()) else {
            return json_error("worktree branch 不能为空");
        };
        let Some(path) = cstr_opt(path).filter(|value| !value.trim().is_empty()) else {
            return json_error("worktree path 不能为空");
        };
        let source = parse_workspace_id(&source_workspace_id);
        let spec = crate::runtime::WorktreeCreateSpec {
            branch,
            path,
            base: cstr_opt(base).filter(|value| !value.trim().is_empty()),
            label: cstr_opt(label).filter(|value| !value.trim().is_empty()),
        };
        let handle = &mut *h;
        let result = {
            let (rt, runtime_registry, transport_registry, connections, pool) = (
                &handle.rt,
                &handle.runtime_registry,
                &handle.transport_registry,
                &mut handle.connections,
                &mut handle.pool,
            );
            rt.block_on(Muxterm::create_native_worktree_with_pool(
                runtime_registry,
                transport_registry,
                connections,
                &handle.templates,
                pool,
                &source,
                &spec,
                None,
                None,
            ))
        };
        match result {
            Ok(workspace_id) => {
                let Some(workspace) = handle.pool().get(&workspace_id) else {
                    return json_error("worktree workspace 创建后未进入 Core pool");
                };
                json_string(serde_json::json!({
                    "ok": true,
                    "id": workspace.id().as_str(),
                    "name": workspace.name(),
                    "resolved_target": workspace.resolved_target().map(resolved_target_json),
                }))
            }
            Err(error) => json_error(error),
        }
    }))
    .unwrap_or_else(|_| json_error("workspace worktree create panic"))
}

pub(crate) fn target_config_from_json(
    v: &serde_json::Value,
) -> Option<crate::projects::TargetConfig> {
    use crate::projects::{TargetConfig, TargetRuntime, TargetTransport};

    let name = v.get("name")?.as_str()?.to_string();
    let runtime = TargetRuntime::from_str(v.get("runtime")?.as_str()?)?;
    let transport = match v.get("transport").and_then(serde_json::Value::as_str) {
        Some("ssh") => TargetTransport::Ssh {
            name: v.get("target")?.as_str()?.to_string(),
        },
        _ => TargetTransport::Local,
    };
    let path = v
        .get("path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut config = TargetConfig::new(name, runtime, transport, path);
    config.session = v
        .get("session")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    config.socket = v
        .get("socket")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    config.workspace_id = v
        .get("workspace_id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    Some(config)
}

pub(crate) fn resolved_target_json(resolved: &crate::catalog::ResolvedTarget) -> serde_json::Value {
    let canonical = &resolved.canonical;
    serde_json::json!({
        "canonical": {
            "name": canonical.name,
            "runtime": canonical.runtime.as_str(),
            "transport": match &canonical.transport {
                crate::projects::TargetTransport::Local => "local",
                crate::projects::TargetTransport::Ssh { .. } => "ssh",
            },
            "target": match &canonical.transport {
                crate::projects::TargetTransport::Ssh { name } => name,
                crate::projects::TargetTransport::Local => "",
            },
            "path": canonical.path,
            "session": canonical.session,
            "socket": canonical.socket,
            "workspace_id": canonical.workspace_id,
        },
        "spec": {
            "transport": resolved.spec.transport,
            "alias": resolved.spec.alias,
            "session": resolved.spec.session,
            "runtime": resolved.spec.runtime,
            "path": resolved.spec.path,
            "socket": resolved.spec.socket,
            "provenance": resolved.spec.provenance.as_ref().map(|provenance| {
                serde_json::json!({
                    "project_id": provenance.project_id.as_ref().map(ToString::to_string),
                    "worktree_id": provenance.worktree_id.as_ref().map(ToString::to_string),
                })
            }),
            "template": resolved.spec.template.as_ref().map(ToString::to_string),
        },
    })
}
