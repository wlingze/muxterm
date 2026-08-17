//! 可复用管道。同一 `(transport_id, target)` 在 Catalog 里只保留一份 Arc。

use std::sync::Arc;

/// 一条可复用管道（local 空操作 / ssh 主控 / 测试 mock）。
#[derive(Debug)]
pub struct Connect {
    transport_id: String,
    target: String,
}

impl Connect {
    /// 新建一条管道身份。Catalog 负责缓存 Arc。
    pub fn new(transport_id: impl Into<String>, target: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            transport_id: transport_id.into(),
            target: target.into(),
        })
    }

    pub fn transport_id(&self) -> &str {
        &self.transport_id
    }

    pub fn target(&self) -> &str {
        &self.target
    }
}
