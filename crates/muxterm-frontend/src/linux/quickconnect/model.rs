//! Linux frontend-owned QuickConnect models and Project JSON DTOs.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ffi_client::{ClientCandidateRef, ClientOpenIntent, ClientOpenRequest};

/// Runtime selected by a QuickConnect target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetRuntime {
    Shell,
    Tmux,
    Herdr,
}

impl TargetRuntime {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::Tmux => "tmux",
            Self::Herdr => "herdr",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "shell" => Some(Self::Shell),
            "tmux" => Some(Self::Tmux),
            "herdr" => Some(Self::Herdr),
            _ => None,
        }
    }

    fn from_query_token(value: &str) -> Option<Self> {
        if let Some(exact) = Self::from_str(value) {
            return Some(exact);
        }
        if value.len() < 2 {
            return None;
        }
        let matches = [Self::Shell, Self::Tmux, Self::Herdr]
            .into_iter()
            .filter(|runtime| runtime.as_str().starts_with(value))
            .collect::<Vec<_>>();
        (matches.len() == 1).then(|| matches[0])
    }
}

/// Transport selected by a QuickConnect target.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TargetTransport {
    Local,
    Ssh { name: String },
}

impl TargetTransport {
    pub fn label(&self) -> String {
        match self {
            Self::Local => "local".into(),
            Self::Ssh { name } => name.clone(),
        }
    }

    pub fn is_ssh(&self) -> bool {
        matches!(self, Self::Ssh { .. })
    }

    pub fn create_backend(&self) -> (&'static str, Option<&str>) {
        match self {
            Self::Local => ("local", None),
            Self::Ssh { name } => ("ssh", Some(name.as_str())),
        }
    }

    pub fn attach_backend(&self) -> (&'static str, Option<&str>) {
        match self {
            Self::Local => ("tmux", None),
            Self::Ssh { name } => ("tmux-ssh", Some(name.as_str())),
        }
    }
}

/// A frontend target used for Recent and Project rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetConfigDraft {
    pub name: String,
    pub runtime: TargetRuntime,
    pub transport: TargetTransport,
    pub path: String,
    pub socket: Option<String>,
    pub session: Option<String>,
    pub workspace_id: Option<String>,
}

impl TargetConfigDraft {
    pub fn new(
        name: impl Into<String>,
        runtime: TargetRuntime,
        transport: TargetTransport,
        path: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            runtime,
            transport,
            path: path.into(),
            socket: None,
            session: None,
            workspace_id: None,
        }
    }

    pub fn tmux_session(session: impl Into<String>, transport: TargetTransport) -> Self {
        let session = session.into();
        Self::new(session, TargetRuntime::Tmux, transport, "~")
    }

    pub fn identity_key(&self) -> String {
        let (transport, target) = match &self.transport {
            TargetTransport::Local => ("local", ""),
            TargetTransport::Ssh { name } => ("ssh", name.as_str()),
        };
        let components = match self.runtime {
            TargetRuntime::Shell => vec![
                "shell".to_string(),
                transport.to_string(),
                target.to_string(),
                if self.path.is_empty() {
                    self.name.clone()
                } else {
                    self.path.clone()
                },
            ],
            TargetRuntime::Tmux => vec![
                "tmux".to_string(),
                transport.to_string(),
                target.to_string(),
                self.session
                    .clone()
                    .filter(|session| !session.is_empty())
                    .unwrap_or_else(|| self.name.clone()),
                self.socket.clone().unwrap_or_default(),
            ],
            TargetRuntime::Herdr
                if self
                    .session
                    .as_deref()
                    .is_some_and(|value| !value.is_empty())
                    && self
                        .socket
                        .as_deref()
                        .is_some_and(|value| !value.is_empty())
                    && self
                        .workspace_id
                        .as_deref()
                        .is_some_and(|value| !value.is_empty()) =>
            {
                vec![
                    "herdr".to_string(),
                    transport.to_string(),
                    target.to_string(),
                    self.session.clone().unwrap_or_default(),
                    self.socket.clone().unwrap_or_default(),
                    self.workspace_id.clone().unwrap_or_default(),
                ]
            }
            TargetRuntime::Herdr => vec![
                "herdr-provisional".to_string(),
                transport.to_string(),
                target.to_string(),
                self.name.clone(),
                self.path.clone(),
            ],
        };
        components
            .iter()
            .map(|component| format!("{}:{component}", component.len()))
            .collect::<Vec<_>>()
            .join("|")
    }

    fn search_fields(&self) -> Vec<String> {
        let transport = match &self.transport {
            TargetTransport::Local => "local".to_string(),
            TargetTransport::Ssh { name } => format!("ssh {name}"),
        };
        vec![
            self.name.clone(),
            self.runtime.as_str().to_string(),
            transport,
            self.path.clone(),
            self.session.clone().unwrap_or_default(),
            self.socket.clone().unwrap_or_default(),
            self.workspace_id.clone().unwrap_or_default(),
        ]
    }
}

/// A persisted-free descriptor for a workspace shown in the Recent list.
///
/// Recent rows describe an already opened workspace; they are not editable
/// Project records.  A [`TargetConfigDraft`] is created only as a short-lived
/// compatibility projection when the panel needs the shared row renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentWorkspaceDescriptor {
    pub name: String,
    pub runtime: TargetRuntime,
    pub transport: TargetTransport,
    pub path: String,
    pub socket: Option<String>,
    pub session: Option<String>,
    pub workspace_id: Option<String>,
}

impl RecentWorkspaceDescriptor {
    pub fn from_draft(config: &TargetConfigDraft) -> Self {
        Self {
            name: config.name.clone(),
            runtime: config.runtime,
            transport: config.transport.clone(),
            path: config.path.clone(),
            socket: config.socket.clone(),
            session: config.session.clone(),
            workspace_id: config.workspace_id.clone(),
        }
    }

    pub fn to_draft_config(&self) -> TargetConfigDraft {
        TargetConfigDraft {
            name: self.name.clone(),
            runtime: self.runtime,
            transport: self.transport.clone(),
            path: self.path.clone(),
            socket: self.socket.clone(),
            session: self.session.clone(),
            workspace_id: self.workspace_id.clone(),
        }
    }

    pub fn identity_key(&self) -> String {
        self.to_draft_config().identity_key()
    }
}

/// Search fields shared by list rows without forcing every row into the
/// editable TargetConfigDraft shape.
pub(crate) trait QuickConnectSearchTarget {
    fn runtime_name(&self) -> &str;
    fn is_local_transport(&self) -> bool;
    fn ssh_alias(&self) -> Option<&str>;
    fn search_fields(&self) -> Vec<String>;
}

impl QuickConnectSearchTarget for TargetConfigDraft {
    fn runtime_name(&self) -> &str {
        self.runtime.as_str()
    }

    fn is_local_transport(&self) -> bool {
        matches!(self.transport, TargetTransport::Local)
    }

    fn ssh_alias(&self) -> Option<&str> {
        match &self.transport {
            TargetTransport::Ssh { name } => Some(name),
            TargetTransport::Local => None,
        }
    }

    fn search_fields(&self) -> Vec<String> {
        self.search_fields()
    }
}

/// JSON shape of one persisted Project record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectDocument {
    pub id: String,
    pub name: String,
    pub path: String,
    pub runtime: ProjectRuntime,
    pub transport: ProjectTransport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub worktrees: Vec<WorktreeDocument>,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorktreeDocument {
    pub id: String,
    pub path: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub repo_root: String,
    #[serde(default)]
    pub linked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectRuntime {
    pub id: String,
    #[serde(default)]
    pub options: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectTransport {
    pub id: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub options: BTreeMap<String, Value>,
}

impl ProjectDocument {
    pub fn from_draft(config: &TargetConfigDraft) -> Self {
        let (transport_id, target) = match &config.transport {
            TargetTransport::Local => ("local".to_string(), String::new()),
            TargetTransport::Ssh { name } => ("ssh".to_string(), name.clone()),
        };
        Self {
            id: format!("{}@{}", config.name, transport_id),
            name: config.name.clone(),
            path: config.path.clone(),
            runtime: ProjectRuntime {
                id: config.runtime.as_str().to_string(),
                options: BTreeMap::new(),
                session: config.session.clone(),
                socket: config.socket.clone(),
                workspace_id: config.workspace_id.clone(),
            },
            transport: ProjectTransport {
                id: transport_id,
                target,
                options: BTreeMap::new(),
            },
            template: None,
            worktrees: Vec::new(),
            command: Vec::new(),
            env: BTreeMap::new(),
        }
    }

    pub fn to_draft(&self) -> anyhow::Result<TargetConfigDraft> {
        let runtime = TargetRuntime::from_str(&self.runtime.id)
            .ok_or_else(|| anyhow::anyhow!("unsupported project runtime: {}", self.runtime.id))?;
        let transport = match self.transport.id.to_ascii_lowercase().as_str() {
            "local" => TargetTransport::Local,
            "ssh" => {
                let alias = if self.transport.target.trim().is_empty() {
                    self.transport
                        .options
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                } else {
                    self.transport.target.as_str()
                };
                if alias.trim().is_empty() {
                    return Err(anyhow::anyhow!(
                        "project {} is missing an SSH transport target",
                        self.id
                    ));
                }
                TargetTransport::Ssh {
                    name: alias.to_string(),
                }
            }
            other => return Err(anyhow::anyhow!("unsupported project transport: {other}")),
        };
        let mut target = TargetConfigDraft::new(&self.name, runtime, transport, &self.path);
        target.session = self.runtime.session.clone().or_else(|| {
            self.runtime
                .options
                .get("session")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
        target.socket = self.runtime.socket.clone().or_else(|| {
            self.runtime
                .options
                .get("socket")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
        target.workspace_id = self.runtime.workspace_id.clone().or_else(|| {
            self.runtime
                .options
                .get("workspace_id")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
        Ok(target)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkspaceQuery {
    terms: Vec<String>,
    runtime_filters: Vec<TargetRuntime>,
    local_only: bool,
    ssh_alias_filters: Vec<String>,
}

impl WorkspaceQuery {
    pub fn parse(raw: &str) -> Self {
        let mut query = Self::default();
        for token in raw.split_whitespace() {
            let Some(filter) = token.strip_prefix('@') else {
                query.terms.push(token.to_lowercase());
                continue;
            };
            if filter.is_empty() {
                continue;
            }
            let filter = filter.to_lowercase();
            if let Some(runtime) = TargetRuntime::from_query_token(&filter) {
                query.runtime_filters.push(runtime);
            } else if filter == "local" || unique_local_prefix(&filter) {
                query.local_only = true;
            } else {
                query.ssh_alias_filters.push(filter);
            }
        }
        query
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
            && self.runtime_filters.is_empty()
            && !self.local_only
            && self.ssh_alias_filters.is_empty()
    }

    pub(crate) fn score<T: QuickConnectSearchTarget>(&self, config: &T) -> Option<u32> {
        if self
            .runtime_filters
            .iter()
            .any(|runtime| runtime.as_str() != config.runtime_name())
        {
            return None;
        }
        if self.local_only && !config.is_local_transport() {
            return None;
        }
        for alias in &self.ssh_alias_filters {
            let name = config.ssh_alias()?;
            if !ssh_alias_token_matches(name, alias) {
                return None;
            }
        }
        let fields = config.search_fields();
        let mut score = 10_000u32;
        for term in &self.terms {
            let term_score = fields
                .iter()
                .filter_map(|field| fuzzy_field_score(field, term))
                .max()?;
            score = score.saturating_add(term_score);
        }
        score = score
            .saturating_add((self.runtime_filters.len() as u32) * 2_000)
            .saturating_add(if self.local_only || !self.ssh_alias_filters.is_empty() {
                2_000
            } else {
                0
            });
        Some(score)
    }

    pub fn host_score(&self, alias: &str) -> Option<u32> {
        if self.local_only {
            return None;
        }
        if self
            .ssh_alias_filters
            .iter()
            .any(|want| !ssh_alias_token_matches(alias, want))
        {
            return None;
        }
        if !self.runtime_filters.is_empty()
            && self.ssh_alias_filters.is_empty()
            && self.terms.is_empty()
        {
            return None;
        }
        let mut score = 1_000u32;
        for term in &self.terms {
            score = score.saturating_add(fuzzy_field_score(alias, term)?);
        }
        if !self.ssh_alias_filters.is_empty() {
            score = score.saturating_add(2_000);
        }
        Some(score)
    }

    pub fn completion_candidates(raw: &str, ssh_aliases: &[String]) -> Vec<String> {
        let Some(token) = current_token(raw) else {
            return Vec::new();
        };
        let Some(prefix) = token.strip_prefix('@') else {
            return Vec::new();
        };
        let mut candidates = vec![
            "@shell".to_string(),
            "@tmux".to_string(),
            "@herdr".to_string(),
            "@local".to_string(),
        ];
        let mut seen: HashSet<String> = candidates
            .iter()
            .map(|value| value.to_lowercase())
            .collect();
        for alias in ssh_aliases {
            let alias = alias.trim();
            if !alias.is_empty() {
                let candidate = format!("@{alias}");
                if seen.insert(candidate.to_lowercase()) {
                    candidates.push(candidate);
                }
            }
        }
        let prefix = prefix.to_lowercase();
        candidates.retain(|candidate| {
            let candidate = candidate.strip_prefix('@').unwrap_or(candidate);
            prefix.is_empty() || fuzzy_subsequence(candidate, &prefix).is_some()
        });
        candidates
    }

    pub fn replace_current_token(raw: &str, replacement: &str) -> String {
        let start = raw
            .char_indices()
            .rev()
            .find(|(_, ch)| ch.is_whitespace())
            .map(|(index, ch)| index + ch.len_utf8())
            .unwrap_or(0);
        format!("{}{}", &raw[..start], replacement)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuickBadge {
    Recent,
    Project,
}

impl QuickBadge {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Recent => "Recent",
            Self::Project => "Project",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickConnectEntry {
    pub config: TargetConfigDraft,
    pub badges: Vec<QuickBadge>,
    pub project_id: Option<String>,
}

impl QuickConnectEntry {
    pub fn new(config: TargetConfigDraft, badges: Vec<QuickBadge>) -> Self {
        Self {
            config,
            badges,
            project_id: None,
        }
    }

    pub fn with_project_id(mut self, project_id: impl Into<String>) -> Self {
        self.project_id = Some(project_id.into());
        self
    }

    pub fn candidate_ref(&self) -> ClientCandidateRef {
        if let Some(project_id) = &self.project_id {
            ClientCandidateRef::Project {
                project_id: project_id.clone(),
            }
        } else {
            ClientCandidateRef::Recent {
                key: QuickConnect::unique_id(&self.config),
            }
        }
    }

    pub fn open_request(&self) -> ClientOpenRequest {
        let intent = if self.project_id.is_some() {
            ClientOpenIntent::CreateIfMissing
        } else {
            ClientOpenIntent::AttachOnly
        };
        ClientOpenRequest {
            candidate: self.candidate_ref(),
            intent,
            template: None,
            activate: true,
        }
    }
}

pub enum QuickConnect {}

impl QuickConnect {
    pub fn default_name(for_path: &str) -> String {
        let trimmed = for_path.trim();
        if trimmed.is_empty() {
            return "workspace".into();
        }
        let last = Path::new(trimmed)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_string();
        if last.is_empty() || last == "/" {
            "workspace".into()
        } else {
            last
        }
    }

    pub fn subtitle(config: &TargetConfigDraft) -> String {
        format!("{} @ {}", config.runtime.as_str(), config.transport.label())
    }

    pub fn search_text(config: &TargetConfigDraft) -> String {
        config.search_fields().join(" ").to_lowercase()
    }

    pub fn unique_id(config: &TargetConfigDraft) -> String {
        config.identity_key()
    }

    pub fn badges(
        config: &TargetConfigDraft,
        recents: &[TargetConfigDraft],
        projects: &[TargetConfigDraft],
    ) -> Vec<QuickBadge> {
        let id = Self::unique_id(config);
        let mut badges = Vec::new();
        if recents.iter().any(|recent| Self::unique_id(recent) == id) {
            badges.push(QuickBadge::Recent);
        }
        if projects
            .iter()
            .any(|project| Self::unique_id(project) == id)
        {
            badges.push(QuickBadge::Project);
        }
        badges
    }

    pub fn entries(
        recents: &[TargetConfigDraft],
        projects: &[TargetConfigDraft],
        recent_limit: usize,
    ) -> Vec<QuickConnectEntry> {
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        for config in recents.iter().take(recent_limit) {
            let id = Self::unique_id(config);
            if seen.insert(id) {
                result.push(QuickConnectEntry::new(
                    config.clone(),
                    Self::badges(config, recents, projects),
                ));
            }
        }
        for config in projects {
            let id = Self::unique_id(config);
            if seen.insert(id) {
                result.push(QuickConnectEntry::new(
                    config.clone(),
                    Self::badges(config, recents, projects),
                ));
            }
        }
        result
    }
}

fn unique_local_prefix(filter: &str) -> bool {
    filter.len() >= 2 && "local".starts_with(filter)
}

fn ssh_alias_token_matches(alias: &str, want: &str) -> bool {
    let alias = alias.to_ascii_lowercase();
    let want = want.to_ascii_lowercase();
    want.is_empty()
        || alias == want
        || alias.starts_with(&want)
        || fuzzy_subsequence(&alias, &want).is_some()
}

fn current_token(raw: &str) -> Option<&str> {
    if raw.chars().last().is_some_and(char::is_whitespace) {
        return None;
    }
    raw.split_whitespace().last()
}

fn fuzzy_field_score(field: &str, query: &str) -> Option<u32> {
    let field = field.to_lowercase();
    let query = query.to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    if field.contains(&query) {
        return Some(2_000u32.saturating_sub(field.chars().count() as u32));
    }
    fuzzy_subsequence(&field, &query).map(|gaps| {
        1_000u32
            .saturating_sub(gaps)
            .saturating_sub(field.chars().count() as u32 / 4)
    })
}

fn fuzzy_subsequence(candidate: &str, query: &str) -> Option<u32> {
    let candidate: Vec<char> = candidate.chars().collect();
    let mut gaps = 0u32;
    let mut previous_position = None;
    for wanted in query.chars() {
        let mut found = false;
        for (position, actual) in candidate.iter().enumerate() {
            if previous_position.is_some_and(|previous| position <= previous) {
                continue;
            }
            if *actual == wanted {
                if let Some(previous) = previous_position {
                    gaps = gaps.saturating_add(position.saturating_sub(previous + 1) as u32);
                }
                previous_position = Some(position);
                found = true;
                break;
            }
        }
        if !found {
            return None;
        }
    }
    Some(gaps)
}
