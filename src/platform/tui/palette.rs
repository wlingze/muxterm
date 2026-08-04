//! 连接向导（opencode 风格悬浮面板）。
//!
//! 多步流程：
//!   1. 选来源：local / ssh
//!   2. （ssh）选机器（`~/.ssh/config` 的 Host alias）
//!   3. 选动作：顶部默认「new（创建新会话）」，下面是已存在的 tmux session 可直接 attach
//!   4. （new）选起始目录
//!
//! 结束时返回一个 [`ConnectAction`]，由 `app.rs` 据此重连 / 创建。
//!
//! 纯逻辑模块（不碰 FFI / IO），目录与主机列表由外部注入，方便单元测试。

use ratatui::widgets::ListState;

/// 连接来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectSource {
    Local,
    Ssh,
}

/// 向导步骤。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardStep {
    /// 选来源（local / ssh）。
    Source,
    /// 选机器（仅 ssh）。
    Host,
    /// 选动作：new 或 attach 某 session。
    Action,
    /// 选起始目录（仅 new）。
    Directory,
}

impl WizardStep {
    pub fn title(&self) -> &'static str {
        match self {
            WizardStep::Source => "选择来源",
            WizardStep::Host => "选择 SSH 机器",
            WizardStep::Action => "选择会话（new 创建 / 选中 attach）",
            WizardStep::Directory => "选择起始目录",
        }
    }
}

/// 向导最终动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectAction {
    /// attach 到已存在会话。
    Attach {
        source: ConnectSource,
        /// ssh 主机 alias（source==Ssh 时有值）。
        host: Option<String>,
        session: String,
    },
    /// 创建新会话并 attach。
    New {
        source: ConnectSource,
        host: Option<String>,
        /// 起始目录（None = 用默认 / 当前目录）。
        directory: Option<String>,
    },
}

/// 向导可选项（每个 step 的列表项）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WizardItem {
    /// 显示文本。
    pub label: String,
    /// 是否「目录项」（Directory step 里可进入 / 回到上级）。
    pub is_dir: bool,
    /// 该条目对应的原始值（session 名 / host alias / 目录路径）。
    pub value: String,
    /// 是否为「new（创建）」特殊项（仅 Action step 顶部）。
    pub is_new: bool,
}

impl WizardItem {
    pub fn plain(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            is_dir: false,
            is_new: false,
        }
    }
    pub fn dir(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            is_dir: true,
            is_new: false,
        }
    }
    pub fn new_item() -> Self {
        Self {
            label: "new（创建新会话）".into(),
            value: String::new(),
            is_dir: false,
            is_new: true,
        }
    }
}

/// 向导状态。
#[derive(Debug, Clone)]
pub struct PaletteState {
    pub step: WizardStep,
    pub source: ConnectSource,
    /// 选中的 ssh host。
    pub host: Option<String>,
    /// 当前目录路径（Directory step 的起点）。
    pub dir: Option<String>,
    /// 当前 tmux socket（`-L`），None = 默认 socket。
    pub socket: Option<String>,
    /// 当前 step 的列表项。
    pub items: Vec<WizardItem>,
    pub list: ListState,
    /// 是否完成（拿到动作后置 true）。
    pub done: bool,
    /// 完成时产生的动作。
    pub action: Option<ConnectAction>,
}

impl Default for PaletteState {
    fn default() -> Self {
        Self::new()
    }
}

impl PaletteState {
    pub fn new() -> Self {
        let mut list = ListState::default();
        list.select(Some(0));
        Self {
            step: WizardStep::Source,
            source: ConnectSource::Local,
            host: None,
            dir: None,
            socket: None,
            items: vec![
                WizardItem::plain("local（本机 tmux）", "local"),
                WizardItem::plain("ssh（远程机器）", "ssh"),
            ],
            list,
            done: false,
            action: None,
        }
    }

    /// 设置当前 step 的列表项并重置选中。
    pub fn set_items(&mut self, items: Vec<WizardItem>) {
        self.items = items;
        self.list.select(Some(0));
    }

    /// 当前选中项。
    pub fn selected(&self) -> Option<&WizardItem> {
        self.items.get(self.list.selected().unwrap_or(0))
    }

    /// 按当前 step 处理 Enter。
    ///
    /// 返回 `Some(())` 表示完成了向导（拿到动作）。
    pub fn advance(&mut self) -> Option<ConnectAction> {
        if self.done {
            return self.action.clone();
        }
        let item = self.selected()?.clone();
        match self.step {
            WizardStep::Source => {
                self.source = if item.value == "ssh" {
                    ConnectSource::Ssh
                } else {
                    ConnectSource::Local
                };
                if self.source == ConnectSource::Ssh {
                    // 进入选机器
                    self.step = WizardStep::Host;
                    self.set_items(vec![]);
                } else {
                    // local：直接进入选动作
                    self.step = WizardStep::Action;
                }
            }
            WizardStep::Host => {
                self.host = Some(item.value);
                self.step = WizardStep::Action;
            }
            WizardStep::Action => {
                if item.is_new {
                    // 进入选目录
                    self.step = WizardStep::Directory;
                } else {
                    // attach
                    let action = ConnectAction::Attach {
                        source: self.source,
                        host: self.host.clone(),
                        session: item.value,
                    };
                    self.finish(action);
                }
            }
            WizardStep::Directory => {
                // 选中的是目录 → new 会话
                let action = ConnectAction::New {
                    source: self.source,
                    host: self.host.clone(),
                    directory: Some(item.value),
                };
                self.finish(action);
            }
        }
        if self.done {
            self.action.clone()
        } else {
            None
        }
    }

    /// 返回上一步。
    pub fn back(&mut self) {
        self.done = false;
        self.action = None;
        match self.step {
            WizardStep::Source => { /* 已是最上 */ }
            WizardStep::Host => {
                self.step = WizardStep::Source;
                self.set_items(vec![
                    WizardItem::plain("local（本机 tmux）", "local"),
                    WizardItem::plain("ssh（远程机器）", "ssh"),
                ]);
            }
            WizardStep::Action => {
                if self.source == ConnectSource::Ssh {
                    self.step = WizardStep::Host;
                } else {
                    self.step = WizardStep::Source;
                }
            }
            WizardStep::Directory => {
                self.step = WizardStep::Action;
            }
        }
    }

    fn finish(&mut self, action: ConnectAction) {
        self.done = true;
        self.action = Some(action);
    }
}

/// 模糊匹配（子串或子序列，大小写不敏感）。
pub fn fuzzy_match(query: &str, target: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();
    let t = target.to_lowercase();
    if t.contains(&q) {
        return true;
    }
    let mut ti = t.chars();
    for qc in q.chars() {
        loop {
            match ti.next() {
                Some(tc) if tc == qc => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}

/// 按查询过滤列表项。
pub fn filter_items(items: &[WizardItem], query: &str) -> Vec<WizardItem> {
    if query.is_empty() {
        return items.to_vec();
    }
    items
        .iter()
        .filter(|i| fuzzy_match(query, &i.label))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_wizard_attach() {
        let mut p = PaletteState::new();
        // step Source: select local (index 0) + Enter
        p.advance();
        assert_eq!(p.step, WizardStep::Action);
        // inject sessions
        p.set_items(vec![
            WizardItem::new_item(),
            WizardItem::plain("dev", "dev"),
            WizardItem::plain("prod", "prod"),
        ]);
        // select "prod" (index 2) + Enter
        p.list.select(Some(2));
        let action = p.advance();
        assert_eq!(
            action,
            Some(ConnectAction::Attach {
                source: ConnectSource::Local,
                host: None,
                session: "prod".into(),
            })
        );
        assert!(p.done);
    }

    #[test]
    fn ssh_wizard_new_with_dir() {
        let mut p = PaletteState::new();
        // Source: ssh
        p.list.select(Some(1));
        p.advance();
        assert_eq!(p.step, WizardStep::Host);
        // inject hosts
        p.set_items(vec![
            WizardItem::plain("server-a", "server-a"),
            WizardItem::plain("server-b", "server-b"),
        ]);
        p.list.select(Some(0));
        p.advance();
        assert_eq!(p.step, WizardStep::Action);
        // Action: new
        p.set_items(vec![
            WizardItem::new_item(),
            WizardItem::plain("existing", "existing"),
        ]);
        p.list.select(Some(0)); // new
        p.advance();
        assert_eq!(p.step, WizardStep::Directory);
        // Directory: pick a dir
        p.set_items(vec![
            WizardItem::dir("..", ".."),
            WizardItem::dir("~/work", "/home/u/work"),
        ]);
        p.list.select(Some(1));
        let action = p.advance();
        assert_eq!(
            action,
            Some(ConnectAction::New {
                source: ConnectSource::Ssh,
                host: Some("server-a".into()),
                directory: Some("/home/u/work".into()),
            })
        );
        assert!(p.done);
    }

    #[test]
    fn back_from_action_goes_to_host_for_ssh() {
        let mut p = PaletteState::new();
        p.list.select(Some(1)); // ssh
        p.advance();
        p.set_items(vec![WizardItem::plain("host-a", "host-a")]);
        p.advance();
        assert_eq!(p.step, WizardStep::Action);
        p.back();
        assert_eq!(p.step, WizardStep::Host);
    }

    #[test]
    fn filter_items_matches_subsequence() {
        let items = vec![
            WizardItem::plain("dev session", "dev"),
            WizardItem::plain("prod session", "prod"),
        ];
        assert_eq!(filter_items(&items, "dvs").len(), 1);
        assert_eq!(filter_items(&items, "prod").len(), 1);
        assert!(filter_items(&items, "").len() == 2);
    }
}
