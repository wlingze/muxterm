//! Catalog resolver：TargetConfig → ResolvedTarget 的唯一入口（W6 §11.2）。
//!
//! Project/Recent/Existing 三路都走 [`resolve_target`]；platform 不得复制
//! 第二套 resolver。identity key 只由身份字段构成（transport target /
//! runtime / session / target-side socket / workspace_id），name/path 是
//! 显示/项目元数据，不参与身份。

use crate::core::quickconnect::model::{TargetConfig, TargetRuntime, TargetTransport};
use crate::core::workspace::spec::WorkspaceSpec;

/// 打开意图：Existing/Recent/普通 Project 重连 = AttachOnly（无匹配不创建）；
/// 初次新建 Project 才允许 CreateIfMissing。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveIntent {
    AttachOnly,
    CreateIfMissing,
}

/// 解析失败阶段（用户通知显示阶段 + 身份摘要）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveErrorStage {
    Discovery,
    IdentityResolution,
    WorkspaceCreate,
    SocketForward,
    RuntimeConnect,
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

/// 解析后的打开目标：规范化 TargetConfig（identity + 显示元数据）+
/// 实际打开用 WorkspaceSpec + 稳定 WorkspaceId。
///
/// Catalog open 后 Workspace 保存这份 descriptor；Recent/重连/高亮只读它，
/// 禁止从 WorkspaceId 五段字符串反向猜 path/socket/workspace_id。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTarget {
    /// 规范化身份与显示元数据（Catalog 打开时保存）。
    pub canonical: TargetConfig,
    /// 实际打开用 spec（Herdr SSH：spec.socket 是转发后的本地路径，
    /// canonical.socket 永远是 target-side 远端路径，Project/Recent 不保存临时转发）。
    pub spec: WorkspaceSpec,
}

impl ResolvedTarget {
    /// 稳定 WorkspaceId（由 spec 的五段身份字段构成）。
    pub fn workspace_id(&self) -> crate::core::workspace::id::WorkspaceId {
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

/// 从规范化 TargetConfig 构造打开用 WorkspaceSpec。
///
/// - shell/tmux：session/socket 直通；path 是工作目录。
/// - herdr local：socket = target-side 本机 socket（无转发）。
/// - herdr ssh：spec.socket 保持 target-side 远端路径；`HerdrDriver::open`
///   在 attach 时创建本地 forward，Runtime shutdown 清理，保存的永远不
///   是临时转发路径。
pub fn config_to_spec(config: &TargetConfig) -> WorkspaceSpec {
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
    }
}

/// 把一条 Herdr 候选转换为 TargetConfig（Core 内完成；Linux 不按 HOME 猜
/// socket、不读 `extra`）。`candidate_name` 是用户可见名；缺权威 project
/// path 时 path 保持空（合并同 identity 的已保存 Project path 由调用方
/// 完成，绝不回填 workspace id 当目录）。
pub fn herdr_candidate_to_config(
    candidate_name: String,
    transport: TargetTransport,
    session: Option<String>,
    target_side_socket: Option<String>,
    workspace_id: String,
) -> TargetConfig {
    let mut config = TargetConfig::new(
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
