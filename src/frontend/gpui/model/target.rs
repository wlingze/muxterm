//! 目标 runtime / transport 词汇与新建 Project 的单选卡状态。
//!
//! 移植自 `chrome/QuickConnectModel.swift` 里的 `TargetRuntime` /
//! `TargetTransport`，以及 `chrome/TargetOptionSelection.swift`。
//!
//! 这两个枚举是全前端共享词汇，所以单独成模块，避免每个窗口各写一份。

/// 连接 runtime。顺序也是单选卡的展示顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetRuntime {
    Shell,
    Tmux,
    Herdr,
}

impl TargetRuntime {
    /// 全部取值，顺序稳定。
    pub const ALL: [TargetRuntime; 3] = [
        TargetRuntime::Shell,
        TargetRuntime::Tmux,
        TargetRuntime::Herdr,
    ];

    /// 中文注释：展示名与查询字面量一致。
    pub fn label(self) -> &'static str {
        match self {
            TargetRuntime::Shell => "shell",
            TargetRuntime::Tmux => "tmux",
            TargetRuntime::Herdr => "herdr",
        }
    }

    /// 解析 JSON / 查询里的字面量。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "shell" => Some(TargetRuntime::Shell),
            "tmux" => Some(TargetRuntime::Tmux),
            "herdr" => Some(TargetRuntime::Herdr),
            _ => None,
        }
    }

    /// 查询 token：完整名字，或长度 >= 2 且在三个 runtime 中唯一的前缀。
    pub fn from_query_token(value: &str) -> Option<Self> {
        if let Some(exact) = Self::parse(value) {
            return Some(exact);
        }
        if value.chars().count() < 2 {
            return None;
        }
        let hits: Vec<Self> = Self::ALL
            .into_iter()
            .filter(|candidate| candidate.label().starts_with(value))
            .collect();
        if hits.len() == 1 {
            hits.first().copied()
        } else {
            None
        }
    }
}

/// 连接传输：ssh 需要别名，local 不需要。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TargetTransport {
    Local,
    Ssh { name: String },
}

impl TargetTransport {
    /// 中文注释：展示名；ssh 显示别名。
    pub fn label(&self) -> &str {
        match self {
            TargetTransport::Local => "local",
            TargetTransport::Ssh { name } => name,
        }
    }

    /// 中文注释：只有 ssh 需要别名。
    pub fn is_ssh(&self) -> bool {
        matches!(self, TargetTransport::Ssh { .. })
    }
}

/// 新建/编辑 Project 时 runtime / transport 单选卡的纯状态模型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetOptionSelection {
    pub runtime: TargetRuntime,
    pub transport: TargetTransport,
}

impl Default for TargetOptionSelection {
    fn default() -> Self {
        Self {
            runtime: TargetRuntime::Tmux,
            transport: TargetTransport::Local,
        }
    }
}

impl TargetOptionSelection {
    /// 中文注释：默认 tmux + local。
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_runtime_selected(&self, candidate: TargetRuntime) -> bool {
        self.runtime == candidate
    }

    pub fn is_transport_selected(&self, candidate: &TargetTransport) -> bool {
        self.transport == *candidate
    }

    pub fn select_runtime(&mut self, candidate: TargetRuntime) {
        self.runtime = candidate;
    }

    pub fn select_transport(&mut self, candidate: TargetTransport) {
        self.transport = candidate;
    }
}

/// 选项卡的无障碍标识：显式带上 selected / unselected。
pub fn option_identifier(kind: &str, option: &str, selected: bool) -> String {
    let state = if selected { "selected" } else { "unselected" };
    format!("muxterm.target.{kind}.{option}.{state}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对照 `TargetOptionSelectionTests.testDefaultSelectionIsTMUXLocal`。
    #[test]
    fn default_selection_is_tmux_local() {
        let selection = TargetOptionSelection::new();
        assert!(selection.is_runtime_selected(TargetRuntime::Tmux));
        assert!(!selection.is_runtime_selected(TargetRuntime::Shell));
        assert!(!selection.is_runtime_selected(TargetRuntime::Herdr));
        assert!(selection.is_transport_selected(&TargetTransport::Local));
        assert!(!selection.is_transport_selected(&TargetTransport::Ssh {
            name: "ryzen".to_string()
        }));
    }

    /// 对照 `testSelectingRuntimeKeepsExactlyOneSelected`。
    #[test]
    fn selecting_runtime_keeps_exactly_one_selected() {
        let mut selection = TargetOptionSelection::new();
        selection.select_runtime(TargetRuntime::Shell);
        assert!(selection.is_runtime_selected(TargetRuntime::Shell));
        assert!(!selection.is_runtime_selected(TargetRuntime::Tmux));

        selection.select_runtime(TargetRuntime::Tmux);
        assert!(selection.is_runtime_selected(TargetRuntime::Tmux));
        assert!(!selection.is_runtime_selected(TargetRuntime::Shell));

        selection.select_runtime(TargetRuntime::Herdr);
        assert!(selection.is_runtime_selected(TargetRuntime::Herdr));
        assert!(!selection.is_runtime_selected(TargetRuntime::Tmux));
        assert!(!selection.is_runtime_selected(TargetRuntime::Shell));
    }

    /// 对照 `testRuntimeCatalogModelIncludesHerdr`。
    #[test]
    fn runtime_catalog_includes_herdr() {
        assert_eq!(
            TargetRuntime::ALL,
            [
                TargetRuntime::Shell,
                TargetRuntime::Tmux,
                TargetRuntime::Herdr
            ]
        );
    }

    /// 对照 `testSelectingTransportKeepsExactlyOneSelected`。
    #[test]
    fn selecting_transport_keeps_exactly_one_selected() {
        let ryzen = TargetTransport::Ssh {
            name: "ryzen".to_string(),
        };
        let mut selection = TargetOptionSelection::new();
        selection.select_transport(ryzen.clone());
        assert!(selection.is_transport_selected(&ryzen));
        assert!(!selection.is_transport_selected(&TargetTransport::Local));

        selection.select_transport(TargetTransport::Local);
        assert!(selection.is_transport_selected(&TargetTransport::Local));
        assert!(!selection.is_transport_selected(&ryzen));
    }

    /// 对照 `testAccessibilityIdentifiersAreStableAndExposeSelection`。
    #[test]
    fn accessibility_identifiers_are_stable_and_expose_selection() {
        assert_eq!(
            option_identifier("runtime", "tmux", true),
            "muxterm.target.runtime.tmux.selected"
        );
        assert_eq!(
            option_identifier("runtime", "tmux", false),
            "muxterm.target.runtime.tmux.unselected"
        );
        assert_eq!(
            option_identifier("transport", "local", true),
            "muxterm.target.transport.local.selected"
        );
        assert_eq!(
            option_identifier("transport", "ssh", false),
            "muxterm.target.transport.ssh.unselected"
        );
    }

    /// 查询 token 只接受唯一前缀，`h` 这种长度 1 的不算。
    #[test]
    fn query_token_requires_two_characters_and_a_unique_prefix() {
        assert_eq!(
            TargetRuntime::from_query_token("tmux"),
            Some(TargetRuntime::Tmux)
        );
        assert_eq!(
            TargetRuntime::from_query_token("he"),
            Some(TargetRuntime::Herdr)
        );
        assert_eq!(TargetRuntime::from_query_token("h"), None);
        assert_eq!(TargetRuntime::from_query_token("xyz"), None);
    }

    #[test]
    fn transport_label_is_the_ssh_alias() {
        assert_eq!(TargetTransport::Local.label(), "local");
        assert_eq!(
            TargetTransport::Ssh {
                name: "ryzen".to_string()
            }
            .label(),
            "ryzen"
        );
        assert!(!TargetTransport::Local.is_ssh());
    }
}
