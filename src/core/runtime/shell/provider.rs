//! ShellDriver：local / SSH shell，共享同一 Runtime tab/pane 语义。

use std::sync::Arc;

use crate::protocol::candidate::ExistingCandidate;
use crate::runtime::shell::ShellRuntime;
use crate::runtime::RuntimeProvider;
use crate::runtime::RuntimeSpec;
use crate::runtime::{Runtime, RuntimeCapability};
use crate::transport::TargetConnection;

/// shell 插件：transport 差异由 TargetConnection 在 Runtime 内归一化。
pub struct ShellDriver;

impl RuntimeProvider for ShellDriver {
    fn id(&self) -> &'static str {
        "shell"
    }

    fn name(&self) -> &'static str {
        "shell"
    }

    fn support(&self) -> &'static [RuntimeCapability] {
        &[RuntimeCapability::MultiTab, RuntimeCapability::SplitPane]
    }

    fn discover(
        &self,
        _connect: &dyn TargetConnection,
        _namespace: Option<&str>,
    ) -> crate::runtime::RuntimeResult<Vec<ExistingCandidate>> {
        Ok(Vec::new())
    }

    fn new_instance(
        &self,
        connect: Arc<dyn TargetConnection>,
        spec: &RuntimeSpec,
    ) -> crate::runtime::RuntimeResult<Box<dyn Runtime>> {
        Ok(Box::new(ShellRuntime::new_with_connection(
            connect, "$SHELL", &spec.path,
        )))
    }
}
