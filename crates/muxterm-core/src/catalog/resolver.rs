//! Catalog resolver：target descriptor → ResolvedTarget 的唯一入口（W6 §11.2）。
//!
//! Project/Recent/Existing 三路都走 [`resolve_target`]；platform 不得复制
//! 第二套 resolver。identity key 只由身份字段构成（transport target /
//! runtime / session / target-side socket / workspace_id），name/path 是
//! 显示/项目元数据，不参与身份。旧 TargetConfig 仅在兼容 API 边界转换。

use crate::projects::{ProjectTarget, TargetConfig, TargetRuntime, TargetTransport};
use crate::workspace::spec::WorkspaceSpec;

pub use crate::protocol::candidate::{OpenRequest, ResolveIntent};

/// 解析失败阶段（用户通知显示阶段 + 身份摘要）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveErrorStage {
    Discovery,
    IdentityResolution,
    WorkspaceCreate,
    SocketForward,
    RuntimeConnect,
}

/// Structured failures produced while resolving or constructing a product
/// workspace. FFI converts this domain error into an error envelope; callers
/// must not infer a local fallback from its display text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResolveError {
    #[error("unknown runtime '{id}' 未注册")]
    UnknownRuntime { id: String },
    #[error("unknown transport '{id}' 未注册")]
    UnknownTransport { id: String },
    #[error(
        "runtime '{runtime_id}' requires channels {required:?}, but transport '{transport_id}' supports {supported:?}"
    )]
    IncompatibleChannels {
        runtime_id: String,
        transport_id: String,
        required: Vec<crate::transport::ChannelKind>,
        supported: Vec<crate::transport::ChannelKind>,
    },
    #[error("target connection failed (transport={transport_id}, target={target}): {message}")]
    TargetConnection {
        transport_id: String,
        target: String,
        message: String,
    },
    #[error(
        "runtime discovery failed (runtime={runtime_id}, transport={transport_id}, target={target}): {message}"
    )]
    Discovery {
        runtime_id: String,
        transport_id: String,
        target: String,
        message: String,
    },
    #[error(
        "runtime open failed (runtime={runtime_id}, transport={transport_id}, target={target}): {message}"
    )]
    RuntimeOpen {
        runtime_id: String,
        transport_id: String,
        target: String,
        message: String,
    },
    #[error("project 不存在: {id}")]
    ProjectNotFound { id: String },
    #[error("worktree 不存在: project={project_id}, worktree={worktree_id}")]
    WorktreeNotFound {
        project_id: String,
        worktree_id: String,
    },
    #[error("existing candidate identity 不存在: {key}")]
    ExistingCandidateNotFound { key: String },
    #[error("recent candidate 不存在: {key}")]
    RecentNotFound { key: String },
    #[error("没有匹配的 Runtime workspace（identity={identity}, intent={intent}）")]
    NoMatch { identity: String, intent: String },
    #[error("不能创建 Runtime workspace（identity={identity}）：{reason}")]
    CreateNotAllowed { identity: String, reason: String },
    #[error("同名候选 ambiguity（identity={identity}）：{candidates:?}")]
    AmbiguousCandidate {
        identity: String,
        candidates: Vec<String>,
    },
    #[error("workspace identity 不完整（identity={identity}）：{reason}")]
    InvalidIdentity { identity: String, reason: String },
    #[error("template 名称无效（name={name}）：{reason}")]
    InvalidTemplate { name: String, reason: String },
}

impl ResolveError {
    pub fn stage(&self) -> ResolveErrorStage {
        match self {
            Self::Discovery { .. } => ResolveErrorStage::Discovery,
            Self::UnknownRuntime { .. }
            | Self::UnknownTransport { .. }
            | Self::IncompatibleChannels { .. }
            | Self::TargetConnection { .. }
            | Self::ProjectNotFound { .. }
            | Self::WorktreeNotFound { .. }
            | Self::ExistingCandidateNotFound { .. }
            | Self::RecentNotFound { .. }
            | Self::NoMatch { .. }
            | Self::CreateNotAllowed { .. }
            | Self::AmbiguousCandidate { .. }
            | Self::InvalidIdentity { .. }
            | Self::InvalidTemplate { .. } => ResolveErrorStage::IdentityResolution,
            Self::RuntimeOpen { .. } => ResolveErrorStage::RuntimeConnect,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownRuntime { .. } => "unknown_runtime",
            Self::UnknownTransport { .. } => "unknown_transport",
            Self::IncompatibleChannels { .. } => "incompatible_channels",
            Self::TargetConnection { .. } => "target_connection",
            Self::Discovery { .. } => "discovery",
            Self::RuntimeOpen { .. } => "runtime_open",
            Self::ProjectNotFound { .. } => "project_not_found",
            Self::WorktreeNotFound { .. } => "worktree_not_found",
            Self::ExistingCandidateNotFound { .. } => "existing_candidate_not_found",
            Self::RecentNotFound { .. } => "recent_not_found",
            Self::NoMatch { .. } => "no_match",
            Self::CreateNotAllowed { .. } => "create_not_allowed",
            Self::AmbiguousCandidate { .. } => "ambiguous_candidate",
            Self::InvalidIdentity { .. } => "invalid_identity",
            Self::InvalidTemplate { .. } => "invalid_template",
        }
    }
}

impl std::fmt::Display for ResolveErrorStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveErrorStage::Discovery => write!(f, "discovery"),
            ResolveErrorStage::IdentityResolution => write!(f, "identity"),
            ResolveErrorStage::WorkspaceCreate => write!(f, "workspace-create"),
            ResolveErrorStage::SocketForward => write!(f, "socket-forward"),
            ResolveErrorStage::RuntimeConnect => write!(f, "runtime-connect"),
        }
    }
}

/// Resolver 产出的规范化 target descriptor。
///
/// 这是 Catalog 自己拥有的 identity/display 记录，不是 Project 持久化记录，
/// 也不是 frontend 的 QuickConnect 编辑模型。旧 `TargetConfig` 只在兼容
/// 输入边界转换为该类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTargetDescriptor {
    pub name: String,
    pub runtime: TargetRuntime,
    pub transport: TargetTransport,
    pub path: String,
    pub socket: Option<String>,
    pub session: Option<String>,
    pub workspace_id: Option<String>,
}

impl ResolvedTargetDescriptor {
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

    pub fn from_target_config(config: &TargetConfig) -> Self {
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

    pub fn from_project_target(name: impl Into<String>, target: &ProjectTarget) -> Self {
        Self {
            name: name.into(),
            runtime: target.runtime(),
            transport: target.transport().clone(),
            path: target.path().to_string(),
            socket: target.socket().map(str::to_string),
            session: target.session().map(str::to_string),
            workspace_id: target.workspace_id().map(str::to_string),
        }
    }

    /// Build the old record only for compatibility callers that still need it.
    pub fn to_target_config(&self) -> TargetConfig {
        TargetConfig {
            name: self.name.clone(),
            runtime: self.runtime,
            transport: self.transport.clone(),
            path: self.path.clone(),
            socket: self.socket.clone(),
            session: self.session.clone(),
            workspace_id: self.workspace_id.clone(),
        }
    }

    /// Return the stable identity key used by Recent candidate references.
    pub fn identity_key(&self) -> String {
        crate::projects::target_identity_key(
            &self.name,
            self.runtime,
            &self.transport,
            &self.path,
            self.session.as_deref(),
            self.socket.as_deref(),
            self.workspace_id.as_deref(),
        )
    }
}

/// 解析后的打开目标：规范化 descriptor（identity + 显示元数据）+
/// 实际打开用 WorkspaceSpec + 稳定 WorkspaceId。
///
/// Catalog open 后 Workspace 保存这份 descriptor；Recent/重连/高亮只读它，
/// 禁止从 WorkspaceId 五段字符串反向猜 path/socket/workspace_id。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTarget {
    /// 规范化身份与显示元数据（Catalog 打开时保存）。
    pub canonical: ResolvedTargetDescriptor,
    /// 实际打开用 spec（Herdr SSH：spec.socket 是转发后的本地路径，
    /// canonical.socket 永远是 target-side 远端路径，Project/Recent 不保存临时转发）。
    pub spec: WorkspaceSpec,
}

impl ResolvedTarget {
    /// 稳定 WorkspaceId（由 spec 的五段身份字段构成）。
    pub fn workspace_id(&self) -> crate::protocol::WorkspaceId {
        self.spec.id()
    }

    /// 用户可见名称：非空 canonical.name；空时回退 spec.name()。
    /// 禁止把 Herdr named session 当成 Project 名。
    pub fn display_name(&self) -> String {
        if self.canonical.name.trim().is_empty() {
            self.spec.name()
        } else {
            self.canonical.name.clone()
        }
    }
}

/// 从规范化 descriptor 构造打开用 WorkspaceSpec。
///
/// - shell/tmux：session/socket 直通；path 是工作目录。
/// - herdr local：socket = target-side 本机 socket（无转发）。
/// - herdr ssh：spec.socket 保持 target-side 远端路径；`HerdrDriver::open`
///   在 attach 时创建本地 forward，Runtime shutdown 清理，保存的永远不
///   是临时转发路径。
pub fn descriptor_to_spec(config: &ResolvedTargetDescriptor) -> WorkspaceSpec {
    let transport = match &config.transport {
        TargetTransport::Local => "local",
        TargetTransport::Ssh { .. } => "ssh",
    };
    let alias = match &config.transport {
        TargetTransport::Ssh { name } => Some(name.clone()),
        TargetTransport::Local => None,
    };
    let session = config.session.clone().unwrap_or_default();
    // Herdr：identity path 段 = workspace_id（wN）；其它 runtime = 项目 path。
    let path = match config.runtime {
        TargetRuntime::Herdr => config
            .workspace_id
            .clone()
            .unwrap_or_else(|| config.path.clone()),
        _ => config.path.clone(),
    };
    let socket = config.socket.clone();
    WorkspaceSpec {
        transport: transport.to_string(),
        alias,
        session,
        runtime: config.runtime.as_str().to_string(),
        path,
        socket,
        create: false,
        scrollback_lines: 10_000,
        provenance: None,
        template: None,
    }
}

/// Convert a legacy TargetConfig at a compatibility boundary.
pub fn config_to_spec(config: &TargetConfig) -> WorkspaceSpec {
    descriptor_to_spec(&ResolvedTargetDescriptor::from_target_config(config))
}

/// 把一条 Herdr 候选转换为 resolver descriptor（Core 内完成；Linux 不按 HOME 猜
/// socket、不读 `extra`）。`candidate_name` 是用户可见名；缺权威 project
/// path 时 path 保持空（合并同 identity 的已保存 Project path 由调用方
/// 完成，绝不回填 workspace id 当目录）。
pub fn herdr_candidate_to_descriptor(
    candidate_name: String,
    transport: TargetTransport,
    session: Option<String>,
    target_side_socket: Option<String>,
    workspace_id: String,
) -> ResolvedTargetDescriptor {
    let mut config = ResolvedTargetDescriptor::new(
        candidate_name,
        TargetRuntime::Herdr,
        transport,
        String::new(),
    );
    config.session = session;
    config.socket = target_side_socket;
    config.workspace_id = Some(workspace_id);
    config
}

/// Compatibility helper for callers that still consume TargetConfig.
pub fn herdr_candidate_to_config(
    candidate_name: String,
    transport: TargetTransport,
    session: Option<String>,
    target_side_socket: Option<String>,
    workspace_id: String,
) -> TargetConfig {
    herdr_candidate_to_descriptor(
        candidate_name,
        transport,
        session,
        target_side_socket,
        workspace_id,
    )
    .to_target_config()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::candidate::CandidateRef;

    #[test]
    fn open_request_round_trips_typed_reference_and_defaults_activation() {
        let request = OpenRequest {
            candidate: CandidateRef::Worktree {
                project_id: "project-a".into(),
                worktree_id: "worktree-a".into(),
            },
            intent: ResolveIntent::CreateIfMissing,
            template: Some("review".into()),
            activate: true,
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["intent"], "create_if_missing");
        assert_eq!(json["candidate"]["kind"], "worktree");
        assert_eq!(json["template"], "review");

        let decoded: OpenRequest = serde_json::from_value(serde_json::json!({
            "candidate": {
                "kind": "project",
                "value": {"project_id": "project-a"}
            },
            "intent": "attach_only"
        }))
        .unwrap();
        assert!(decoded.activate);
        assert_eq!(decoded.intent, ResolveIntent::AttachOnly);
    }

    #[test]
    fn resolved_descriptor_keeps_legacy_identity_without_project_record() {
        let mut legacy = TargetConfig::new(
            "agents",
            TargetRuntime::Herdr,
            TargetTransport::Ssh {
                name: "buildbox".into(),
            },
            "/work/muxterm",
        );
        legacy.session = Some("agents".into());
        legacy.socket = Some("/remote/herdr.sock".into());
        legacy.workspace_id = Some("w7".into());

        let descriptor = ResolvedTargetDescriptor::from_target_config(&legacy);
        assert_eq!(descriptor.identity_key(), legacy.identity_key());
        assert_eq!(descriptor.to_target_config(), legacy);
        assert_eq!(descriptor.name, "agents");
        assert_eq!(descriptor.path, "/work/muxterm");
    }

    #[test]
    fn resolved_descriptor_reads_project_target_without_project_display_name() {
        let mut project_target =
            ProjectTarget::new(TargetRuntime::Tmux, TargetTransport::Local, "/repo");
        project_target.set_session(Some("demo".into()));
        project_target.set_socket(Some("muxterm-test-resolved-descriptor".into()));

        let descriptor = ResolvedTargetDescriptor::from_project_target("Project", &project_target);
        assert_eq!(descriptor.name, "Project");
        assert_eq!(descriptor.runtime, TargetRuntime::Tmux);
        assert_eq!(descriptor.path, "/repo");
        assert_eq!(descriptor.session.as_deref(), Some("demo"));
        assert_eq!(
            descriptor.socket.as_deref(),
            Some("muxterm-test-resolved-descriptor")
        );
    }
}
