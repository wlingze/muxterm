//! Catalog candidate discovery and product-level open requests.

use std::ffi::{c_char, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::core::catalog::OpenRequest;
use crate::core::protocol::candidate::CandidateRef;

use super::super::api::{json_error, json_string, resolved_target_json, MuxtermHandle};

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
        let existing = match handle.catalog.discover_sessions("all", "") {
            Ok(rows) => rows,
            Err(error) => return json_error(error),
        };
        let candidates = handle.catalog.candidates(
            handle.projects.list_projects(),
            &existing,
            recent_limit as usize,
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
        let previous_active = handle.catalog.pool().active_id().cloned();
        let resolved = match handle
            .catalog
            .resolve_open_request(&request, handle.projects.list_projects())
        {
            Ok(resolved) => resolved,
            Err(error) => return json_error(error),
        };
        let workspace_id = resolved.workspace_id();
        let result = handle.rt.block_on(handle.catalog.open_resolved(resolved));
        let (name, resolved_target) = match result {
            Ok(workspace) => (
                workspace.name().to_string(),
                workspace.resolved_target().map(resolved_target_json),
            ),
            Err(error) => return json_error(error),
        };

        if !request.activate {
            if let Some(previous_active) = previous_active {
                handle.catalog.pool_mut().activate(&previous_active);
            }
        }
        if let CandidateRef::Worktree {
            project_id,
            worktree_id,
        } = &request.candidate
        {
            let project_id = crate::core::projects::ProjectId::from(project_id.as_str());
            let worktree_id = crate::core::projects::WorktreeId::from(worktree_id.as_str());
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
