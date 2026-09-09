//! Catalog：backend 总状态。FFI 持有这一份。
//!
//! 契约：`docs/CATALOG.md`。施工：`docs/CATALOG-PLAN.md`。
//!
//! `trait Runtime` 只表示已经 attach 的格子。Catalog 保留 provider 视图、
//! Inventory 和 resolver；可复用连接由组合根通过显式 registry 提供。

pub mod inventory;
pub mod resolver;

use std::sync::Arc;
use std::thread;

use crate::projects::Project;
use crate::protocol::candidate::{Candidate, CandidateRef, ExistingCandidateRef};
use crate::runtime::registry::RuntimeRegistry;
use crate::runtime::runtime_supports_channels;
use crate::transport::registry::ConnectionRegistry;
use crate::transport::registry::TransportRegistry;
use crate::transport::{ChannelKind, TargetConnection};
use crate::workspace::pool::WorkspacePool;
use muxterm_protocol::WorkspaceId;

pub use crate::protocol::candidate::ExistingCandidate;
pub use crate::runtime::{RuntimeInfo, RuntimeProvider};
#[allow(unused_imports)] // 给 FFI / 测试用的公开类型
pub use inventory::{Inventory, InventorySnapshot, Reach};
pub use muxterm_transport::provider::{TargetInfo, TransportInfo, TransportProvider};
pub use muxterm_transport::Connect;
pub use resolver::{
    config_to_spec, OpenRequest, ResolveError, ResolveErrorStage, ResolveIntent, ResolvedTarget,
};

type DiscoveryJob = (String, Option<Arc<dyn TargetConnection>>, Vec<ChannelKind>);

/// 进程内一份 backend 总状态。
pub struct Catalog {
    /// Read-only views of registries owned by the product composition root.
    runtimes: Arc<RuntimeRegistry>,
    transports: Arc<TransportRegistry>,
    inventory: Inventory,
}

impl Default for Catalog {
    fn default() -> Self {
        Self::new()
    }
}

impl Catalog {
    /// 空 Catalog（测试用）。不注册内置插件。
    pub fn new() -> Self {
        Self {
            runtimes: Arc::new(RuntimeRegistry::new()),
            transports: Arc::new(TransportRegistry::new()),
            inventory: Inventory::new(),
        }
    }

    /// 生产入口：注册内置 Driver / TransportProvider。
    ///
    /// 只注册，不 connect、不探用户默认 herdr.sock。
    pub fn with_builtins() -> Self {
        Self::from_registries(
            Arc::new(RuntimeRegistry::with_builtins()),
            Arc::new(TransportRegistry::with_builtins()),
        )
    }

    /// Build a Catalog view over registries owned by the product root.
    pub(crate) fn from_registries(
        runtimes: Arc<RuntimeRegistry>,
        transports: Arc<TransportRegistry>,
    ) -> Self {
        Self {
            runtimes,
            transports,
            inventory: Inventory::new(),
        }
    }

    /// Clone the runtime registry handle for `Muxterm` ownership.
    pub(crate) fn runtime_registry(&self) -> Arc<RuntimeRegistry> {
        Arc::clone(&self.runtimes)
    }

    /// Clone the transport registry handle for `Muxterm` ownership.
    pub(crate) fn transport_registry(&self) -> Arc<TransportRegistry> {
        Arc::clone(&self.transports)
    }

    /// 注册一个 Runtime 插件。同 id 原地覆盖（保持位置）；新 id 追加到末尾。
    pub fn register_runtime(&mut self, driver: Box<dyn RuntimeProvider>) {
        Arc::get_mut(&mut self.runtimes)
            .expect("Catalog provider registries must be configured before sharing")
            .register(driver);
    }

    /// 注册一个 TransportProvider 插件。同 id 原地覆盖；新 id 追加。
    pub fn register_transport(&mut self, transport: Box<dyn TransportProvider>) {
        Arc::get_mut(&mut self.transports)
            .expect("Catalog provider registries must be configured before sharing")
            .register(transport);
    }

    /// 已注册 Driver 的静态信息（新建项目卡的数据源）。顺序 = 注册顺序。
    pub fn runtime_list(&self) -> Vec<RuntimeInfo> {
        self.runtimes
            .providers()
            .iter()
            .map(|runtime| RuntimeInfo {
                id: runtime.id().to_string(),
                name: runtime.name().to_string(),
                support: runtime.support().to_vec(),
                // Keep this legacy DTO field as a projection for existing
                // clients; the relation itself is channel-based.
                accepted_transports: self
                    .transports
                    .providers()
                    .iter()
                    .filter(|transport| {
                        runtime_supports_channels(runtime.as_ref(), transport.supported_channels())
                    })
                    .map(|transport| transport.id().to_string())
                    .collect(),
            })
            .collect()
    }

    /// 已注册 TransportProvider 的静态信息。顺序 = 注册顺序。
    pub fn transport_list(&self) -> Vec<TransportInfo> {
        self.transports
            .providers()
            .iter()
            .map(|t| t.info())
            .collect()
    }

    fn runtime(&self, id: &str) -> Option<&dyn RuntimeProvider> {
        self.runtimes.get(id)
    }

    fn transport(&self, id: &str) -> Option<&dyn TransportProvider> {
        self.transports.get(id)
    }

    /// 列出某个 TransportProvider 的 target（Local 单例 / SSH hosts）。
    pub fn discover_targets(&self, transport_id: &str) -> anyhow::Result<Vec<TargetInfo>> {
        let t = self
            .transport(transport_id)
            .ok_or_else(|| anyhow::anyhow!("unknown transport '{transport_id}'"))?;
        t.list_targets()
    }

    /// 取出或新建一条可复用管道。同一 `(transport, target)` 返回同一 `Arc`。
    pub fn connect(
        &self,
        connections: &mut ConnectionRegistry,
        transport_id: &str,
        target: &str,
    ) -> anyhow::Result<Arc<dyn TargetConnection>> {
        if let Some(existing) = connections.get(transport_id, target) {
            return Ok(existing);
        }
        let t = self
            .transport(transport_id)
            .ok_or_else(|| anyhow::anyhow!("unknown transport '{transport_id}'"))?;
        let connect = t.connect(target)?;
        connections.acquire(transport_id, target, || Ok(connect.clone()))
    }

    /// 扇出到接受该 transport 的 Driver。单个 Driver 失败则跳过，不让整表失败。
    ///
    /// `transport_id == "all"` 时，对 local 单例 + 每个 SSH target 各扇出一次，
    /// 拼接成一张表（同一 session 经 local 和 ssh-self 出现两行，禁止去重）。
    /// SSH host 最多 4 路并发，慢/死 host 不能把整表拖成串行超时之和。
    pub fn discover_sessions(
        &self,
        connections: &mut ConnectionRegistry,
        transport_id: &str,
        target: &str,
    ) -> anyhow::Result<Vec<ExistingCandidate>> {
        if transport_id == "all" {
            let names = self.all_connect_names();
            let mut jobs: Vec<DiscoveryJob> = Vec::new();
            for (tid, tgt) in names {
                let supported_channels = self
                    .transport(&tid)
                    .map(|transport| transport.supported_channels().to_vec())
                    .unwrap_or_default();
                let connect = self.connect(connections, &tid, &tgt).ok();
                jobs.push((tid, connect, supported_channels));
            }
            let runtimes = self.runtimes.providers();
            let mut out = Vec::new();
            for chunk in jobs.chunks(4) {
                thread::scope(|scope| {
                    let handles: Vec<_> = chunk
                        .iter()
                        .map(|(tid, connect, supported_channels)| {
                            let transport_id = tid.as_str();
                            let connect = connect.clone();
                            let supported_channels = supported_channels.clone();
                            scope.spawn(move || {
                                let Some(connect) = connect else {
                                    return Vec::new();
                                };
                                let mut rows = list_sessions_on_connect(
                                    runtimes,
                                    &supported_channels,
                                    connect.as_ref(),
                                );
                                if transport_id == "local" {
                                    for row in &mut rows {
                                        row.target = "local".to_string();
                                    }
                                }
                                rows
                            })
                        })
                        .collect();
                    for handle in handles {
                        out.append(&mut handle.join().unwrap_or_default());
                    }
                });
            }
            return Ok(out);
        }
        let supported_channels = self
            .transport(transport_id)
            .map(|transport| transport.supported_channels().to_vec())
            .unwrap_or_default();
        let connect = match self.connect(connections, transport_id, target) {
            Ok(c) => c,
            Err(_) => return Ok(Vec::new()),
        };
        Ok(list_sessions_on_connect(
            self.runtimes.providers(),
            &supported_channels,
            connect.as_ref(),
        ))
    }

    /// C9：connect name 表 = local 单例 + 每个 SSH Host alias。
    fn all_connect_names(&self) -> Vec<(String, String)> {
        let mut names = vec![("local".to_string(), "".to_string())];
        if let Some(ssh) = self.transport("ssh") {
            if let Ok(targets) = ssh.list_targets() {
                for t in targets {
                    names.push(("ssh".to_string(), t.id));
                }
            }
        }
        names
    }

    /// 唯一 TargetConfig→ResolvedTarget 解析入口（W6 §11.2）。
    ///
    /// Project/Recent/Existing 三路都走这里；platform 不得复制第二套。
    /// 只做身份解析（含 Herdr workspace 存在性检查），不建 Runtime。
    pub fn resolve_target(
        &self,
        connections: &mut ConnectionRegistry,
        config: &crate::quickconnect::model::TargetConfig,
        intent: ResolveIntent,
    ) -> Result<ResolvedTarget, resolver::ResolveError> {
        use crate::quickconnect::model::{TargetRuntime, TargetTransport};

        let identity = config.identity_key();
        match config.runtime {
            TargetRuntime::Herdr => {
                // Herdr：核对 workspace 存在（AttachOnly 无匹配不创建；
                // CreateIfMissing 且 local 才可创建，SSH 两意图都零创建命令）。
                let transport = match &config.transport {
                    TargetTransport::Local => "local",
                    TargetTransport::Ssh { .. } => "ssh",
                };
                let target = match &config.transport {
                    TargetTransport::Ssh { name } => name.as_str(),
                    TargetTransport::Local => "",
                };
                if self.transport(transport).is_none() {
                    return Err(resolver::ResolveError::UnknownTransport {
                        id: transport.to_string(),
                    });
                }
                if self.runtime("herdr").is_none() {
                    return Err(resolver::ResolveError::UnknownRuntime {
                        id: "herdr".to_string(),
                    });
                }
                let connect = self
                    .connect(connections, transport, target)
                    .map_err(|error| resolver::ResolveError::TargetConnection {
                        transport_id: transport.to_string(),
                        target: target.to_string(),
                        message: format!("{error:#}"),
                    })?;
                let driver = self
                    .runtime("herdr")
                    .expect("刚检查过的 Herdr RuntimeProvider 必须仍在");
                let namespace = config.session.clone();
                let candidates = driver
                    .discover(connect.as_ref(), namespace.as_deref())
                    .map_err(|error| resolver::ResolveError::Discovery {
                        runtime_id: "herdr".to_string(),
                        transport_id: transport.to_string(),
                        target: target.to_string(),
                        message: format!("{error:#}"),
                    })?;

                // exact identity：workspace_id 精确命中。
                if let Some(wid) = &config.workspace_id {
                    if let Some(hit) = candidates
                        .iter()
                        .find(|c| c.extra == *wid && c.namespace.as_deref() == namespace.as_deref())
                    {
                        return Ok(self.resolved_from_candidate(config, hit));
                    }
                }
                // name/label 命中；同名两候选 → ambiguity。
                let named: Vec<&ExistingCandidate> = candidates
                    .iter()
                    .filter(|c| c.name == config.name)
                    .collect();
                match named.as_slice() {
                    [] => match intent {
                        ResolveIntent::AttachOnly => Err(resolver::ResolveError::NoMatch {
                            identity,
                            intent: format!("{intent:?}"),
                        }),
                        ResolveIntent::CreateIfMissing => {
                            if transport == "ssh" {
                                Err(resolver::ResolveError::CreateNotAllowed {
                                    identity,
                                    reason: "SSH target 不允许启动 workspace.create".to_string(),
                                })
                            } else {
                                // 只允许显式 named session/socket 且该 session
                                // 已运行；未明确或不可达返回 choice-required，
                                // 禁止偷偷换 default 或启动 server。
                                let Some(session_name) = config.session.clone() else {
                                    return Err(resolver::ResolveError::CreateNotAllowed {
                                        identity,
                                        reason: "需要显式 named session".to_string(),
                                    });
                                };
                                let Some(socket) = config.socket.clone() else {
                                    return Err(resolver::ResolveError::CreateNotAllowed {
                                        identity,
                                        reason: "需要显式 socket 路径".to_string(),
                                    });
                                };
                                let herdr = crate::runtime::herdr::session::HerdrSession::new(
                                    &session_name,
                                    &socket,
                                );
                                if herdr.ping().is_err() {
                                    return Err(resolver::ResolveError::CreateNotAllowed {
                                        identity,
                                        reason: "目标 named session 未运行".to_string(),
                                    });
                                }
                                let created = herdr
                                    .workspace_create(&config.path, &config.name)
                                    .map_err(|error| resolver::ResolveError::CreateNotAllowed {
                                        identity: identity.clone(),
                                        reason: format!("workspace.create 失败: {error:#}"),
                                    })?;
                                let mut canonical = config.clone();
                                canonical.workspace_id = Some(created.workspace_id);
                                let spec = config_to_spec(&canonical);
                                Ok(ResolvedTarget { canonical, spec })
                            }
                        }
                    },
                    [one] => Ok(self.resolved_from_candidate(config, one)),
                    many => Err(resolver::ResolveError::AmbiguousCandidate {
                        identity,
                        candidates: many
                            .iter()
                            .map(|candidate| candidate.extra.clone())
                            .collect(),
                    }),
                }
            }
            _ => {
                // shell/tmux：不建 Runtime，只做规范化 spec 转换。
                let spec = config_to_spec(config);
                Ok(ResolvedTarget {
                    canonical: config.clone(),
                    spec,
                })
            }
        }
    }

    /// Resolve the four product Candidate kinds through one Core entry point.
    ///
    /// Project records are supplied by the Projects domain; Catalog still owns
    /// all runtime/discovery resolution and is the only producer of a spec.
    pub fn resolve_open_request(
        &self,
        connections: &mut ConnectionRegistry,
        request: &OpenRequest,
        projects: &[Project],
    ) -> Result<ResolvedTarget, resolver::ResolveError> {
        self.resolve_open_request_with_recent(connections, request, projects, &[])
    }

    /// Resolve an open request while the live pool is owned by Muxterm.
    ///
    /// The resolver still owns the only `WorkspaceSpec` construction path, but
    /// it receives the recent descriptors as an owned snapshot instead of
    /// borrowing a pool that belongs to the composition root.
    pub(crate) fn resolve_open_request_with_recent(
        &self,
        connections: &mut ConnectionRegistry,
        request: &OpenRequest,
        projects: &[Project],
        recent: &[ResolvedTarget],
    ) -> Result<ResolvedTarget, resolver::ResolveError> {
        match &request.candidate {
            CandidateRef::Project { project_id } => {
                let project = projects
                    .iter()
                    .find(|project| project.id.as_str() == project_id)
                    .ok_or_else(|| resolver::ResolveError::ProjectNotFound {
                        id: project_id.clone(),
                    })?;
                let mut resolved =
                    self.resolve_target(connections, &project.target, request.intent)?;
                resolved.spec.provenance = Some(project.provenance());
                resolved.spec.template = request
                    .template
                    .clone()
                    .or_else(|| project.template.clone());
                resolved.spec.create = request.intent == ResolveIntent::CreateIfMissing;
                Ok(resolved)
            }
            CandidateRef::Worktree {
                project_id,
                worktree_id,
            } => {
                let project = projects
                    .iter()
                    .find(|project| project.id.as_str() == project_id)
                    .ok_or_else(|| resolver::ResolveError::ProjectNotFound {
                        id: project_id.clone(),
                    })?;
                let worktree = project
                    .worktrees
                    .iter()
                    .find(|worktree| worktree.id.as_str() == worktree_id)
                    .ok_or_else(|| resolver::ResolveError::WorktreeNotFound {
                        project_id: project_id.clone(),
                        worktree_id: worktree_id.clone(),
                    })?;

                let mut target = project.target.clone();
                target.name = if worktree.branch.trim().is_empty() {
                    worktree.id.to_string()
                } else {
                    worktree.branch.clone()
                };
                target.path = worktree.path.clone();
                target.workspace_id = None;
                if target.runtime == crate::quickconnect::model::TargetRuntime::Tmux {
                    target.session = Some(worktree.id.to_string());
                }

                let mut resolved = self.resolve_target(connections, &target, request.intent)?;
                if request.intent == ResolveIntent::CreateIfMissing {
                    resolved.spec.create = true;
                }
                resolved.spec.provenance = Some(project.worktree_provenance(&worktree.id));
                resolved.spec.template = request
                    .template
                    .clone()
                    .or_else(|| project.template.clone());
                Ok(resolved)
            }
            CandidateRef::Existing { identity } => {
                let mut resolved = self.resolve_existing_candidate(connections, identity)?;
                resolved.spec.template = request.template.clone();
                // An Existing row is an attach identity even if a caller
                // accidentally supplies CreateIfMissing.
                resolved.spec.create = false;
                Ok(resolved)
            }
            CandidateRef::Recent { key } => {
                let mut resolved = recent
                    .iter()
                    .find(|resolved| resolved.canonical.identity_key() == *key)
                    .cloned()
                    .ok_or_else(|| resolver::ResolveError::RecentNotFound { key: key.clone() })?;
                resolved.spec.template = request.template.clone().or(resolved.spec.template);
                resolved.spec.create = false;
                Ok(resolved)
            }
        }
    }

    /// Build the unified Project/Worktree/Existing/Recent list without
    /// creating Runtime instances or performing discovery itself.
    pub fn candidates(
        &self,
        projects: &[Project],
        existing: &[ExistingCandidate],
        recent_limit: usize,
        pool: &WorkspacePool,
    ) -> Vec<Candidate> {
        self.candidates_with_pool(projects, existing, recent_limit, pool)
    }

    /// Build candidates against a live pool owned by Muxterm.
    pub(crate) fn candidates_with_pool(
        &self,
        projects: &[Project],
        existing: &[ExistingCandidate],
        recent_limit: usize,
        pool: &WorkspacePool,
    ) -> Vec<Candidate> {
        let mut rows = Vec::new();
        for project in projects {
            let mut project_row = Candidate::project(project.id.to_string(), project.name.clone());
            project_row.subtitle = project.target.path.clone();
            project_row.badges = vec![
                project.target.runtime.as_str().into(),
                project.target.transport.label(),
            ];
            project_row.in_pool = self.workspace_for_provenance_in(pool, project.id.as_str(), None);
            rows.push(project_row);

            for worktree in &project.worktrees {
                let mut worktree_row = Candidate::worktree(
                    project.id.to_string(),
                    worktree.id.to_string(),
                    if worktree.branch.trim().is_empty() {
                        worktree.id.to_string()
                    } else {
                        worktree.branch.clone()
                    },
                );
                worktree_row.subtitle = worktree.path.clone();
                worktree_row.badges = vec!["worktree".into()];
                worktree_row.in_pool = self.workspace_for_provenance_in(
                    pool,
                    project.id.as_str(),
                    Some(worktree.id.as_str()),
                );
                rows.push(worktree_row);
            }
        }

        for candidate in existing {
            let mut row = Candidate::existing(candidate, None);
            let identity = match &row.reference {
                CandidateRef::Existing { identity } => identity,
                _ => unreachable!("Candidate::existing must retain Existing reference"),
            };
            row.in_pool = self.workspace_for_existing_in(pool, identity);
            rows.push(row);
        }

        for workspace in pool.recent_workspaces(recent_limit) {
            let Some(resolved) = workspace.resolved_target() else {
                continue;
            };
            let mut row =
                Candidate::recent(resolved.canonical.identity_key(), resolved.display_name());
            row.subtitle = resolved.spec.path.clone();
            row.badges = vec![
                resolved.spec.runtime.clone(),
                resolved.spec.transport.clone(),
            ];
            row.in_pool = Some(workspace.id().clone());
            rows.push(row);
        }
        rows
    }

    fn workspace_for_provenance_in(
        &self,
        pool: &WorkspacePool,
        project_id: &str,
        worktree_id: Option<&str>,
    ) -> Option<WorkspaceId> {
        pool.list().into_iter().find_map(|workspace| {
            let provenance = workspace.provenance()?;
            let same_project = provenance
                .project_id
                .as_ref()
                .is_some_and(|id| id.as_str() == project_id);
            let same_worktree = provenance
                .worktree_id
                .as_ref()
                .map(|id| Some(id.as_str()) == worktree_id)
                .unwrap_or(worktree_id.is_none());
            (same_project && same_worktree).then(|| workspace.id().clone())
        })
    }

    fn workspace_for_existing_in(
        &self,
        pool: &WorkspacePool,
        identity: &ExistingCandidateRef,
    ) -> Option<WorkspaceId> {
        pool.list().into_iter().find_map(|workspace| {
            let resolved = workspace.resolved_target()?;
            let canonical = &resolved.canonical;
            let target = match &canonical.transport {
                crate::quickconnect::model::TargetTransport::Local => "",
                crate::quickconnect::model::TargetTransport::Ssh { name } => name.as_str(),
            };
            let transport_id = match &canonical.transport {
                crate::quickconnect::model::TargetTransport::Local => "local",
                crate::quickconnect::model::TargetTransport::Ssh { .. } => "ssh",
            };
            let target_matches =
                identity.target == target || (target.is_empty() && identity.target == "local");
            (canonical.runtime.as_str() == identity.runtime_id
                && transport_id == identity.transport_id
                && target_matches
                && canonical.session == identity.session
                && canonical.socket == identity.socket
                && canonical.workspace_id == identity.workspace_id)
                .then(|| workspace.id().clone())
        })
    }

    fn resolve_existing_candidate(
        &self,
        connections: &mut ConnectionRegistry,
        identity: &ExistingCandidateRef,
    ) -> Result<ResolvedTarget, resolver::ResolveError> {
        let connect_target = existing_connect_target(identity);
        if self.transport(&identity.transport_id).is_none() {
            return Err(resolver::ResolveError::UnknownTransport {
                id: identity.transport_id.clone(),
            });
        }
        if self.runtime(&identity.runtime_id).is_none() {
            return Err(resolver::ResolveError::UnknownRuntime {
                id: identity.runtime_id.clone(),
            });
        }
        let connect = self
            .connect(connections, &identity.transport_id, connect_target)
            .map_err(|error| resolver::ResolveError::TargetConnection {
                transport_id: identity.transport_id.clone(),
                target: connect_target.to_string(),
                message: format!("{error:#}"),
            })?;
        let driver = self
            .runtime(&identity.runtime_id)
            .expect("刚检查过的 RuntimeProvider 必须仍在");
        let candidates = driver
            .discover(connect.as_ref(), identity.session.as_deref())
            .map_err(|error| resolver::ResolveError::Discovery {
                runtime_id: identity.runtime_id.clone(),
                transport_id: identity.transport_id.clone(),
                target: connect_target.to_string(),
                message: format!("{error:#}"),
            })?;
        let candidate = candidates
            .iter()
            .find(|candidate| existing_identity_matches(candidate, identity))
            .ok_or_else(|| resolver::ResolveError::ExistingCandidateNotFound {
                key: identity.key(),
            })?;
        let config = target_config_from_existing(candidate).map_err(|error| {
            resolver::ResolveError::InvalidIdentity {
                identity: identity.key(),
                reason: format!("{error:#}"),
            }
        })?;
        Ok(self.resolved_from_candidate(&config, candidate))
    }

    /// ExistingCandidate → ResolvedTarget（identity 字段保留；W6 §11.1 用
    /// typed session/socket/workspace_id，禁止从 extra 猜身份）。
    fn resolved_from_candidate(
        &self,
        config: &crate::quickconnect::model::TargetConfig,
        candidate: &ExistingCandidate,
    ) -> ResolvedTarget {
        let mut canonical = config.clone();
        // 缺权威 project path 时保持空；绝不回填 workspace id 当目录。
        if canonical.workspace_id.is_none() {
            canonical.workspace_id = candidate
                .workspace_id
                .clone()
                .or_else(|| (!candidate.extra.is_empty()).then(|| candidate.extra.clone()));
        }
        if canonical.session.is_none() {
            canonical.session = candidate
                .session
                .clone()
                .or_else(|| candidate.namespace.clone());
        }
        if canonical.socket.is_none() {
            canonical.socket = candidate.socket.clone();
        }
        let spec = config_to_spec(&canonical);
        ResolvedTarget { canonical, spec }
    }

    /// 探活未打开的 target。禁止为此 attach Runtime。
    ///
    /// 对每个 TransportProvider 的 target：connect 失败 → Reach::Err；成功 →
    /// 各接受该 transport 的 Driver.list（短命令）成功 → Reach::Ok。
    /// 只写 Inventory，不打开 Workspace。
    pub fn refresh_inventory(
        &mut self,
        connections: &mut ConnectionRegistry,
    ) -> anyhow::Result<()> {
        let transport_ids: Vec<String> = self
            .transports
            .providers()
            .iter()
            .map(|t| t.id().to_string())
            .collect();
        for transport_id in transport_ids {
            let supported_channels = self
                .transport(&transport_id)
                .map(|transport| transport.supported_channels().to_vec())
                .unwrap_or_default();
            let targets: Vec<TargetInfo> = match self.transport(&transport_id) {
                Some(t) => t.list_targets().unwrap_or_default(),
                None => continue,
            };
            for target in targets {
                let reach = match self.connect(connections, &transport_id, &target.id) {
                    Ok(connect) => {
                        let mut ok = false;
                        for driver in self.runtimes.providers() {
                            if !runtime_supports_channels(driver.as_ref(), &supported_channels) {
                                continue;
                            }
                            if driver.discover(connect.as_ref(), None).is_ok() {
                                ok = true;
                                break;
                            }
                        }
                        if ok {
                            Reach::Ok
                        } else {
                            Reach::Err
                        }
                    }
                    Err(_) => Reach::Err,
                };
                self.inventory.mark(&transport_id, &target.id, reach);
            }
        }
        Ok(())
    }

    pub fn inventory_snapshot(&self) -> InventorySnapshot {
        self.inventory.snapshot()
    }

    pub fn inventory_mut(&mut self) -> &mut Inventory {
        &mut self.inventory
    }
}

fn existing_identity_matches(
    candidate: &ExistingCandidate,
    identity: &ExistingCandidateRef,
) -> bool {
    candidate.runtime_id == identity.runtime_id
        && candidate.transport_id == identity.transport_id
        && existing_target_matches(candidate, identity)
        && candidate.session == identity.session
        && candidate.socket == identity.socket
        && candidate.workspace_id == identity.workspace_id
}

/// The all-target discovery view uses `local` as the user-facing connect name,
/// while the local provider's canonical target is the empty string. Keep that
/// display alias out of ConnectionRegistry keys and identity comparisons.
fn existing_connect_target(identity: &ExistingCandidateRef) -> &str {
    if identity.transport_id == "local" && identity.target == "local" {
        ""
    } else {
        identity.target.as_str()
    }
}

fn existing_target_matches(candidate: &ExistingCandidate, identity: &ExistingCandidateRef) -> bool {
    candidate.target == identity.target
        || (identity.transport_id == "local"
            && identity.target == "local"
            && candidate.target.is_empty())
}

fn target_config_from_existing(
    candidate: &ExistingCandidate,
) -> anyhow::Result<crate::quickconnect::model::TargetConfig> {
    use crate::quickconnect::model::{TargetRuntime, TargetTransport};

    let runtime = TargetRuntime::from_str(&candidate.runtime_id)
        .ok_or_else(|| anyhow::anyhow!("unknown runtime '{}'", candidate.runtime_id))?;
    let transport = match candidate.transport_id.as_str() {
        "local" => TargetTransport::Local,
        "ssh" => TargetTransport::Ssh {
            name: candidate.target.clone(),
        },
        other => anyhow::bail!("unknown transport '{other}'"),
    };
    let path = if runtime == TargetRuntime::Herdr {
        candidate.workspace_id.clone().unwrap_or_default()
    } else {
        String::new()
    };
    let mut config = crate::quickconnect::model::TargetConfig::new(
        candidate.name.clone(),
        runtime,
        transport,
        path,
    );
    config.session = candidate
        .session
        .clone()
        .or_else(|| candidate.namespace.clone());
    config.socket = candidate.socket.clone();
    config.workspace_id = candidate.workspace_id.clone();
    Ok(config)
}

/// 对一条 Connect 扇出所有接受该 transport 的 Driver。
/// tmux 与 herdr 并行，避免死 SSH host 把 2s+2s 串成 4s。
fn list_sessions_on_connect(
    runtimes: &[Box<dyn RuntimeProvider>],
    supported_channels: &[ChannelKind],
    connect: &dyn TargetConnection,
) -> Vec<ExistingCandidate> {
    thread::scope(|scope| {
        let handles: Vec<_> = runtimes
            .iter()
            .filter(|driver| runtime_supports_channels(driver.as_ref(), supported_channels))
            .map(|driver| scope.spawn(|| driver.discover(connect, None).unwrap_or_default()))
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap_or_default())
            .collect()
    })
}

#[cfg(test)]
mod tests;
