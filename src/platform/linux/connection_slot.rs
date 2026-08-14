//! Warm connection pool 的真实 slot：持有 `CoreBridge`。
//!
//! 切换目标时旧 slot 进入 background 继续 poll（保持 warm），不立即
//! shutdown；只有淘汰时才 evict。tmux/ssh 淘汰用 detach 保留远端
//! server/session，local shell 直接回收 handle。

use std::time::Instant;

use crate::platform::linux::ffi_bridge::CoreBridge;
use crate::platform::linux::quickconnect::model::{TargetConfig, TargetRuntime, TargetTransport};
use crate::platform::linux::quickconnect::pool::{
    ConnectionEvictionReason, ConnectionKey, ConnectionLifecycle, ConnectionSlotProtocol,
};

/// 连接池中一个真实连接。
pub struct WarmConnectionSlot {
    key: ConnectionKey,
    pub bridge: CoreBridge,
    lifecycle: ConnectionLifecycle,
    last_used_at: Instant,
}

impl WarmConnectionSlot {
    pub fn new(key: ConnectionKey, bridge: CoreBridge) -> Self {
        WarmConnectionSlot {
            key,
            bridge,
            lifecycle: ConnectionLifecycle::Active,
            last_used_at: Instant::now(),
        }
    }
}

impl ConnectionSlotProtocol for WarmConnectionSlot {
    fn key(&self) -> &ConnectionKey {
        &self.key
    }

    fn lifecycle(&self) -> ConnectionLifecycle {
        self.lifecycle
    }

    fn set_lifecycle(&mut self, lifecycle: ConnectionLifecycle) {
        self.lifecycle = lifecycle;
    }

    fn last_used_at(&self) -> Instant {
        self.last_used_at
    }

    fn set_last_used_at(&mut self, now: Instant) {
        self.last_used_at = now;
    }

    fn poll_background(&mut self) {
        let _ = self.bridge.poll_events();
    }

    fn evict(&mut self, _reason: ConnectionEvictionReason) {
        self.lifecycle = ConnectionLifecycle::Evicting;
        if self.bridge.uses_tmux() {
            let _ = self.bridge.detach();
        }
        self.bridge.stop_polling();
    }

    fn shutdown(&mut self) {
        self.lifecycle = ConnectionLifecycle::Evicting;
        self.bridge.stop_polling();
    }
}

/// 从 QuickConnect 目标构造连接池 key。
pub fn connection_key(config: &TargetConfig, session: &str) -> ConnectionKey {
    let alias = match &config.transport {
        TargetTransport::Ssh { name } => Some(name.as_str()),
        TargetTransport::Local => None,
    };
    let transport = if config.transport.is_ssh() {
        "ssh"
    } else {
        "local"
    };
    let runtime = config.runtime.as_str();
    ConnectionKey::new(transport, alias, session, runtime, &config.path)
}

/// 启动时的默认连接 key。
pub fn startup_connection_key(uses_tmux: bool, session: Option<&str>) -> ConnectionKey {
    if uses_tmux {
        ConnectionKey::new(
            "local",
            None,
            session.unwrap_or(""),
            TargetRuntime::Tmux.as_str(),
            "",
        )
    } else {
        ConnectionKey::new("local", None, "", TargetRuntime::Shell.as_str(), "")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_tmux_key_uses_alias_and_session() {
        let cfg = TargetConfig::new(
            "muxterm",
            TargetRuntime::Tmux,
            TargetTransport::Ssh {
                name: "ryzen".into(),
            },
            "~/src/muxterm",
        );
        let key = connection_key(&cfg, "muxterm");
        assert_eq!(key.transport, "ssh");
        assert_eq!(key.alias.as_deref(), Some("ryzen"));
        assert_eq!(key.session, "muxterm");
        assert_eq!(key.runtime, "tmux");
    }

    #[test]
    fn local_shell_startup_key() {
        let key = startup_connection_key(false, None);
        assert_eq!(key.transport, "local");
        assert_eq!(key.runtime, "shell");
    }

    #[test]
    fn tmux_startup_key_keeps_session() {
        let key = startup_connection_key(true, Some("legion"));
        assert_eq!(key.transport, "local");
        assert_eq!(key.runtime, "tmux");
        assert_eq!(key.session, "legion");
        assert_eq!(key.path, "");
    }

    #[test]
    fn local_tmux_key_uses_session_and_path() {
        let cfg = TargetConfig::new(
            "muxterm",
            TargetRuntime::Tmux,
            TargetTransport::Local,
            "~/src/muxterm",
        );
        let key = connection_key(&cfg, "muxterm");
        assert_eq!(key.transport, "local");
        assert_eq!(key.alias, None);
        assert_eq!(key.session, "muxterm");
        assert_eq!(key.runtime, "tmux");
        assert_eq!(key.path, "~/src/muxterm");
    }
}
