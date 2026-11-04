//! 目标配置窗口的保存规则。
//!
//! 移植自 macOS `App/TargetConfigWindow.swift`：编辑已有 Project 时保留 attach
//! 身份（session / socket / workspace），新建时按 manifest 与路径派生默认值。
//! 只有保存规则是纯逻辑，控件留给 `chrome/`。

use crate::frontend::gpui::model::directory::{DirectoryPathModel, DirectorySuggestionController};
use crate::frontend::gpui::model::quick_connect::{QuickConnect, TargetConfig};
use crate::frontend::gpui::model::target::{TargetOptionSelection, TargetRuntime, TargetTransport};

/// 保存被拒绝的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveError {
    /// SSH 传输没有填 Host alias。
    MissingSshHost,
}

impl SaveError {
    /// 弹窗正文，与 macOS 文案一致。
    pub fn message(self) -> &'static str {
        match self {
            Self::MissingSshHost => "Select or enter an SSH Host before saving.",
        }
    }
}

/// 编辑中的输入焦点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetEditorFocus {
    Path,
    Name,
    SshAlias,
}

impl TargetEditorFocus {
    /// Tab 循环顺序：路径 → 名字 → SSH Host（不可见时跳过）→ 路径。
    pub fn next(self, ssh_visible: bool) -> Self {
        match (self, ssh_visible) {
            (Self::Path, _) => Self::Name,
            (Self::Name, true) => Self::SshAlias,
            (Self::Name, false) => Self::Path,
            (Self::SshAlias, _) => Self::Path,
        }
    }

    pub fn previous(self, ssh_visible: bool) -> Self {
        match (self, ssh_visible) {
            (Self::Path, true) => Self::SshAlias,
            (Self::Path, false) => Self::Name,
            (Self::Name, _) => Self::Path,
            (Self::SshAlias, _) => Self::Name,
        }
    }
}

/// 目标编辑器的草稿。
#[derive(Debug, Clone)]
pub struct TargetEditor {
    editing: Option<TargetConfig>,
    pub selection: TargetOptionSelection,
    ssh_alias: String,
    name: String,
    path: DirectorySuggestionController,
    /// 用户手改过 name；改过就不再跟着路径自动更新。
    name_manually_edited: bool,
}

impl Default for TargetEditor {
    fn default() -> Self {
        Self::new_project()
    }
}

impl TargetEditor {
    /// 新建：路径从 `~` 开始，name 跟随路径。
    pub fn new_project() -> Self {
        Self {
            editing: None,
            selection: TargetOptionSelection::new(),
            ssh_alias: String::new(),
            name: String::new(),
            path: DirectorySuggestionController::new("~"),
            name_manually_edited: false,
        }
    }

    /// 编辑已有目标：回填 runtime/transport/alias/name，并视为手改过 name。
    pub fn editing(config: &TargetConfig) -> Self {
        let mut editor = Self {
            editing: Some(config.clone()),
            selection: TargetOptionSelection {
                runtime: config.runtime,
                transport: config.transport.clone(),
            },
            ssh_alias: match &config.transport {
                TargetTransport::Ssh { name } => name.clone(),
                TargetTransport::Local => String::new(),
            },
            name: config.name.clone(),
            path: DirectorySuggestionController::new(if config.path.is_empty() {
                "~"
            } else {
                &config.path
            }),
            name_manually_edited: true,
        };
        if let TargetTransport::Ssh { name } = &config.transport {
            editor.path.set_transport(true, Some(name));
        }
        editor
    }

    pub fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn path_text(&self) -> &str {
        self.path.text()
    }

    pub fn path(&self) -> &DirectorySuggestionController {
        &self.path
    }

    pub fn ssh_alias(&self) -> &str {
        &self.ssh_alias
    }

    pub fn selection(&self) -> &TargetOptionSelection {
        &self.selection
    }

    /// 运行时卡片的一行小字。
    pub fn runtime_subtitle(runtime: TargetRuntime) -> &'static str {
        match runtime {
            TargetRuntime::Tmux => "attach/create tmux",
            TargetRuntime::Herdr => "attach/create Herdr workspace",
            TargetRuntime::Shell => "plain shell",
        }
    }

    /// 输入框下的默认名提示。
    pub fn name_hint(&self) -> String {
        format!(
            "默认: {}",
            QuickConnect::default_name(&DirectoryPathModel::resolved_path(self.path.text()))
        )
    }

    /// 路径变化：输入框跟着走；name 没被手改过就一起更新。
    pub fn set_path(&mut self, text: &str) {
        self.path.update_input(text);
        if !self.name_manually_edited {
            self.name =
                QuickConnect::default_name(&DirectoryPathModel::resolved_path(self.path.text()));
        }
    }

    /// 手改 name 之后不再自动跟随路径。
    pub fn set_name(&mut self, name: &str) {
        self.name = name.to_string();
        self.name_manually_edited = true;
    }

    pub fn select_runtime(&mut self, runtime: TargetRuntime) {
        self.selection.select_runtime(runtime);
    }

    /// 切到 SSH 时从远端 home 开始浏览，和 macOS 一致。
    pub fn select_transport(&mut self, transport: TargetTransport) {
        let ssh_alias = match &transport {
            TargetTransport::Ssh { name } => Some(name.clone()),
            TargetTransport::Local => None,
        };
        self.selection.select_transport(transport);
        match ssh_alias {
            Some(alias) => {
                self.path.set_transport(true, Some(&alias));
                if self.path.text() == "~" || self.path.text().is_empty() {
                    self.path.update_input("~");
                }
            }
            None => {
                self.path.set_transport(false, None);
            }
        }
    }

    /// SSH Host 输入框只在 SSH 传输下显示。
    pub fn ssh_field_visible(&self) -> bool {
        self.selection.transport.is_ssh()
    }

    /// 输入 SSH Host；同时把地址交给目录浏览。
    pub fn set_ssh_alias(&mut self, alias: &str) {
        self.ssh_alias = alias.to_string();
        let trimmed = alias.trim();
        self.path.set_transport(
            true,
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            },
        );
    }

    /// 往当前焦点字段追加文本（按键与 IME 提交共用）。
    pub fn append(&mut self, focus: TargetEditorFocus, text: &str) {
        match focus {
            TargetEditorFocus::Path => {
                let next = format!("{}{text}", self.path.text());
                self.set_path(&next);
            }
            TargetEditorFocus::Name => {
                let next = format!("{}{text}", self.name);
                self.set_name(&next);
            }
            TargetEditorFocus::SshAlias => {
                let next = format!("{}{text}", self.ssh_alias);
                self.set_ssh_alias(&next);
            }
        }
    }

    /// 退格删掉当前焦点字段最后一个字符。
    pub fn backspace(&mut self, focus: TargetEditorFocus) {
        let pop = |value: &str| {
            let mut chars: Vec<char> = value.chars().collect();
            chars.pop();
            chars.into_iter().collect::<String>()
        };
        match focus {
            TargetEditorFocus::Path => {
                let next = pop(self.path.text());
                self.set_path(&next);
            }
            TargetEditorFocus::Name => {
                let next = pop(&self.name);
                self.set_name(&next);
            }
            TargetEditorFocus::SshAlias => {
                let next = pop(&self.ssh_alias);
                self.set_ssh_alias(&next);
            }
        }
    }

    /// 焦点字段的当前文本。
    pub fn focus_text(&self, focus: TargetEditorFocus) -> String {
        match focus {
            TargetEditorFocus::Path => self.path.text().to_string(),
            TargetEditorFocus::Name => self.name.clone(),
            TargetEditorFocus::SshAlias => self.ssh_alias.clone(),
        }
    }

    /// 保存：校验 → 归一化 → 需要时保留 attach 身份。
    pub fn save(&self) -> Result<TargetConfig, SaveError> {
        let transport = match &self.selection.transport {
            TargetTransport::Ssh { .. } => {
                let name = self.ssh_alias.trim();
                if name.is_empty() {
                    return Err(SaveError::MissingSshHost);
                }
                TargetTransport::Ssh {
                    name: name.to_string(),
                }
            }
            TargetTransport::Local => TargetTransport::Local,
        };

        let path = DirectoryPathModel::resolved_path(self.path.text());
        let mut name = self.name.trim().to_string();
        if name.is_empty() {
            name = QuickConnect::default_name(&path);
        }
        let preserves_identity = self.editing.as_ref().is_some_and(|editing| {
            editing.runtime == self.selection.runtime && editing.transport == transport
        });
        let editing = self.editing.as_ref();
        Ok(TargetConfig {
            name,
            runtime: self.selection.runtime,
            transport,
            path,
            session: if preserves_identity {
                editing.and_then(|editing| editing.session.clone())
            } else {
                None
            },
            socket: if preserves_identity {
                editing.and_then(|editing| editing.socket.clone())
            } else {
                None
            },
            workspace_id: if preserves_identity {
                editing.and_then(|editing| editing.workspace_id.clone())
            } else {
                None
            },
            persisted_id: editing.and_then(|editing| editing.persisted_id.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssh_project() -> TargetConfig {
        TargetConfig {
            name: "api".to_string(),
            runtime: TargetRuntime::Herdr,
            transport: TargetTransport::Ssh {
                name: "ryzen".to_string(),
            },
            path: "/srv/api".to_string(),
            session: Some("named".to_string()),
            socket: Some("/run/herdr.sock".to_string()),
            workspace_id: Some("w3".to_string()),
            persisted_id: Some("project-1".to_string()),
        }
    }

    /// 新建时路径是 `~`，name 跟随路径。
    #[test]
    fn new_project_starts_at_home_with_a_derived_name() {
        let mut editor = TargetEditor::new_project();
        assert_eq!(editor.path_text(), "~");
        assert_eq!(editor.name(), "", "新建时 name 留空，由提示给默认名");
        assert_eq!(
            editor.name_hint(),
            "默认: ~",
            "`~` 本身没有目录段，Swift 也把 ~ 当最后一段"
        );
        editor.set_path("/Users/wlz/Developer/muxterm");
        assert_eq!(editor.name(), "muxterm");
        assert_eq!(editor.name_hint(), "默认: muxterm");
        assert!(!editor.ssh_field_visible());
    }

    /// 手改 name 之后路径不再改它（对照 `TargetConfigWindow.autoUpdateNameIfNeeded`）。
    #[test]
    fn a_manually_edited_name_stops_following_the_path() {
        let mut editor = TargetEditor::new_project();
        editor.set_name("我的项目");
        editor.set_path("/Users/wlz/Developer/muxterm");
        assert_eq!(editor.name(), "我的项目");
        assert_eq!(
            editor.name_hint(),
            "默认: muxterm",
            "提示始终显示路径默认名"
        );
    }

    /// 对照 `SettingsWindowE2ETests.testProjectEditorPreservesSSHHostWhenEditing`。
    #[test]
    fn editing_an_ssh_project_keeps_the_host_and_the_attach_identity() {
        let project = ssh_project();
        let mut editor = TargetEditor::editing(&project);
        assert_eq!(editor.ssh_alias(), "ryzen");
        assert!(editor.ssh_field_visible());
        assert_eq!(editor.name(), "api");
        assert_eq!(editor.path_text(), "/srv/api");

        // 只改路径，identity 不变。
        editor.set_path("/srv/api-v2");
        let saved = editor.save().expect("save");
        assert_eq!(saved.path, "/srv/api-v2");
        assert_eq!(saved.transport, project.transport, "SSH alias 不能被清空");
        assert_eq!(saved.session, project.session);
        assert_eq!(saved.socket, project.socket);
        assert_eq!(saved.workspace_id, project.workspace_id);
        assert_eq!(
            QuickConnect::unique_id(&saved),
            QuickConnect::unique_id(&project),
            "attach identity 必须保持不变"
        );
    }

    /// 切到别的 runtime / transport 就是新目标，不再保留旧 identity。
    #[test]
    fn switching_runtime_or_transport_drops_the_old_identity() {
        let project = ssh_project();
        let mut editor = TargetEditor::editing(&project);
        editor.select_transport(TargetTransport::Local);
        editor.set_path("/Users/wlz/Developer/muxterm");
        let saved = editor.save().expect("save");
        assert_eq!(saved.transport, TargetTransport::Local);
        assert_eq!(saved.session, None);
        assert_eq!(saved.socket, None);
        assert_eq!(saved.workspace_id, None);
        assert_eq!(
            saved.persisted_id,
            Some("project-1".to_string()),
            "项目 id 仍然带上"
        );
        assert_eq!(
            saved.name, "api",
            "name 在编辑态是手改过的，改路径不会覆盖它"
        );
    }

    /// SSH 传输没填 Host 时拒绝保存。
    #[test]
    fn ssh_without_a_host_is_rejected() {
        let mut editor = TargetEditor::new_project();
        editor.select_transport(TargetTransport::Ssh {
            name: String::new(),
        });
        editor.set_path("/srv/api");
        assert_eq!(editor.save(), Err(SaveError::MissingSshHost));
        assert_eq!(
            SaveError::MissingSshHost.message(),
            "Select or enter an SSH Host before saving."
        );

        editor.set_ssh_alias("  ryzen  ");
        let saved = editor.save().expect("save");
        assert_eq!(
            saved.transport,
            TargetTransport::Ssh {
                name: "ryzen".to_string()
            },
            "alias 要 trim"
        );
        assert_eq!(
            saved.runtime,
            TargetRuntime::Tmux,
            "runtime 不因 SSH 自动改"
        );
    }

    /// 空 name 退回路径默认名；路径带 `.` / `..` 时先归一化。
    #[test]
    fn saving_normalizes_the_path_and_falls_back_to_the_default_name() {
        let mut editor = TargetEditor::new_project();
        editor.set_path("~/a/../muxterm/");
        let saved = editor.save().expect("save");
        assert_eq!(saved.path, "~/muxterm");
        assert_eq!(saved.name, "muxterm");

        editor.set_name("   ");
        let saved = editor.save().expect("save");
        assert_eq!(saved.name, "muxterm", "全空白的 name 用默认名");
    }

    /// Tab 顺序跳过隐藏的 SSH 字段；打字与退格都作用于焦点字段。
    #[test]
    fn focus_cycles_and_edits_the_focused_field_only() {
        assert_eq!(TargetEditorFocus::Path.next(false), TargetEditorFocus::Name);
        assert_eq!(TargetEditorFocus::Name.next(false), TargetEditorFocus::Path);
        assert_eq!(
            TargetEditorFocus::Name.next(true),
            TargetEditorFocus::SshAlias
        );
        assert_eq!(
            TargetEditorFocus::SshAlias.next(true),
            TargetEditorFocus::Path
        );
        assert_eq!(
            TargetEditorFocus::Path.previous(true),
            TargetEditorFocus::SshAlias
        );
        assert_eq!(
            TargetEditorFocus::Path.previous(false),
            TargetEditorFocus::Name
        );

        let mut editor = TargetEditor::new_project();
        editor.append(TargetEditorFocus::Name, "api");
        assert_eq!(editor.name(), "api");
        assert_eq!(editor.path_text(), "~", "改名字不能动路径");
        editor.backspace(TargetEditorFocus::Name);
        assert_eq!(editor.name(), "ap");
        editor.append(TargetEditorFocus::Path, "/srv");
        assert_eq!(editor.path_text(), "~/srv", "追加在现有文本后");
        assert_eq!(editor.name(), "ap", "手改过的名字不再跟随路径");
    }

    /// SSH Host 字段只在 SSH 传输下可编辑。
    #[test]
    fn ssh_alias_editing_updates_the_directory_request() {
        let mut editor = TargetEditor::new_project();
        editor.set_ssh_alias("ryz");
        editor.select_transport(TargetTransport::Ssh {
            name: "ryz".to_string(),
        });
        editor.append(TargetEditorFocus::SshAlias, "en");
        assert_eq!(editor.ssh_alias(), "ryzen", "打字只动 Host 字段");
        assert_eq!(editor.path().request().alias.as_deref(), Some("ryzen"));
    }

    /// 切到 SSH 时目录浏览改走远端，候选前缀保持不变。
    #[test]
    fn ssh_transport_points_the_directory_browser_at_the_remote_host() {
        let mut editor = TargetEditor::new_project();
        assert!(!editor.path().request().is_ssh);
        editor.select_transport(TargetTransport::Ssh {
            name: "ryzen".to_string(),
        });
        assert!(editor.path().request().is_ssh);
        assert_eq!(editor.path().request().alias.as_deref(), Some("ryzen"));

        editor.select_transport(TargetTransport::Local);
        assert!(!editor.path().request().is_ssh);
        assert_eq!(editor.path().request().alias, None);
    }
}
