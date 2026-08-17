//! ShellDriver：本地 shell。

use std::sync::Arc;

use anyhow::{anyhow, Result};

use crate::core::catalog::connect::Connect;
use crate::core::catalog::driver::{RuntimeDriver, SessionCandidate};
use crate::core::model::backend::{Runtime, RuntimeCapability};
use crate::core::runtime::shell::ShellRuntime;
use crate::core::workspace::spec::WorkspaceSpec;

/// shell 插件（只接受 local）。
pub struct ShellDriver;

impl RuntimeDriver for ShellDriver {
    fn id(&self) -> &'static str {
        "shell"
    }

    fn name(&self) -> &'static str {
        "shell"
    }

    fn support(&self) -> &'static [RuntimeCapability] {
        &[RuntimeCapability::MultiTab, RuntimeCapability::SplitPane]
    }

    fn accepted_transports(&self) -> &'static [&'static str] {
        &["local"]
    }

    fn list(&self, _connect: &Connect, _namespace: Option<&str>) -> Result<Vec<SessionCandidate>> {
        Ok(Vec::new())
    }

    fn open(&self, _connect: Arc<Connect>, spec: &WorkspaceSpec) -> Result<Box<dyn Runtime>> {
        if spec.transport != "local" {
            return Err(anyhow!("shell 只接受 local transport"));
        }
        Ok(Box::new(ShellRuntime::new("$SHELL", &spec.path)))
    }
}
