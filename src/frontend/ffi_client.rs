//! Shared safe wrapper around the public `muxterm` C ABI.
//!
//! Frontends own this client instead of borrowing `MuxtermHandle` directly or
//! repeating raw-pointer copying logic.  The wrapper deliberately returns
//! owned Rust values: pointers returned by the C ABI are only valid until the
//! next query/poll and must never escape this module.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::ptr::{self, NonNull};

use crate::ffi::{
    self, CLayoutNode, CPane, CStateChange, CTab, CTask, CWorkspaceStateChange,
    BACKEND_STATUS_CONNECTED, BACKEND_STATUS_CONNECTING, BACKEND_STATUS_DISCONNECTED,
    BACKEND_STATUS_ERROR, BACKEND_STATUS_EXITED, LAYOUT_LEAF, LAYOUT_SPLIT_H, LAYOUT_SPLIT_V,
    STATE_ACTIVE_PANE_CHANGED, STATE_ACTIVE_TAB_CHANGED, STATE_BACKEND_STATUS,
    STATE_LAYOUT_CHANGED, STATE_MUTATION_SETTLED, STATE_OTHER, STATE_PANE_ADDED,
    STATE_PANE_AGENT_CHANGED, STATE_PANE_CLOSED, STATE_PANE_FRAME, STATE_PANE_HISTORY,
    STATE_PANE_OUTPUT, STATE_PANE_RESIZED, STATE_PANE_SNAPSHOT, STATE_POOL_CHANGED,
    STATE_STATUS_SUBSCRIPTION, STATE_TAB_ADDED, STATE_TAB_CLOSED, STATE_TAB_ORDER_CHANGED,
    STATE_TAB_RENAMED, STATE_WORKSPACE_RENAMED,
};

const DISCOVERY_TIMEOUT_MS: u32 = 10_000;
const EVENT_CAPACITY: usize = 64;
const WORKSPACE_EVENT_CAPACITY: usize = 64;
const TAB_CAPACITY: usize = 32;
const PANE_CAPACITY: usize = 64;
const PANE_OUTPUT_CAPACITY: usize = 256 * 1024;

/// An owned event copied from a single C ABI poll.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
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
    #[serde(default)]
    pub resolved_target: Option<serde_json::Value>,
}

/// An owned workspace event.  The workspace identity is copied before the C
/// buffer is released, so callers never retain a pointer into the handle.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct ClientWorkspaceEvent {
    pub workspace_id: String,
    pub event: ClientEvent,
}

impl ClientWorkspaceEvent {
    /// Convert this owned FFI event to the daemon's semantic JSON wire.
    pub fn to_wire_json(&self) -> serde_json::Value {
        self.event.to_wire_json(&self.workspace_id)
    }
}

fn client_layout_to_json(layout: &ClientLayout) -> serde_json::Value {
    match layout {
        ClientLayout::Leaf { pane_id } => serde_json::json!({ "Leaf": pane_id }),
        ClientLayout::Split {
            horizontal,
            ratio,
            first,
            second,
        } => serde_json::json!({
            "Split": {
                "dir": if *horizontal { "Horizontal" } else { "Vertical" },
                "ratio": ratio,
                "first": client_layout_to_json(first),
                "second": client_layout_to_json(second),
            }
        }),
    }
}

fn client_layout_first_pane(layout: &ClientLayout) -> u32 {
    match layout {
        ClientLayout::Leaf { pane_id } => *pane_id,
        ClientLayout::Split { first, .. } => client_layout_first_pane(first),
    }
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

/// Structured error envelope returned by a JSON FFI operation.
///
/// Older Core endpoints returned a string in the error field; those responses
/// are represented with code and stage absent so callers can migrate without
/// losing the human-readable message. Unknown object fields remain available
/// in details for endpoint-specific diagnostics.
#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
pub struct ClientError {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub stage: Option<String>,
    pub message: String,
    #[serde(flatten)]
    pub details: BTreeMap<String, serde_json::Value>,
}

impl ClientError {
    fn from_json(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Object(_) => {
                serde_json::from_value(value.clone()).unwrap_or_else(|error| {
                    Self::legacy(format!("Core returned invalid error envelope: {error}"))
                })
            }
            serde_json::Value::String(message) => Self::legacy(message.clone()),
            other => Self::legacy(format!("Core returned invalid error value: {other}")),
        }
    }

    fn legacy(message: String) -> Self {
        Self {
            code: None,
            stage: None,
            message,
            details: BTreeMap::new(),
        }
    }
}

/// Owned RGB value returned by a Core FFI DTO.
#[derive(Debug, Clone, Copy, Default, serde::Deserialize, PartialEq, Eq)]
pub struct ClientRgb(pub u8, pub u8, pub u8);

impl ClientRgb {
    pub fn to_u32(self) -> u32 {
        ((self.0 as u32) << 16) | ((self.1 as u32) << 8) | self.2 as u32
    }
}

/// Resolved theme owned by the frontend after crossing the FFI boundary.
#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
pub struct ClientTheme {
    pub name: String,
    pub background: ClientRgb,
    pub foreground: ClientRgb,
    pub cursor: ClientRgb,
    pub colors: [ClientRgb; 16],
}

/// One effective key binding returned by the Core shortcut resolver.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ClientKeyBinding {
    pub key: String,
    #[serde(default)]
    pub mods: Vec<String>,
    pub action: String,
}

/// Frontend-owned font settings decoded from the Core configuration snapshot.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct ClientFontConfig {
    #[serde(default = "default_client_font_family")]
    pub family: String,
    #[serde(default = "default_client_font_size")]
    pub size: f32,
    #[serde(default)]
    pub fallback: Vec<String>,
}

fn default_client_font_family() -> String {
    "JetBrains Mono".into()
}

fn default_client_font_size() -> f32 {
    13.0
}

impl Default for ClientFontConfig {
    fn default() -> Self {
        Self {
            family: default_client_font_family(),
            size: default_client_font_size(),
            fallback: vec!["Noto Sans Mono".into(), "monospace".into()],
        }
    }
}

/// Frontend-owned theme selection settings, distinct from resolved colors.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ClientThemeConfig {
    #[serde(default = "default_client_theme_name")]
    pub name: String,
    #[serde(default = "default_client_light_theme")]
    pub light: String,
    #[serde(default = "default_client_dark_theme")]
    pub dark: String,
}

fn default_client_theme_name() -> String {
    "system".into()
}

fn default_client_light_theme() -> String {
    "white".into()
}

fn default_client_dark_theme() -> String {
    "black".into()
}

impl Default for ClientThemeConfig {
    fn default() -> Self {
        Self {
            name: default_client_theme_name(),
            light: default_client_light_theme(),
            dark: default_client_dark_theme(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ClientStatusbarConfig {
    #[serde(default = "default_client_statusbar_mode")]
    pub mode: String,
}

fn default_client_statusbar_mode() -> String {
    "tmux".into()
}

impl Default for ClientStatusbarConfig {
    fn default() -> Self {
        Self {
            mode: default_client_statusbar_mode(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ClientPoolConfig {
    #[serde(default = "default_client_pool_max_slots")]
    pub max_slots: u32,
}

fn default_client_pool_max_slots() -> u32 {
    20
}

impl Default for ClientPoolConfig {
    fn default() -> Self {
        Self {
            max_slots: default_client_pool_max_slots(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ClientTmuxConfig {
    #[serde(default = "default_client_auto_mouse")]
    pub auto_mouse: bool,
    #[serde(default)]
    pub default_session: String,
    #[serde(default)]
    pub socket: String,
}

fn default_client_auto_mouse() -> bool {
    true
}

impl Default for ClientTmuxConfig {
    fn default() -> Self {
        Self {
            auto_mouse: default_client_auto_mouse(),
            default_session: String::new(),
            socket: String::new(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ClientScrollbackConfig {
    #[serde(default = "default_client_scrollback_lines")]
    pub lines: u32,
}

fn default_client_scrollback_lines() -> u32 {
    10_000
}

impl Default for ClientScrollbackConfig {
    fn default() -> Self {
        Self {
            lines: default_client_scrollback_lines(),
        }
    }
}

/// Owned configuration snapshot returned by the Core configuration ABI.
#[derive(Debug, Clone, serde::Deserialize, PartialEq)]
pub struct ClientConfigSnapshot {
    #[serde(default)]
    pub path: String,
    pub revision: String,
    pub raw: serde_json::Value,
    pub values: serde_json::Value,
    pub defaults: serde_json::Value,
    pub schema: serde_json::Value,
    pub manifest: serde_json::Value,
    pub action_catalog: serde_json::Value,
    #[serde(default)]
    pub resolved_theme: Option<ClientTheme>,
    #[serde(default)]
    pub effective_keybindings: Vec<ClientKeyBinding>,
}

/// RFC 6902-style patch operation accepted by the Core configuration ABI.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct ClientJsonPatchOperation {
    pub op: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
}

/// Draft values returned after a configuration patch preview.
#[derive(Debug, Clone, serde::Deserialize, PartialEq)]
pub struct ClientConfigDraft {
    pub transaction: String,
    pub values: serde_json::Value,
    #[serde(default)]
    pub diagnostics: Vec<String>,
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ClientError {}

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

/// Owned attention configuration sent through the public FFI JSON boundary.
///
/// This DTO intentionally does not expose the Core config type to frontend
/// code. The frontend copies the user-facing fields at the call site, while
/// Core remains responsible for validation and runtime behavior.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ClientAttentionConfig {
    pub enabled: bool,
    #[serde(default)]
    pub blocked_regex: Vec<String>,
    pub debounce_ms: u64,
}

impl Default for ClientAttentionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            blocked_regex: Vec::new(),
            debounce_ms: 100,
        }
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ClientOnLastPaneExit {
    #[default]
    CloseWindow,
    KeepEmpty,
    NewShell,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ClientOnProgramExitAbnormal {
    #[default]
    Notify,
    Close,
    Keep,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ClientBehaviorConfig {
    #[serde(default)]
    pub on_last_pane_exit: ClientOnLastPaneExit,
    #[serde(default)]
    pub on_program_exit_abnormal: ClientOnProgramExitAbnormal,
}

impl Default for ClientBehaviorConfig {
    fn default() -> Self {
        Self {
            on_last_pane_exit: ClientOnLastPaneExit::CloseWindow,
            on_program_exit_abnormal: ClientOnProgramExitAbnormal::Notify,
        }
    }
}

/// Frontend configuration DTO decoded from the Core settings snapshot.
///
/// This intentionally contains only fields consumed by the Linux shell. Extra
/// Core-owned sections remain in the JSON snapshot and are ignored here.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct ClientConfig {
    #[serde(default)]
    pub font: ClientFontConfig,
    #[serde(default)]
    pub theme: ClientThemeConfig,
    #[serde(default)]
    pub statusbar: ClientStatusbarConfig,
    #[serde(default)]
    pub pool: ClientPoolConfig,
    #[serde(default)]
    pub tmux: ClientTmuxConfig,
    #[serde(default)]
    pub scrollback: ClientScrollbackConfig,
    #[serde(default)]
    pub attention: ClientAttentionConfig,
    #[serde(default)]
    pub behavior: ClientBehaviorConfig,
    #[serde(default)]
    pub keybindings: Vec<ClientKeyBinding>,
}

/// Product-level event kinds exposed to frontends instead of raw ABI numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientEventKind {
    PaneOutput,
    PaneFrame,
    PaneSnapshot,
    PaneHistory,
    PaneClosed,
    PaneResized,
    Other(u32),
}

impl ClientEvent {
    /// Convert the borrowed-ABI event into the daemon's semantic JSON wire.
    ///
    /// The daemon process must not forward raw C field names as its long-term
    /// contract: `type_` is an ABI detail, while `kind` is the product event
    /// vocabulary understood by Core's shell runtime.  The raw type and the
    /// common fields remain present so newer/unknown events can be diagnosed
    /// without making an older daemon client fail.
    pub fn to_wire_json(&self, workspace_id: &str) -> serde_json::Value {
        let mut value = serde_json::json!({
            "workspace_id": workspace_id,
            "kind": self.wire_kind(),
            "type": self.type_,
            "pane_id": self.pane_id,
            "tab_id": self.tab_id,
            "window_id": self.window_id,
            "data": self.data,
            "name": self.name,
        });

        let object = value
            .as_object_mut()
            .expect("event wire JSON is always an object");
        match self.type_ {
            STATE_BACKEND_STATUS => {
                object.insert(
                    "status".into(),
                    serde_json::Value::String(backend_status_wire_name(self.pane_id).into()),
                );
            }
            STATE_PANE_RESIZED => {
                if let Some((cols, rows)) = decode_resize_data(&self.data) {
                    object.insert("cols".into(), serde_json::Value::from(cols));
                    object.insert("rows".into(), serde_json::Value::from(rows));
                }
            }
            STATE_STATUS_SUBSCRIPTION => {
                object.insert(
                    "value".into(),
                    serde_json::Value::String(String::from_utf8_lossy(&self.data).into_owned()),
                );
            }
            STATE_PANE_AGENT_CHANGED | STATE_MUTATION_SETTLED => {
                if let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&self.data) {
                    object.insert("payload".into(), payload);
                }
            }
            _ => {}
        }

        value
    }

    fn wire_kind(&self) -> &'static str {
        match self.type_ {
            STATE_PANE_OUTPUT => "pane_output",
            STATE_PANE_FRAME => "pane_frame",
            STATE_PANE_SNAPSHOT => "pane_snapshot",
            STATE_PANE_HISTORY => "pane_history",
            STATE_TAB_ADDED => "tab_added",
            STATE_TAB_CLOSED => "tab_closed",
            STATE_LAYOUT_CHANGED => "layout_changed",
            STATE_PANE_ADDED => "pane_added",
            STATE_PANE_CLOSED => "pane_closed",
            STATE_ACTIVE_TAB_CHANGED => "active_tab_changed",
            STATE_ACTIVE_PANE_CHANGED => "active_pane_changed",
            STATE_TAB_RENAMED => "tab_renamed",
            STATE_TAB_ORDER_CHANGED => "tab_order_changed",
            STATE_PANE_RESIZED => "pane_resized",
            STATE_BACKEND_STATUS => "backend_status",
            STATE_STATUS_SUBSCRIPTION => "status_subscription",
            STATE_WORKSPACE_RENAMED => "workspace_renamed",
            STATE_POOL_CHANGED => "pool_changed",
            STATE_PANE_AGENT_CHANGED => "pane_agent_changed",
            STATE_MUTATION_SETTLED => "mutation_settled",
            STATE_OTHER => "other",
            _ => "unknown",
        }
    }

    pub fn kind(&self) -> ClientEventKind {
        match self.type_ {
            STATE_PANE_OUTPUT => ClientEventKind::PaneOutput,
            STATE_PANE_FRAME => ClientEventKind::PaneFrame,
            STATE_PANE_SNAPSHOT => ClientEventKind::PaneSnapshot,
            STATE_PANE_HISTORY => ClientEventKind::PaneHistory,
            STATE_PANE_CLOSED => ClientEventKind::PaneClosed,
            STATE_PANE_RESIZED => ClientEventKind::PaneResized,
            type_ => ClientEventKind::Other(type_),
        }
    }

    /// Whether this event requires a fresh owned workspace topology snapshot.
    pub fn is_topology(&self) -> bool {
        matches!(
            self.type_,
            STATE_TAB_ADDED
                | STATE_TAB_CLOSED
                | STATE_LAYOUT_CHANGED
                | STATE_PANE_ADDED
                | STATE_PANE_CLOSED
                | STATE_ACTIVE_TAB_CHANGED
                | STATE_ACTIVE_PANE_CHANGED
                | STATE_TAB_RENAMED
                | STATE_PANE_RESIZED
                | STATE_WORKSPACE_RENAMED
                | STATE_POOL_CHANGED
                | STATE_TAB_ORDER_CHANGED
        )
    }
}

fn decode_resize_data(data: &[u8]) -> Option<(u16, u16)> {
    let [cols_lo, cols_hi, rows_lo, rows_hi, ..] = data else {
        return None;
    };
    Some((
        u16::from_le_bytes([*cols_lo, *cols_hi]),
        u16::from_le_bytes([*rows_lo, *rows_hi]),
    ))
}

fn backend_status_wire_name(status: u32) -> &'static str {
    match status {
        BACKEND_STATUS_DISCONNECTED => "disconnected",
        BACKEND_STATUS_CONNECTING => "connecting",
        BACKEND_STATUS_CONNECTED => "connected",
        BACKEND_STATUS_ERROR => "error",
        BACKEND_STATUS_EXITED => "exited",
        _ => "unknown",
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

/// One owned search hit returned by the Core index.
#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
pub struct ClientSearchHit {
    pub workspace_id: String,
    pub tab_id: u32,
    pub pane_id: u32,
    pub seq: u64,
    pub line: String,
}

/// Owned attention/activity state for one pane.
#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
pub struct ClientAttentionPane {
    #[serde(default)]
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

/// Frontend view of the Core attention state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientAttentionStatus {
    Unknown,
    Working,
    Done,
    Blocked,
    Idle,
}

impl ClientAttentionStatus {
    pub fn parse(value: &str) -> Self {
        match value {
            "working" => Self::Working,
            "done" => Self::Done,
            "blocked" => Self::Blocked,
            "idle" => Self::Idle,
            _ => Self::Unknown,
        }
    }
}

impl ClientAttentionPane {
    pub fn status_kind(&self) -> ClientAttentionStatus {
        ClientAttentionStatus::parse(&self.status)
    }
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
#[derive(Debug, Clone, Default, serde::Deserialize, PartialEq, Eq)]
pub struct ClientActivitySnapshot {
    #[serde(default)]
    pub blocked_count: usize,
    #[serde(default)]
    pub workspaces: Vec<ClientWorkspaceAttention>,
}

/// Owned Herdr stream diagnostics used by the Linux E2E watchdog.
#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
pub struct ClientHerdrProbe {
    pub stream_starts: u64,
    pub control_takeover_starts: u64,
    pub takeover_suppressed: bool,
    pub actual_mode: String,
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
#[derive(Debug, Clone, Default, serde::Deserialize, PartialEq, Eq)]
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

/// Runtime capabilities exposed to frontend code through the FFI catalog view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientRuntimeCapability {
    PersistDetach,
    Discover,
    MultiTab,
    SplitPane,
    SharedClientResize,
    WorktreeList,
    WorktreeCreate,
    WorktreeOpen,
    WorktreeRemove,
}

impl ClientRuntimeCapability {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PersistDetach => "PersistDetach",
            Self::Discover => "Discover",
            Self::MultiTab => "MultiTab",
            Self::SplitPane => "SplitPane",
            Self::SharedClientResize => "SharedClientResize",
            Self::WorktreeList => "WorktreeList",
            Self::WorktreeCreate => "WorktreeCreate",
            Self::WorktreeOpen => "WorktreeOpen",
            Self::WorktreeRemove => "WorktreeRemove",
        }
    }
}

impl ClientRuntimeInfo {
    pub fn supports(&self, capability: ClientRuntimeCapability) -> bool {
        self.support
            .iter()
            .any(|value| value == capability.as_str())
    }
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

/// An owned tmux pane row returned by SSH transport discovery.
#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
pub struct ClientTmuxPane {
    pub id: u32,
    pub active: bool,
    pub cols: u16,
    pub rows: u16,
    pub title: String,
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
    SwitchPane { pane_id: u32 },
    TogglePaneFullscreen { pane_id: u32 },
    BreakPane { pane_id: u32 },
    RefreshTabs,
    RequestPaneSnapshot { pane_id: u32 },
    Detach,
    Shutdown,
}

/// Axis used by the Core pane-resize FFI operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientResizeAxis {
    Horizontal,
    Vertical,
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

    /// Reconnect all Core-owned workspace runtimes through the single handle.
    pub fn reconnect(&self) -> anyhow::Result<()> {
        self.connect()
    }

    /// Read the Core-owned configuration snapshot through the public FFI.
    pub fn config_describe(&self) -> anyhow::Result<ClientConfigSnapshot> {
        let value = Self::discovery_json(|| unsafe {
            ffi::muxterm_config_describe_json(self.handle.as_ptr())
        })?;
        Ok(serde_json::from_value(value["data"].clone())?)
    }

    /// Validate the default configuration or one explicit file through Core.
    pub fn config_validate(
        &self,
        path: Option<&std::path::Path>,
    ) -> anyhow::Result<serde_json::Value> {
        let path = path.map(|value| cstring(value.to_string_lossy().as_ref()));
        let value = Self::discovery_json(|| {
            ffi::muxterm_config_validate_json(
                path.as_ref().map_or(ptr::null(), |value| value.as_ptr()),
            )
        })?;
        Ok(value["data"].clone())
    }

    /// Start a Core-owned draft configuration transaction.
    pub fn config_begin(&self) -> anyhow::Result<String> {
        let value = Self::discovery_json(|| unsafe {
            ffi::muxterm_config_begin_json(self.handle.as_ptr())
        })?;
        value["data"]["transaction"]
            .as_str()
            .map(ToOwned::to_owned)
            .ok_or_else(|| anyhow::anyhow!("Core config begin returned no transaction"))
    }

    /// Apply a JSON patch to a Core-owned draft and return its owned preview.
    pub fn config_patch(
        &self,
        transaction: &str,
        patch: &[ClientJsonPatchOperation],
    ) -> anyhow::Result<ClientConfigDraft> {
        let transaction = cstring(transaction);
        let patch = cstring(&serde_json::to_string(patch)?);
        let value = Self::discovery_json(|| unsafe {
            ffi::muxterm_config_patch_json(
                self.handle.as_ptr(),
                transaction.as_ptr(),
                patch.as_ptr(),
            )
        })?;
        Ok(serde_json::from_value(value["data"].clone())?)
    }

    /// Commit a Core-owned draft configuration transaction.
    pub fn config_commit(&self, transaction: &str) -> anyhow::Result<String> {
        let transaction = cstring(transaction);
        let value = Self::discovery_json(|| unsafe {
            ffi::muxterm_config_commit_json(self.handle.as_ptr(), transaction.as_ptr())
        })?;
        value["data"]["revision"]
            .as_str()
            .map(ToOwned::to_owned)
            .ok_or_else(|| anyhow::anyhow!("Core config commit returned no revision"))
    }

    /// Cancel a Core-owned draft configuration transaction.
    pub fn config_cancel(&self, transaction: &str) -> anyhow::Result<()> {
        let transaction = cstring(transaction);
        Self::discovery_json(|| unsafe {
            ffi::muxterm_config_cancel_json(self.handle.as_ptr(), transaction.as_ptr())
        })?;
        Ok(())
    }

    /// Apply a complete configuration patch through one Core-owned draft
    /// transaction and return the committed snapshot.
    pub fn config_apply(
        &self,
        patch: &[ClientJsonPatchOperation],
    ) -> anyhow::Result<ClientConfigSnapshot> {
        let transaction = self.config_begin()?;
        if let Err(error) = self.config_patch(&transaction, patch) {
            let _ = self.config_cancel(&transaction);
            return Err(error);
        }
        if let Err(error) = self.config_commit(&transaction) {
            let _ = self.config_cancel(&transaction);
            return Err(error);
        }
        self.config_describe()
    }

    /// Apply one dotted configuration field through the Core-owned transaction.
    ///
    /// Frontends use dotted paths because they mirror the settings manifest;
    /// the FFI contract itself receives RFC 6902 JSON Pointers.
    pub fn config_apply_path(
        &self,
        dotted: &str,
        value: serde_json::Value,
    ) -> anyhow::Result<ClientConfigSnapshot> {
        let path = dotted_config_pointer(dotted)?;
        self.config_apply(&[ClientJsonPatchOperation {
            op: "replace".into(),
            path,
            value: Some(value),
        }])
    }

    /// Reload the Core-owned configuration from disk and return its revision.
    pub fn config_reload(&self) -> anyhow::Result<String> {
        let value = Self::discovery_json(|| unsafe {
            ffi::muxterm_config_reload_json(self.handle.as_ptr())
        })?;
        value["data"]["revision"]
            .as_str()
            .map(ToOwned::to_owned)
            .ok_or_else(|| anyhow::anyhow!("Core config reload returned no revision"))
    }

    /// Drain Core-owned configuration events as owned JSON values.
    pub fn config_events(&self) -> anyhow::Result<Vec<serde_json::Value>> {
        let value = Self::discovery_json(|| unsafe {
            ffi::muxterm_config_events_json(self.handle.as_ptr())
        })?;
        Ok(serde_json::from_value(value["data"]["events"].clone())?)
    }

    /// Configure the Core-owned attention engine before frontend polling starts.
    pub fn configure_attention(&self, config: &ClientAttentionConfig) -> anyhow::Result<()> {
        let config = serde_json::to_string(config)?;
        let config = cstring(&config);
        let rc =
            unsafe { ffi::muxterm_attention_configure_json(self.handle.as_ptr(), config.as_ptr()) };
        if rc == 0 {
            Ok(())
        } else {
            anyhow::bail!("Core attention configuration failed with code {rc}")
        }
    }

    /// Read Core-owned Herdr stream diagnostics for one workspace pane.
    pub fn herdr_probe(&self, workspace_id: &str, pane_id: u32) -> Option<ClientHerdrProbe> {
        let workspace_id = cstring(workspace_id);
        let value = Self::discovery_json(|| unsafe {
            ffi::muxterm_workspace_herdr_probe_json(
                self.handle.as_ptr(),
                workspace_id.as_ptr(),
                pane_id,
            )
        })
        .ok()?;
        serde_json::from_value(value.get("probe")?.clone()).ok()
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

    /// Create a tab with an optional frontend-provided name.
    pub fn new_workspace_tab(&self, workspace_id: &str, name: Option<&str>) -> i32 {
        let workspace_id = cstring(workspace_id);
        let name = cstring_opt(name);
        let raw = CTask {
            type_: ffi::TASK_NEW_TAB,
            target_pane: 0,
            target_tab: 0,
            dir: 0,
            name: name.as_ref().map_or(ptr::null(), |value| value.as_ptr()),
        };
        unsafe { ffi::muxterm_execute_workspace(self.handle.as_ptr(), workspace_id.as_ptr(), &raw) }
    }

    /// Rename a Core-owned workspace without exposing a C task DTO to callers.
    pub fn rename_workspace(&self, workspace_id: &str, name: &str) -> i32 {
        let workspace_id = cstring(workspace_id);
        let name = cstring(name);
        let raw = CTask {
            type_: ffi::TASK_RENAME_WORKSPACE,
            target_pane: 0,
            target_tab: 0,
            dir: 0,
            name: name.as_ptr(),
        };
        unsafe { ffi::muxterm_execute_workspace(self.handle.as_ptr(), workspace_id.as_ptr(), &raw) }
    }

    /// Rename a tab in a Core-owned workspace without exposing a C task DTO.
    pub fn rename_workspace_tab(&self, workspace_id: &str, tab_id: u32, name: &str) -> i32 {
        let workspace_id = cstring(workspace_id);
        let name = cstring(name);
        let raw = CTask {
            type_: ffi::TASK_RENAME_TAB,
            target_pane: 0,
            target_tab: tab_id,
            dir: 0,
            name: name.as_ptr(),
        };
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
        axis: ClientResizeAxis,
        size: u16,
    ) -> i32 {
        let workspace_id = cstring(workspace_id);
        let axis = match axis {
            ClientResizeAxis::Horizontal => ffi::DIR_HORIZONTAL,
            ClientResizeAxis::Vertical => ffi::DIR_VERTICAL,
        };
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

    /// Build a control-lane topology baseline for the daemon event stream.
    ///
    /// This deliberately queries only workspace metadata, tabs, panes, and
    /// layouts.  Pane output is never read here: render data must travel as
    /// incremental FFI events so the daemon client does not recreate the old
    /// cumulative snapshot protocol.
    pub fn workspace_topology_json(&self, workspace_id: &str) -> anyhow::Result<serde_json::Value> {
        let workspace = self
            .workspace_list()?
            .into_iter()
            .find(|item| item.id == workspace_id)
            .ok_or_else(|| anyhow::anyhow!("Core workspace not found: {workspace_id}"))?;
        let tabs = self.get_workspace_tabs(workspace_id);
        let panes_by_tab: Vec<(u32, Vec<ClientPane>)> = tabs
            .iter()
            .map(|tab| (tab.id, self.get_workspace_panes(workspace_id, tab.id)))
            .collect();
        let layouts: Vec<serde_json::Value> = tabs
            .iter()
            .filter_map(|tab| {
                let layout = self.get_workspace_layout(workspace_id, tab.id)?;
                let active = panes_by_tab
                    .iter()
                    .find(|(tab_id, _)| *tab_id == tab.id)
                    .and_then(|(_, panes)| panes.iter().find(|pane| pane.is_active))
                    .map(|pane| pane.id)
                    .unwrap_or_else(|| client_layout_first_pane(&layout));
                Some(serde_json::json!({
                    "tab": tab.id,
                    "tree": client_layout_to_json(&layout),
                    "active": active,
                }))
            })
            .collect();
        let tabs_json: Vec<serde_json::Value> = tabs
            .iter()
            .map(|tab| {
                serde_json::json!({
                    "id": tab.id,
                    "name": tab.name,
                    "active": tab.is_active,
                })
            })
            .collect();
        let panes_json: Vec<serde_json::Value> = panes_by_tab
            .iter()
            .flat_map(|(tab_id, panes)| {
                panes.iter().map(move |pane| {
                    serde_json::json!({
                        "id": pane.id,
                        "tab": tab_id,
                        "active": pane.is_active,
                        "title": pane.title,
                        "cols": pane.cols,
                        "rows": pane.rows,
                    })
                })
            })
            .collect();
        let active_tab = tabs.iter().find(|tab| tab.is_active).map(|tab| tab.id);
        let active_pane = active_tab.and_then(|tab_id| {
            panes_by_tab
                .iter()
                .find(|(id, _)| *id == tab_id)
                .and_then(|(_, panes)| panes.iter().find(|pane| pane.is_active))
                .map(|pane| pane.id)
        });

        Ok(serde_json::json!({
            "kind": "workspace_topology",
            "workspace_id": workspace_id,
            "snapshot": {
                "workspace_name": workspace.name,
                "workspace_runtime": workspace.runtime,
                "tabs": tabs_json,
                "panes": panes_json,
                "layouts": layouts,
                "active_tab": active_tab,
                "active_pane": active_pane,
            },
        }))
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

    /// Create and open a Runtime-native worktree through Core.
    pub fn create_native_worktree(
        &self,
        source_workspace_id: &str,
        branch: &str,
        path: &str,
        base: Option<&str>,
        label: Option<&str>,
    ) -> anyhow::Result<ClientOpenedWorkspace> {
        let source_workspace_id = cstring(source_workspace_id);
        let branch = cstring(branch);
        let path = cstring(path);
        let base = cstring_opt(base);
        let label = cstring_opt(label);
        let value = Self::discovery_json(|| unsafe {
            ffi::muxterm_workspace_worktree_create_json(
                self.handle.as_ptr(),
                source_workspace_id.as_ptr(),
                branch.as_ptr(),
                path.as_ptr(),
                base.as_ref().map_or(ptr::null(), |value| value.as_ptr()),
                label.as_ref().map_or(ptr::null(), |value| value.as_ptr()),
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

    /// Take and immediately copy parser-generated replies for one pane.
    pub fn take_workspace_pane_reply(&self, workspace_id: &str, pane_id: u32) -> Vec<u8> {
        let workspace_id = cstring(workspace_id);
        let mut len = 0usize;
        let data = unsafe {
            ffi::muxterm_workspace_take_pane_reply(
                self.handle.as_ptr(),
                workspace_id.as_ptr(),
                pane_id,
                &mut len,
            )
        };
        copy_bytes(data, len)
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

    /// Search the Core-owned indexes without activating a workspace.
    pub fn search_all(&self, query: &str) -> anyhow::Result<Vec<ClientSearchHit>> {
        let query = cstring(query);
        let value = Self::discovery_json(|| unsafe {
            ffi::muxterm_search_all(self.handle.as_ptr(), query.as_ptr())
        })?;
        Ok(serde_json::from_value(value["hits"].clone())?)
    }

    pub fn send_input(&self, pane_id: u32, data: &[u8]) -> i32 {
        if data.is_empty() {
            return 0;
        }
        unsafe { ffi::muxterm_send_input(self.handle.as_ptr(), pane_id, data.as_ptr(), data.len()) }
    }

    pub fn send_input_quiet(&self, pane_id: u32, data: &[u8]) -> i32 {
        if data.is_empty() {
            return 0;
        }
        unsafe {
            ffi::muxterm_send_input_quiet(self.handle.as_ptr(), pane_id, data.as_ptr(), data.len())
        }
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
        let mut snapshot: ClientActivitySnapshot = serde_json::from_value(value)?;
        for workspace in &mut snapshot.workspaces {
            for pane in &mut workspace.panes {
                if pane.workspace_id.is_empty() {
                    pane.workspace_id.clone_from(&workspace.workspace_id);
                }
            }
        }
        Ok(snapshot)
    }

    /// Drain owned activity notifications without exposing Core references.
    pub fn take_activity_notifications(&self) -> anyhow::Result<ClientActivityNotifications> {
        let value = Self::discovery_json(|| unsafe {
            ffi::muxterm_attention_take_notifications(self.handle.as_ptr())
        })?;
        Ok(serde_json::from_value(value)?)
    }

    /// Read the revisioned Core Activity records after a workspace poll.
    pub fn activity_records(&self) -> anyhow::Result<Vec<serde_json::Value>> {
        let value = Self::discovery_json(|| unsafe {
            ffi::muxterm_activity_snapshot_json(self.handle.as_ptr())
        })?;
        Ok(value
            .get("records")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    /// Drain Core Activity lane events into owned JSON values.
    pub fn take_activity_events(&self) -> anyhow::Result<Vec<serde_json::Value>> {
        let value = Self::discovery_json(|| unsafe {
            ffi::muxterm_activity_take_events_json(self.handle.as_ptr())
        })?;
        Ok(value
            .get("events")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default())
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
                .map(ClientError::from_json)
                .unwrap_or_else(|| ClientError::legacy("unknown Core discovery error".into()));
            return Err(anyhow::Error::new(error));
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

    /// Discover pane snapshots in one SSH tmux session through the public FFI.
    pub fn discover_ssh_tmux_panes(
        target: &str,
        socket: Option<&str>,
        session: &str,
    ) -> anyhow::Result<Vec<ClientTmuxPane>> {
        let target = cstring(target);
        let socket = cstring_opt(socket);
        let session = cstring(session);
        let config_path = std::env::var("MUXTERM_SSH_CONFIG_PATH").ok();
        let config_path = cstring_opt(config_path.as_deref());
        let value = Self::discovery_json(|| {
            ffi::muxterm_discover_ssh_tmux_panes_json(
                target.as_ptr(),
                socket.as_ref().map_or(ptr::null(), |value| value.as_ptr()),
                session.as_ptr(),
                config_path
                    .as_ref()
                    .map_or(ptr::null(), |value| value.as_ptr()),
                DISCOVERY_TIMEOUT_MS,
            )
        })?;
        Ok(serde_json::from_value(value["panes"].clone())?)
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
        ClientTask::SwitchPane { pane_id } => (ffi::TASK_SWITCH_PANE, pane_id, 0, 0),
        ClientTask::TogglePaneFullscreen { pane_id } => {
            (ffi::TASK_TOGGLE_PANE_FULLSCREEN, pane_id, 0, 0)
        }
        ClientTask::BreakPane { pane_id } => (ffi::TASK_BREAK_PANE, pane_id, 0, 0),
        ClientTask::RefreshTabs => (ffi::TASK_REFRESH_TABS, 0, 0, 0),
        ClientTask::RequestPaneSnapshot { pane_id } => {
            (ffi::TASK_REQUEST_PANE_SNAPSHOT, pane_id, 0, 0)
        }
        ClientTask::Detach => (ffi::TASK_DETACH, 0, 0, 0),
        ClientTask::Shutdown => (ffi::TASK_SHUTDOWN, 0, 0, 0),
    };
    CTask {
        type_,
        target_pane,
        target_tab,
        dir,
        name: ptr::null(),
    }
}

/// Convert a settings-manifest dotted path to an RFC 6902 JSON Pointer.
fn dotted_config_pointer(path: &str) -> anyhow::Result<String> {
    if path.trim().is_empty() {
        anyhow::bail!("配置路径不能为空");
    }
    Ok(format!(
        "/{}",
        path.split('.')
            .map(|part| part.replace('~', "~0").replace('/', "~1"))
            .collect::<Vec<_>>()
            .join("/")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attention_config_serializes_as_public_ffi_shape() {
        let config = ClientAttentionConfig {
            enabled: false,
            blocked_regex: vec!["^needs-input$".into()],
            debounce_ms: 250,
        };

        assert_eq!(
            serde_json::to_value(config).expect("attention config serializes"),
            serde_json::json!({
                "enabled": false,
                "blocked_regex": ["^needs-input$"],
                "debounce_ms": 250,
            })
        );
    }

    #[test]
    fn config_patch_serializes_as_core_json_patch() {
        let patch = vec![ClientJsonPatchOperation {
            op: "replace".into(),
            path: "/font/size".into(),
            value: Some(serde_json::json!(14.0)),
        }];

        assert_eq!(
            serde_json::to_value(patch).expect("config patch serializes"),
            serde_json::json!([{
                "op": "replace",
                "path": "/font/size",
                "value": 14.0,
            }])
        );
    }

    #[test]
    fn dotted_config_pointer_escapes_json_pointer_tokens() {
        assert_eq!(
            dotted_config_pointer("theme.name").expect("valid config path"),
            "/theme/name"
        );
        assert_eq!(
            dotted_config_pointer("extensions.vendor~name.value/name").expect("valid config path"),
            "/extensions/vendor~0name/value~1name"
        );
    }

    #[test]
    fn dotted_config_pointer_rejects_empty_path() {
        assert!(dotted_config_pointer(" ").is_err());
    }

    #[test]
    fn config_snapshot_and_draft_decode_as_owned_values() {
        let colors = vec![serde_json::json!([0, 0, 0]); 16];
        let snapshot: ClientConfigSnapshot = serde_json::from_value(serde_json::json!({
            "path": "/tmp/config.toml",
            "revision": "rev-1",
            "raw": {"font": {"size": 13.0}},
            "values": {"font": {"size": 13.0}},
            "defaults": {"font": {"size": 12.0}},
            "schema": {"type": "object"},
            "manifest": {"schema_id": "muxterm.config.v1"},
            "action_catalog": {"actions": []},
            "resolved_theme": {
                "name": "white",
                "background": [255, 255, 255],
                "foreground": [31, 35, 40],
                "cursor": [31, 35, 40],
            "colors": colors,
            },
            "effective_keybindings": [{
                "key": "n",
                "mods": ["alt"],
                "action": "new_window",
            }],
        }))
        .expect("config snapshot decodes");
        assert_eq!(snapshot.path, "/tmp/config.toml");
        assert_eq!(snapshot.revision, "rev-1");
        assert_eq!(snapshot.values["font"]["size"], 13.0);
        assert_eq!(
            snapshot.resolved_theme.as_ref().unwrap().background,
            ClientRgb(255, 255, 255)
        );
        assert_eq!(snapshot.effective_keybindings[0].action, "new_window");

        let draft: ClientConfigDraft = serde_json::from_value(serde_json::json!({
            "transaction": "tx-1",
            "values": {"font": {"size": 14.0}},
            "diagnostics": [],
        }))
        .expect("config draft decodes");
        assert_eq!(draft.transaction, "tx-1");
        assert!(draft.diagnostics.is_empty());
    }

    #[test]
    fn config_values_decode_to_frontend_config_dto() {
        let config: ClientConfig = serde_json::from_value(serde_json::json!({
            "font": {"family": "Iosevka", "size": 14.0, "fallback": ["monospace"]},
            "theme": {"name": "black", "light": "white", "dark": "black"},
            "statusbar": {"mode": "theme"},
            "pool": {"max_slots": 7},
            "tmux": {"auto_mouse": false, "default_session": "dev", "socket": "muxterm-test"},
            "scrollback": {"lines": 500},
            "attention": {"enabled": false, "blocked_regex": ["secret"], "debounce_ms": 250},
            "behavior": {
                "on_last_pane_exit": "keep_empty",
                "on_program_exit_abnormal": "keep",
            },
        }))
        .expect("config values decode");
        assert_eq!(config.font.family, "Iosevka");
        assert_eq!(config.tmux.socket, "muxterm-test");
        assert_eq!(config.pool.max_slots, 7);
        assert_eq!(
            config.behavior.on_last_pane_exit,
            ClientOnLastPaneExit::KeepEmpty
        );
        assert_eq!(
            config.behavior.on_program_exit_abnormal,
            ClientOnProgramExitAbnormal::Keep
        );
    }

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
    fn frontend_lifecycle_tasks_translate_to_abi() {
        let switch = task_to_ffi(ClientTask::SwitchPane { pane_id: 13 });
        assert_eq!(switch.type_, ffi::TASK_SWITCH_PANE);
        assert_eq!(switch.target_pane, 13);

        let snapshot = task_to_ffi(ClientTask::RequestPaneSnapshot { pane_id: 14 });
        assert_eq!(snapshot.type_, ffi::TASK_REQUEST_PANE_SNAPSHOT);
        assert_eq!(snapshot.target_pane, 14);

        assert_eq!(
            task_to_ffi(ClientTask::RefreshTabs).type_,
            ffi::TASK_REFRESH_TABS
        );
        assert_eq!(task_to_ffi(ClientTask::Detach).type_, ffi::TASK_DETACH);
        assert_eq!(task_to_ffi(ClientTask::Shutdown).type_, ffi::TASK_SHUTDOWN);
    }

    #[test]
    fn search_hits_decode_as_owned_frontend_dtos() {
        let hit: ClientSearchHit = serde_json::from_value(serde_json::json!({
            "workspace_id": "local//demo/shell/",
            "tab_id": 1,
            "pane_id": 7,
            "seq": 42,
            "line": "cargo test",
        }))
        .expect("search hit DTO");
        assert_eq!(hit.workspace_id, "local//demo/shell/");
        assert_eq!(hit.seq, 42);
        assert_eq!(hit.line, "cargo test");
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
    fn event_wire_uses_semantic_kind_and_keeps_common_fields() {
        let event = ClientEvent {
            type_: STATE_PANE_OUTPUT,
            pane_id: 7,
            tab_id: 3,
            window_id: 0,
            data: vec![0, 255, 27],
            name: String::new(),
        };
        let wire = event.to_wire_json("local//demo/shell/");
        assert_eq!(wire["workspace_id"], "local//demo/shell/");
        assert_eq!(wire["kind"], "pane_output");
        assert_eq!(wire["pane_id"], 7);
        assert_eq!(wire["tab_id"], 3);
        assert_eq!(wire["data"], serde_json::json!([0, 255, 27]));
    }

    #[test]
    fn event_wire_adds_resize_and_payload_semantics() {
        let resized = ClientEvent {
            type_: STATE_PANE_RESIZED,
            pane_id: 7,
            tab_id: 3,
            window_id: 0,
            data: vec![120, 0, 40, 0],
            name: String::new(),
        };
        let wire = resized.to_wire_json("ws");
        assert_eq!(wire["kind"], "pane_resized");
        assert_eq!(wire["cols"], 120);
        assert_eq!(wire["rows"], 40);

        let agent = ClientEvent {
            type_: STATE_PANE_AGENT_CHANGED,
            pane_id: 7,
            tab_id: 3,
            window_id: 0,
            data: br#"{"initial":true,"agent":null}"#.to_vec(),
            name: String::new(),
        };
        let wire = agent.to_wire_json("ws");
        assert_eq!(wire["kind"], "pane_agent_changed");
        assert_eq!(wire["payload"]["initial"], true);
        assert!(wire["payload"]["agent"].is_null());
    }

    #[test]
    fn topology_classification_includes_active_and_resize_changes() {
        let event = ClientEvent {
            type_: STATE_ACTIVE_TAB_CHANGED,
            pane_id: 0,
            tab_id: 1,
            window_id: 0,
            data: Vec::new(),
            name: String::new(),
        };
        assert!(event.is_topology());
        assert!(ClientEvent {
            type_: STATE_PANE_RESIZED,
            ..event.clone()
        }
        .is_topology());
        assert!(!ClientEvent {
            type_: STATE_PANE_OUTPUT,
            ..event
        }
        .is_topology());
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

    #[test]
    fn discovery_json_preserves_structured_error_envelope() {
        let raw = CString::new(
            r#"{"ok":false,"error":{"code":"incompatible_channels","stage":"identity","message":"channel mismatch","runtime_id":"herdr"}}"#,
        )
        .unwrap();
        let error = FfiClient::discovery_json(|| raw.into_raw()).unwrap_err();
        let error = error
            .downcast_ref::<ClientError>()
            .expect("structured Core errors should remain typed");

        assert_eq!(error.code.as_deref(), Some("incompatible_channels"));
        assert_eq!(error.stage.as_deref(), Some("identity"));
        assert_eq!(error.message, "channel mismatch");
        assert_eq!(error.details["runtime_id"], "herdr");
    }

    #[test]
    fn discovery_json_keeps_legacy_string_errors_compatible() {
        let raw = CString::new(r#"{"ok":false,"error":"legacy failure"}"#).unwrap();
        let error = FfiClient::discovery_json(|| raw.into_raw()).unwrap_err();
        let error = error
            .downcast_ref::<ClientError>()
            .expect("legacy errors should use the common client error type");

        assert_eq!(error.code, None);
        assert_eq!(error.stage, None);
        assert_eq!(error.message, "legacy failure");
        assert!(error.details.is_empty());
    }

    #[test]
    fn client_tmux_pane_decodes_owned_snapshot() {
        let pane: ClientTmuxPane = serde_json::from_value(serde_json::json!({
            "id": 7,
            "active": true,
            "cols": 120,
            "rows": 40,
            "title": "build shell",
        }))
        .expect("SSH pane discovery DTO should decode");

        assert_eq!(pane.id, 7);
        assert!(pane.active);
        assert_eq!(pane.cols, 120);
        assert_eq!(pane.rows, 40);
        assert_eq!(pane.title, "build shell");
    }
}
