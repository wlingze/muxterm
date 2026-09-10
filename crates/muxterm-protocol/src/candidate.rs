//! Candidate and open-request DTOs shared by Catalog and frontends.
//!
//! A Candidate is a selectable list row. Its reference is the only identity
//! carried back into Core; display text is deliberately not used for resolve.

use crate::WorkspaceId;

/// The four product-level sources shown by QuickConnect.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    Project,
    Worktree,
    Existing,
    Recent,
}

/// Typed identity for a discovered runtime-owned candidate.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct ExistingCandidateRef {
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

impl ExistingCandidateRef {
    /// Build an opaque, delimiter-safe key for Recent/inventory lookup.
    pub fn key(&self) -> String {
        [
            self.runtime_id.as_str(),
            self.transport_id.as_str(),
            self.target.as_str(),
            self.session.as_deref().unwrap_or_default(),
            self.socket.as_deref().unwrap_or_default(),
            self.workspace_id.as_deref().unwrap_or_default(),
        ]
        .iter()
        .map(|value| format!("{}:{value}", value.len()))
        .collect::<Vec<_>>()
        .join("|")
    }
}

/// Identity used to resolve a Candidate. It is never a display label.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum CandidateRef {
    Project {
        project_id: String,
    },
    Worktree {
        project_id: String,
        worktree_id: String,
    },
    Existing {
        identity: ExistingCandidateRef,
    },
    Recent {
        key: String,
    },
}

impl CandidateRef {
    pub fn kind(&self) -> CandidateKind {
        match self {
            Self::Project { .. } => CandidateKind::Project,
            Self::Worktree { .. } => CandidateKind::Worktree,
            Self::Existing { .. } => CandidateKind::Existing,
            Self::Recent { .. } => CandidateKind::Recent,
        }
    }
}

/// One row in the unified openable-candidate list.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct Candidate {
    pub kind: CandidateKind,
    pub title: String,
    #[serde(default)]
    pub subtitle: String,
    #[serde(default)]
    pub badges: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_pool: Option<WorkspaceId>,
    #[serde(rename = "ref")]
    pub reference: CandidateRef,
}

impl Candidate {
    pub fn project(project_id: impl Into<String>, title: impl Into<String>) -> Self {
        Self::new(
            CandidateRef::Project {
                project_id: project_id.into(),
            },
            title,
        )
    }

    pub fn worktree(
        project_id: impl Into<String>,
        worktree_id: impl Into<String>,
        title: impl Into<String>,
    ) -> Self {
        Self::new(
            CandidateRef::Worktree {
                project_id: project_id.into(),
                worktree_id: worktree_id.into(),
            },
            title,
        )
    }

    pub fn existing(candidate: &ExistingCandidate, in_pool: Option<WorkspaceId>) -> Self {
        let identity = ExistingCandidateRef {
            runtime_id: candidate.runtime_id.clone(),
            transport_id: candidate.transport_id.clone(),
            target: candidate.target.clone(),
            session: candidate.session.clone(),
            socket: candidate.socket.clone(),
            workspace_id: candidate.workspace_id.clone(),
        };
        let mut row = Self::new(CandidateRef::Existing { identity }, &candidate.name);
        row.subtitle = candidate.target.clone();
        row.badges = vec![candidate.runtime_id.clone(), candidate.transport_id.clone()];
        row.in_pool = in_pool;
        row
    }

    pub fn recent(key: impl Into<String>, title: impl Into<String>) -> Self {
        Self::new(CandidateRef::Recent { key: key.into() }, title)
    }

    fn new(reference: CandidateRef, title: impl Into<String>) -> Self {
        Self {
            kind: reference.kind(),
            title: title.into(),
            subtitle: String::new(),
            badges: Vec::new(),
            in_pool: None,
            reference,
        }
    }
}

/// Intent carried with a product-level open request.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ResolveIntent {
    AttachOnly,
    CreateIfMissing,
}

/// Frontend-visible request resolved by Core into a WorkspaceSpec.
///
/// Template names remain strings at the protocol boundary. Core validates and
/// converts them into its domain-owned `TemplateName` before opening.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct OpenRequest {
    pub candidate: CandidateRef,
    pub intent: ResolveIntent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    #[serde(default = "default_activate")]
    pub activate: bool,
}

fn default_activate() -> bool {
    true
}

/// Runtime-owned discovery row retained for compatibility with provider APIs.
#[derive(
    Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
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

#[cfg(test)]
mod tests {
    use super::*;

    fn existing() -> ExistingCandidate {
        ExistingCandidate {
            runtime_id: "tmux".into(),
            transport_id: "ssh".into(),
            target: "build-host".into(),
            namespace: Some("work".into()),
            name: "muxterm".into(),
            extra: "opaque-wire-detail".into(),
            session: Some("muxterm".into()),
            socket: Some("muxterm.sock".into()),
            workspace_id: None,
        }
    }

    #[test]
    fn candidate_reference_preserves_all_four_kinds() {
        let rows = [
            Candidate::project("p1", "Project"),
            Candidate::worktree("p1", "wt1", "Worktree"),
            Candidate::existing(&existing(), None),
            Candidate::recent("recent-key", "Recent"),
        ];
        assert_eq!(
            rows.iter().map(|row| row.kind).collect::<Vec<_>>(),
            vec![
                CandidateKind::Project,
                CandidateKind::Worktree,
                CandidateKind::Existing,
                CandidateKind::Recent
            ]
        );
        assert!(matches!(
            rows[1].reference,
            CandidateRef::Worktree { ref project_id, .. } if project_id == "p1"
        ));
    }

    #[test]
    fn existing_reference_key_is_typed_and_json_uses_ref() {
        let row = Candidate::existing(
            &existing(),
            Some(WorkspaceId::new(
                "ssh",
                Some("build-host"),
                "muxterm",
                "tmux",
                "",
            )),
        );
        let json = serde_json::to_value(&row).unwrap();
        assert!(json.get("ref").is_some());
        assert_eq!(json["kind"], "existing");
        assert!(json["in_pool"].is_object());
        let identity = match &row.reference {
            CandidateRef::Existing { identity } => identity,
            _ => panic!("expected existing identity"),
        };
        assert!(identity.key().contains("tmux"));
        assert!(!identity.key().contains("opaque-wire-detail"));

        let mut renamed = existing();
        renamed.name = "renamed-display-only".into();
        let renamed_row = Candidate::existing(&renamed, None);
        assert_eq!(row.reference, renamed_row.reference);
    }

    #[test]
    fn open_request_json_defaults_activation_and_keeps_template_name() {
        let request = OpenRequest {
            candidate: CandidateRef::Project {
                project_id: "project-a".into(),
            },
            intent: ResolveIntent::CreateIfMissing,
            template: Some("review".into()),
            activate: true,
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["intent"], "create_if_missing");
        assert_eq!(json["template"], "review");

        let decoded: OpenRequest = serde_json::from_value(serde_json::json!({
            "candidate": {
                "kind": "project",
                "value": {"project_id": "project-a"}
            },
            "intent": "attach_only",
            "template": "review"
        }))
        .unwrap();
        assert!(decoded.activate);
        assert_eq!(decoded.intent, ResolveIntent::AttachOnly);
        assert_eq!(decoded.template.as_deref(), Some("review"));
    }
}
