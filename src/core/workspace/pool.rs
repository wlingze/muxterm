//! WorkspacePool：旧连接池进 core。
//!
//! 池负责 open / list / activate / 后台 `take_events` 喂 PaneBuf / 淘汰
//! （tmux Detach、shell Shutdown）。容量、TTL、按 `WorkspaceId` 复用。
//! platform 不得再实现第二套淘汰/复用。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::core::model::backend::Runtime;
use crate::core::model::state::StateChange;
use crate::core::model::task::Task;
use crate::core::workspace::id::WorkspaceId;
use crate::core::workspace::workspace::Workspace;

/// 池里一个工作区的生命周期。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceLifecycle {
    Active,
    Background,
    Evicting,
}

/// 淘汰原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceEvictionReason {
    Capacity,
    Ttl,
    MemoryPressure,
    Closed,
}

/// 池策略：容量 + 后台 TTL。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspacePoolPolicy {
    pub max_slots: usize,
    pub ttl: Option<Duration>,
}

impl WorkspacePoolPolicy {
    pub fn new(max_slots: usize) -> Self {
        Self {
            max_slots: max_slots.max(1),
            ttl: None,
        }
    }
}

/// 池里一格：Workspace + 生命周期元数据。
struct PooledWorkspace {
    workspace: Workspace,
    lifecycle: WorkspaceLifecycle,
    last_used_at: Instant,
}

/// WorkspacePool：连接池的 core 版。
pub struct WorkspacePool {
    slots: HashMap<WorkspaceId, PooledWorkspace>,
    active_id: Option<WorkspaceId>,
    policy: WorkspacePoolPolicy,
    recently_evicted: Vec<WorkspaceId>,
}

impl Default for WorkspacePool {
    fn default() -> Self {
        Self::new(WorkspacePoolPolicy::new(5))
    }
}

impl WorkspacePool {
    pub fn new(policy: WorkspacePoolPolicy) -> Self {
        Self {
            slots: HashMap::new(),
            active_id: None,
            policy,
            recently_evicted: Vec::new(),
        }
    }

    /// 池里工作区数量。
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// 当前前台工作区 id。
    pub fn active_id(&self) -> Option<&WorkspaceId> {
        self.active_id.as_ref()
    }

    /// 当前前台工作区。
    pub fn active(&self) -> Option<&Workspace> {
        self.active_id
            .as_ref()
            .and_then(|id| self.slots.get(id))
            .map(|p| &p.workspace)
    }

    /// 当前前台工作区（可变）。
    pub fn active_mut(&mut self) -> Option<&mut Workspace> {
        let id = self.active_id.clone()?;
        self.slots.get_mut(&id).map(|p| &mut p.workspace)
    }

    /// 按 id 取工作区。
    pub fn get(&self, id: &WorkspaceId) -> Option<&Workspace> {
        self.slots.get(id).map(|p| &p.workspace)
    }

    /// 按 id 取工作区（可变）。
    pub fn get_mut(&mut self, id: &WorkspaceId) -> Option<&mut Workspace> {
        self.slots.get_mut(id).map(|p| &mut p.workspace)
    }

    /// 池里全部工作区（含后台）。
    pub fn list(&self) -> Vec<&Workspace> {
        self.slots.values().map(|p| &p.workspace).collect()
    }

    /// 打开（或复用）一个工作区并设为前台。`create` 只在不存在时调用。
    pub async fn open(
        &mut self,
        id: WorkspaceId,
        name: String,
        create: impl FnOnce(&WorkspaceId) -> Box<dyn Runtime>,
    ) -> anyhow::Result<&mut Workspace> {
        let now = Instant::now();
        if self.slots.contains_key(&id) {
            if let Some(active_id) = self.active_id.clone() {
                if active_id != id {
                    if let Some(active) = self.slots.get_mut(&active_id) {
                        active.lifecycle = WorkspaceLifecycle::Background;
                    }
                }
            }
            let slot = self.slots.get_mut(&id).expect("exists 分支必须命中");
            slot.last_used_at = now;
            slot.lifecycle = WorkspaceLifecycle::Active;
            self.active_id = Some(id.clone());
            self.evict_for_capacity();
            return Ok(&mut self.slots.get_mut(&id).expect("slot 必须存在").workspace);
        }

        // 切走旧 active（如果不是同一个 key）。
        if let Some(active_id) = self.active_id.clone() {
            if active_id != id {
                if let Some(active) = self.slots.get_mut(&active_id) {
                    active.lifecycle = WorkspaceLifecycle::Background;
                }
            }
        }
        let runtime = create(&id);
        let mut workspace = Workspace::new(id.clone(), name, runtime);
        workspace.connect().await?;
        self.slots.insert(
            id.clone(),
            PooledWorkspace {
                workspace,
                lifecycle: WorkspaceLifecycle::Active,
                last_used_at: now,
            },
        );
        self.active_id = Some(id.clone());
        self.evict_for_capacity();
        Ok(&mut self
            .slots
            .get_mut(&id)
            .expect("刚插入的 slot 必须存在")
            .workspace)
    }

    /// 把某工作区设为前台；其余降为后台（不 shutdown）。
    pub fn activate(&mut self, id: &WorkspaceId) -> Option<&mut Workspace> {
        if !self.slots.contains_key(id) {
            return None;
        }
        if let Some(active_id) = self.active_id.clone() {
            if active_id != *id {
                if let Some(active) = self.slots.get_mut(&active_id) {
                    active.lifecycle = WorkspaceLifecycle::Background;
                }
            }
        }
        let slot = self.slots.get_mut(id).expect("activate 目标必须存在");
        slot.lifecycle = WorkspaceLifecycle::Active;
        slot.last_used_at = Instant::now();
        self.active_id = Some(id.clone());
        Some(
            &mut self
                .slots
                .get_mut(id)
                .expect("activate 目标必须存在")
                .workspace,
        )
    }

    /// 拉取全部后台工作区的事件，并喂进各自 PaneBuf。
    pub fn poll_background(&mut self) -> Vec<(WorkspaceId, Vec<StateChange>)> {
        let keys: Vec<WorkspaceId> = self
            .slots
            .iter()
            .filter(|(_, s)| s.lifecycle == WorkspaceLifecycle::Background)
            .map(|(k, _)| k.clone())
            .collect();
        let mut out = Vec::new();
        for key in keys {
            if let Some(slot) = self.slots.get_mut(&key) {
                let events = slot.workspace.refresh();
                if !events.is_empty() {
                    out.push((key, events));
                }
            }
        }
        out
    }

    /// 跨全部工作区搜索（含后台），返回带 tab 的命中。
    pub fn search_all(&self, query: &str) -> Vec<crate::core::workspace::workspace::SearchHit> {
        let mut out = Vec::new();
        for slot in self.slots.values() {
            out.extend(slot.workspace.search_workspace(query));
        }
        out
    }

    /// 关闭一个工作区：tmux Detach，shell Shutdown。
    pub fn close(&mut self, id: &WorkspaceId) -> bool {
        if !self.slots.contains_key(id) {
            return false;
        }
        self.evict(id, WorkspaceEvictionReason::Closed);
        true
    }

    /// 淘汰超过容量的后台工作区（LRU）。
    pub fn evict_for_capacity(&mut self) {
        let max_slots = self.policy.max_slots.max(1);
        if self.slots.len() <= max_slots {
            return;
        }
        let mut background: Vec<WorkspaceId> = self
            .slots
            .iter()
            .filter(|(_, s)| s.lifecycle == WorkspaceLifecycle::Background)
            .map(|(k, _)| k.clone())
            .collect();
        background.sort_by(|a, b| self.slots[a].last_used_at.cmp(&self.slots[b].last_used_at));
        let mut overflow = self.slots.len() - max_slots;
        for key in background {
            if overflow == 0 {
                break;
            }
            self.evict(&key, WorkspaceEvictionReason::Capacity);
            overflow -= 1;
        }
    }

    /// 淘汰 TTL 到期的后台工作区。
    pub fn evict_expired(&mut self) {
        let Some(ttl) = self.policy.ttl else { return };
        let now = Instant::now();
        let expired: Vec<WorkspaceId> = self
            .slots
            .iter()
            .filter(|(_, s)| {
                s.lifecycle == WorkspaceLifecycle::Background
                    && now.duration_since(s.last_used_at) > ttl
            })
            .map(|(k, _)| k.clone())
            .collect();
        for key in expired {
            self.evict(&key, WorkspaceEvictionReason::Ttl);
        }
    }

    /// memory pressure：淘汰全部后台工作区。
    pub fn evict_under_memory_pressure(&mut self) {
        let keys: Vec<WorkspaceId> = self
            .slots
            .iter()
            .filter(|(_, s)| s.lifecycle == WorkspaceLifecycle::Background)
            .map(|(k, _)| k.clone())
            .collect();
        for key in keys {
            self.evict(&key, WorkspaceEvictionReason::MemoryPressure);
        }
    }

    /// 取走本轮淘汰的 id。
    pub fn take_evicted(&mut self) -> Vec<WorkspaceId> {
        std::mem::take(&mut self.recently_evicted)
    }

    /// 关闭全部工作区（tmux Detach，shell Shutdown）。
    pub fn shutdown_all(&mut self) {
        let slots: Vec<(WorkspaceId, PooledWorkspace)> = self.slots.drain().collect();
        for (id, mut slot) in slots {
            slot.lifecycle = WorkspaceLifecycle::Evicting;
            release_runtime(&mut slot.workspace, &id);
        }
        self.active_id = None;
    }

    fn evict(&mut self, id: &WorkspaceId, _reason: WorkspaceEvictionReason) {
        let mut slot = self.slots.remove(id).expect("evict 目标必须存在");
        slot.lifecycle = WorkspaceLifecycle::Evicting;
        release_runtime(&mut slot.workspace, id);
        if self.active_id.as_ref() == Some(id) {
            self.active_id = None;
        }
        self.recently_evicted.push(id.clone());
    }
}

/// 按 runtime 释放：tmux 类 detach 保远端，shell shutdown 结束进程。
fn release_runtime(workspace: &mut Workspace, id: &WorkspaceId) {
    let task = if is_tmux_runtime(&id.runtime) {
        Task::Detach
    } else {
        Task::Shutdown
    };
    let _ = workspace.execute(task);
}

fn is_tmux_runtime(runtime: &str) -> bool {
    matches!(runtime, "tmux" | "ssh" | "tmux-ssh")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::backend::mock::MockRuntime;
    use crate::core::model::task::Task;
    use crate::core::types::{PaneId, TabId};
    use std::sync::{Arc, Mutex};

    fn id(name: &str, runtime: &str) -> WorkspaceId {
        WorkspaceId::new("local", None, name, runtime, "")
    }

    /// 两个 mock workspace：A 前台、B 后台仍吃字节；activate(B) 后 A 仍能读到已索引文本。
    #[tokio::test]
    async fn background_workspace_still_eats_bytes_and_keeps_indexed_text() {
        let mut pool = WorkspacePool::new(WorkspacePoolPolicy::new(4));
        let a = id("a", "tmux");
        let b = id("b", "tmux");
        pool.open(a.clone(), "a".into(), |_| {
            Box::new(MockRuntime::with_single_pane())
        })
        .await
        .unwrap();
        pool.open(b.clone(), "b".into(), |_| {
            Box::new(MockRuntime::with_single_pane())
        })
        .await
        .unwrap();
        assert_eq!(pool.active_id(), Some(&b));

        // A 已降为后台；往 A 写字节后 poll_background 仍应喂进 A 的 PaneBuf。
        pool.get_mut(&a)
            .unwrap()
            .execute(Task::WriteRaw {
                target: PaneId(1),
                data: b"background-token\r\n".to_vec(),
            })
            .unwrap();
        let events = pool.poll_background();
        assert!(
            events.iter().any(|(wid, evs)| wid == &a
                && evs
                    .iter()
                    .any(|e| matches!(e, StateChange::PaneOutput { .. }))),
            "后台工作区事件应被池拉取"
        );
        assert!(
            pool.get(&a)
                .unwrap()
                .pane_text(PaneId(1))
                .contains("background-token"),
            "后台工作区字节应喂进 PaneBuf"
        );

        // 切回 A：已索引文本仍在。
        pool.activate(&a);
        assert_eq!(pool.active_id(), Some(&a));
        assert!(
            pool.active()
                .unwrap()
                .pane_text(PaneId(1))
                .contains("background-token"),
            "activate 后已索引文本仍可读"
        );
    }

    /// W6：search_all 跨工作区/pane 返回带 tab 的命中；同 pane 不同 tab 不串。
    #[tokio::test]
    async fn search_all_finds_hits_across_workspaces_and_panes() {
        let mut pool = WorkspacePool::new(WorkspacePoolPolicy::new(4));
        let a = id("a", "tmux");
        let b = id("b", "tmux");
        pool.open(a.clone(), "a".into(), |_| {
            Box::new(MockRuntime::with_single_pane())
        })
        .await
        .unwrap();
        pool.open(b.clone(), "b".into(), |_| {
            Box::new(MockRuntime::with_single_pane())
        })
        .await
        .unwrap();

        pool.get_mut(&a)
            .unwrap()
            .feed_pane_bytes(PaneId(1), b"alpha TOKEN_BODY one\r\n", 80, 24);
        pool.get_mut(&a)
            .unwrap()
            .feed_pane_bytes(PaneId(2), b"beta\r\n", 80, 24);
        pool.get_mut(&b)
            .unwrap()
            .feed_pane_bytes(PaneId(1), b"gamma TOKEN_BODY two\r\n", 80, 24);

        let hits = pool.search_all("TOKEN_BODY");
        assert_eq!(hits.len(), 2, "两个 pane 各命中一次");
        assert!(hits.iter().any(|h| {
            h.workspace_id.contains("a")
                && h.pane_id == PaneId(1)
                && h.tab_id == TabId(1)
                && h.line.contains("TOKEN_BODY")
        }));
        assert!(hits.iter().any(|h| {
            h.workspace_id.contains("b")
                && h.pane_id == PaneId(1)
                && h.tab_id == TabId(1)
                && h.line.contains("TOKEN_BODY")
        }));
        assert!(pool.search_all("missing").is_empty());
    }

    /// 超容量：tmux mock 计数 detach，shell 计数 shutdown。
    #[tokio::test]
    async fn over_capacity_evicts_tmux_with_detach_and_shell_with_shutdown() {
        let log = Arc::new(Mutex::new(Vec::new()));

        // tmux：容量 2，开 3 个 → 最早的后台被 Detach。
        let mut tmux_pool = WorkspacePool::new(WorkspacePoolPolicy::new(2));
        for n in ["a", "b", "c"] {
            let wid = id(n, "tmux");
            let log_cb = log.clone();
            tmux_pool
                .open(wid.clone(), n.to_string(), move |_| {
                    let mut b = MockRuntime::with_single_pane();
                    b.executed_log = Some(log_cb);
                    Box::new(b)
                })
                .await
                .unwrap();
        }
        assert_eq!(tmux_pool.len(), 2);
        assert!(
            log.lock()
                .unwrap()
                .iter()
                .any(|t| matches!(t, Task::Detach)),
            "tmux 淘汰应发 Detach: {:?}",
            log.lock().unwrap()
        );

        // shell：容量 1，开 2 个 → 第一个被 Shutdown。
        let shell_log = Arc::new(Mutex::new(Vec::new()));
        let mut shell_pool = WorkspacePool::new(WorkspacePoolPolicy::new(1));
        for n in ["s1", "s2"] {
            let wid = id(n, "shell");
            let log_cb = shell_log.clone();
            shell_pool
                .open(wid.clone(), n.to_string(), move |_| {
                    let mut b = MockRuntime::with_single_pane();
                    b.executed_log = Some(log_cb);
                    Box::new(b)
                })
                .await
                .unwrap();
        }
        assert_eq!(shell_pool.len(), 1);
        assert!(
            shell_log
                .lock()
                .unwrap()
                .iter()
                .any(|t| matches!(t, Task::Shutdown)),
            "shell 淘汰应发 Shutdown: {:?}",
            shell_log.lock().unwrap()
        );
    }

    /// 同一 WorkspaceId 再 open 复用，不新建 Runtime。
    #[tokio::test]
    async fn reopen_same_id_reuses_without_new_runtime() {
        let mut pool = WorkspacePool::new(WorkspacePoolPolicy::new(4));
        let a = id("a", "tmux");
        let created = Arc::new(Mutex::new(0u32));
        let c1 = created.clone();
        pool.open(a.clone(), "a".into(), move |_| {
            *c1.lock().unwrap() += 1;
            Box::new(MockRuntime::with_single_pane())
        })
        .await
        .unwrap();
        let c2 = created.clone();
        pool.open(a.clone(), "a".into(), move |_| {
            *c2.lock().unwrap() += 1;
            panic!("复用路径不应新建 Runtime")
        })
        .await
        .unwrap();
        assert_eq!(*created.lock().unwrap(), 1, "create 只应调用一次");
        assert_eq!(pool.len(), 1);
    }

    /// TTL：后台超时被淘汰。
    #[test]
    fn ttl_evicts_background_workspace() {
        let mut pool = WorkspacePool::new(WorkspacePoolPolicy {
            max_slots: 4,
            ttl: Some(Duration::from_millis(1)),
        });
        let a = id("a", "tmux");
        let b = id("b", "tmux");
        // 直接塞两个已连接 mock（open 的 connect 对 mock 是幂等空操作）。
        pool.slots.insert(
            a.clone(),
            PooledWorkspace {
                workspace: Workspace::new(
                    a.clone(),
                    "a".into(),
                    Box::new(MockRuntime::with_single_pane()),
                ),
                lifecycle: WorkspaceLifecycle::Background,
                last_used_at: Instant::now() - Duration::from_secs(60),
            },
        );
        pool.slots.insert(
            b.clone(),
            PooledWorkspace {
                workspace: Workspace::new(
                    b.clone(),
                    "b".into(),
                    Box::new(MockRuntime::with_single_pane()),
                ),
                lifecycle: WorkspaceLifecycle::Active,
                last_used_at: Instant::now(),
            },
        );
        pool.active_id = Some(b.clone());
        pool.evict_expired();
        assert_eq!(pool.len(), 1);
        assert!(pool.get(&a).is_none());
        assert_eq!(pool.take_evicted(), vec![a]);
    }

    /// list 返回全部工作区；close 走 tmux Detach。
    #[tokio::test]
    async fn list_and_close() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut pool = WorkspacePool::new(WorkspacePoolPolicy::new(4));
        let a = id("a", "tmux");
        let b = id("b", "tmux");
        let log_cb = log.clone();
        pool.open(a.clone(), "a".into(), move |_| {
            let mut bk = MockRuntime::with_single_pane();
            bk.executed_log = Some(log_cb);
            Box::new(bk)
        })
        .await
        .unwrap();
        pool.open(b.clone(), "b".into(), |_| {
            Box::new(MockRuntime::with_single_pane())
        })
        .await
        .unwrap();
        assert_eq!(pool.list().len(), 2);
        assert!(pool.close(&a));
        assert_eq!(pool.len(), 1);
        assert!(
            log.lock()
                .unwrap()
                .iter()
                .any(|t| matches!(t, Task::Detach)),
            "close tmux 应发 Detach"
        );
        assert_eq!(pool.take_evicted(), vec![a]);
    }
}
