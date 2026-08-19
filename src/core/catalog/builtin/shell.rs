//! ShellDriver：local / SSH shell，共享同一 Runtime tab/pane 语义。

use std::sync::Arc;

use anyhow::{anyhow, Result};

use crate::core::catalog::connect::Connect;
use crate::core::catalog::driver::{RuntimeDriver, SessionCandidate};
use crate::core::model::backend::{Runtime, RuntimeCapability};
use crate::core::runtime::shell::ShellRuntime;
use crate::core::workspace::spec::WorkspaceSpec;

/// shell 插件：transport 差异在 Runtime 构造时归一化。
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
        &["local", "ssh"]
    }

    fn list(&self, _connect: &Connect, _namespace: Option<&str>) -> Result<Vec<SessionCandidate>> {
        Ok(Vec::new())
    }

    fn open(&self, connect: Arc<Connect>, spec: &WorkspaceSpec) -> Result<Box<dyn Runtime>> {
        match spec.transport.as_str() {
            "local" => Ok(Box::new(ShellRuntime::new("$SHELL", &spec.path))),
            "ssh" => {
                let alias = spec
                    .alias
                    .as_deref()
                    .filter(|alias| !alias.is_empty())
                    .unwrap_or_else(|| connect.target());
                if alias.is_empty() {
                    return Err(anyhow!("SSH shell 缺少 alias"));
                }
                Ok(Box::new(ShellRuntime::new_ssh(alias, "$SHELL", &spec.path)))
            }
            transport => Err(anyhow!("shell 不接受 {transport} transport")),
        }
    }
}
