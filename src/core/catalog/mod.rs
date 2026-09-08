//! Catalog：backend 总状态。FFI 持有这一份。
//!
//! 契约：`docs/CATALOG.md`。施工：`docs/CATALOG-PLAN.md`。
//!
//! `trait Runtime` 只表示已经 attach 的格子。列出候选、拿管道、探活
//! 都在 Catalog：provider 视图、ConnectionRegistry、Inventory、Pool。

pub mod connect;
pub mod driver;
pub mod inventory;
pub mod resolver;
pub mod transport;

use std::sync::Arc;
use std::thread;

use crate::core::projects::Project;
use crate::core::protocol::candidate::{
    Candidate, CandidateRef, ExistingCandidate, ExistingCandidateRef,
};
use crate::core::runtime::Runtime;
use crate::core::transport::registry::ConnectionRegistry;
use crate::core::transport::TargetConnection;
use crate::core::workspace::id::WorkspaceId;
use crate::core::workspace::pool::WorkspacePool;
use crate::core::workspace::provenance::WorkspaceProvenance;
use crate::core::workspace::spec::WorkspaceSpec;
use crate::core::workspace::template::{TemplateName, TemplateRegistry, WorkspaceTemplate};
use crate::core::workspace::workspace::Workspace;

pub use crate::core::runtime::provider::{RuntimeInfo, RuntimeProvider};
pub use connect::Connect;
pub use driver::SessionCandidate;
#[allow(unused_imports)] // 给 FFI / 测试用的公开类型
pub use inventory::{Inventory, InventorySnapshot, Reach};
pub use resolver::{config_to_spec, OpenRequest, ResolveIntent, ResolvedTarget};
pub use transport::{TargetInfo, TransportInfo, TransportProvider};

/// 进程内一份 backend 总状态。
pub struct Catalog {
    /// Driver 表。顺序 = 注册顺序；`with_builtins` 按 tmux, herdr, shell 登记。
    runtimes: Vec<Box<dyn RuntimeProvider>>,
    /// TransportProvider 表。顺序 = 注册顺序；`with_builtins` 按 local, ssh 登记。
    transports: Vec<Box<dyn TransportProvider>>,
    connections: ConnectionRegistry,
    inventory: Inventory,
    pool: WorkspacePool,
    templates: TemplateRegistry,
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
            runtimes: Vec::new(),
            transports: Vec::new(),
            connections: ConnectionRegistry::new(),
            inventory: Inventory::new(),
            pool: WorkspacePool::default(),
            templates: TemplateRegistry::default(),
        }
    }

    /// Construct a production Catalog with an initial template registry.
    pub fn with_builtins_and_templates(templates: Vec<WorkspaceTemplate>) -> anyhow::Result<Self> {
        let mut catalog = Self::with_builtins();
        catalog.set_templates(templates)?;
        Ok(catalog)
    }

    pub fn template_registry(&self) -> &TemplateRegistry {
        &self.templates
    }

    pub fn template_registry_mut(&mut self) -> &mut TemplateRegistry {
        &mut self.templates
    }

    pub fn set_templates(&mut self, templates: Vec<WorkspaceTemplate>) -> anyhow::Result<()> {
        self.templates = TemplateRegistry::new(templates)?;
        Ok(())
    }

    pub fn register_template(&mut self, template: WorkspaceTemplate) -> anyhow::Result<()> {
        self.templates.insert(template)
    }

    /// 生产入口：注册内置 Driver / TransportProvider。
    ///
    /// 只注册，不 connect、不探用户默认 herdr.sock。
    pub fn with_builtins() -> Self {
        let mut cat = Self::new();
        for driver in crate::core::runtime::registry::with_builtins() {
            cat.register_runtime(driver);
        }
        for transport in crate::core::transport::registry::with_builtins() {
            cat.register_transport(transport);
        }
        cat
    }

    /// 注册一个 Runtime 插件。同 id 原地覆盖（保持位置）；新 id 追加到末尾。
    pub fn register_runtime(&mut self, driver: Box<dyn RuntimeProvider>) {
        let id = driver.id();
        if let Some(i) = self.runtimes.iter().position(|d| d.id() == id) {
            self.runtimes[i] = driver;
        } else {
            self.runtimes.push(driver);
        }
    }

    /// 注册一个 TransportProvider 插件。同 id 原地覆盖；新 id 追加。
    pub fn register_transport(&mut self, transport: Box<dyn TransportProvider>) {
        let id = transport.id();
        if let Some(i) = self.transports.iter().position(|t| t.id() == id) {
            self.transports[i] = transport;
        } else {
            self.transports.push(transport);
        }
    }

    /// 已注册 Driver 的静态信息（新建项目卡的数据源）。顺序 = 注册顺序。
    pub fn runtime_list(&self) -> Vec<RuntimeInfo> {
        self.runtimes.iter().map(|d| d.info()).collect()
    }

    /// 已注册 TransportProvider 的静态信息。顺序 = 注册顺序。
    pub fn transport_list(&self) -> Vec<TransportInfo> {
        self.transports.iter().map(|t| t.info()).collect()
    }

    fn runtime(&self, id: &str) -> Option<&dyn RuntimeProvider> {
        self.runtimes
            .iter()
            .find(|d| d.id() == id)
            .map(|d| d.as_ref())
    }

    fn transport(&self, id: &str) -> Option<&dyn TransportProvider> {
        self.transports
            .iter()
            .find(|t| t.id() == id)
            .map(|t| t.as_ref())
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
        &mut self,
        transport_id: &str,
        target: &str,
    ) -> anyhow::Result<Arc<dyn TargetConnection>> {
        if let Some(existing) = self.connections.get(transport_id, target) {
            return Ok(existing);
        }
        let t = self
            .transport(transport_id)
            .ok_or_else(|| anyhow::anyhow!("unknown transport '{transport_id}'"))?;
        let connect = t.connect(target)?;
        self.connections
            .acquire(transport_id, target, || Ok(connect.clone()))
    }

    /// 扇出到接受该 transport 的 Driver。单个 Driver 失败则跳过，不让整表失败。
    ///
    /// `transport_id == "all"` 时，对 local 单例 + 每个 SSH target 各扇出一次，
    /// 拼接成一张表（同一 session 经 local 和 ssh-self 出现两行，禁止去重）。
    /// SSH host 最多 4 路并发，慢/死 host 不能把整表拖成串行超时之和。
    pub fn discover_sessions(
        &mut self,
        transport_id: &str,
        target: &str,
    ) -> anyhow::Result<Vec<SessionCandidate>> {
        if transport_id == "all" {
            let names = self.all_connect_names();
            let mut jobs: Vec<(String, Option<Arc<dyn TargetConnection>>)> = Vec::new();
            for (tid, tgt) in names {
                let connect = self.connect(&tid, &tgt).ok();
                jobs.push((tid, connect));
            }
            let runtimes = &self.runtimes;
            let mut out = Vec::new();
            for chunk in jobs.chunks(4) {
                thread::scope(|scope| {
                    let handles: Vec<_> = chunk
                        .iter()
                        .map(|(tid, connect)| {
                            let transport_id = tid.as_str();
                            let connect = connect.clone();
                            scope.spawn(move || {
                                let Some(connect) = connect else {
                                    return Vec::new();
                                };
                                let mut rows = list_sessions_on_connect(
                                    runtimes,
                                    transport_id,
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
        let connect = match self.connect(transport_id, target) {
            Ok(c) => c,
            Err(_) => return Ok(Vec::new()),
        };
        Ok(list_sessions_on_connect(
            &self.runtimes,
            transport_id,
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

    /// 按 spec 通过 provider 构造尚未连接的 Runtime。
    ///
    /// 未知 runtime / 不接受的 transport → Err。禁止悄悄变成 Shell。
    pub fn new_runtime(&mut self, spec: &WorkspaceSpec) -> anyhow::Result<Box<dyn Runtime>> {
        let runtime_id = spec.runtime.as_str();
        let transport_id = spec.transport.as_str();
        let (accepted, requirements): (
            Vec<String>,
            &'static [crate::core::transport::ChannelKind],
        ) = {
            let driver = self
                .runtime(runtime_id)
                .ok_or_else(|| anyhow::anyhow!("unknown runtime '{runtime_id}'"))?;
            (
                driver
                    .accepted_transports()
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
                driver.channel_requirements(),
            )
        };
        if !accepted.iter().any(|t| t == transport_id) {
            return Err(anyhow::anyhow!(
                "runtime '{runtime_id}' does not accept transport '{transport_id}'"
            ));
        }
        let compatible = {
            let transport = self
                .transport(transport_id)
                .ok_or_else(|| anyhow::anyhow!("unknown transport '{transport_id}'"))?;
            requirements
                .iter()
                .all(|kind| transport.supported_channels().contains(kind))
        };
        if !compatible {
            return Err(anyhow::anyhow!(
                "runtime '{runtime_id}' requires channels {requirements:?}, but transport '{transport_id}' supports a different set"
            ));
        }
        let target = spec.alias.as_deref().unwrap_or("");
        let connect = self.connect(transport_id, target)?;
        let runtime = self
            .runtime(runtime_id)
            .expect("刚查过的 Driver 必须仍在")
            .new_instance(Arc::clone(&connect), spec)?;
        Ok(runtime)
    }

    /// 按 spec 打开工作区：查 provider → 复用 Connect → 构造 Runtime → 进 Pool。
    pub async fn open(&mut self, spec: &WorkspaceSpec) -> anyhow::Result<&mut Workspace> {
        let workspace_id = spec.id();
        let should_apply_template = self.pool.get(&workspace_id).is_none() && spec.create;
        let template = spec
            .template
            .as_ref()
            .and_then(|name| self.templates.get(name))
            .cloned();
        let runtime = self.new_runtime(spec)?;
        let workspace = self.pool.open_spec_with_runtime(spec, runtime).await?;
        if should_apply_template {
            if let Some(template) = template {
                workspace.start_template(template)?;
            }
        }
        Ok(workspace)
    }

    /// Native Runtime worktree path: ask the source Runtime for a new spec,
    /// then construct and insert the resulting Workspace through the pool.
    pub async fn create_native_worktree(
        &mut self,
        source: &WorkspaceId,
        worktree: &crate::core::runtime::WorktreeCreateSpec,
        provenance: Option<WorkspaceProvenance>,
        template: Option<TemplateName>,
    ) -> anyhow::Result<WorkspaceId> {
        let mut spec = {
            let workspace = self
                .pool
                .get(source)
                .ok_or_else(|| anyhow::anyhow!("workspace {source} 不在池里"))?;
            if !workspace
                .runtime()
                .support()
                .contains(&crate::core::runtime::RuntimeCapability::WorktreeCreate)
            {
                anyhow::bail!("runtime 不支持 native WorktreeCreate");
            }
            workspace.runtime().create_worktree_spec(worktree)?
        };
        spec.provenance = provenance.clone();
        spec.template = template;
        let workspace_id = spec.id();
        let should_apply_template = self.pool.get(&workspace_id).is_none() && spec.create;
        let template_record = spec
            .template
            .as_ref()
            .and_then(|name| self.templates.get(name))
            .cloned();
        let runtime = self.new_runtime(&spec)?;
        let workspace = self.pool.open_spec_with_runtime(&spec, runtime).await?;
        workspace.set_provenance(provenance);
        if should_apply_template {
            if let Some(template) = template_record {
                workspace.start_template(template)?;
            }
        }
        Ok(workspace_id)
    }

    /// 打开一个 spec 并返回**自有** Workspace（不进本 Catalog 池）。
    ///
    /// GUI 后台线程需要：Catalog 只做身份解析 + Driver open（共享 Connect），
    /// 结果 Workspace 由 platform 自己的池收编，避免第二份 pool 拷贝。
    pub async fn open_owned(&mut self, spec: &WorkspaceSpec) -> anyhow::Result<Workspace> {
        self.build_owned(spec).await
    }

    /// Driver open + Workspace 构造（不进池）；descriptor 由调用方按需设置。
    async fn build_owned(&mut self, spec: &WorkspaceSpec) -> anyhow::Result<Workspace> {
        let runtime = self.new_runtime(spec)?;
        let id = spec.id();
        let name = spec.name();
        Ok(Workspace::new_with_scrollback(
            id,
            name,
            runtime,
            spec.scrollback_lines as usize,
        ))
    }

    /// TargetConfig → owned Workspace（resolve 后 build_owned；GUI 后台线程用）。
    pub async fn open_target_owned(
        &mut self,
        config: &crate::core::quickconnect::model::TargetConfig,
        intent: ResolveIntent,
    ) -> anyhow::Result<Workspace> {
        let resolved = self.resolve_target(config, intent)?;
        self.open_resolved_owned(resolved).await
    }

    /// 打开已解析目标并返回 owned Workspace（不进池）。
    pub async fn open_resolved_owned(
        &mut self,
        resolved: ResolvedTarget,
    ) -> anyhow::Result<Workspace> {
        let spec = resolved.spec.clone();
        let canonical = resolved.canonical.clone();
        let mut workspace = self.build_owned(&spec).await?;
        workspace.set_resolved_target(ResolvedTarget { canonical, spec });
        Ok(workspace)
    }

    /// 唯一 TargetConfig→ResolvedTarget 解析入口（W6 §11.2）。
    ///
    /// Project/Recent/Existing 三路都走这里；platform 不得复制第二套。
    /// 只做身份解析（含 Herdr workspace 存在性检查），不建 Runtime。
    pub fn resolve_target(
        &mut self,
        config: &crate::core::quickconnect::model::TargetConfig,
        intent: ResolveIntent,
    ) -> anyhow::Result<ResolvedTarget> {
        use crate::core::quickconnect::model::{TargetRuntime, TargetTransport};

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
                let connect = self.connect(transport, target)?;
                let driver = self
                    .runtime("herdr")
                    .ok_or_else(|| anyhow::anyhow!("herdr runtime 未注册"))?;
                let namespace = config.session.clone();
                let candidates = driver.list(connect.as_ref(), namespace.as_deref())?;

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
                let named: Vec<&SessionCandidate> = candidates
                    .iter()
                    .filter(|c| c.name == config.name)
                    .collect();
                match named.as_slice() {
                    [] => match intent {
                        ResolveIntent::AttachOnly => Err(anyhow::anyhow!(
                            "AttachOnly 无匹配不创建（identity={identity}）"
                        )),
                        ResolveIntent::CreateIfMissing => {
                            if transport == "ssh" {
                                Err(anyhow::anyhow!(
                                    "CreateIfMissing 禁止 SSH 启动创建命令（identity={identity}）"
                                ))
                            } else {
                                // 只允许显式 named session/socket 且该 session
                                // 已运行；未明确或不可达返回 choice-required，
                                // 禁止偷偷换 default 或启动 server。
                                let Some(session_name) = config.session.clone() else {
                                    return Err(anyhow::anyhow!(
                                        "CreateIfMissing 需要显式 named session（identity={identity}）"
                                    ));
                                };
                                let Some(socket) = config.socket.clone() else {
                                    return Err(anyhow::anyhow!(
                                        "CreateIfMissing 需要显式 socket 路径（identity={identity}）"
                                    ));
                                };
                                let herdr = crate::core::runtime::herdr::session::HerdrSession::new(
                                    &session_name,
                                    &socket,
                                );
                                if herdr.ping().is_err() {
                                    return Err(anyhow::anyhow!(
                                        "CreateIfMissing 目标 named session 未运行（identity={identity}）"
                                    ));
                                }
                                let created = herdr
                                    .workspace_create(&config.path, &config.name)
                                    .map_err(|e| {
                                        anyhow::anyhow!(
                                            "CreateIfMissing workspace.create 失败（identity={identity}）: {e:#}"
                                        )
                                    })?;
                                let mut canonical = config.clone();
                                canonical.workspace_id = Some(created.workspace_id);
                                let spec = config_to_spec(&canonical);
                                Ok(ResolvedTarget { canonical, spec })
                            }
                        }
                    },
                    [one] => Ok(self.resolved_from_candidate(config, one)),
                    many => Err(anyhow::anyhow!(
                        "同名候选 ambiguity（identity={identity}）：{}；请按 id 选择",
                        many.iter()
                            .map(|c| c.extra.clone())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
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
        &mut self,
        request: &OpenRequest,
        projects: &[Project],
    ) -> anyhow::Result<ResolvedTarget> {
        match &request.candidate {
            CandidateRef::Project { project_id } => {
                let project = projects
                    .iter()
                    .find(|project| project.id.as_str() == project_id)
                    .ok_or_else(|| anyhow::anyhow!("project 不存在: {project_id}"))?;
                let mut resolved = self.resolve_target(&project.target, request.intent)?;
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
                    .ok_or_else(|| anyhow::anyhow!("project 不存在: {project_id}"))?;
                let worktree = project
                    .worktrees
                    .iter()
                    .find(|worktree| worktree.id.as_str() == worktree_id)
                    .ok_or_else(|| anyhow::anyhow!("worktree 不存在: {worktree_id}"))?;

                let mut target = project.target.clone();
                target.name = if worktree.branch.trim().is_empty() {
                    worktree.id.to_string()
                } else {
                    worktree.branch.clone()
                };
                target.path = worktree.path.clone();
                target.workspace_id = None;
                if target.runtime == crate::core::quickconnect::model::TargetRuntime::Tmux {
                    target.session = Some(worktree.id.to_string());
                }

                let mut resolved = self.resolve_target(&target, request.intent)?;
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
                let mut resolved = self.resolve_existing_candidate(identity)?;
                resolved.spec.template = request.template.clone();
                // An Existing row is an attach identity even if a caller
                // accidentally supplies CreateIfMissing.
                resolved.spec.create = false;
                Ok(resolved)
            }
            CandidateRef::Recent { key } => {
                let mut resolved = self
                    .pool
                    .list()
                    .into_iter()
                    .filter_map(|workspace| workspace.resolved_target().cloned())
                    .find(|resolved| resolved.canonical.identity_key() == *key)
                    .ok_or_else(|| anyhow::anyhow!("recent candidate 不存在: {key}"))?;
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
    ) -> Vec<Candidate> {
        let mut rows = Vec::new();
        for project in projects {
            let mut project_row = Candidate::project(project.id.to_string(), project.name.clone());
            project_row.subtitle = project.target.path.clone();
            project_row.badges = vec![
                project.target.runtime.as_str().into(),
                project.target.transport.label(),
            ];
            project_row.in_pool = self.workspace_for_provenance(project.id.as_str(), None);
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
                worktree_row.in_pool =
                    self.workspace_for_provenance(project.id.as_str(), Some(worktree.id.as_str()));
                rows.push(worktree_row);
            }
        }

        for candidate in existing {
            let mut row = Candidate::existing(candidate, None);
            let identity = match &row.reference {
                CandidateRef::Existing { identity } => identity,
                _ => unreachable!("Candidate::existing must retain Existing reference"),
            };
            row.in_pool = self.workspace_for_existing(identity);
            rows.push(row);
        }

        for workspace in self.pool.recent_workspaces(recent_limit) {
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

    fn workspace_for_provenance(
        &self,
        project_id: &str,
        worktree_id: Option<&str>,
    ) -> Option<WorkspaceId> {
        self.pool.list().into_iter().find_map(|workspace| {
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

    fn workspace_for_existing(&self, identity: &ExistingCandidateRef) -> Option<WorkspaceId> {
        self.pool.list().into_iter().find_map(|workspace| {
            let resolved = workspace.resolved_target()?;
            let canonical = &resolved.canonical;
            let target = match &canonical.transport {
                crate::core::quickconnect::model::TargetTransport::Local => "",
                crate::core::quickconnect::model::TargetTransport::Ssh { name } => name.as_str(),
            };
            let transport_id = match &canonical.transport {
                crate::core::quickconnect::model::TargetTransport::Local => "local",
                crate::core::quickconnect::model::TargetTransport::Ssh { .. } => "ssh",
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
        &mut self,
        identity: &ExistingCandidateRef,
    ) -> anyhow::Result<ResolvedTarget> {
        let connect_target = existing_connect_target(identity);
        let connect = self.connect(&identity.transport_id, connect_target)?;
        let driver = self
            .runtime(&identity.runtime_id)
            .ok_or_else(|| anyhow::anyhow!("unknown runtime '{}'", identity.runtime_id))?;
        let candidates = driver.list(connect.as_ref(), identity.session.as_deref())?;
        let candidate = candidates
            .iter()
            .find(|candidate| existing_identity_matches(candidate, identity))
            .ok_or_else(|| {
                anyhow::anyhow!("existing candidate identity 不存在: {}", identity.key())
            })?;
        let config = target_config_from_existing(candidate)?;
        Ok(self.resolved_from_candidate(&config, candidate))
    }

    /// SessionCandidate → ResolvedTarget（identity 字段保留；W6 §11.1 用
    /// typed session/socket/workspace_id，禁止从 extra 猜身份）。
    fn resolved_from_candidate(
        &self,
        config: &crate::core::quickconnect::model::TargetConfig,
        candidate: &SessionCandidate,
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

    /// TargetConfig → 打开（resolve 后 attach）。Project/Recent/Existing 共用。
    pub async fn open_target(
        &mut self,
        config: &crate::core::quickconnect::model::TargetConfig,
        intent: ResolveIntent,
    ) -> anyhow::Result<&mut Workspace> {
        let resolved = self.resolve_target(config, intent)?;
        self.open_resolved(resolved).await
    }

    /// 打开已解析目标；Workspace 保存 canonical descriptor（Core 唯一所有权）。
    pub async fn open_resolved(
        &mut self,
        resolved: ResolvedTarget,
    ) -> anyhow::Result<&mut Workspace> {
        let id = resolved.workspace_id();
        // 存在性检查用不可变借用；命中后重新取可变借用返回。
        if let Some(existing) = self.pool.get(&id) {
            // 同 identity slot 复用只允许整值补全 canonical name/path，
            // 不能改变 attach identity（spec 一致才能复用）。
            if existing.resolved_target().map(|r| &r.spec) == Some(&resolved.spec) {
                return Ok(self.pool.get_mut(&id).expect("刚查过必须存在"));
            }
            anyhow::bail!("identity key 撞到已打开 WorkspaceId {}（spec 不一致）", id);
        }
        let spec = resolved.spec.clone();
        let canonical = resolved.canonical.clone();
        let workspace = self.open(&spec).await?;
        workspace.set_resolved_target(ResolvedTarget { canonical, spec });
        Ok(workspace)
    }

    /// 探活未打开的 target。禁止为此 attach Runtime。
    ///
    /// 对每个 TransportProvider 的 target：connect 失败 → Reach::Err；成功 →
    /// 各接受该 transport 的 Driver.list（短命令）成功 → Reach::Ok。
    /// 只写 Inventory，不打开 Workspace。
    pub fn refresh_inventory(&mut self) -> anyhow::Result<()> {
        let transport_ids: Vec<String> =
            self.transports.iter().map(|t| t.id().to_string()).collect();
        for transport_id in transport_ids {
            let targets: Vec<TargetInfo> = match self.transport(&transport_id) {
                Some(t) => t.list_targets().unwrap_or_default(),
                None => continue,
            };
            for target in targets {
                let reach = match self.connect(&transport_id, &target.id) {
                    Ok(connect) => {
                        let mut ok = false;
                        for driver in &self.runtimes {
                            if !driver
                                .accepted_transports()
                                .contains(&transport_id.as_str())
                            {
                                continue;
                            }
                            if driver.list(connect.as_ref(), None).is_ok() {
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

    pub fn pool(&self) -> &WorkspacePool {
        &self.pool
    }

    pub fn pool_mut(&mut self) -> &mut WorkspacePool {
        &mut self.pool
    }
}

fn existing_identity_matches(
    candidate: &SessionCandidate,
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

fn existing_target_matches(candidate: &SessionCandidate, identity: &ExistingCandidateRef) -> bool {
    candidate.target == identity.target
        || (identity.transport_id == "local"
            && identity.target == "local"
            && candidate.target.is_empty())
}

fn target_config_from_existing(
    candidate: &SessionCandidate,
) -> anyhow::Result<crate::core::quickconnect::model::TargetConfig> {
    use crate::core::quickconnect::model::{TargetRuntime, TargetTransport};

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
    let mut config = crate::core::quickconnect::model::TargetConfig::new(
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
    transport_id: &str,
    connect: &dyn TargetConnection,
) -> Vec<SessionCandidate> {
    thread::scope(|scope| {
        let handles: Vec<_> = runtimes
            .iter()
            .filter(|driver| driver.accepted_transports().contains(&transport_id))
            .map(|driver| scope.spawn(|| driver.list(connect, None).unwrap_or_default()))
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap_or_default())
            .collect()
    })
}

#[cfg(test)]
mod tests;
