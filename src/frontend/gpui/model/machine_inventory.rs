//! 快速面板的「按机器」目录发现状态。
//!
//! 一轮 discovery 按机器展开：本机一台，`~/.ssh/config` 的每个别名一台。
//! 慢主机不能挡住已经回来的机器，所以每台机器单独回包、单独替换；某台还在
//! 路上时，其余机器上一轮的结果继续可见，界面用「加载中」标出它。
//!
//! 这里只有纯状态：Core 调用与 UI 通知在 `state/picker.rs`。

use crate::frontend::gpui::model::existing::ExistingRuntime;
use crate::frontend::gpui::model::target::TargetTransport;

/// 一台机器的发现阶段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MachinePhase {
    /// 这一轮还在路上（可能有上一轮的结果）。
    Loading,
    Loaded,
    /// 这一轮失败；错误只用于日志/调试，界面只显示「未能检查」。
    Failed(String),
}

impl MachinePhase {
    pub fn is_loading(&self) -> bool {
        matches!(self, Self::Loading)
    }
}

/// 一台机器（本机或一个 SSH 别名）与它最近一次的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineEntry {
    pub transport: TargetTransport,
    pub phase: MachinePhase,
    rows: Vec<ExistingRuntime>,
}

impl MachineEntry {
    pub fn rows(&self) -> &[ExistingRuntime] {
        &self.rows
    }

    pub fn is_loading(&self) -> bool {
        self.phase.is_loading()
    }
}

/// 一轮按机器展开的 discovery。
///
/// 顺序由 `begin` 的机器清单决定：本机在前，其余按 ssh config 顺序。
/// 清单里没有的机器在下一轮被丢掉，但本轮迟到的回包仍然会补进来（宁可多一行，
/// 也不要把刚发现的结果丢掉）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MachineInventory {
    entries: Vec<MachineEntry>,
}

impl MachineInventory {
    /// 规划新一轮。已有的机器保留上一次的结果，只把阶段改回 `Loading`；
    /// 新机器从空列表开始。
    pub fn begin(&mut self, transports: &[TargetTransport]) {
        let mut entries = Vec::with_capacity(transports.len());
        for transport in transports {
            let previous = self
                .entries
                .iter()
                .position(|entry| &entry.transport == transport);
            match previous {
                Some(index) => {
                    let mut entry = self.entries[index].clone();
                    entry.phase = MachinePhase::Loading;
                    entries.push(entry);
                }
                None => entries.push(MachineEntry {
                    transport: transport.clone(),
                    phase: MachinePhase::Loading,
                    rows: Vec::new(),
                }),
            }
        }
        self.entries = entries;
    }

    /// 一台机器回包：只替换这台机器的结果，其他机器保持原样。
    pub fn finish(&mut self, transport: &TargetTransport, rows: Vec<ExistingRuntime>) {
        let entry = self.entry_mut(transport);
        entry.rows = rows;
        entry.phase = MachinePhase::Loaded;
    }

    /// 一台机器这一轮失败。保留上一轮的行：attach 仍可能成功，而且比整段
    /// 列表突然消失更容易理解；阶段标出来让界面知道它没被检查完。
    pub fn fail(&mut self, transport: &TargetTransport, error: String) {
        let entry = self.entry_mut(transport);
        entry.phase = MachinePhase::Failed(error);
    }

    /// 外部整表注入（测试替身 / 兼容旧路径）：按机器分组，全部当作已加载。
    pub fn loaded(rows: Vec<ExistingRuntime>) -> Self {
        let mut inventory = Self::default();
        for row in rows {
            let transport = row.transport_key();
            match inventory
                .entries
                .iter_mut()
                .find(|entry| entry.transport == transport)
            {
                Some(entry) => entry.rows.push(row),
                None => inventory.entries.push(MachineEntry {
                    transport,
                    phase: MachinePhase::Loaded,
                    rows: vec![row],
                }),
            }
        }
        inventory
    }

    pub fn entries(&self) -> &[MachineEntry] {
        &self.entries
    }

    /// 扁平行列表：顺序与 `entries()` 一致。
    pub fn rows(&self) -> Vec<ExistingRuntime> {
        self.entries
            .iter()
            .flat_map(|entry| entry.rows.iter().cloned())
            .collect()
    }

    /// 还有机器在路上。
    pub fn is_pending(&self) -> bool {
        self.entries.iter().any(MachineEntry::is_loading)
    }

    /// 有多少台机器这一轮没检查成功。
    pub fn failed_machines(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| matches!(entry.phase, MachinePhase::Failed(_)))
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn entry_mut(&mut self, transport: &TargetTransport) -> &mut MachineEntry {
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| &entry.transport == transport)
        {
            return &mut self.entries[index];
        }
        // 清单里没有这台机器（ssh config 在发现途中改了）：本轮迟到就补在末尾，
        // 不能因为规划时没看见它就把结果丢掉。
        self.entries.push(MachineEntry {
            transport: transport.clone(),
            phase: MachinePhase::Loaded,
            rows: Vec::new(),
        });
        self.entries.last_mut().expect("just pushed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime(machine: &str, session: &str) -> ExistingRuntime {
        let ssh = machine != "local";
        ExistingRuntime {
            name: session.into(),
            runtime: "tmux".into(),
            transport: if ssh { "ssh".into() } else { "local".into() },
            target: if ssh { machine.into() } else { String::new() },
            session: session.into(),
            socket: None,
            windows: 1,
            attached: false,
            workspace_id: None,
            path: String::new(),
        }
    }

    fn ssh(name: &str) -> TargetTransport {
        TargetTransport::Ssh { name: name.into() }
    }

    #[test]
    fn begin_keeps_the_previous_rows_while_refreshing() {
        let mut inventory = MachineInventory::default();
        inventory.begin(&[TargetTransport::Local, ssh("ryzen")]);
        inventory.finish(&TargetTransport::Local, vec![runtime("local", "demo")]);
        inventory.finish(&ssh("ryzen"), vec![runtime("ryzen", "api")]);

        inventory.begin(&[TargetTransport::Local, ssh("ryzen")]);

        assert!(inventory.is_pending(), "刷新开始后每台机器都在路上");
        assert_eq!(inventory.rows().len(), 2, "刷新期间旧结果继续可见");
    }

    #[test]
    fn a_result_replaces_only_its_own_machine() {
        let mut inventory = MachineInventory::default();
        inventory.begin(&[TargetTransport::Local, ssh("ryzen")]);
        inventory.finish(&TargetTransport::Local, vec![runtime("local", "demo")]);
        inventory.finish(&ssh("ryzen"), vec![runtime("ryzen", "api")]);

        // 远端回包不能动本机结果。
        inventory.finish(&ssh("ryzen"), vec![runtime("ryzen", "web")]);
        let rows = inventory.rows();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|row| row.session == "demo"));
        assert!(rows.iter().any(|row| row.session == "web"));
        assert!(!rows.iter().any(|row| row.session == "api"));
    }

    #[test]
    fn pending_machines_clear_one_by_one() {
        let mut inventory = MachineInventory::default();
        inventory.begin(&[TargetTransport::Local, ssh("slow"), ssh("slower")]);
        inventory.finish(&TargetTransport::Local, vec![runtime("local", "demo")]);
        assert!(inventory.is_pending(), "两台远端仍未回包");

        // 下一轮清单只剩本机：清单外的机器不再等待。
        inventory.begin(&[TargetTransport::Local]);
        assert_eq!(
            inventory.entries().len(),
            1,
            "清单外的机器不再占位：{:?}",
            inventory.entries()
        );
        assert!(inventory.rows().iter().any(|row| row.session == "demo"));
    }

    #[test]
    fn a_failed_machine_keeps_its_previous_rows() {
        let mut inventory = MachineInventory::default();
        inventory.begin(&[TargetTransport::Local, ssh("ryzen")]);
        inventory.finish(&TargetTransport::Local, vec![runtime("local", "demo")]);
        inventory.finish(&ssh("ryzen"), vec![runtime("ryzen", "api")]);
        inventory.fail(&ssh("ryzen"), "ssh: connect timed out".into());

        assert_eq!(inventory.failed_machines(), 1);
        assert!(!inventory.is_pending());
        assert_eq!(inventory.rows().len(), 2, "失败不清空上一次的结果");
    }

    #[test]
    fn loaded_groups_rows_by_machine_in_first_seen_order() {
        let inventory = MachineInventory::loaded(vec![
            runtime("ryzen", "api"),
            runtime("local", "demo"),
            runtime("ryzen", "web"),
        ]);
        let labels: Vec<_> = inventory
            .entries()
            .iter()
            .map(|entry| entry.transport.label().to_string())
            .collect();
        assert_eq!(labels, ["ryzen", "local"]);
        assert_eq!(inventory.entries()[0].rows().len(), 2, "同机器结果不拆组");
    }

    #[test]
    fn a_late_result_for_an_unplanned_machine_is_kept() {
        let mut inventory = MachineInventory::default();
        inventory.begin(&[TargetTransport::Local]);
        inventory.finish(&ssh("new-host"), vec![runtime("new-host", "api")]);
        assert_eq!(inventory.entries().len(), 2);
        let rows = inventory.rows();
        assert_eq!(rows.len(), 1, "本机还没有结果：{rows:?}");
        assert_eq!(rows[0].target, "new-host");
    }
}
