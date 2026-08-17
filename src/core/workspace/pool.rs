//! WorkspacePool：旧连接池进 core。
//!
//! 池负责 open / list / activate / 后台 `take_events` 喂 PaneBuf / 淘汰
//! （tmux Detach、shell Shutdown）。容量、TTL、按 `WorkspaceId` 复用。
//! platform 不得再实现第二套淘汰/复用。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::core::model::backend::{Runtime, RuntimeCapability, WorktreeCreateSpec, WorktreeInfo};
use crate::core::model::state::StateChange;
use crate::core::model::task::Task;
use crate::core::protocol::terminal::emulate::DEFAULT_SCROLLBACK_LINES;
use crate::core::runtime::{HerdrRuntime, HerdrSession};
use crate::core::workspace::id::WorkspaceId;
use crate::core::workspace::workspace::Workspace;
use std::sync::Arc;

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
    /// Herdr named session 共享注册表：(session 名, socket 路径) → Arc<HerdrSession>。
    /// 同一 socket 上多格 Workspace 必须共享一条连接身份。
    herdr_sessions: HashMap<(String, String), Arc<HerdrSession>>,
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
            herdr_sessions: HashMap::new(),
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
        self.open_with_scrollback(id, name, DEFAULT_SCROLLBACK_LINES, create)
            .await
    }

    /// 把 `new_id` 切为前台、旧前台降为后台，并恰好通知一次
    /// `Runtime::set_foreground`（旧 false、新 true；无实际转换时零调用）。
    fn transition_active(&mut self, new_id: &WorkspaceId) {
        if self.active_id.as_ref() == Some(new_id) {
            return;
        }
        if let Some(active_id) = self.active_id.clone() {
            if let Some(active) = self.slots.get_mut(&active_id) {
                active.lifecycle = WorkspaceLifecycle::Background;
                active.workspace.set_foreground(false);
            }
        }
        if let Some(slot) = self.slots.get_mut(new_id) {
            slot.lifecycle = WorkspaceLifecycle::Active;
            slot.last_used_at = Instant::now();
            slot.workspace.set_foreground(true);
        }
        self.active_id = Some(new_id.clone());
    }

    /// 打开工作区并为其 PaneBuf 指定 scrollback 上限。
    ///
    /// `open` 保留默认值给旧调用方；WorkspaceSpec/FFI 走此入口，确保
    /// tmux capture 与 core 索引面使用同一配置。
    pub async fn open_with_scrollback(
        &mut self,
        id: WorkspaceId,
        name: String,
        scrollback_lines: usize,
        create: impl FnOnce(&WorkspaceId) -> Box<dyn Runtime>,
    ) -> anyhow::Result<&mut Workspace> {
        let now = Instant::now();
        if self.slots.contains_key(&id) {
            self.transition_active(&id);
            let slot = self.slots.get_mut(&id).expect("exists 分支必须命中");
            slot.last_used_at = now;
            self.evict_for_capacity();
            return Ok(&mut self.slots.get_mut(&id).expect("slot 必须存在").workspace);
        }

        let runtime = create(&id);
        let mut workspace =
            Workspace::new_with_scrollback(id.clone(), name, runtime, scrollback_lines);
        workspace.connect().await?;
        self.slots.insert(
            id.clone(),
            PooledWorkspace {
                workspace,
                lifecycle: WorkspaceLifecycle::Active,
                last_used_at: now,
            },
        );
        self.transition_active(&id);
        self.evict_for_capacity();
        Ok(&mut self
            .slots
            .get_mut(&id)
            .expect("刚插入的 slot 必须存在")
            .workspace)
    }

    /// 按产品规格打开工作区（platform 只传字段，Runtime 构造在 core）。
    pub async fn open_spec(
        &mut self,
        spec: &crate::core::workspace::spec::WorkspaceSpec,
    ) -> anyhow::Result<&mut Workspace> {
        let id = spec.id();
        let name = spec.name();
        if spec.runtime == "herdr" {
            // H3：同一 named session + socket 只建一条 HerdrSession，多格共享。
            let key = (
                spec.session.clone(),
                spec.socket.clone().unwrap_or_default(),
            );
            let session = self
                .herdr_sessions
                .entry(key)
                .or_insert_with(|| {
                    Arc::new(HerdrSession::new(
                        &spec.session,
                        spec.socket.clone().unwrap_or_default(),
                    ))
                })
                .clone();
            let path = spec.path.clone();
            return self
                .open(id, name, move |_| {
                    Box::new(HerdrRuntime::new(session, &path))
                })
                .await;
        }
        // build_runtime 放进 create 闭包：复用已有 slot 时零构造
        // （对得上 reopen_same_id_reuses_without_new_runtime）。
        // HerdrSession 共享已迁到 HerdrSession::shared（不用再按字符串旁路）。
        self.open_with_scrollback(id, name, spec.scrollback_lines as usize, move |_| {
            spec.build_runtime()
        })
        .await
    }

    /// 列出当前工作区所在仓库的 checkout（需 `WorktreeList`）。
    ///
    /// 无能力 → `Err`，零 git、零 socket；有能力的 Runtime 提供产品方法。
    pub async fn list_worktrees(&self, ws: &WorkspaceId) -> anyhow::Result<Vec<WorktreeInfo>> {
        let Some(slot) = self.slots.get(ws) else {
            return Err(anyhow::anyhow!("workspace {ws} 不在池里"));
        };
        let caps = slot.workspace.runtime().support();
        if !caps.contains(&RuntimeCapability::WorktreeList) {
            return Err(anyhow::anyhow!("runtime 不支持 WorktreeList"));
        }
        slot.workspace.runtime().list_worktrees()
    }

    /// 创建 worktree 并作为新工作区开进池里（需 `WorktreeCreate`）。
    ///
    /// 无能力 → `Err`，零 git、零 socket。
    pub async fn create_worktree(
        &mut self,
        ws: &WorkspaceId,
        spec: &WorktreeCreateSpec,
    ) -> anyhow::Result<WorkspaceId> {
        let Some(slot) = self.slots.get(ws) else {
            return Err(anyhow::anyhow!("workspace {ws} 不在池里"));
        };
        let caps = slot.workspace.runtime().support();
        if !caps.contains(&RuntimeCapability::WorktreeCreate) {
            return Err(anyhow::anyhow!("runtime 不支持 WorktreeCreate"));
        }
        let new_spec = slot.workspace.runtime().create_worktree_spec(spec)?;
        let new_id = new_spec.id();
        self.open_spec(&new_spec).await?;
        Ok(new_id)
    }

    /// 打开已有 checkout 并作为新工作区开进池里（需 `WorktreeOpen`）。
    ///
    /// 无能力 → `Err`，零 git、零 socket。
    pub async fn open_worktree(
        &mut self,
        ws: &WorkspaceId,
        path: &str,
    ) -> anyhow::Result<WorkspaceId> {
        let Some(slot) = self.slots.get(ws) else {
            return Err(anyhow::anyhow!("workspace {ws} 不在池里"));
        };
        let caps = slot.workspace.runtime().support();
        if !caps.contains(&RuntimeCapability::WorktreeOpen) {
            return Err(anyhow::anyhow!("runtime 不支持 WorktreeOpen"));
        }
        let new_spec = slot.workspace.runtime().open_worktree_spec(path)?;
        let new_id = new_spec.id();
        self.open_spec(&new_spec).await?;
        Ok(new_id)
    }

    /// 列出当前工作区所在仓库的 checkout（需 `WorktreeList`）。
    ///
    /// 无能力 → `Err`，零 git、零 socket；有能力的实现（H4 HerdrRuntime）
    /// 才真正走远端查询。
    pub async fn list_worktrees(&self, ws: &WorkspaceId) -> anyhow::Result<Vec<WorktreeInfo>> {
        let Some(slot) = self.slots.get(ws) else {
            return Err(anyhow::anyhow!("workspace {ws} 不在池里"));
        };
        let caps = slot.workspace.runtime().support();
        if !caps.contains(&RuntimeCapability::WorktreeList) {
            return Err(anyhow::anyhow!("runtime 不支持 WorktreeList"));
        }
        Err(anyhow::anyhow!("WorktreeList 尚未实现"))
    }

    /// 创建 worktree 并作为新工作区开进池里（需 `WorktreeCreate`）。
    ///
    /// 无能力 → `Err`，零 git、零 socket。
    pub async fn create_worktree(
        &mut self,
        ws: &WorkspaceId,
        _spec: &WorktreeCreateSpec,
    ) -> anyhow::Result<WorkspaceId> {
        let Some(slot) = self.slots.get(ws) else {
            return Err(anyhow::anyhow!("workspace {ws} 不在池里"));
        };
        let caps = slot.workspace.runtime().support();
        if !caps.contains(&RuntimeCapability::WorktreeCreate) {
            return Err(anyhow::anyhow!("runtime 不支持 WorktreeCreate"));
        }
        Err(anyhow::anyhow!("WorktreeCreate 尚未实现"))
    }

    /// 打开已有 checkout 并作为新工作区开进池里（需 `WorktreeOpen`）。
    ///
    /// 无能力 → `Err`，零 git、零 socket。
    pub async fn open_worktree(
        &mut self,
        ws: &WorkspaceId,
        _path: &str,
    ) -> anyhow::Result<WorkspaceId> {
        let Some(slot) = self.slots.get(ws) else {
            return Err(anyhow::anyhow!("workspace {ws} 不在池里"));
        };
        let caps = slot.workspace.runtime().support();
        if !caps.contains(&RuntimeCapability::WorktreeOpen) {
            return Err(anyhow::anyhow!("runtime 不支持 WorktreeOpen"));
        }
        Err(anyhow::anyhow!("WorktreeOpen 尚未实现"))
    }

    /// 插入一个已在后台线程完成 `connect()` 的工作区并设为前台。
    ///
    /// W15c：SSH 连接不能 `rt.block_on` 堵 GTK 主线程；连接在后台线程完成，
    /// 结果经 idle 回主线程后由这里收编（复用/淘汰语义与 `open` 一致）。
    pub fn insert_connected(&mut self, workspace: Workspace) -> WorkspaceId {
        let now = Instant::now();
        let id = workspace.id().clone();
        self.slots.insert(
            id.clone(),
            PooledWorkspace {
                workspace,
                lifecycle: WorkspaceLifecycle::Active,
                last_used_at: now,
            },
        );
        self.transition_active(&id);
        self.evict_for_capacity();
        id
    }

    /// 把某工作区设为前台；其余降为后台（不 shutdown）。
    pub fn activate(&mut self, id: &WorkspaceId) -> Option<&mut Workspace> {
        if !self.slots.contains_key(id) {
            return None;
        }
        self.transition_active(id);
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
        // C8：空 query 不扫 replica（emulate 已返回空）。
        if query.trim().is_empty() {
            return Vec::new();
        }
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

    /// 关闭全部工作区（PersistDetach → Detach，其余 Shutdown）。
    pub fn shutdown_all(&mut self) {
        let slots: Vec<(WorkspaceId, PooledWorkspace)> = self.slots.drain().collect();
        for (id, mut slot) in slots {
            slot.lifecycle = WorkspaceLifecycle::Evicting;
            slot.workspace.set_foreground(false);
            release_runtime(&mut slot.workspace, &id);
        }
        self.active_id = None;
    }

    fn evict(&mut self, id: &WorkspaceId, _reason: WorkspaceEvictionReason) {
        let mut slot = self.slots.remove(id).expect("evict 目标必须存在");
        slot.lifecycle = WorkspaceLifecycle::Evicting;
        slot.workspace.set_foreground(false);
        release_runtime(&mut slot.workspace, id);
        if self.active_id.as_ref() == Some(id) {
            self.active_id = None;
        }
        self.recently_evicted.push(id.clone());
    }
}

/// 按 Runtime 能力释放：PersistDetach 类 detach 保远端，其余 shutdown 结束进程。
fn release_runtime(workspace: &mut Workspace, id: &WorkspaceId) {
    let persist = workspace
        .runtime()
        .support()
        .contains(&RuntimeCapability::PersistDetach);
    let task = if persist {
        Task::Detach
    } else {
        Task::Shutdown
    };
    let _ = workspace.execute(task);
    let _ = id;
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

    /// 读取某 workspace 的 MockRuntime 收到的 set_foreground 调用序列。
    fn foreground_calls(pool: &WorkspacePool, id: &WorkspaceId) -> Vec<bool> {
        pool.get(id)
            .and_then(|w| w.runtime().as_any().downcast_ref::<MockRuntime>())
            .map(|rt| rt.foreground_calls.clone())
            .unwrap_or_default()
    }

    /// H0：Mock 只报 WorktreeList（不报 WorktreeCreate）时，create 必须被拒，
    /// 且不得触碰任何 git/socket（纯能力检查）。
    #[tokio::test]
    async fn pool_create_worktree_rejected_without_capability() {
        let mut pool = WorkspacePool::new(WorkspacePoolPolicy::new(4));
        let a = id("a", "tmux");
        pool.open(a.clone(), "a".into(), |_| {
            let mut b = MockRuntime::with_single_pane();
            b.capabilities = &[RuntimeCapability::WorktreeList];
            Box::new(b)
        })
        .await
        .unwrap();
        let spec = WorktreeCreateSpec {
            branch: "muxterm-test-wt-x".into(),
            path: "/tmp/muxterm-test-herdr-wt-x".into(),
            base: None,
            label: None,
        };
        let err = pool.create_worktree(&a, &spec).await.unwrap_err();
        assert!(
            err.to_string().contains("WorktreeCreate"),
            "缺 WorktreeCreate 能力必须拒绝: {err}"
        );
        assert_eq!(pool.len(), 1, "拒绝时不得新开工作区");
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

    /// 超容量：PersistDetach mock 计数 detach，非持久 mock 计数 shutdown。
    #[tokio::test]
    async fn over_capacity_evicts_tmux_with_detach_and_shell_with_shutdown() {
        let log = Arc::new(Mutex::new(Vec::new()));

        // PersistDetach：容量 2，开 3 个 → 最早的后台被 Detach。
        let mut tmux_pool = WorkspacePool::new(WorkspacePoolPolicy::new(2));
        for n in ["a", "b", "c"] {
            let wid = id(n, "tmux");
            let log_cb = log.clone();
            tmux_pool
                .open(wid.clone(), n.to_string(), move |_| {
                    let mut b = MockRuntime::with_single_pane();
                    b.capabilities = &[RuntimeCapability::PersistDetach];
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
            "PersistDetach 淘汰应发 Detach: {:?}",
            log.lock().unwrap()
        );

        // 非持久：容量 1，开 2 个 → 第一个被 Shutdown。
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
            "非持久 runtime 淘汰应发 Shutdown: {:?}",
            shell_log.lock().unwrap()
        );
    }

    /// W1：open 第一个 workspace 时 runtime 恰好收到一次 [true]。
    #[tokio::test]
    async fn open_first_workspace_sets_foreground_true() {
        let mut pool = WorkspacePool::new(WorkspacePoolPolicy::new(4));
        let a = id("a", "tmux");
        pool.open(a.clone(), "a".into(), |_| {
            Box::new(MockRuntime::with_single_pane())
        })
        .await
        .unwrap();
        assert_eq!(foreground_calls(&pool, &a), vec![true]);
    }

    /// W1：open/activate 第二个 → 旧 [false]、新 [true]；重复 activate 零新增。
    #[tokio::test]
    async fn activate_second_notifies_old_false_new_true_without_repeat() {
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
        assert_eq!(foreground_calls(&pool, &a), vec![true, false]);
        assert_eq!(foreground_calls(&pool, &b), vec![true]);

        pool.activate(&b);
        pool.activate(&b);
        assert_eq!(foreground_calls(&pool, &a), vec![true, false]);
        assert_eq!(foreground_calls(&pool, &b), vec![true]);

        pool.activate(&a);
        assert_eq!(foreground_calls(&pool, &a), vec![true, false, true]);
        assert_eq!(foreground_calls(&pool, &b), vec![true, false]);
    }

    /// W1：insert_connected 与 activate 同语义（旧 false、新 true）。
    #[tokio::test]
    async fn insert_connected_notifies_foreground_like_activate() {
        let mut pool = WorkspacePool::new(WorkspacePoolPolicy::new(4));
        let a = id("a", "tmux");
        pool.open(a.clone(), "a".into(), |_| {
            Box::new(MockRuntime::with_single_pane())
        })
        .await
        .unwrap();
        let b = id("b", "tmux");
        let workspace = Workspace::new(
            b.clone(),
            "b".into(),
            Box::new(MockRuntime::with_single_pane()),
        );
        pool.insert_connected(workspace);
        assert_eq!(pool.active_id(), Some(&b));
        assert_eq!(foreground_calls(&pool, &a), vec![true, false]);
        assert_eq!(foreground_calls(&pool, &b), vec![true]);
    }

    /// W1：close 先降前台（false）再按能力释放。
    #[tokio::test]
    async fn close_notifies_false_before_capability_release() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let fg = Arc::new(Mutex::new(Vec::new()));
        let mut pool = WorkspacePool::new(WorkspacePoolPolicy::new(4));
        let a = id("a", "tmux");
        let log_cb = log.clone();
        let fg_cb = fg.clone();
        pool.open(a.clone(), "a".into(), move |_| {
            let mut b = MockRuntime::with_single_pane();
            b.capabilities = &[RuntimeCapability::PersistDetach];
            b.executed_log = Some(log_cb);
            b.foreground_log = Some(fg_cb);
            Box::new(b)
        })
        .await
        .unwrap();
        assert_eq!(foreground_calls(&pool, &a), vec![true]);
        assert!(pool.close(&a));
        assert_eq!(
            *fg.lock().unwrap(),
            vec![true, false],
            "close 必须先降前台再释放"
        );
        assert!(
            log.lock()
                .unwrap()
                .iter()
                .any(|t| matches!(t, Task::Detach)),
            "PersistDetach 能力应走 Detach"
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

    /// list 返回全部工作区；close 走 PersistDetach 能力 Detach。
    #[tokio::test]
    async fn list_and_close() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut pool = WorkspacePool::new(WorkspacePoolPolicy::new(4));
        let a = id("a", "tmux");
        let b = id("b", "tmux");
        let log_cb = log.clone();
        pool.open(a.clone(), "a".into(), move |_| {
            let mut bk = MockRuntime::with_single_pane();
            bk.capabilities = &[RuntimeCapability::PersistDetach];
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
