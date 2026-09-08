//! Discovery DTOs shared by Catalog, FFI and frontend clients.

/// A runtime-owned thing that can be attached as a new Workspace.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExistingCandidate {
    pub runtime_id: String,
    pub transport_id: String,
    pub target: String,
    pub namespace: Option<String>,
    pub name: String,
    /// Runtime-specific display detail; product identity uses typed fields below.
    pub extra: String,
    pub session: Option<String>,
    pub socket: Option<String>,
    pub workspace_id: Option<String>,
}
