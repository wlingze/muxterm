//! Runtime 更新的产品状态；安装与协议细节留在 provider 内。
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RuntimeUpdateStatus {
    pub phase: String,
    pub message: String,
    pub version: Option<String>,
}
impl RuntimeUpdateStatus {
    pub fn new(phase: &str, message: impl Into<String>) -> Self {
        Self {
            phase: phase.into(),
            message: message.into(),
            version: None,
        }
    }
    pub fn busy(&self) -> bool {
        matches!(
            self.phase.as_str(),
            "checking" | "installing" | "handoff" | "reconnecting"
        )
    }
}
