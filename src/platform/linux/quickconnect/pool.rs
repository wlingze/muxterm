//! Warm connection pool 的纯逻辑（无 GTK 依赖）。
//!
//! 连接池持有多个后台连接；前台同一时刻至多一个 active slot。
//! 切换目标时优先复用已有连接（不 shutdown），只在容量超限 / TTL 到期 /
//! memory pressure 时才淘汰（tmux 用 detach，保留 server/session）。

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use super::model::{QuickConnect, TargetConfig, TargetRuntime, TargetTransport};

/// 决定“实际连接身份”的字段，避免仅用 name 冲突。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionKey {
    pub transport: String,
    pub alias: Option<String>,
    pub session: String,
    pub runtime: String,
    pub path: String,
}

impl ConnectionKey {
    pub fn new(
        transport: &str,
        alias: Option<&str>,
        session: &str,
        runtime: &str,
        path: &str,
    ) -> Self {
        ConnectionKey {
            transport: transport.to_string(),
            alias: alias.map(|s| s.to_string()),
            session: session.to_string(),
            runtime: runtime.to_string(),
            path: path.to_string(),
        }
    }

    /// 连接池 key → QuickConnect 目标：tmux 用 session 名，shell 用路径目录名。
    pub fn target_config(&self) -> TargetConfig {
        let name = if self.session.is_empty() {
            QuickConnect::default_name(&self.path)
        } else {
            self.session.clone()
        };
        let runtime = TargetRuntime::from_str(&self.runtime).unwrap_or(TargetRuntime::Tmux);
        let transport = if self.transport == "ssh" {
            if let Some(alias) = &self.alias {
                TargetTransport::Ssh {
                    name: alias.clone(),
                }
            } else {
                TargetTransport::Local
            }
        } else {
            TargetTransport::Local
        };
        TargetConfig::new(name, runtime, transport, self.path.clone())
    }
}

impl Hash for ConnectionKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.transport.hash(state);
        self.alias.hash(state);
        self.session.hash(state);
        self.runtime.hash(state);
        self.path.hash(state);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionLifecycle {
    Active,
    Background,
    Evicting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionEvictionReason {
    Capacity,
    Ttl,
    MemoryPressure,
}

/// 连接池中一个连接的抽象：真实实现持有 CoreBridge + 视图。
pub trait ConnectionSlotProtocol {
    fn key(&self) -> &ConnectionKey;
    fn lifecycle(&self) -> ConnectionLifecycle;
    fn set_lifecycle(&mut self, lifecycle: ConnectionLifecycle);
    fn last_used_at(&self) -> Instant;
    fn set_last_used_at(&mut self, now: Instant);
    /// 后台继续 poll 事件、维护 warm 状态；不得同步重绘。
    fn poll_background(&mut self);
    /// 淘汰：tmux 用 detach 保留 session；local shell 按实现策略处理。
    fn evict(&mut self, reason: ConnectionEvictionReason);
    /// 窗口/应用关闭：直接回收 handle。
    fn shutdown(&mut self);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionPoolPolicy {
    pub max_slots: usize,
    pub ttl: Option<Duration>,
}

impl ConnectionPoolPolicy {
    pub fn new(max_slots: usize) -> Self {
        ConnectionPoolPolicy {
            max_slots: max_slots.max(1),
            ttl: None,
        }
    }
}

/// 连接池（纯逻辑）。
pub struct ConnectionPool<Slot: ConnectionSlotProtocol> {
    slots: HashMap<ConnectionKey, Slot>,
    active_key: Option<ConnectionKey>,
    pub policy: ConnectionPoolPolicy,
}

impl<Slot: ConnectionSlotProtocol> Default for ConnectionPool<Slot> {
    fn default() -> Self {
        ConnectionPool {
            slots: HashMap::new(),
            active_key: None,
            policy: ConnectionPoolPolicy::new(5),
        }
    }
}

impl<Slot: ConnectionSlotProtocol> ConnectionPool<Slot> {
    pub fn new(policy: ConnectionPoolPolicy) -> Self {
        ConnectionPool {
            slots: HashMap::new(),
            active_key: None,
            policy,
        }
    }

    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    pub fn get(&self, key: &ConnectionKey) -> Option<&Slot> {
        self.slots.get(key)
    }

    pub fn get_mut(&mut self, key: &ConnectionKey) -> Option<&mut Slot> {
        self.slots.get_mut(key)
    }

    /// 当前前台 slot。
    pub fn active_slot(&self) -> Option<&Slot> {
        self.active_key.as_ref().and_then(|k| self.slots.get(k))
    }

    /// 当前前台 slot（可变）。
    pub fn active_slot_mut(&mut self) -> Option<&mut Slot> {
        let key = self.active_key.clone()?;
        self.slots.get_mut(&key)
    }

    pub fn active_key(&self) -> Option<&ConnectionKey> {
        self.active_key.as_ref()
    }

    /// 最近打开的目标（按 lastUsedAt 倒序），供 QuickConnect 的 Recent 列表。
    pub fn recent_target_configs(&self, limit: usize) -> Vec<TargetConfig> {
        let mut slots: Vec<&Slot> = self
            .slots
            .values()
            .filter(|s| s.lifecycle() != ConnectionLifecycle::Evicting)
            .collect();
        slots.sort_by_key(|s| std::cmp::Reverse(s.last_used_at()));
        slots
            .iter()
            .take(limit)
            .map(|s| s.key().target_config())
            .collect()
    }

    /// 当前前台连接对应的目标（用于 QuickConnect 行高亮）。
    pub fn current_target_config(&self) -> Option<TargetConfig> {
        self.active_key.as_ref().map(|k| k.target_config())
    }

    /// 获取目标连接：已存在则复用并提升为 active；不存在则用 `create` 新建。
    /// 返回 (slot, created)。
    pub fn acquire(
        &mut self,
        key: ConnectionKey,
        create: impl FnOnce(&ConnectionKey) -> Slot,
    ) -> (&mut Slot, bool) {
        let now = Instant::now();
        let existed = self.slots.contains_key(&key);

        // 切走旧 active（如果不是同一个 key）
        if let Some(active_key) = self.active_key.clone() {
            if active_key != key {
                if let Some(active) = self.slots.get_mut(&active_key) {
                    active.set_lifecycle(ConnectionLifecycle::Background);
                }
            }
        }

        if existed {
            let existing = self.slots.get_mut(&key).expect("exists 分支必须命中");
            existing.set_last_used_at(now);
            existing.set_lifecycle(ConnectionLifecycle::Active);
            self.active_key = Some(key.clone());
            self.evict_for_capacity();
            return (self.slots.get_mut(&key).expect("slot 必须存在"), false);
        }

        let slot = create(&key);
        self.slots.insert(key.clone(), slot);
        let slot = self.slots.get_mut(&key).expect("刚插入的 slot 必须存在");
        slot.set_last_used_at(now);
        slot.set_lifecycle(ConnectionLifecycle::Active);
        self.active_key = Some(key.clone());
        self.evict_for_capacity();
        (self.slots.get_mut(&key).expect("slot 必须存在"), true)
    }

    /// 把 active 连接降为 background，不 shutdown（warm）。
    pub fn release(&mut self, key: &ConnectionKey) {
        if self.active_key.as_ref() != Some(key) {
            return;
        }
        if let Some(slot) = self.slots.get_mut(key) {
            slot.set_lifecycle(ConnectionLifecycle::Background);
        }
        self.active_key = None;
    }

    /// 淘汰超过 maxSlots 的 background（LRU：lastUsedAt 升序）。
    pub fn evict_for_capacity(&mut self) {
        let max_slots = self.policy.max_slots.max(1);
        if self.slots.len() <= max_slots {
            return;
        }
        let mut background: Vec<ConnectionKey> = self
            .slots
            .iter()
            .filter(|(_, s)| s.lifecycle() == ConnectionLifecycle::Background)
            .map(|(k, _)| k.clone())
            .collect();
        background.sort_by(|a, b| {
            self.slots[a]
                .last_used_at()
                .cmp(&self.slots[b].last_used_at())
        });
        let mut overflow = self.slots.len() - max_slots;
        for key in background {
            if overflow == 0 {
                break;
            }
            self.evict(&key, ConnectionEvictionReason::Capacity);
            overflow -= 1;
        }
    }

    /// TTL 到期：淘汰超时的 background 连接。
    pub fn evict_expired(&mut self) {
        let Some(ttl) = self.policy.ttl else { return };
        let now = Instant::now();
        let expired: Vec<ConnectionKey> = self
            .slots
            .iter()
            .filter(|(_, s)| {
                s.lifecycle() == ConnectionLifecycle::Background
                    && now.duration_since(s.last_used_at()) > ttl
            })
            .map(|(k, _)| k.clone())
            .collect();
        for key in expired {
            self.evict(&key, ConnectionEvictionReason::Ttl);
        }
    }

    /// memory pressure：淘汰全部 background 连接。
    pub fn evict_under_memory_pressure(&mut self) {
        let keys: Vec<ConnectionKey> = self
            .slots
            .iter()
            .filter(|(_, s)| s.lifecycle() == ConnectionLifecycle::Background)
            .map(|(k, _)| k.clone())
            .collect();
        for key in keys {
            self.evict(&key, ConnectionEvictionReason::MemoryPressure);
        }
    }

    /// 后台连接继续 poll，保持 warm。
    pub fn poll_background_slots(&mut self) {
        let keys: Vec<ConnectionKey> = self
            .slots
            .iter()
            .filter(|(_, s)| s.lifecycle() == ConnectionLifecycle::Background)
            .map(|(k, _)| k.clone())
            .collect();
        for key in keys {
            if let Some(slot) = self.slots.get_mut(&key) {
                slot.poll_background();
            }
        }
    }

    /// 窗口/应用关闭：回收全部连接。
    pub fn shutdown_all(&mut self) {
        for (_, slot) in self.slots.drain() {
            let mut slot = slot;
            slot.shutdown();
        }
        self.active_key = None;
    }

    fn evict(&mut self, key: &ConnectionKey, reason: ConnectionEvictionReason) {
        let mut slot = self.slots.remove(key).expect("evict 目标必须存在");
        slot.set_lifecycle(ConnectionLifecycle::Evicting);
        slot.evict(reason);
        if self.active_key.as_ref() == Some(key) {
            self.active_key = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Clone)]
    struct FakeSlot {
        key: ConnectionKey,
        lifecycle: ConnectionLifecycle,
        last_used_at: Instant,
        evictions: Rc<RefCell<Vec<ConnectionEvictionReason>>>,
        polled: Rc<RefCell<u32>>,
    }

    impl ConnectionSlotProtocol for FakeSlot {
        fn key(&self) -> &ConnectionKey {
            &self.key
        }
        fn lifecycle(&self) -> ConnectionLifecycle {
            self.lifecycle
        }
        fn set_lifecycle(&mut self, l: ConnectionLifecycle) {
            self.lifecycle = l;
        }
        fn last_used_at(&self) -> Instant {
            self.last_used_at
        }
        fn set_last_used_at(&mut self, now: Instant) {
            self.last_used_at = now;
        }
        fn poll_background(&mut self) {
            *self.polled.borrow_mut() += 1;
        }
        fn evict(&mut self, reason: ConnectionEvictionReason) {
            self.lifecycle = ConnectionLifecycle::Evicting;
            self.evictions.borrow_mut().push(reason);
        }
        fn shutdown(&mut self) {
            self.lifecycle = ConnectionLifecycle::Evicting;
        }
    }

    fn key(n: &str) -> ConnectionKey {
        ConnectionKey::new("local", None, n, "tmux", "~/x")
    }

    type PoolTestHarness = (
        ConnectionPool<FakeSlot>,
        Rc<RefCell<Vec<ConnectionEvictionReason>>>,
        Rc<RefCell<u32>>,
    );

    fn make_pool() -> PoolTestHarness {
        let evictions = Rc::new(RefCell::new(Vec::new()));
        let polled = Rc::new(RefCell::new(0u32));
        let ev = evictions.clone();
        let po = polled.clone();
        let mut pool = ConnectionPool::new(ConnectionPoolPolicy::new(2));
        for n in ["a", "b", "c"] {
            let k = key(n);
            let s = FakeSlot {
                key: k.clone(),
                lifecycle: ConnectionLifecycle::Background,
                last_used_at: Instant::now(),
                evictions: ev.clone(),
                polled: po.clone(),
            };
            pool.acquire(k, |_| s.clone());
        }
        (pool, evictions, polled)
    }

    #[test]
    fn capacity_evicts_lru_background() {
        let (pool, evictions, _) = make_pool();
        assert_eq!(pool.slot_count(), 2);
        assert_eq!(evictions.borrow().len(), 1);
        assert_eq!(evictions.borrow()[0], ConnectionEvictionReason::Capacity);
        // a 最早使用，应被淘汰
        assert!(!pool.slots.contains_key(&key("a")));
    }

    #[test]
    fn reuse_does_not_create() {
        let (mut pool, _, _) = make_pool();
        let before = pool.slot_count();
        let k = key("b");
        let (_, created) = pool.acquire(k.clone(), |_| panic!("不应新建"));
        assert!(!created);
        assert_eq!(pool.slot_count(), before);
        assert_eq!(pool.active_key(), Some(&k));
    }

    #[test]
    fn background_poll_and_release() {
        let (mut pool, _, polled) = make_pool();
        pool.release(&pool.active_key().unwrap().clone());
        pool.poll_background_slots();
        assert!(*polled.borrow() >= 1);
    }

    #[test]
    fn ttl_evicts_background() {
        let mut pool = ConnectionPool::new(ConnectionPoolPolicy {
            max_slots: 4,
            ttl: Some(Duration::from_millis(1)),
        });
        let k = key("a");
        let (slot, _) = pool.acquire(k.clone(), |_| FakeSlot {
            key: k.clone(),
            lifecycle: ConnectionLifecycle::Background,
            last_used_at: Instant::now(),
            evictions: Rc::new(RefCell::new(Vec::new())),
            polled: Rc::new(RefCell::new(0)),
        });
        slot.set_last_used_at(Instant::now() - Duration::from_secs(60));
        pool.release(&k);
        pool.evict_expired();
        assert_eq!(pool.slot_count(), 0);
    }

    #[test]
    fn recents_ordered_and_limited() {
        let (pool, _, _) = make_pool();
        let recents = pool.recent_target_configs(1);
        assert_eq!(recents.len(), 1);
        assert_eq!(recents[0].name, "c");
    }

    #[test]
    fn current_target_and_active_slot() {
        let (mut pool, _, _) = make_pool();
        let current = pool.current_target_config().expect("应有前台连接");
        assert_eq!(current.name, "c");
        assert!(pool.active_slot().is_some());
        assert!(pool.get_mut(&key("c")).is_some());
        assert!(pool.get_mut(&key("a")).is_none());
    }
}
