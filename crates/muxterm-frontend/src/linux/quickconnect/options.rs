//! 新建/编辑 Project 时 runtime / transport 单选卡的纯状态模型。

use super::model::{TargetRuntime, TargetTransport};

/// runtime / transport 单选卡状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetOptionSelection {
    pub runtime: TargetRuntime,
    pub transport: TargetTransport,
}

impl Default for TargetOptionSelection {
    fn default() -> Self {
        TargetOptionSelection {
            runtime: TargetRuntime::Tmux,
            transport: TargetTransport::Local,
        }
    }
}

impl TargetOptionSelection {
    pub fn new(runtime: TargetRuntime, transport: TargetTransport) -> Self {
        TargetOptionSelection { runtime, transport }
    }

    pub fn is_runtime_selected(&self, candidate: TargetRuntime) -> bool {
        self.runtime == candidate
    }

    pub fn is_transport_selected(&self, candidate: &TargetTransport) -> bool {
        &self.transport == candidate
    }

    pub fn select_runtime(&mut self, candidate: TargetRuntime) {
        self.runtime = candidate;
    }

    pub fn select_transport(&mut self, candidate: TargetTransport) {
        self.transport = candidate;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_selection_keeps_given_values() {
        let sel = TargetOptionSelection::new(
            TargetRuntime::Shell,
            TargetTransport::Ssh {
                name: "ryzen".into(),
            },
        );
        assert!(sel.is_runtime_selected(TargetRuntime::Shell));
        assert!(sel.is_transport_selected(&TargetTransport::Ssh {
            name: "ryzen".into()
        }));
    }

    #[test]
    fn exactly_one_runtime_and_transport_selected() {
        let mut sel = TargetOptionSelection::default();
        assert!(sel.is_runtime_selected(TargetRuntime::Tmux));
        assert!(!sel.is_runtime_selected(TargetRuntime::Shell));
        assert!(sel.is_transport_selected(&TargetTransport::Local));
        sel.select_runtime(TargetRuntime::Shell);
        sel.select_transport(TargetTransport::Ssh {
            name: "ryzen".into(),
        });
        assert!(sel.is_runtime_selected(TargetRuntime::Shell));
        assert!(sel.is_transport_selected(&TargetTransport::Ssh {
            name: "ryzen".into()
        }));
    }
}
