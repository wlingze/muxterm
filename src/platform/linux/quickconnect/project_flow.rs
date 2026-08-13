//! Project 连接流程：先 attach 已有 session，明确失败后再创建 detached
//! session（twork 语义：session 名 = 显式 name / path basename），创建成功
//! 后 attach 同一 session。local 与 ssh 共用同一状态机（纯逻辑）。

use super::model::{QuickConnect, TargetConfig};

/// 连接失败（区分 attach / create / attach-after-create）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectConnectFailure {
    pub stage: ProjectConnectStage,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectConnectStage {
    AttachExisting,
    Create,
    AttachCreated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectConnectState {
    /// 尝试 attach 已有 session。
    AttachExisting { session: String },
    /// attach 明确失败：创建 detached session。
    CreateDetached { session: String, directory: String },
    /// 创建成功：attach 刚创建的 session。
    AttachCreated { session: String },
    /// 全部成功。
    Done,
    /// 某一步失败。
    Failed(ProjectConnectFailure),
}

/// Project tmux 目标的连接状态机（纯逻辑，可单测）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectConnectFlow {
    /// 最终 session 名：显式 name 优先，空 name 用 path basename。
    pub session: String,
    /// 创建 detached session 时使用的目录（twork 的 `-c <dir>`）。
    pub directory: String,
    pub state: ProjectConnectState,
}

impl ProjectConnectFlow {
    pub fn new(config: &TargetConfig) -> Self {
        let trimmed_name = config.name.trim();
        let session = if trimmed_name.is_empty() {
            QuickConnect::default_name(&config.path)
        } else {
            trimmed_name.to_string()
        };
        let trimmed_path = config.path.trim();
        let directory = if trimmed_path.is_empty() {
            "~".into()
        } else {
            trimmed_path.to_string()
        };
        ProjectConnectFlow {
            session: session.clone(),
            directory,
            state: ProjectConnectState::AttachExisting { session },
        }
    }

    pub fn attach_existing_succeeded(&mut self) {
        self.state = ProjectConnectState::Done;
    }

    pub fn attach_existing_failed(&mut self, _message: &str) {
        if matches!(self.state, ProjectConnectState::AttachExisting { .. }) {
            self.state = ProjectConnectState::CreateDetached {
                session: self.session.clone(),
                directory: self.directory.clone(),
            };
        }
    }

    pub fn create_succeeded(&mut self) {
        if matches!(self.state, ProjectConnectState::CreateDetached { .. }) {
            self.state = ProjectConnectState::AttachCreated {
                session: self.session.clone(),
            };
        }
    }

    pub fn attach_created_succeeded(&mut self) {
        if matches!(self.state, ProjectConnectState::AttachCreated { .. }) {
            self.state = ProjectConnectState::Done;
        }
    }

    pub fn create_failed(&mut self, message: &str) {
        self.state = ProjectConnectState::Failed(ProjectConnectFailure {
            stage: ProjectConnectStage::Create,
            detail: message.to_string(),
        });
    }

    pub fn attach_created_failed(&mut self, message: &str) {
        self.state = ProjectConnectState::Failed(ProjectConnectFailure {
            stage: ProjectConnectStage::AttachCreated,
            detail: message.to_string(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::linux::quickconnect::model::{TargetRuntime, TargetTransport};

    fn cfg(name: &str, path: &str) -> TargetConfig {
        TargetConfig::new(name, TargetRuntime::Tmux, TargetTransport::Local, path)
    }

    #[test]
    fn explicit_name_wins_over_path_basename() {
        let flow = ProjectConnectFlow::new(&cfg("myproj", "~/Developer/self/muxterm"));
        assert_eq!(flow.session, "myproj");
        assert_eq!(flow.directory, "~/Developer/self/muxterm");
    }

    #[test]
    fn empty_name_uses_path_basename() {
        let flow = ProjectConnectFlow::new(&cfg("", "~/Developer/self/muxterm"));
        assert_eq!(flow.session, "muxterm");
    }

    #[test]
    fn attach_failure_leads_to_create_then_attach() {
        let mut flow = ProjectConnectFlow::new(&cfg("s", "~/x"));
        flow.attach_existing_failed("can't find session: s");
        assert!(matches!(
            flow.state,
            ProjectConnectState::CreateDetached { .. }
        ));
        flow.create_succeeded();
        assert!(matches!(
            flow.state,
            ProjectConnectState::AttachCreated { .. }
        ));
        flow.attach_created_succeeded();
        assert_eq!(flow.state, ProjectConnectState::Done);
    }

    #[test]
    fn create_failure_is_distinct() {
        let mut flow = ProjectConnectFlow::new(&cfg("s", "~/x"));
        flow.attach_existing_failed("nope");
        flow.create_failed("already exists");
        assert!(
            matches!(flow.state, ProjectConnectState::Failed(f) if f.stage == ProjectConnectStage::Create)
        );
    }

    #[test]
    fn direct_attach_success_skips_create() {
        let mut flow = ProjectConnectFlow::new(&cfg("s", "~/x"));
        flow.attach_existing_succeeded();
        assert_eq!(flow.state, ProjectConnectState::Done);
    }
}
