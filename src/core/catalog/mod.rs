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
pub mod transport;

use std::collections::HashMap;
use std::sync::Arc;

use crate::core::workspace::pool::WorkspacePool;
use crate::core::workspace::spec::WorkspaceSpec;
use crate::core::workspace::workspace::Workspace;

pub use connect::Connect;
pub use driver::{RuntimeDriver, RuntimeInfo, SessionCandidate};
#[allow(unused_imports)] // 给 FFI / 测试用的公开类型
pub use inventory::{Inventory, InventorySnapshot, Reach};
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
    pub fn discover_sessions(
        &mut self,
        transport_id: &str,
        target: &str,
    ) -> anyhow::Result<Vec<SessionCandidate>> {
        let connect = match self.connect(transport_id, target) {
            Ok(c) => c,
            Err(_) => return Ok(Vec::new()),
        };
        let mut out = Vec::new();
        for driver in &self.runtimes {
            if !driver.accepted_transports().contains(&transport_id) {
                continue;
            }
            match driver.list(&connect, None) {
                Ok(mut rows) => out.append(&mut rows),
                Err(_) => continue,
            }
        }
        Ok(out)
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

#[cfg(test)]
mod tests;
