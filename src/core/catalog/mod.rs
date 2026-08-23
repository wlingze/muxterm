//! Catalog：backend 总状态。FFI 持有这一份。
//!
//! 契约：`docs/CATALOG.md`。施工：`docs/CATALOG-PLAN.md`。
//!
//! `trait Runtime` 只表示已经 attach 的格子。列出候选、拿管道、探活
//! 都在 Catalog：Driver 表、Transport 表、Connect 缓存、Inventory、Pool。

pub mod builtin;
pub mod connect;
pub mod driver;
pub mod inventory;
pub mod resolver;
pub mod transport;

use std::collections::HashMap;
use std::sync::Arc;
use std::thread;

use crate::core::workspace::pool::WorkspacePool;
use crate::core::workspace::spec::WorkspaceSpec;
use crate::core::workspace::workspace::Workspace;

pub use connect::Connect;
pub use driver::{RuntimeDriver, RuntimeInfo, SessionCandidate};
#[allow(unused_imports)] // 给 FFI / 测试用的公开类型
pub use inventory::{Inventory, InventorySnapshot, Reach};
pub use resolver::{config_to_spec, ResolveIntent, ResolvedTarget};
pub use transport::{TargetInfo, Transport, TransportInfo};

/// 进程内一份 backend 总状态。
pub struct Catalog {
    /// Driver 表。顺序 = 注册顺序；`with_builtins` 按 tmux, herdr, shell 登记。
    runtimes: Vec<Box<dyn RuntimeDriver>>,
    /// Transport 表。顺序 = 注册顺序；`with_builtins` 按 local, ssh 登记。
    transports: Vec<Box<dyn Transport>>,
    connects: HashMap<(String, String), Arc<Connect>>,
    inventory: Inventory,
    pool: WorkspacePool,
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
            connects: HashMap::new(),
            inventory: Inventory::new(),
            pool: WorkspacePool::default(),
        }
    }

    /// 生产入口：注册内置 Driver / Transport。
    ///
    /// 只注册，不 connect、不探用户默认 herdr.sock。
    pub fn with_builtins() -> Self {
        let mut cat = Self::new();
        for driver in builtin::builtin_runtimes() {
            cat.register_runtime(driver);
        }
        for transport in builtin::builtin_transports() {
            cat.register_transport(transport);
        }
        cat
    }

    /// 注册一个 Runtime 插件。同 id 原地覆盖（保持位置）；新 id 追加到末尾。
    pub fn register_runtime(&mut self, driver: Box<dyn RuntimeDriver>) {
        let id = driver.id();
        if let Some(i) = self.runtimes.iter().position(|d| d.id() == id) {
            self.runtimes[i] = driver;
        } else {
            self.runtimes.push(driver);
        }
    }

    /// 注册一个 Transport 插件。同 id 原地覆盖；新 id 追加。
    pub fn register_transport(&mut self, transport: Box<dyn Transport>) {
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

    /// 已注册 Transport 的静态信息。顺序 = 注册顺序。
    pub fn transport_list(&self) -> Vec<TransportInfo> {
        self.transports.iter().map(|t| t.info()).collect()
    }

    fn runtime(&self, id: &str) -> Option<&dyn RuntimeDriver> {
        self.runtimes
            .iter()
            .find(|d| d.id() == id)
            .map(|d| d.as_ref())
    }

    fn transport(&self, id: &str) -> Option<&dyn Transport> {
        self.transports
            .iter()
            .find(|t| t.id() == id)
            .map(|t| t.as_ref())
    }

    /// 列出某个 Transport 的 target（Local 单例 / SSH hosts）。
    pub fn discover_targets(&self, transport_id: &str) -> anyhow::Result<Vec<TargetInfo>> {
        let t = self
            .transport(transport_id)
            .ok_or_else(|| anyhow::anyhow!("unknown transport '{transport_id}'"))?;
        t.list_targets()
    }

    /// 取出或新建一条可复用管道。同一 `(transport, target)` 返回同一 `Arc`。
    pub fn connect(&mut self, transport_id: &str, target: &str) -> anyhow::Result<Arc<Connect>> {
        let key = (transport_id.to_string(), target.to_string());
        if let Some(existing) = self.connects.get(&key) {
            return Ok(Arc::clone(existing));
        }
        let t = self
            .transport(transport_id)
            .ok_or_else(|| anyhow::anyhow!("unknown transport '{transport_id}'"))?;
        let connect = t.connect(target)?;
        self.connects.insert(key, Arc::clone(&connect));
        Ok(connect)
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
            let mut jobs: Vec<(String, Option<Arc<Connect>>)> = Vec::new();
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
                                let mut rows =
                                    list_sessions_on_connect(runtimes, transport_id, &connect);
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
            &connect,
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

    /// 按 spec 打开工作区：查 Driver → 复用 Connect → Driver.open → 进 Pool。
    ///
    /// 未知 runtime / 不接受的 transport → Err。禁止悄悄变成 Shell。
    pub async fn open(&mut self, spec: &WorkspaceSpec) -> anyhow::Result<&mut Workspace> {
        let runtime_id = spec.runtime.as_str();
        let transport_id = spec.transport.as_str();
        let accepted: Vec<String> = {
            let driver = self
                .runtime(runtime_id)
                .ok_or_else(|| anyhow::anyhow!("unknown runtime '{runtime_id}'"))?;
            driver
                .accepted_transports()
                .iter()
                .map(|s| (*s).to_string())
                .collect()
        };
        if !accepted.iter().any(|t| t == transport_id) {
            return Err(anyhow::anyhow!(
                "runtime '{runtime_id}' does not accept transport '{transport_id}'"
            ));
        }
        let target = spec.alias.as_deref().unwrap_or("");
        let connect = self.connect(transport_id, target)?;
        let runtime = self
            .runtime(runtime_id)
            .expect("刚查过的 Driver 必须仍在")
            .open(Arc::clone(&connect), spec)?;
        let id = spec.id();
        let name = spec.name();
        self.pool.open(id, name, |_| runtime).await
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
        let runtime_id = spec.runtime.as_str();
        let transport_id = spec.transport.as_str();
        let accepted: Vec<String> = {
            let driver = self
                .runtime(runtime_id)
                .ok_or_else(|| anyhow::anyhow!("unknown runtime '{runtime_id}'"))?;
            driver
                .accepted_transports()
                .iter()
                .map(|s| (*s).to_string())
                .collect()
        };
        if !accepted.iter().any(|t| t == transport_id) {
            return Err(anyhow::anyhow!(
                "runtime '{runtime_id}' does not accept transport '{transport_id}'"
            ));
        }
        let target = spec.alias.as_deref().unwrap_or("");
        let connect = self.connect(transport_id, target)?;
        let runtime = self
            .runtime(runtime_id)
            .expect("刚查过的 Driver 必须仍在")
            .open(Arc::clone(&connect), spec)?;
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
                let candidates = driver.list(&connect, namespace.as_deref())?;

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
    /// 对每个 Transport 的 target：connect 失败 → Reach::Err；成功 →
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
                            if driver.list(&connect, None).is_ok() {
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

/// 对一条 Connect 扇出所有接受该 transport 的 Driver。
/// tmux 与 herdr 并行，避免死 SSH host 把 2s+2s 串成 4s。
fn list_sessions_on_connect(
    runtimes: &[Box<dyn RuntimeDriver>],
    transport_id: &str,
    connect: &Connect,
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
