//! Core-owned configuration document, schema, validation and draft transactions.
//!
//! `core::config` remains the compatibility facade used by the existing GTK code.
//! New callers should use this module.  Keeping the service separate for the first
//! migration step lets the old UI move to the transaction API without a flag day.

use anyhow::{anyhow, Context, Result};
use schemars::schema_for;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

use crate::core::config::{Config, KeyBinding};
use crate::core::quickconnect::model::{TargetConfig, TargetRuntime, TargetTransport};

pub const CONFIG_VERSION: u32 = 1;

/// The persisted, versioned configuration document.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
pub struct ConfigDocument {
    #[serde(default = "default_config_version")]
    pub config_version: u32,
    #[serde(flatten)]
    pub config: Config,
    #[serde(default)]
    pub projects: Vec<ProjectDocument>,
    #[serde(default)]
    pub shortcuts: ShortcutConfig,
    #[serde(default)]
    pub platform: PlatformConfig,
    #[serde(default)]
    pub extensions: BTreeMap<String, Value>,
}

fn default_config_version() -> u32 {
    CONFIG_VERSION
}

impl Default for ConfigDocument {
    fn default() -> Self {
        let mut config = Config::default();
        // These are the new service defaults. The compatibility Config facade is
        // intentionally left unchanged until the platform consumers migrate.
        config.font.family = "JetBrains Mono".into();
        config.font.size = 13.0;
        config.font.fallback = vec!["Noto Sans Mono".into(), "monospace".into()];
        config.theme.name = "system".into();
        config.theme.light = "white".into();
        config.theme.dark = "black".into();
        Self {
            config_version: CONFIG_VERSION,
            config,
            projects: Vec::new(),
            shortcuts: ShortcutConfig::default(),
            platform: PlatformConfig::default(),
            extensions: BTreeMap::new(),
        }
    }
}

impl ConfigDocument {
    pub fn from_toml(raw: &str) -> Result<Self> {
        let value: toml::Value = raw.parse().context("配置 TOML 解析失败")?;
        validate_toml_shape(&value)?;
        let mut document: Self = toml::from_str(raw).context("配置文档反序列化失败")?;
        document.normalize_legacy_defaults(raw);
        document.validate()?;
        Ok(document)
    }

    pub fn to_toml(&self) -> Result<String> {
        toml::to_string_pretty(self).context("配置文档序列化失败")
    }

    pub fn validate(&self) -> Result<()> {
        if self.config_version > CONFIG_VERSION {
            return Err(anyhow!(
                "不支持的 config_version={}，当前最高版本为 {}",
                self.config_version,
                CONFIG_VERSION
            ));
        }
        if !(9.0..=72.0).contains(&self.config.font.size) {
            return Err(anyhow!("font.size 必须在 9 到 72 之间"));
        }
        if self.config.scrollback.lines == 0 {
            return Err(anyhow!("scrollback.lines 必须大于 0"));
        }
        if self.config.pool.max_slots == 0 {
            return Err(anyhow!("pool.max_slots 必须大于 0"));
        }
        if self.config.theme.name.trim().is_empty() {
            return Err(anyhow!("theme.name 不能为空"));
        }
        if !matches!(self.config.statusbar.mode.as_str(), "tmux" | "theme") {
            return Err(anyhow!("statusbar.mode 只能是 tmux 或 theme"));
        }
        self.validate_projects()?;
        self.validate_shortcuts()?;
        Ok(())
    }

    fn normalize_legacy_defaults(&mut self, raw: &str) {
        if !raw.lines().any(|line| line.trim() == "[font]") {
            self.config.font.family = "JetBrains Mono".into();
            self.config.font.size = 13.0;
            self.config.font.fallback = vec!["Noto Sans Mono".into(), "monospace".into()];
        } else if !raw
            .lines()
            .any(|line| line.trim_start().starts_with("family"))
        {
            self.config.font.family = "JetBrains Mono".into();
        }
        if !raw.lines().any(|line| line.trim() == "[theme]") {
            self.config.theme.name = "system".into();
        }
        if self.config.theme.name == "light" {
            self.config.theme.name = "white".into();
        } else if self.config.theme.name == "dark" {
            self.config.theme.name = "black".into();
        }
        if self.config.keybindings.is_empty()
            && raw.lines().all(|line| !line.contains("[[keybindings]]"))
        {
            self.config.keybindings = crate::core::config::default_keybindings();
        }
        if self.shortcuts.overrides.is_empty() && !self.config.keybindings.is_empty() {
            self.shortcuts.overrides = self
                .config
                .keybindings
                .iter()
                .map(ShortcutOverride::from_legacy)
                .collect();
        }
    }

    fn validate_projects(&self) -> Result<()> {
        let mut ids = BTreeSet::new();
        for project in &self.projects {
            if project.id.trim().is_empty() || project.name.trim().is_empty() {
                return Err(anyhow!("projects.id 和 projects.name 不能为空"));
            }
            if !ids.insert(project.id.to_ascii_lowercase()) {
                return Err(anyhow!("重复的 project id: {}", project.id));
            }
            if project.path.trim().is_empty() {
                return Err(anyhow!("project {} 的 path 不能为空", project.id));
            }
            if project.runtime.id.trim().is_empty() || project.transport.id.trim().is_empty() {
                return Err(anyhow!(
                    "project {} 必须指定 runtime 和 transport",
                    project.id
                ));
            }
            if !matches!(
                project.runtime.id.to_ascii_lowercase().as_str(),
                "shell" | "tmux" | "herdr"
            ) {
                return Err(anyhow!(
                    "project {} 使用了不支持的 runtime: {}",
                    project.id,
                    project.runtime.id
                ));
            }
            if !matches!(
                project.transport.id.to_ascii_lowercase().as_str(),
                "local" | "ssh"
            ) {
                return Err(anyhow!(
                    "project {} 使用了不支持的 transport: {}",
                    project.id,
                    project.transport.id
                ));
            }
            if project.transport.id.eq_ignore_ascii_case("ssh")
                && project.transport.target.trim().is_empty()
                && project
                    .transport
                    .options
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .is_empty()
            {
                return Err(anyhow!(
                    "project {} 的 SSH transport 缺少 target",
                    project.id
                ));
            }
        }
        Ok(())
    }

    fn validate_shortcuts(&self) -> Result<()> {
        if !matches!(self.shortcuts.preset.as_str(), "qwerty" | "colemak") {
            return Err(anyhow!("shortcuts.preset 只能是 qwerty 或 colemak"));
        }
        if !matches!(
            self.shortcuts.primary_key.as_str(),
            "auto" | "alt" | "command" | "control" | "super"
        ) {
            return Err(anyhow!("shortcuts.primary_key 无效"));
        }
        let mut seen = BTreeSet::new();
        for override_item in &self.shortcuts.overrides {
            if override_item.action.trim().is_empty() {
                return Err(anyhow!("shortcut action 不能为空"));
            }
            for binding in &override_item.bindings {
                if binding.key.trim().is_empty() {
                    return Err(anyhow!("shortcut {} 的 key 不能为空", override_item.action));
                }
                let mut mods = binding.modifiers.clone();
                mods.sort();
                let key = format!("{}+{}", mods.join("+"), binding.key.to_ascii_lowercase());
                if !seen.insert(key.clone()) {
                    return Err(anyhow!("快捷键冲突: {key}"));
                }
            }
        }
        Ok(())
    }

    pub fn schema_json() -> Value {
        let mut schema = serde_json::to_value(schema_for!(ConfigDocument)).unwrap_or_else(|_| {
            Value::Object(Map::from_iter([(
                String::from("type"),
                Value::String("object".into()),
            )]))
        });
        // `Config` remains a compatibility facade with its historical light /
        // monospace defaults. Publish modern service defaults in the schema so
        // renderers never copy defaults into platform code.
        if let Some(defs) = schema.get_mut("$defs").and_then(Value::as_object_mut) {
            if let Some(font) = defs.get_mut("FontConfig") {
                font["properties"]["family"]["default"] = Value::String("JetBrains Mono".into());
                font["properties"]["size"]["default"] = Value::from(13.0);
                font["properties"]["fallback"]["default"] =
                    serde_json::json!(["Noto Sans Mono", "monospace"]);
            }
            if let Some(theme) = defs.get_mut("ThemeConfig") {
                theme["properties"]["name"]["default"] = Value::String("system".into());
                theme["properties"]["light"]["default"] = Value::String("white".into());
                theme["properties"]["dark"]["default"] = Value::String("black".into());
            }
        }
        schema["properties"]["font"]["default"] = serde_json::json!({
            "family": "JetBrains Mono",
            "size": 13.0,
            "fallback": ["Noto Sans Mono", "monospace"]
        });
        schema["properties"]["theme"]["default"] = serde_json::json!({
            "name": "system",
            "light": "white",
            "dark": "black"
        });
        schema
    }

    pub fn manifest_json() -> Value {
        serde_json::json!({
            "manifest_version": 1,
            "schema_id": "muxterm.config.v1",
            "groups": [
                {"id":"appearance","title_key":"settings.appearance","fields":[
                    {"path":"/font/family","control":"font_picker","apply":"immediate","title_key":"settings.font.family"},
                    {"path":"/font/size","control":"number","apply":"immediate","title_key":"settings.font.size"},
                    {"path":"/font/fallback","control":"font_fallback","apply":"immediate","title_key":"settings.font.fallback"},
                    {"path":"/theme/name","control":"theme_picker","options":["system","black","white"],"apply":"immediate","title_key":"settings.theme"},
                    {"path":"/theme/light","control":"theme_picker","options":["white","black"],"apply":"immediate","title_key":"settings.theme.light"},
                    {"path":"/theme/dark","control":"theme_picker","options":["black","white"],"apply":"immediate","title_key":"settings.theme.dark"},
                    {"path":"/statusbar/mode","control":"select","options":["tmux","theme"],"apply":"immediate","title_key":"settings.statusbar"}
                ]},
                {"id":"runtime","title_key":"settings.runtime","fields":[
                    {"path":"/tmux/auto_mouse","control":"switch","apply":"next_workspace","title_key":"settings.tmux.auto_mouse"},
                    {"path":"/tmux/default_session","control":"text","apply":"next_workspace","title_key":"settings.tmux.default_session"},
                    {"path":"/tmux/socket","control":"text","apply":"next_workspace","title_key":"settings.tmux.socket"},
                    {"path":"/pool/max_slots","control":"number","apply":"next_workspace","title_key":"settings.pool"},
                    {"path":"/scrollback/lines","control":"number","apply":"next_workspace","title_key":"settings.scrollback"},
                    {"path":"/pane/default_command","control":"text","apply":"next_workspace","title_key":"settings.pane.command"},
                    {"path":"/pane/workdir","control":"directory","apply":"next_workspace","title_key":"settings.pane.workdir"}
                ]},
                {"id":"attention","title_key":"settings.attention","fields":[
                    {"path":"/attention/enabled","control":"switch","apply":"immediate","title_key":"settings.attention.enabled"},
                    {"path":"/attention/blocked_regex","control":"multiline","apply":"immediate","title_key":"settings.attention.blocked_regex"},
                    {"path":"/attention/debounce_ms","control":"number","apply":"immediate","title_key":"settings.attention.debounce"}
                ]},
                {"id":"ui","title_key":"settings.ui","fields":[
                    {"path":"/ui/tab_bar_position","control":"select","options":["top","bottom"],"apply":"next_workspace","title_key":"settings.ui.tab_bar_position"},
                    {"path":"/ui/tab_bar_height","control":"number","apply":"next_workspace","title_key":"settings.ui.tab_bar_height"},
                    {"path":"/ui/show_title_bar","control":"switch","apply":"next_workspace","title_key":"settings.ui.show_title_bar"},
                    {"path":"/ui/borderless","control":"switch","apply":"next_workspace","title_key":"settings.ui.borderless"}
                ]},
                {"id":"ssh","title_key":"settings.ssh","fields":[
                    {"path":"/ssh/host","control":"text","apply":"commit","title_key":"settings.ssh.host"},
                    {"path":"/ssh/port","control":"number","apply":"commit","title_key":"settings.ssh.port"},
                    {"path":"/ssh/user","control":"text","apply":"commit","title_key":"settings.ssh.user"},
                    {"path":"/ssh/key_path","control":"file","apply":"commit","title_key":"settings.ssh.key_path"}
                ]},
                {"id":"behavior","title_key":"settings.behavior","fields":[
                    {"path":"/behavior/on_last_pane_exit","control":"select","options":["close_window","keep_empty","new_shell"],"apply":"next_workspace","title_key":"settings.behavior.last_pane"},
                    {"path":"/behavior/on_program_exit_abnormal","control":"select","options":["notify","close","keep"],"apply":"next_workspace","title_key":"settings.behavior.abnormal_exit"}
                ]},
                {"id":"platform","title_key":"settings.platform","fields":[
                    {"path":"/platform/linux/client_side_decorations","control":"switch","apply":"next_workspace","title_key":"settings.platform.linux_csd"},
                    {"path":"/platform/macos/option_as_alt","control":"switch","apply":"next_workspace","title_key":"settings.platform.macos_option_as_alt"}
                ]},
                {"id":"projects","title_key":"settings.projects","fields":[
                    {"path":"/projects","control":"project_editor","apply":"commit","title_key":"settings.projects"}
                ]},
                {"id":"shortcuts","title_key":"settings.shortcuts","fields":[
                    {"path":"/shortcuts/preset","control":"select","options":["qwerty","colemak"],"apply":"immediate","title_key":"settings.shortcuts.preset"},
                    {"path":"/shortcuts/primary_key","control":"select","options":["auto","alt","command","control","super"],"apply":"immediate","title_key":"settings.shortcuts.primary_key"},
                    {"path":"/shortcuts/overrides","control":"shortcut_editor","apply":"immediate","title_key":"settings.shortcuts.overrides"}
                ]}
            ]
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
pub struct ProjectDocument {
    pub id: String,
    pub name: String,
    pub path: String,
    pub runtime: ProjectRuntime,
    pub transport: ProjectTransport,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
pub struct ProjectRuntime {
    pub id: String,
    #[serde(default)]
    pub options: BTreeMap<String, Value>,
    /// tmux/Herdr named session, when the runtime supports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// Runtime socket (tmux `-L` name or Herdr API socket).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
pub struct ProjectTransport {
    pub id: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub options: BTreeMap<String, Value>,
}

impl ProjectDocument {
    /// Convert a QuickConnect target into the serializable Project contract.
    pub fn from_target(config: &TargetConfig) -> Self {
        let (transport_id, target) = match &config.transport {
            TargetTransport::Local => ("local".to_string(), String::new()),
            TargetTransport::Ssh { name } => ("ssh".to_string(), name.clone()),
        };
        Self {
            id: format!("{}@{}", config.name, transport_id),
            name: config.name.clone(),
            path: config.path.clone(),
            runtime: ProjectRuntime {
                id: config.runtime.as_str().to_string(),
                options: BTreeMap::new(),
                session: config.session.clone(),
                socket: config.socket.clone(),
            },
            transport: ProjectTransport {
                id: transport_id,
                target,
                options: BTreeMap::new(),
            },
            command: Vec::new(),
            env: BTreeMap::new(),
        }
    }

    /// Convert the portable Project contract back into a QuickConnect target.
    pub fn to_target(&self) -> Result<TargetConfig> {
        let runtime = TargetRuntime::from_str(&self.runtime.id)
            .ok_or_else(|| anyhow!("不支持的 project runtime: {}", self.runtime.id))?;
        let transport = match self.transport.id.to_ascii_lowercase().as_str() {
            "local" => TargetTransport::Local,
            "ssh" => {
                let alias = if self.transport.target.trim().is_empty() {
                    self.transport
                        .options
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                } else {
                    self.transport.target.as_str()
                };
                if alias.trim().is_empty() {
                    return Err(anyhow!("project {} 的 SSH transport 缺少 target", self.id));
                }
                TargetTransport::Ssh {
                    name: alias.to_string(),
                }
            }
            other => return Err(anyhow!("不支持的 project transport: {other}")),
        };
        let mut target = TargetConfig::new(&self.name, runtime, transport, &self.path);
        target.session = self.runtime.session.clone().or_else(|| {
            self.runtime
                .options
                .get("session")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
        target.socket = self.runtime.socket.clone().or_else(|| {
            self.runtime
                .options
                .get("socket")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
        Ok(target)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Default)]
#[serde(default)]
pub struct PlatformConfig {
    pub linux: LinuxPlatformConfig,
    pub macos: MacosPlatformConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(default)]
pub struct LinuxPlatformConfig {
    pub client_side_decorations: bool,
}

impl Default for LinuxPlatformConfig {
    fn default() -> Self {
        Self {
            client_side_decorations: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(default)]
pub struct MacosPlatformConfig {
    pub option_as_alt: bool,
}

impl Default for MacosPlatformConfig {
    fn default() -> Self {
        Self {
            option_as_alt: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(default)]
pub struct ShortcutConfig {
    pub preset: String,
    pub primary_key: String,
    pub overrides: Vec<ShortcutOverride>,
}

impl Default for ShortcutConfig {
    fn default() -> Self {
        Self {
            preset: "qwerty".into(),
            primary_key: "auto".into(),
            overrides: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
pub struct ShortcutOverride {
    pub action: String,
    #[serde(default)]
    pub bindings: Vec<ShortcutBinding>,
}

impl ShortcutOverride {
    fn from_legacy(binding: &KeyBinding) -> Self {
        Self {
            action: binding.action.clone(),
            bindings: vec![ShortcutBinding {
                key: binding.key.clone(),
                modifiers: binding.mods.clone(),
            }],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
pub struct ShortcutBinding {
    pub key: String,
    #[serde(default)]
    pub modifiers: Vec<String>,
}

fn validate_toml_shape(value: &toml::Value) -> Result<()> {
    let table = value
        .as_table()
        .ok_or_else(|| anyhow!("配置根节点必须是 TOML table"))?;
    check_keys(
        table,
        &[
            "config_version",
            "font",
            "theme",
            "statusbar",
            "pool",
            "tmux",
            "ssh",
            "scrollback",
            "attention",
            "ui",
            "pane",
            "behavior",
            "keybindings",
            "projects",
            "shortcuts",
            "platform",
            "extensions",
        ],
        "root",
    )?;
    check_table(table, "font", &["family", "size", "fallback"])?;
    check_table(table, "theme", &["name", "light", "dark"])?;
    check_table(table, "statusbar", &["mode"])?;
    check_table(table, "pool", &["max_slots"])?;
    check_table(table, "tmux", &["auto_mouse", "default_session", "socket"])?;
    check_table(table, "ssh", &["host", "port", "user", "key_path"])?;
    check_table(table, "scrollback", &["lines"])?;
    check_table(
        table,
        "attention",
        &["enabled", "blocked_regex", "debounce_ms"],
    )?;
    check_table(
        table,
        "ui",
        &[
            "tab_bar_position",
            "tab_bar_height",
            "show_title_bar",
            "borderless",
        ],
    )?;
    check_table(table, "pane", &["default_command", "workdir"])?;
    check_table(
        table,
        "behavior",
        &["on_last_pane_exit", "on_program_exit_abnormal"],
    )?;
    check_table(table, "shortcuts", &["preset", "primary_key", "overrides"])?;
    check_table(table, "platform", &["linux", "macos"])?;
    if let Some(platform) = table.get("platform").and_then(toml::Value::as_table) {
        check_keys(
            platform
                .get("linux")
                .and_then(toml::Value::as_table)
                .unwrap_or(&toml::map::Map::new()),
            &["client_side_decorations"],
            "platform.linux",
        )?;
        check_keys(
            platform
                .get("macos")
                .and_then(toml::Value::as_table)
                .unwrap_or(&toml::map::Map::new()),
            &["option_as_alt"],
            "platform.macos",
        )?;
    }
    Ok(())
}

fn check_table(
    root: &toml::map::Map<String, toml::Value>,
    name: &str,
    allowed: &[&str],
) -> Result<()> {
    if let Some(table) = root.get(name).and_then(toml::Value::as_table) {
        check_keys(table, allowed, name)?;
    }
    Ok(())
}

fn check_keys(
    table: &toml::map::Map<String, toml::Value>,
    allowed: &[&str],
    section: &str,
) -> Result<()> {
    for key in table.keys() {
        let unknown_table_is_legacy_extension =
            section == "root" && table.get(key).and_then(toml::Value::as_table).is_some();
        if !allowed.iter().any(|candidate| candidate == key)
            && !section.starts_with("extensions")
            && !unknown_table_is_legacy_extension
        {
            return Err(anyhow!(
                "未知配置字段 {section}.{key}；扩展数据请放入 extensions.<vendor>"
            ));
        }
    }
    Ok(())
}
