//! Shared safe wrapper around the public `muxterm` C ABI.
//!
//! Frontends own this client instead of borrowing `MuxtermHandle` directly or
//! repeating raw-pointer copying logic.  The wrapper deliberately returns
//! owned Rust values: pointers returned by the C ABI are only valid until the
//! next query/poll and must never escape this module.

use std::cell::Cell;
use std::ffi::{CStr, CString};
use std::ptr::{self, NonNull};

use crate::ffi::{
    self, CLayoutNode, CPane, CStateChange, CTab, CTask, CWorkspaceStateChange, LAYOUT_LEAF,
    LAYOUT_SPLIT_H, LAYOUT_SPLIT_V, STATE_BACKEND_STATUS, STATE_PANE_CLOSED, STATE_PANE_FRAME,
    STATE_PANE_OUTPUT, STATE_PANE_RESIZED, STATE_PANE_SNAPSHOT,
};

const DISCOVERY_TIMEOUT_MS: u32 = 10_000;
const EVENT_CAPACITY: usize = 64;
const WORKSPACE_EVENT_CAPACITY: usize = 64;
const TAB_CAPACITY: usize = 32;
const PANE_CAPACITY: usize = 64;
const PANE_OUTPUT_CAPACITY: usize = 256 * 1024;

/// An owned event copied from a single C ABI poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientEvent {
    pub type_: u32,
    pub pane_id: u32,
    pub tab_id: u32,
    pub window_id: u32,
    pub data: Vec<u8>,
    pub name: String,
}

/// An owned row from the Core-owned workspace pool.
#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
pub struct ClientWorkspace {
    pub id: String,
    pub name: String,
    pub runtime: String,
    #[serde(default)]
    pub active: bool,
}

/// An owned workspace event.  The workspace identity is copied before the C
/// buffer is released, so callers never retain a pointer into the handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientWorkspaceEvent {
    pub workspace_id: String,
    pub event: ClientEvent,
}

/// Target data accepted by the semantic workspace-open ABI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientTarget {
    pub name: String,
    pub runtime: String,
    pub transport: String,
    pub target: Option<String>,
    pub path: String,
    pub session: Option<String>,
    pub socket: Option<String>,
}

/// Resolver intent for [`FfiClient::open_target`].
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClientOpenIntent {
    AttachOnly,
    CreateIfMissing,
}

/// Result of opening a semantic target through Core.
#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
pub struct ClientOpenedWorkspace {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub resolved_target: Option<serde_json::Value>,
}

/// Candidate source exposed by the Catalog FFI.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClientCandidateKind {
    Project,
    Worktree,
    Existing,
    Recent,
}

/// Owned identity for an Existing candidate.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ClientExistingCandidateRef {
    pub runtime_id: String,
    pub transport_id: String,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
}

/// Owned candidate identity sent back to Core when a row is selected.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ClientCandidateRef {
    Project {
        project_id: String,
    },
    Worktree {
        project_id: String,
        worktree_id: String,
    },
    Existing {
        identity: ClientExistingCandidateRef,
    },
    Recent {
        key: String,
    },
}

/// One owned row from the unified Catalog candidate list.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ClientCandidate {
    pub kind: ClientCandidateKind,
    pub title: String,
    #[serde(default)]
    pub subtitle: String,
    #[serde(default)]
    pub badges: Vec<String>,
    #[serde(default)]
    pub in_pool: Option<String>,
    #[serde(rename = "ref")]
    pub reference: ClientCandidateRef,
}

/// Owned product-level open request.  Frontends never construct a
/// WorkspaceSpec; Core resolves this request through Catalog.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ClientOpenRequest {
    pub candidate: ClientCandidateRef,
    pub intent: ClientOpenIntent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    #[serde(default = "default_activate")]
    pub activate: bool,
}

fn default_activate() -> bool {
    true
}

/// Product-level event kinds exposed to frontends instead of raw ABI numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientEventKind {
    PaneOutput,
    PaneFrame,
    PaneSnapshot,
    PaneClosed,
    PaneResized,
    Other(u32),
}

impl ClientEvent {
    pub fn kind(&self) -> ClientEventKind {
        match self.type_ {
            STATE_PANE_OUTPUT => ClientEventKind::PaneOutput,
            STATE_PANE_FRAME => ClientEventKind::PaneFrame,
            STATE_PANE_SNAPSHOT => ClientEventKind::PaneSnapshot,
            STATE_PANE_CLOSED => ClientEventKind::PaneClosed,
            STATE_PANE_RESIZED => ClientEventKind::PaneResized,
            type_ => ClientEventKind::Other(type_),
        }
    }
}

/// An owned layout tree copied from the C ABI node pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientLayout {
    Leaf {
        pane_id: u32,
    },
    Split {
        horizontal: bool,
        ratio: u32,
        first: Box<ClientLayout>,
        second: Box<ClientLayout>,
    },
}

/// An owned tab snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientTab {
    pub id: u32,
    pub name: String,
    pub is_active: bool,
}

/// An owned pane snapshot.  `title` is kept for the TUI view model; the
/// current C ABI does not expose a separate title field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientPane {
    pub id: u32,
    pub cols: u16,
    pub rows: u16,
    pub is_active: bool,
    pub title: String,
}

/// One owned command mark returned by the activity/history query surface.
#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
pub struct ClientCommandMark {
    pub seq: u64,
    pub command: String,
    pub exit_code: Option<u8>,
    pub history_offset: Option<u32>,
}

/// Owned attention/activity state for one pane.
#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
pub struct ClientAttentionPane {
    pub workspace_id: String,
    pub pane_id: u32,
    pub status: String,
    #[serde(default)]
    pub acknowledged: bool,
    #[serde(default)]
    pub last_line: String,
    #[serde(default)]
    pub seq: u64,
    #[serde(default)]
    pub process_name: Option<String>,
    #[serde(default)]
    pub process_is_agent: bool,
    #[serde(default)]
    pub agent_name: Option<String>,
    #[serde(default)]
    pub shell_name: Option<String>,
}

/// Owned cross-workspace activity/attention aggregate.
#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
pub struct ClientWorkspaceAttention {
    pub workspace_id: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub blocked: usize,
    #[serde(default)]
    pub done: usize,
    #[serde(default)]
    pub working: usize,
    #[serde(default)]
    pub panes: Vec<ClientAttentionPane>,
}

/// Owned activity snapshot.  The current Core implementation projects the
/// attention aggregate; the frontend does not need to know that detail.
#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
pub struct ClientActivitySnapshot {
    #[serde(default)]
    pub blocked_count: usize,
    #[serde(default)]
    pub workspaces: Vec<ClientWorkspaceAttention>,
}

/// One owned activity notification.
#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
pub struct ClientActivityNotification {
    pub workspace_id: String,
    pub pane_id: u32,
    pub kind: String,
    #[serde(default)]
    pub process_name: Option<String>,
    #[serde(default)]
    pub last_line: String,
    #[serde(default)]
    pub seq: u64,
}

/// Owned activity notifications drained from Core.
#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
pub struct ClientActivityNotifications {
    #[serde(default)]
    pub notifications: Vec<ClientActivityNotification>,
    #[serde(default)]
    pub blocked: Vec<String>,
    #[serde(default)]
    pub done: Vec<String>,
}

/// SSH host entry returned by Core discovery.
#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
pub struct SshHostEntry {
    pub alias: String,
    pub hostname: String,
    pub port: u16,
    pub user: String,
}

/// Runtime provider metadata returned by the public FFI catalog view.
#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
pub struct ClientRuntimeInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub support: Vec<String>,
    #[serde(default)]
    pub accepted_transports: Vec<String>,
}

/// Existing workspace candidate returned by Core discovery.
///
/// The identity fields are intentionally owned here.  A frontend must not
/// retain a Core candidate or reconstruct attach data from its display name.
#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
pub struct ExistingCandidate {
    pub name: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub runtime: String,
    #[serde(default)]
    pub transport: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub in_pool: bool,
    #[serde(default)]
    pub windows: u32,
    #[serde(default)]
    pub attached: bool,
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub socket: Option<String>,
    #[serde(default)]
    pub workspace_id: Option<String>,
}

/// An owned tmux session row returned by the transport discovery ABI.
#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
pub struct TmuxSessionEntry {
    pub name: String,
    #[serde(default)]
    pub windows: u32,
    #[serde(default)]
    pub attached: bool,
    #[serde(default)]
    pub created: u64,
}

/// Filesystem entry returned by Core discovery.
#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
pub struct FsEntry {
    pub name: String,
    pub is_dir: bool,
}

/// Frontend task intent translated to the C ABI inside [`FfiClient`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientTask {
    NewTab,
    SwitchTab { tab_id: u32 },
    ClosePane { pane_id: u32 },
    CloseTab { tab_id: u32 },
    SplitPane { pane_id: u32, horizontal: bool },
    NextPane,
    PreviousPane,
    TogglePaneFullscreen { pane_id: u32 },
}

/// Safe ownership boundary for one Core FFI handle.
pub struct FfiClient {
    handle: NonNull<ffi::MuxtermHandle>,
    last_status: Cell<u32>,
}

impl FfiClient {
    /// Create a handle for catalog-only queries without opening a workspace.
    pub fn new_catalog() -> anyhow::Result<Self> {
        Self::from_raw(ffi::muxterm_catalog_new())
    }

    /// Create a handle and connect its initial workspace.
    pub fn new(
        runtime_type: &str,
        socket: Option<&str>,
        session: Option<&str>,
    ) -> anyhow::Result<Self> {
        let runtime = cstring(runtime_type);
        let socket = cstring_opt(socket);
        let session = cstring_opt(session);
        let handle = ffi::muxterm_new(
            runtime.as_ptr(),
            socket.as_ref().map_or(ptr::null(), |value| value.as_ptr()),
            session.as_ref().map_or(ptr::null(), |value| value.as_ptr()),
        );
        let client = Self::from_raw(handle)?;
        client.connect()?;
        Ok(client)
    }

    /// Create a handle and connect it using the combined constructor.
    pub fn new_connect(
        runtime_type: &str,
        socket: Option<&str>,
        session: Option<&str>,
        ssh_alias: Option<&str>,
        start_directory: Option<&str>,
    ) -> anyhow::Result<Self> {
        let runtime = cstring(runtime_type);
        let socket = cstring_opt(socket);
        let session = cstring_opt(session);
        let ssh_alias = cstring_opt(ssh_alias);
        let start_directory = cstring_opt(start_directory);
        let handle = ffi::muxterm_new_connect(
            runtime.as_ptr(),
            socket.as_ref().map_or(ptr::null(), |value| value.as_ptr()),
            session.as_ref().map_or(ptr::null(), |value| value.as_ptr()),
            ssh_alias
                .as_ref()
                .map_or(ptr::null(), |value| value.as_ptr()),
            start_directory
                .as_ref()
                .map_or(ptr::null(), |value| value.as_ptr()),
        );
        Self::from_raw(handle)
    }

    fn from_raw(handle: *mut ffi::MuxtermHandle) -> anyhow::Result<Self> {
        let handle = NonNull::new(handle)
            .ok_or_else(|| anyhow::anyhow!("Core FFI handle construction failed"))?;
        Ok(Self {
            handle,
            last_status: Cell::new(ffi::BACKEND_STATUS_DISCONNECTED),
        })
    }

    fn connect(&self) -> anyhow::Result<()> {
        let rc = unsafe { ffi::muxterm_connect(self.handle.as_ptr()) };
        if rc == 0 {
            Ok(())
        } else {
            Err(anyhow::anyhow!("Core FFI connect failed with code {rc}"))
        }
    }

    /// Execute a C ABI task.  The task is borrowed only for the duration of
    /// the call and its optional name pointer is never retained by Core.
    pub fn execute(&self, task: &CTask) -> i32 {
        unsafe { ffi::muxterm_execute(self.handle.as_ptr(), task) }
    }

    /// Execute a frontend task after constructing its borrowed C DTO locally.
    ///
    /// Frontends should use this method instead of importing `CTask` and the
    /// task constants from the public ABI module.
    pub fn execute_task(&self, task: ClientTask) -> i32 {
        let raw = task_to_ffi(task);
        self.execute(&raw)
    }

    /// Execute a task against a specific workspace without changing the
    /// Core pool's active workspace.
    pub fn execute_workspace_task(&self, workspace_id: &str, task: ClientTask) -> i32 {
        let workspace_id = cstring(workspace_id);
        let raw = task_to_ffi(task);
        unsafe { ffi::muxterm_execute_workspace(self.handle.as_ptr(), workspace_id.as_ptr(), &raw) }
    }

    /// Write input to a pane in a specific workspace without activating it.
    pub fn send_workspace_input(&self, workspace_id: &str, pane_id: u32, data: &[u8]) -> i32 {
        if data.is_empty() {
            return 0;
        }
        let workspace_id = cstring(workspace_id);
        unsafe {
            ffi::muxterm_workspace_send_input(
                self.handle.as_ptr(),
                workspace_id.as_ptr(),
                pane_id,
                data.as_ptr(),
                data.len(),
            )
        }
    }

    /// Write input without changing attention state in a specific workspace.
    pub fn send_workspace_input_quiet(&self, workspace_id: &str, pane_id: u32, data: &[u8]) -> i32 {
        if data.is_empty() {
            return 0;
        }
        let workspace_id = cstring(workspace_id);
        unsafe {
            ffi::muxterm_workspace_send_input_quiet(
                self.handle.as_ptr(),
                workspace_id.as_ptr(),
                pane_id,
                data.as_ptr(),
                data.len(),
            )
        }
    }

    pub fn resize_workspace_client(&self, workspace_id: &str, cols: u16, rows: u16) -> i32 {
        let workspace_id = cstring(workspace_id);
        unsafe {
            ffi::muxterm_workspace_resize_client(
                self.handle.as_ptr(),
                workspace_id.as_ptr(),
                cols,
                rows,
            )
        }
    }

    pub fn resize_workspace_pane(
        &self,
        workspace_id: &str,
        pane_id: u32,
        cols: u16,
        rows: u16,
    ) -> i32 {
        let workspace_id = cstring(workspace_id);
        unsafe {
            ffi::muxterm_workspace_resize_pane(
                self.handle.as_ptr(),
                workspace_id.as_ptr(),
                pane_id,
                cols,
                rows,
            )
        }
    }

    pub fn resize_workspace_pane_axis(
        &self,
        workspace_id: &str,
        pane_id: u32,
        axis: u32,
        size: u16,
    ) -> i32 {
        let workspace_id = cstring(workspace_id);
        unsafe {
            ffi::muxterm_workspace_resize_pane_axis(
                self.handle.as_ptr(),
                workspace_id.as_ptr(),
                pane_id,
                axis,
                size,
            )
        }
    }

    /// Explicitly detach the current control client.
    pub fn detach(&self) -> i32 {
        unsafe { ffi::muxterm_detach(self.handle.as_ptr()) }
    }

    /// Explicitly shut down all workspaces without freeing the handle.
    pub fn shutdown(&self) -> i32 {
        unsafe { ffi::muxterm_shutdown(self.handle.as_ptr()) }
    }

    /// List the live workspaces owned by this Core handle.
    pub fn workspace_list(&self) -> anyhow::Result<Vec<ClientWorkspace>> {
        let value =
            Self::discovery_json(|| unsafe { ffi::muxterm_workspace_list(self.handle.as_ptr()) })?;
        Ok(serde_json::from_value(value["workspaces"].clone())?)
    }

    /// Activate a Core-owned workspace by its stable product identity.
    pub fn activate_workspace(&self, id: &str) -> anyhow::Result<()> {
        let id = cstring(id);
        let rc = unsafe { ffi::muxterm_workspace_activate(self.handle.as_ptr(), id.as_ptr()) };
        if rc == 0 {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Core workspace activation failed with code {rc}"
            ))
        }
    }

    /// Close a Core-owned workspace by its stable product identity.
    pub fn close_workspace(&self, id: &str) -> anyhow::Result<()> {
        let id = cstring(id);
        let rc = unsafe { ffi::muxterm_workspace_close(self.handle.as_ptr(), id.as_ptr()) };
        if rc == 0 {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Core workspace close failed with code {rc}"
            ))
        }
    }

    /// List the unified Project/Worktree/Existing/Recent Catalog candidates.
    pub fn candidates(&self, recent_limit: u32) -> anyhow::Result<Vec<ClientCandidate>> {
        let value = Self::discovery_json(|| unsafe {
            ffi::muxterm_candidates_json(self.handle.as_ptr(), recent_limit)
        })?;
        Ok(serde_json::from_value(value["candidates"].clone())?)
    }

    /// Open a selected Catalog candidate through the product-level resolver.
    pub fn open(&self, request: &ClientOpenRequest) -> anyhow::Result<ClientOpenedWorkspace> {
        let request = cstring(&serde_json::to_string(request)?);
        let value = Self::discovery_json(|| unsafe {
            ffi::muxterm_open_json(self.handle.as_ptr(), request.as_ptr())
        })?;
        Ok(serde_json::from_value(value)?)
    }

    /// Open a target through the Catalog resolver and return only owned DTOs.
    pub fn open_target(
        &self,
        target: &ClientTarget,
        intent: ClientOpenIntent,
    ) -> anyhow::Result<ClientOpenedWorkspace> {
        let target_json = client_target_json(target);
        let target = cstring(&target_json.to_string());
        let intent = cstring(match intent {
            ClientOpenIntent::AttachOnly => "attach_only",
            ClientOpenIntent::CreateIfMissing => "create_if_missing",
        });
        let value = Self::discovery_json(|| unsafe {
            ffi::muxterm_workspace_open_target_json(
                self.handle.as_ptr(),
                target.as_ptr(),
                intent.as_ptr(),
            )
        })?;
        Ok(serde_json::from_value(value)?)
    }

    /// Poll and immediately copy all borrowed C data into owned values.
    pub fn poll_events(&self) -> Vec<ClientEvent> {
        let mut buffer = [CStateChange::default(); EVENT_CAPACITY];
        let count = unsafe {
            ffi::muxterm_poll_events(
                self.handle.as_ptr(),
                buffer.as_mut_ptr(),
                EVENT_CAPACITY as i32,
            )
        };
        if count <= 0 {
            return Vec::new();
        }
        buffer[..(count as usize).min(EVENT_CAPACITY)]
            .iter()
            .map(|event| self.copy_event(event))
            .collect()
    }

    /// Poll active and background workspace events with their full identity.
    pub fn poll_workspace_events(&self) -> Vec<ClientWorkspaceEvent> {
        let mut buffer = [CWorkspaceStateChange {
            workspace_id: ptr::null(),
            event: CStateChange {
                type_: 0,
                pane_id: 0,
                tab_id: 0,
                window_id: 0,
                data: ptr::null(),
                data_len: 0,
                name: ptr::null(),
            },
        }; WORKSPACE_EVENT_CAPACITY];
        let count = unsafe {
            ffi::muxterm_poll_workspace_events(
                self.handle.as_ptr(),
                buffer.as_mut_ptr(),
                WORKSPACE_EVENT_CAPACITY as i32,
            )
        };
        if count <= 0 {
            return Vec::new();
        }
        buffer[..(count as usize).min(WORKSPACE_EVENT_CAPACITY)]
            .iter()
            .map(|event| ClientWorkspaceEvent {
                workspace_id: copy_c_string(event.workspace_id),
                event: self.copy_event(&event.event),
            })
            .collect()
    }

    pub fn status_code(&self) -> u32 {
        self.last_status.get()
    }

    fn copy_event(&self, event: &CStateChange) -> ClientEvent {
        if event.type_ == STATE_BACKEND_STATUS {
            self.last_status.set(event.pane_id);
        }
        ClientEvent {
            type_: event.type_,
            pane_id: event.pane_id,
            tab_id: event.tab_id,
            window_id: event.window_id,
            data: copy_bytes(event.data, event.data_len),
            name: copy_c_string(event.name),
        }
    }

    /// Read the registered runtime provider metadata through FFI.
    pub fn runtime_list(&self) -> anyhow::Result<Vec<ClientRuntimeInfo>> {
        let value = Self::discovery_json(|| unsafe {
            ffi::muxterm_runtime_list_json(self.handle.as_ptr())
        })?;
        Ok(serde_json::from_value(value["runtimes"].clone())?)
    }

    /// Read runtime provider metadata without exposing a handle to the caller.
    pub fn discover_runtimes() -> anyhow::Result<Vec<ClientRuntimeInfo>> {
        Self::new_catalog()?.runtime_list()
    }

    pub fn get_tabs(&self) -> Vec<ClientTab> {
        let mut buffer = [CTab {
            id: 0,
            name: ptr::null(),
            is_active: 0,
        }; TAB_CAPACITY];
        let count = unsafe {
            ffi::muxterm_get_tabs(
                self.handle.as_ptr(),
                buffer.as_mut_ptr(),
                TAB_CAPACITY as i32,
            )
        };
        if count <= 0 {
            return Vec::new();
        }
        buffer[..(count as usize).min(TAB_CAPACITY)]
            .iter()
            .map(|tab| ClientTab {
                id: tab.id,
                name: copy_c_string(tab.name),
                is_active: tab.is_active != 0,
            })
            .collect()
    }

    /// Read tabs from a specific workspace without changing Core activation.
    pub fn get_workspace_tabs(&self, workspace_id: &str) -> Vec<ClientTab> {
        let workspace_id = cstring(workspace_id);
        let mut buffer = [CTab {
            id: 0,
            name: ptr::null(),
            is_active: 0,
        }; TAB_CAPACITY];
        let count = unsafe {
            ffi::muxterm_workspace_get_tabs(
                self.handle.as_ptr(),
                workspace_id.as_ptr(),
                buffer.as_mut_ptr(),
                TAB_CAPACITY as i32,
            )
        };
        if count <= 0 {
            return Vec::new();
        }
        buffer[..(count as usize).min(TAB_CAPACITY)]
            .iter()
            .map(|tab| ClientTab {
                id: tab.id,
                name: copy_c_string(tab.name),
                is_active: tab.is_active != 0,
            })
            .collect()
    }

    pub fn get_panes(&self, tab_id: u32) -> Vec<ClientPane> {
        let mut buffer = [CPane {
            id: 0,
            cols: 0,
            rows: 0,
            is_active: 0,
        }; PANE_CAPACITY];
        let count = unsafe {
            ffi::muxterm_get_panes(
                self.handle.as_ptr(),
                tab_id,
                buffer.as_mut_ptr(),
                PANE_CAPACITY as i32,
            )
        };
        if count <= 0 {
            return Vec::new();
        }
        buffer[..(count as usize).min(PANE_CAPACITY)]
            .iter()
            .map(|pane| ClientPane {
                id: pane.id,
                cols: pane.cols,
                rows: pane.rows,
                is_active: pane.is_active != 0,
                title: String::new(),
            })
            .collect()
    }

    /// Read panes from a specific workspace without changing Core activation.
    pub fn get_workspace_panes(&self, workspace_id: &str, tab_id: u32) -> Vec<ClientPane> {
        let workspace_id = cstring(workspace_id);
        let mut buffer = [CPane {
            id: 0,
            cols: 0,
            rows: 0,
            is_active: 0,
        }; PANE_CAPACITY];
        let count = unsafe {
            ffi::muxterm_workspace_get_panes(
                self.handle.as_ptr(),
                workspace_id.as_ptr(),
                tab_id,
                buffer.as_mut_ptr(),
                PANE_CAPACITY as i32,
            )
        };
        if count <= 0 {
            return Vec::new();
        }
        buffer[..(count as usize).min(PANE_CAPACITY)]
            .iter()
            .map(|pane| ClientPane {
                id: pane.id,
                cols: pane.cols,
                rows: pane.rows,
                is_active: pane.is_active != 0,
                title: String::new(),
            })
            .collect()
    }

    pub fn get_layout(&self, tab_id: u32) -> Option<ClientLayout> {
        let mut root = CLayoutNode {
            type_: LAYOUT_LEAF,
            pane_id: 0,
            ratio: 0,
            first: ptr::null(),
            second: ptr::null(),
        };
        let rc = unsafe { ffi::muxterm_get_layout(self.handle.as_ptr(), tab_id, &mut root) };
        (rc == 0).then(|| unsafe { clone_layout(&root) })
    }

    /// Read a tab layout from a specific workspace without changing
    /// activation.
    pub fn get_workspace_layout(&self, workspace_id: &str, tab_id: u32) -> Option<ClientLayout> {
        let workspace_id = cstring(workspace_id);
        let mut root = CLayoutNode {
            type_: LAYOUT_LEAF,
            pane_id: 0,
            ratio: 0,
            first: ptr::null(),
            second: ptr::null(),
        };
        let rc = unsafe {
            ffi::muxterm_workspace_get_layout(
                self.handle.as_ptr(),
                workspace_id.as_ptr(),
                tab_id,
                &mut root,
            )
        };
        (rc == 0).then(|| unsafe { clone_layout(&root) })
    }

    pub fn get_pane_output(&self, pane_id: u32) -> Vec<u8> {
        let mut buffer = vec![0u8; PANE_OUTPUT_CAPACITY];
        let count = unsafe {
            ffi::muxterm_get_pane_output(
                self.handle.as_ptr(),
                pane_id,
                buffer.as_mut_ptr(),
                buffer.len(),
            )
        };
        if count <= 0 {
            return Vec::new();
        }
        buffer.truncate((count as usize).min(buffer.len()));
        buffer
    }

    /// Read accumulated pane output from a specific workspace without
    /// changing Core activation.
    pub fn get_workspace_pane_output(&self, workspace_id: &str, pane_id: u32) -> Vec<u8> {
        let workspace_id = cstring(workspace_id);
        let mut buffer = vec![0u8; PANE_OUTPUT_CAPACITY];
        let count = unsafe {
            ffi::muxterm_workspace_get_pane_output(
                self.handle.as_ptr(),
                workspace_id.as_ptr(),
                pane_id,
                buffer.as_mut_ptr(),
                buffer.len(),
            )
        };
        if count <= 0 {
            return Vec::new();
        }
        buffer.truncate((count as usize).min(buffer.len()));
        buffer
    }

    /// Read scrollback bytes from a specific workspace without activation.
    pub fn get_workspace_pane_scroll_ansi(
        &self,
        workspace_id: &str,
        pane_id: u32,
        offset: u32,
        rows: u32,
    ) -> Vec<u8> {
        let workspace_id = cstring(workspace_id);
        let mut buffer = vec![0u8; PANE_OUTPUT_CAPACITY];
        let count = unsafe {
            ffi::muxterm_workspace_pane_scroll_ansi(
                self.handle.as_ptr(),
                workspace_id.as_ptr(),
                pane_id,
                offset,
                rows,
                buffer.as_mut_ptr(),
                buffer.len(),
            )
        };
        if count <= 0 {
            return Vec::new();
        }
        buffer.truncate((count as usize).min(buffer.len()));
        buffer
    }

    /// Read a workspace pane's visible grid for compatibility/diagnostics.
    pub fn get_workspace_pane_visible_ansi(&self, workspace_id: &str, pane_id: u32) -> Vec<u8> {
        let workspace_id = cstring(workspace_id);
        let mut buffer = vec![0u8; PANE_OUTPUT_CAPACITY];
        let count = unsafe {
            ffi::muxterm_workspace_pane_visible_ansi(
                self.handle.as_ptr(),
                workspace_id.as_ptr(),
                pane_id,
                buffer.as_mut_ptr(),
                buffer.len(),
            )
        };
        if count <= 0 {
            return Vec::new();
        }
        buffer.truncate((count as usize).min(buffer.len()));
        buffer
    }

    /// Read a one-time workspace pane Surface seed.
    pub fn get_workspace_pane_surface_seed_ansi(
        &self,
        workspace_id: &str,
        pane_id: u32,
    ) -> Vec<u8> {
        let workspace_id = cstring(workspace_id);
        let required = unsafe {
            ffi::muxterm_workspace_pane_surface_seed_ansi(
                self.handle.as_ptr(),
                workspace_id.as_ptr(),
                pane_id,
                ptr::null_mut(),
                0,
            )
        };
        if required <= 0 {
            return Vec::new();
        }
        let mut buffer = vec![0u8; required as usize];
        let count = unsafe {
            ffi::muxterm_workspace_pane_surface_seed_ansi(
                self.handle.as_ptr(),
                workspace_id.as_ptr(),
                pane_id,
                buffer.as_mut_ptr(),
                buffer.len(),
            )
        };
        if count <= 0 {
            return Vec::new();
        }
        buffer.truncate((count as usize).min(buffer.len()));
        buffer
    }

    pub fn workspace_pane_viewport(&self, workspace_id: &str, pane_id: u32) -> Option<u32> {
        let workspace_id = cstring(workspace_id);
        let offset = unsafe {
            ffi::muxterm_workspace_pane_viewport(
                self.handle.as_ptr(),
                workspace_id.as_ptr(),
                pane_id,
            )
        };
        (offset >= 0).then_some(offset as u32)
    }

    pub fn set_workspace_pane_viewport(
        &self,
        workspace_id: &str,
        pane_id: u32,
        offset: u32,
    ) -> i32 {
        let workspace_id = cstring(workspace_id);
        unsafe {
            ffi::muxterm_workspace_set_pane_viewport(
                self.handle.as_ptr(),
                workspace_id.as_ptr(),
                pane_id,
                offset,
            )
        }
    }

    pub fn workspace_pane_history_max_offset(
        &self,
        workspace_id: &str,
        pane_id: u32,
        rows: u32,
    ) -> Option<u32> {
        let workspace_id = cstring(workspace_id);
        let offset = unsafe {
            ffi::muxterm_workspace_pane_history_max_offset(
                self.handle.as_ptr(),
                workspace_id.as_ptr(),
                pane_id,
                rows,
            )
        };
        (offset >= 0).then_some(offset as u32)
    }

    pub fn workspace_pane_command_marks(
        &self,
        workspace_id: &str,
        pane_id: u32,
    ) -> anyhow::Result<Vec<ClientCommandMark>> {
        let workspace_id = cstring(workspace_id);
        let value = Self::discovery_json(|| unsafe {
            ffi::muxterm_workspace_pane_command_marks_json(
                self.handle.as_ptr(),
                workspace_id.as_ptr(),
                pane_id,
            )
        })?;
        Ok(serde_json::from_value(value["marks"].clone())?)
    }

    pub fn workspace_pane_latest_line_seq(&self, workspace_id: &str, pane_id: u32) -> Option<u64> {
        let workspace_id = cstring(workspace_id);
        let seq = unsafe {
            ffi::muxterm_workspace_pane_latest_line_seq(
                self.handle.as_ptr(),
                workspace_id.as_ptr(),
                pane_id,
            )
        };
        u64::try_from(seq).ok()
    }

    pub fn workspace_pane_viewport_for_seq(
        &self,
        workspace_id: &str,
        pane_id: u32,
        seq: u64,
    ) -> Option<u32> {
        let workspace_id = cstring(workspace_id);
        let offset = unsafe {
            ffi::muxterm_workspace_pane_viewport_for_seq(
                self.handle.as_ptr(),
                workspace_id.as_ptr(),
                pane_id,
                seq,
            )
        };
        (offset >= 0).then_some(offset as u32)
    }

    pub fn workspace_pane_last_n_lines(
        &self,
        workspace_id: &str,
        pane_id: u32,
        n: u32,
    ) -> anyhow::Result<Vec<String>> {
        let workspace_id = cstring(workspace_id);
        let value = Self::discovery_json(|| unsafe {
            ffi::muxterm_workspace_pane_last_n_lines(
                self.handle.as_ptr(),
                workspace_id.as_ptr(),
                pane_id,
                n,
            )
        })?;
        Ok(serde_json::from_value(value["lines"].clone())?)
    }

    pub fn send_input(&self, pane_id: u32, data: &[u8]) -> i32 {
        if data.is_empty() {
            return 0;
        }
        unsafe { ffi::muxterm_send_input(self.handle.as_ptr(), pane_id, data.as_ptr(), data.len()) }
    }

    pub fn resize_client(&self, cols: u16, rows: u16) -> i32 {
        unsafe { ffi::muxterm_resize_client(self.handle.as_ptr(), cols, rows) }
    }

    pub fn resize_pane(&self, pane_id: u32, cols: u16, rows: u16) -> i32 {
        unsafe { ffi::muxterm_resize_pane(self.handle.as_ptr(), pane_id, cols, rows) }
    }

    pub fn status_subscription_active(&self) -> bool {
        unsafe { ffi::muxterm_status_subscription_active(self.handle.as_ptr()) != 0 }
    }

    pub fn traffic_bytes(&self) -> (u64, u64) {
        unsafe {
            (
                ffi::muxterm_traffic_down(self.handle.as_ptr()),
                ffi::muxterm_traffic_up(self.handle.as_ptr()),
            )
        }
    }

    pub fn report_pane_colours(&self, pane_id: u32, fg_hex: &str, bg_hex: &str) -> i32 {
        let fg = cstring(fg_hex);
        let bg = cstring(bg_hex);
        unsafe {
            ffi::muxterm_report_pane_colours(
                self.handle.as_ptr(),
                pane_id,
                fg.as_ptr(),
                bg.as_ptr(),
            )
        }
    }

    pub fn report_all_pane_colours(&self, fg_hex: &str, bg_hex: &str) -> i32 {
        let fg = cstring(fg_hex);
        let bg = cstring(bg_hex);
        unsafe {
            ffi::muxterm_report_all_pane_colours(self.handle.as_ptr(), fg.as_ptr(), bg.as_ptr())
        }
    }

    /// Read the owned activity/attention aggregate across all workspaces.
    pub fn activity_snapshot(&self) -> anyhow::Result<ClientActivitySnapshot> {
        let value = Self::discovery_json(|| unsafe {
            ffi::muxterm_attention_snapshot(self.handle.as_ptr())
        })?;
        Ok(serde_json::from_value(value)?)
    }

    /// Drain owned activity notifications without exposing Core references.
    pub fn take_activity_notifications(&self) -> anyhow::Result<ClientActivityNotifications> {
        let value = Self::discovery_json(|| unsafe {
            ffi::muxterm_attention_take_notifications(self.handle.as_ptr())
        })?;
        Ok(serde_json::from_value(value)?)
    }

    pub fn workspace_attention_on_became_visible(&self, workspace_id: &str, pane_id: u32) -> i32 {
        let workspace_id = cstring(workspace_id);
        unsafe {
            ffi::muxterm_workspace_attention_on_became_visible(
                self.handle.as_ptr(),
                workspace_id.as_ptr(),
                pane_id,
            )
        }
    }

    pub fn workspace_attention_acknowledge(&self, workspace_id: &str, pane_id: u32) -> i32 {
        let workspace_id = cstring(workspace_id);
        unsafe {
            ffi::muxterm_workspace_attention_acknowledge(
                self.handle.as_ptr(),
                workspace_id.as_ptr(),
                pane_id,
            )
        }
    }

    pub fn workspace_attention_set_process_name(
        &self,
        workspace_id: &str,
        pane_id: u32,
        name: Option<&str>,
    ) -> i32 {
        let workspace_id = cstring(workspace_id);
        let name = cstring_opt(name);
        unsafe {
            ffi::muxterm_workspace_attention_set_process_name(
                self.handle.as_ptr(),
                workspace_id.as_ptr(),
                pane_id,
                name.as_ref().map_or(ptr::null(), |value| value.as_ptr()),
            )
        }
    }

    pub fn workspace_attention_mute(&self, workspace_id: &str, pane_id: u32, seconds: u64) -> i32 {
        let workspace_id = cstring(workspace_id);
        unsafe {
            ffi::muxterm_workspace_attention_mute(
                self.handle.as_ptr(),
                workspace_id.as_ptr(),
                pane_id,
                seconds,
            )
        }
    }

    /// Call a JSON-returning discovery function and decode its common success
    /// envelope.  The raw C string is always freed before returning.
    pub fn discovery_json<F>(call: F) -> anyhow::Result<serde_json::Value>
    where
        F: FnOnce() -> *mut std::os::raw::c_char,
    {
        let raw = call();
        if raw.is_null() {
            anyhow::bail!("Core discovery returned no response");
        }
        let text = copy_c_string(raw);
        unsafe { ffi::muxterm_free_string(raw) };
        let value: serde_json::Value = serde_json::from_str(&text)
            .map_err(|error| anyhow::anyhow!("Core discovery returned invalid JSON: {error}"))?;
        if value.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
            let error = value
                .get("error")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown Core discovery error");
            anyhow::bail!(error.to_string());
        }
        Ok(value)
    }

    pub fn discover_ssh_hosts() -> anyhow::Result<Vec<SshHostEntry>> {
        let config_path = std::env::var("MUXTERM_SSH_CONFIG_PATH").ok();
        let config_path = cstring_opt(config_path.as_deref());
        let value = Self::discovery_json(|| {
            ffi::muxterm_discover_ssh_hosts_json(
                config_path
                    .as_ref()
                    .map_or(ptr::null(), |value| value.as_ptr()),
            )
        })?;
        Ok(serde_json::from_value(value["hosts"].clone())?)
    }

    pub fn list_dir(
        runtime_type: &str,
        target: Option<&str>,
        path: &str,
    ) -> anyhow::Result<Vec<FsEntry>> {
        let runtime = cstring(runtime_type);
        let target = cstring_opt(target);
        let path = cstring(path);
        let value = Self::discovery_json(|| {
            ffi::muxterm_list_dir_json(
                runtime.as_ptr(),
                target.as_ref().map_or(ptr::null(), |value| value.as_ptr()),
                ptr::null(),
                path.as_ptr(),
                DISCOVERY_TIMEOUT_MS,
            )
        })?;
        Ok(serde_json::from_value(value["entries"].clone())?)
    }

    /// Discover attachable Existing candidates through the public FFI.
    pub fn discover_existing(
        runtime_type: &str,
        target: Option<&str>,
        socket: Option<&str>,
    ) -> anyhow::Result<Vec<ExistingCandidate>> {
        let runtime = cstring(runtime_type);
        let target = cstring_opt(target);
        let socket = cstring_opt(socket);
        let value = Self::discovery_json(|| {
            ffi::muxterm_discover_workspaces_json(
                runtime.as_ptr(),
                target.as_ref().map_or(ptr::null(), |value| value.as_ptr()),
                socket.as_ref().map_or(ptr::null(), |value| value.as_ptr()),
                ptr::null(),
                DISCOVERY_TIMEOUT_MS,
            )
        })?;
        Ok(serde_json::from_value(value["workspaces"].clone())?)
    }

    /// Discover tmux sessions on one explicit target-side socket.
    pub fn discover_tmux_sessions(
        transport_type: &str,
        target: Option<&str>,
        socket: Option<&str>,
    ) -> anyhow::Result<Vec<TmuxSessionEntry>> {
        let transport = cstring(transport_type);
        let target = cstring_opt(target);
        let socket = cstring_opt(socket);
        let value = Self::discovery_json(|| {
            ffi::muxterm_discover_tmux_sessions_json(
                transport.as_ptr(),
                target.as_ref().map_or(ptr::null(), |value| value.as_ptr()),
                socket.as_ref().map_or(ptr::null(), |value| value.as_ptr()),
                ptr::null(),
                DISCOVERY_TIMEOUT_MS,
            )
        })?;
        Ok(serde_json::from_value(value["sessions"].clone())?)
    }

    pub fn create_workspace(
        runtime_type: &str,
        target: Option<&str>,
        socket: Option<&str>,
        session: &str,
        directory: &str,
    ) -> anyhow::Result<String> {
        let runtime = cstring(runtime_type);
        let target = cstring_opt(target);
        let socket = cstring_opt(socket);
        let session_c = cstring(session);
        let directory_c = cstring(directory);
        let value = Self::discovery_json(|| {
            ffi::muxterm_workspace_create(
                runtime.as_ptr(),
                target.as_ref().map_or(ptr::null(), |value| value.as_ptr()),
                socket.as_ref().map_or(ptr::null(), |value| value.as_ptr()),
                ptr::null(),
                session_c.as_ptr(),
                directory_c.as_ptr(),
                DISCOVERY_TIMEOUT_MS,
            )
        })?;
        Ok(value["session"]
            .as_str()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| session.to_string()))
    }

    pub fn status_snapshot_json(
        runtime_type: &str,
        target: Option<&str>,
        socket: Option<&str>,
        session: &str,
    ) -> anyhow::Result<serde_json::Value> {
        let runtime = cstring(runtime_type);
        let target = cstring_opt(target);
        let socket = cstring_opt(socket);
        let session = cstring(session);
        Self::discovery_json(|| {
            ffi::muxterm_status_snapshot_json(
                runtime.as_ptr(),
                target.as_ref().map_or(ptr::null(), |value| value.as_ptr()),
                socket.as_ref().map_or(ptr::null(), |value| value.as_ptr()),
                session.as_ptr(),
            )
        })
    }
}

impl Drop for FfiClient {
    fn drop(&mut self) {
        unsafe { ffi::muxterm_free(self.handle.as_ptr()) };
    }
}

fn cstring(value: &str) -> CString {
    CString::new(value).unwrap_or_default()
}

fn cstring_opt(value: Option<&str>) -> Option<CString> {
    value.map(cstring)
}

fn client_target_json(target: &ClientTarget) -> serde_json::Value {
    serde_json::json!({
        "name": target.name,
        "runtime": target.runtime,
        "transport": target.transport,
        "target": target.target,
        "path": target.path,
        "session": target.session,
        "socket": target.socket,
    })
}

fn copy_bytes(data: *const u8, len: usize) -> Vec<u8> {
    if data.is_null() || len == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(data, len).to_vec() }
    }
}

fn copy_c_string(data: *const std::os::raw::c_char) -> String {
    if data.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(data) }
            .to_string_lossy()
            .into_owned()
    }
}

unsafe fn clone_layout(node: &CLayoutNode) -> ClientLayout {
    match node.type_ {
        LAYOUT_SPLIT_H | LAYOUT_SPLIT_V => {
            let first = if node.first.is_null() {
                ClientLayout::Leaf { pane_id: 0 }
            } else {
                clone_layout(&*node.first)
            };
            let second = if node.second.is_null() {
                ClientLayout::Leaf { pane_id: 0 }
            } else {
                clone_layout(&*node.second)
            };
            ClientLayout::Split {
                horizontal: node.type_ == LAYOUT_SPLIT_H,
                ratio: node.ratio,
                first: Box::new(first),
                second: Box::new(second),
            }
        }
        _ => ClientLayout::Leaf {
            pane_id: node.pane_id,
        },
    }
}

fn task_to_ffi(task: ClientTask) -> CTask {
    let (type_, target_pane, target_tab, dir) = match task {
        ClientTask::NewTab => (ffi::TASK_NEW_TAB, 0, 0, 0),
        ClientTask::SwitchTab { tab_id } => (ffi::TASK_SWITCH_TAB, 0, tab_id, 0),
        ClientTask::ClosePane { pane_id } => (ffi::TASK_CLOSE_PANE, pane_id, 0, 0),
        ClientTask::CloseTab { tab_id } => (ffi::TASK_CLOSE_TAB, 0, tab_id, 0),
        ClientTask::SplitPane {
            pane_id,
            horizontal,
        } => (
            ffi::TASK_SPLIT_PANE,
            pane_id,
            0,
            if horizontal {
                ffi::DIR_HORIZONTAL
            } else {
                ffi::DIR_VERTICAL
            },
        ),
        ClientTask::NextPane => (ffi::TASK_NEXT_PANE, 0, 0, 0),
        ClientTask::PreviousPane => (ffi::TASK_PREV_PANE, 0, 0, 0),
        ClientTask::TogglePaneFullscreen { pane_id } => {
            (ffi::TASK_TOGGLE_PANE_FULLSCREEN, pane_id, 0, 0)
        }
    };
    CTask {
        type_,
        target_pane,
        target_tab,
        dir,
        name: ptr::null(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_c_buffers_become_empty_owned_values() {
        assert_eq!(copy_bytes(ptr::null(), 10), Vec::<u8>::new());
        assert_eq!(copy_c_string(ptr::null()), "");
    }

    #[test]
    fn layout_clone_preserves_split_shape() {
        let leaf_a = CLayoutNode {
            type_: LAYOUT_LEAF,
            pane_id: 7,
            ratio: 0,
            first: ptr::null(),
            second: ptr::null(),
        };
        let leaf_b = CLayoutNode {
            type_: LAYOUT_LEAF,
            pane_id: 8,
            ratio: 0,
            first: ptr::null(),
            second: ptr::null(),
        };
        let root = CLayoutNode {
            type_: LAYOUT_SPLIT_H,
            pane_id: 0,
            ratio: 500,
            first: &leaf_a,
            second: &leaf_b,
        };
        let actual = unsafe { clone_layout(&root) };
        assert_eq!(
            actual,
            ClientLayout::Split {
                horizontal: true,
                ratio: 500,
                first: Box::new(ClientLayout::Leaf { pane_id: 7 }),
                second: Box::new(ClientLayout::Leaf { pane_id: 8 }),
            }
        );
    }

    #[test]
    fn frontend_tasks_translate_to_abi_without_exposing_ctask() {
        let split = task_to_ffi(ClientTask::SplitPane {
            pane_id: 9,
            horizontal: false,
        });
        assert_eq!(split.type_, ffi::TASK_SPLIT_PANE);
        assert_eq!(split.target_pane, 9);
        assert_eq!(split.dir, ffi::DIR_VERTICAL);
        assert!(split.name.is_null());

        let tab = task_to_ffi(ClientTask::SwitchTab { tab_id: 4 });
        assert_eq!(tab.type_, ffi::TASK_SWITCH_TAB);
        assert_eq!(tab.target_tab, 4);
        assert_eq!(tab.target_pane, 0);

        let fullscreen = task_to_ffi(ClientTask::TogglePaneFullscreen { pane_id: 12 });
        assert_eq!(fullscreen.type_, ffi::TASK_TOGGLE_PANE_FULLSCREEN);
        assert_eq!(fullscreen.target_pane, 12);
    }

    #[test]
    fn event_kind_hides_raw_state_constants_from_frontends() {
        let event = ClientEvent {
            type_: STATE_PANE_OUTPUT,
            pane_id: 7,
            tab_id: 0,
            window_id: 0,
            data: Vec::new(),
            name: String::new(),
        };
        assert_eq!(event.kind(), ClientEventKind::PaneOutput);
        assert_eq!(
            ClientEvent {
                type_: u32::MAX,
                ..event
            }
            .kind(),
            ClientEventKind::Other(u32::MAX)
        );
    }

    #[test]
    fn existing_candidate_keeps_attach_identity_owned() {
        let candidate: ExistingCandidate = serde_json::from_value(serde_json::json!({
            "name": "agent-workspace",
            "runtime": "herdr",
            "transport": "ssh",
            "target": "buildbox",
            "namespace": "agents",
            "session": "agents",
            "socket": "/remote/.config/herdr/sessions/agents/herdr.sock",
            "workspace_id": "w7"
        }))
        .expect("Existing candidate JSON should decode");

        assert_eq!(candidate.name, "agent-workspace");
        assert_eq!(candidate.target, "buildbox");
        assert_eq!(candidate.namespace.as_deref(), Some("agents"));
        assert_eq!(candidate.session.as_deref(), Some("agents"));
        assert_eq!(
            candidate.socket.as_deref(),
            Some("/remote/.config/herdr/sessions/agents/herdr.sock")
        );
        assert_eq!(candidate.workspace_id.as_deref(), Some("w7"));
    }

    #[test]
    fn tmux_session_entry_defaults_optional_metadata() {
        let session: TmuxSessionEntry = serde_json::from_value(serde_json::json!({
            "name": "muxterm-test-existing"
        }))
        .expect("tmux session JSON should decode");

        assert_eq!(session.name, "muxterm-test-existing");
        assert_eq!(session.windows, 0);
        assert!(!session.attached);
        assert_eq!(session.created, 0);
    }

    #[test]
    fn runtime_info_is_an_owned_catalog_dto() {
        let info: ClientRuntimeInfo = serde_json::from_value(serde_json::json!({
            "id": "herdr",
            "name": "Herdr",
            "support": ["PersistDetach", "Discover"],
            "accepted_transports": ["local", "ssh"]
        }))
        .expect("runtime list JSON should decode");

        assert_eq!(info.id, "herdr");
        assert_eq!(info.name, "Herdr");
        assert_eq!(info.support, ["PersistDetach", "Discover"]);
        assert_eq!(info.accepted_transports, ["local", "ssh"]);
    }

    #[test]
    fn workspace_dtos_keep_pool_identity_and_open_result() {
        let workspaces: Vec<ClientWorkspace> = serde_json::from_value(serde_json::json!([
            {
                "id": "local//default/shell/",
                "name": "default",
                "runtime": "shell",
                "active": true
            }
        ]))
        .expect("workspace list JSON should decode");
        assert_eq!(workspaces[0].id, "local//default/shell/");
        assert!(workspaces[0].active);

        let opened: ClientOpenedWorkspace = serde_json::from_value(serde_json::json!({
            "ok": true,
            "id": "local//default/tmux/",
            "name": "default",
            "resolved_target": {"canonical": {"runtime": "tmux"}}
        }))
        .expect("workspace open JSON should decode");
        assert_eq!(opened.id, "local//default/tmux/");
        assert_eq!(
            opened.resolved_target.as_ref().expect("resolved target")["canonical"]["runtime"],
            "tmux"
        );
    }

    #[test]
    fn candidate_and_open_request_json_keep_typed_identity() {
        let candidate: ClientCandidate = serde_json::from_value(serde_json::json!({
            "kind": "existing",
            "title": "agent",
            "subtitle": "buildbox",
            "badges": ["herdr", "ssh"],
            "in_pool": "ssh/buildbox/agent/herdr/w7",
            "ref": {
                "kind": "existing",
                "value": {
                    "identity": {
                        "runtime_id": "herdr",
                        "transport_id": "ssh",
                        "target": "buildbox",
                        "session": "agent",
                        "workspace_id": "w7"
                    }
                }
            }
        }))
        .expect("candidate JSON should decode");

        assert_eq!(candidate.kind, ClientCandidateKind::Existing);
        assert_eq!(
            candidate.in_pool.as_deref(),
            Some("ssh/buildbox/agent/herdr/w7")
        );
        let ClientCandidateRef::Existing { identity } = candidate.reference else {
            panic!("expected existing candidate identity");
        };
        assert_eq!(identity.workspace_id.as_deref(), Some("w7"));

        let request = ClientOpenRequest {
            candidate: ClientCandidateRef::Recent {
                key: "recent-key".into(),
            },
            intent: ClientOpenIntent::AttachOnly,
            template: None,
            activate: false,
        };
        let encoded = serde_json::to_value(&request).expect("open request should encode");
        assert_eq!(encoded["candidate"]["kind"], "recent");
        assert_eq!(encoded["candidate"]["value"]["key"], "recent-key");
        assert_eq!(encoded["intent"], "attach_only");
        assert_eq!(encoded["activate"], false);
    }

    #[test]
    fn client_target_json_preserves_attach_identity_fields() {
        let target = ClientTarget {
            name: "buildbox".into(),
            runtime: "herdr".into(),
            transport: "ssh".into(),
            target: Some("devbox".into()),
            path: "/workspace/project".into(),
            session: Some("agent".into()),
            socket: Some("/tmp/herdr.sock".into()),
        };

        let value = client_target_json(&target);
        assert_eq!(value["runtime"], "herdr");
        assert_eq!(value["transport"], "ssh");
        assert_eq!(value["target"], "devbox");
        assert_eq!(value["session"], "agent");
        assert_eq!(value["socket"], "/tmp/herdr.sock");
    }
}
