//! QuickConnect 纯逻辑模型（与 macOS Chrome 层行为一致，无 GTK 依赖）。
//!
//! 目标配置 / 展示 / 派生逻辑，供 QuickConnect 面板与单测共用。

use std::collections::HashSet;
use std::path::Path;

/// 快速连接目标的运行时（shell / tmux）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetRuntime {
    Shell,
    Tmux,
    Herdr,
}

impl TargetRuntime {
    pub fn as_str(self) -> &'static str {
        match self {
            TargetRuntime::Shell => "shell",
            TargetRuntime::Tmux => "tmux",
            TargetRuntime::Herdr => "herdr",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "shell" => Some(TargetRuntime::Shell),
            "tmux" => Some(TargetRuntime::Tmux),
            "herdr" => Some(TargetRuntime::Herdr),
            _ => None,
        }
    }

    /// 查询 token：完整名字，或长度 ≥ 2 且在 shell/tmux/herdr 中唯一的前缀。
    /// `@tm` → tmux；配置解析仍走精确的 `from_str`。
    fn from_query_token(value: &str) -> Option<Self> {
        if let Some(exact) = Self::from_str(value) {
            return Some(exact);
        }
        if value.len() < 2 {
            return None;
        }
        let hits: Vec<Self> = [Self::Shell, Self::Tmux, Self::Herdr]
            .into_iter()
            .filter(|runtime| runtime.as_str().starts_with(value))
            .collect();
        match hits.as_slice() {
            [only] => Some(*only),
            _ => None,
        }
    }
}

/// 连接传输（ssh 需要名字；local 不需要）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TargetTransport {
    Local,
    Ssh { name: String },
}

impl TargetTransport {
    pub fn label(&self) -> String {
        match self {
            TargetTransport::Local => "local".into(),
            TargetTransport::Ssh { name } => name.clone(),
        }
    }

    pub fn is_ssh(&self) -> bool {
        matches!(self, TargetTransport::Ssh { .. })
    }

    /// 创建 detached session 时走 discovery FFI（`local` / `ssh`）。
    pub fn create_backend(&self) -> (&'static str, Option<&str>) {
        match self {
            TargetTransport::Local => ("local", None),
            TargetTransport::Ssh { name } => ("ssh", Some(name.as_str())),
        }
    }

    /// attach 控制模式时走 `tmux` / `tmux-ssh`。
    pub fn attach_backend(&self) -> (&'static str, Option<&str>) {
        match self {
            TargetTransport::Local => ("tmux", None),
            TargetTransport::Ssh { name } => ("tmux-ssh", Some(name.as_str())),
        }
    }
}

/// 一个可快速连接的目标（Recent / Project 共用）。
///
/// 身份字段（transport target / runtime / session / target-side socket /
/// workspace_id）与显示/项目元数据（name / path）分离：
/// identity key 只由身份字段构成，`name`/`path` 变更不改变身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetConfig {
    pub name: String,
    pub runtime: TargetRuntime,
    pub transport: TargetTransport,
    /// 项目目录（显示/元数据）；Herdr 的 `wN` workspace_id 独立存放，互不覆盖。
    pub path: String,
    /// Herdr：target-side API socket 绝对路径（本地 = 本机 socket；
    /// SSH = 远端 socket 路径，转发由 Runtime 创建，保存的永远是 target-side）。
    pub socket: Option<String>,
    /// Herdr：named session 名（默认 socket 为 "default"）。
    pub session: Option<String>,
    /// Herdr：workspace id（`wN`）。tmux/shell 为空。
    pub workspace_id: Option<String>,
}

impl TargetConfig {
    pub fn new(
        name: impl Into<String>,
        runtime: TargetRuntime,
        transport: TargetTransport,
        path: impl Into<String>,
    ) -> Self {
        TargetConfig {
            name: name.into(),
            runtime,
            transport,
            path: path.into(),
            socket: None,
            session: None,
            workspace_id: None,
        }
    }

    /// 命令面板 / SSH 向导：attach 指定 tmux session。
    pub fn tmux_session(session: impl Into<String>, transport: TargetTransport) -> Self {
        let session = session.into();
        TargetConfig::new(session, TargetRuntime::Tmux, transport, "~")
    }

    /// 身份 key：transport target、runtime、session、target-side socket 与
    /// workspace_id。`name`/`path` 是显示/项目元数据，不参与身份。
    pub fn identity_key(&self) -> String {
        let (transport, target) = match &self.transport {
            TargetTransport::Local => ("local", ""),
            TargetTransport::Ssh { name } => ("ssh", name.as_str()),
        };
        let runtime = self.runtime.as_str();
        let components = match self.runtime {
            // Shell 的 cwd 是它的 attach identity；name 只用于显示。
            TargetRuntime::Shell => vec![
                runtime.to_string(),
                transport.to_string(),
                target.to_string(),
                if self.path.is_empty() {
                    self.name.clone()
                } else {
                    self.path.clone()
                },
            ],
            // Project 记录通常没有单独的 session 字段，但 tmux Project
            // 的 name 就是创建/attach 时使用的 session 名。
            TargetRuntime::Tmux => vec![
                runtime.to_string(),
                transport.to_string(),
                target.to_string(),
                self.session
                    .clone()
                    .filter(|session| !session.is_empty())
                    .unwrap_or_else(|| self.name.clone()),
                self.socket.clone().unwrap_or_default(),
            ],
            // Herdr 只有三项 typed identity 都存在时才可跨 Project/Existing
            // 复用；旧配置则保留 name/path 作为 provisional key。
            TargetRuntime::Herdr
                if self.session.as_deref().is_some_and(|v| !v.is_empty())
                    && self.socket.as_deref().is_some_and(|v| !v.is_empty())
                    && self.workspace_id.as_deref().is_some_and(|v| !v.is_empty()) =>
            {
                vec![
                    runtime.to_string(),
                    transport.to_string(),
                    target.to_string(),
                    self.session.clone().unwrap_or_default(),
                    self.socket.clone().unwrap_or_default(),
                    self.workspace_id.clone().unwrap_or_default(),
                ]
            }
            TargetRuntime::Herdr => vec![
                "herdr-provisional".to_string(),
                transport.to_string(),
                target.to_string(),
                self.name.clone(),
                self.path.clone(),
            ],
        };
        // 长度前缀避免 session/socket/path 中的分隔符造成碰撞。
        components
            .iter()
            .map(|component| format!("{}:{component}", component.len()))
            .collect::<Vec<_>>()
            .join("|")
    }

    /// 用于工作区面板搜索的字段集合。
    ///
    /// 这些字段都是目标描述的一部分；尤其是 SSH alias、session、socket
    /// 和 Herdr workspace_id，不能只靠用户可见的 name/path 搜索到。
    fn search_fields(&self) -> Vec<String> {
        let transport = match &self.transport {
            TargetTransport::Local => "local".to_string(),
            TargetTransport::Ssh { name } => format!("ssh {name}"),
        };
        vec![
            self.name.clone(),
            self.runtime.as_str().to_string(),
            transport,
            self.path.clone(),
            self.session.clone().unwrap_or_default(),
            self.socket.clone().unwrap_or_default(),
            self.workspace_id.clone().unwrap_or_default(),
        ]
    }
}

/// 工作区面板查询。
///
/// 普通词使用大小写不敏感的子序列匹配（例如 `mterm` 可以命中
/// `muxterm`）；多个普通词必须全部命中。`@tmux`、`@herdr`、`@shell`
/// 是 runtime 条件（`@tm` 这类唯一前缀也可以），`@local` 是本地传输
/// 条件，其他 `@xxx` 会被当成 SSH alias 条件（精确、前缀或模糊）。
/// 例如 `@tmux @ryzen` 只留下 ryzen 上的 tmux workspace / project。
/// 多个 `@` 条件同样是 AND 关系。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkspaceQuery {
    terms: Vec<String>,
    runtime_filters: Vec<TargetRuntime>,
    local_only: bool,
    ssh_alias_filters: Vec<String>,
}

impl WorkspaceQuery {
    pub fn parse(raw: &str) -> Self {
        let mut query = Self::default();
        for token in raw.split_whitespace() {
            let Some(filter) = token.strip_prefix('@') else {
                query.terms.push(token.to_lowercase());
                continue;
            };
            if filter.is_empty() {
                continue;
            }
            let filter = filter.to_lowercase();
            if let Some(runtime) = TargetRuntime::from_query_token(&filter) {
                query.runtime_filters.push(runtime);
            } else if filter == "local" || unique_local_prefix(&filter) {
                query.local_only = true;
            } else {
                query.ssh_alias_filters.push(filter);
            }
        }
        query
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
            && self.runtime_filters.is_empty()
            && !self.local_only
            && self.ssh_alias_filters.is_empty()
    }

    /// 返回目标是否满足全部搜索条件。
    pub fn matches(&self, config: &TargetConfig) -> bool {
        self.score(config).is_some()
    }

    /// 返回用于排序的匹配分数；不匹配时返回 `None`。
    ///
    /// 分数只用于把更紧密的结果排在前面，不改变过滤语义。精确子串
    /// 优先于稀疏子序列，较短字段优先于较长字段。
    pub fn score(&self, config: &TargetConfig) -> Option<u32> {
        if self
            .runtime_filters
            .iter()
            .any(|runtime| runtime != &config.runtime)
        {
            return None;
        }
        if self.local_only && !matches!(config.transport, TargetTransport::Local) {
            return None;
        }
        for alias in &self.ssh_alias_filters {
            let TargetTransport::Ssh { name } = &config.transport else {
                return None;
            };
            if !ssh_alias_token_matches(name, alias) {
                return None;
            }
        }

        let fields = config.search_fields();
        let mut score = 10_000u32;
        for term in &self.terms {
            let term_score = fields
                .iter()
                .filter_map(|field| fuzzy_field_score(field, term))
                .max()?;
            score = score.saturating_add(term_score);
        }
        // 条件越具体，结果越靠前；普通的 filter query 仍只依赖 matches。
        score = score
            .saturating_add((self.runtime_filters.len() as u32) * 2_000)
            .saturating_add(if self.local_only || !self.ssh_alias_filters.is_empty() {
                2_000
            } else {
                0
            });
        Some(score)
    }

    /// SSH Host 行：`@ryzen` / `@ry` 命中 alias；仅 `@tmux` 时不展示 Host，
    /// 把位置留给已有的 workspace / project。
    pub fn host_score(&self, alias: &str) -> Option<u32> {
        if self.local_only {
            return None;
        }
        for want in &self.ssh_alias_filters {
            if !ssh_alias_token_matches(alias, want) {
                return None;
            }
        }
        if !self.runtime_filters.is_empty()
            && self.ssh_alias_filters.is_empty()
            && self.terms.is_empty()
        {
            return None;
        }
        let mut score = 1_000u32;
        for term in &self.terms {
            score = score.saturating_add(fuzzy_field_score(alias, term)?);
        }
        if !self.ssh_alias_filters.is_empty() {
            score = score.saturating_add(2_000);
        }
        Some(score)
    }

    /// 当前输入 token 的补全候选。返回值带 `@`，可直接替换 token。
    pub fn completion_candidates(raw: &str, ssh_aliases: &[String]) -> Vec<String> {
        let Some(token) = current_token(raw) else {
            return Vec::new();
        };
        let Some(prefix) = token.strip_prefix('@') else {
            return Vec::new();
        };
        let mut candidates = vec![
            "@shell".to_string(),
            "@tmux".to_string(),
            "@herdr".to_string(),
            "@local".to_string(),
        ];
        let mut seen: HashSet<String> = candidates
            .iter()
            .map(|value| value.to_lowercase())
            .collect();
        for alias in ssh_aliases {
            let alias = alias.trim();
            if !alias.is_empty() {
                let candidate = format!("@{alias}");
                if seen.insert(candidate.to_lowercase()) {
                    candidates.push(candidate);
                }
            }
        }
        let prefix = prefix.to_lowercase();
        candidates.retain(|candidate| {
            let candidate = candidate.strip_prefix('@').unwrap_or(candidate);
            prefix.is_empty() || fuzzy_subsequence(candidate, &prefix).is_some()
        });
        candidates
    }

    /// 用补全候选替换输入中的最后一个 token，并保留前面的条件。
    pub fn replace_current_token(raw: &str, replacement: &str) -> String {
        let start = raw
            .char_indices()
            .rev()
            .find(|(_, ch)| ch.is_whitespace())
            .map(|(index, ch)| index + ch.len_utf8())
            .unwrap_or(0);
        format!("{}{}", &raw[..start], replacement)
    }
}

fn unique_local_prefix(filter: &str) -> bool {
    filter.len() >= 2 && "local".starts_with(filter)
}

fn ssh_alias_token_matches(alias: &str, want: &str) -> bool {
    let alias = alias.to_ascii_lowercase();
    let want = want.to_ascii_lowercase();
    if want.is_empty() {
        return true;
    }
    alias == want || alias.starts_with(&want) || fuzzy_subsequence(&alias, &want).is_some()
}

fn current_token(raw: &str) -> Option<&str> {
    if raw.chars().last().is_some_and(char::is_whitespace) {
        return None;
    }
    raw.split_whitespace().last()
}

fn fuzzy_field_score(field: &str, query: &str) -> Option<u32> {
    let field = field.to_lowercase();
    let query = query.to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    if field.contains(&query) {
        return Some(2_000u32.saturating_sub(field.chars().count() as u32));
    }
    fuzzy_subsequence(&field, &query).map(|gaps| {
        1_000u32
            .saturating_sub(gaps)
            .saturating_sub(field.chars().count() as u32 / 4)
    })
}

/// 返回匹配字符之间的 gap 数；越小表示越紧密。
fn fuzzy_subsequence(candidate: &str, query: &str) -> Option<u32> {
    let candidate: Vec<char> = candidate.chars().collect();
    let mut gaps = 0u32;
    let mut previous_position = None;
    for wanted in query.chars() {
        let mut found = false;
        for (position, actual) in candidate.iter().enumerate() {
            if previous_position.is_some_and(|previous| position <= previous) {
                continue;
            }
            if *actual == wanted {
                if let Some(previous) = previous_position {
                    gaps = gaps.saturating_add(position.saturating_sub(previous + 1) as u32);
                }
                previous_position = Some(position);
                found = true;
                break;
            }
        }
        if !found {
            return None;
        }
    }
    Some(gaps)
}

/// 快速连接条目上的小标记：目标同时是 Recent 和/或 Project。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuickBadge {
    Recent,
    Project,
}

impl QuickBadge {
    pub fn label(self) -> &'static str {
        match self {
            QuickBadge::Recent => "Recent",
            QuickBadge::Project => "Project",
        }
    }
}

/// 面板中的一行：目标 + 应显示的标记。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickConnectEntry {
    pub config: TargetConfig,
    pub badges: Vec<QuickBadge>,
}

impl QuickConnectEntry {
    pub fn new(config: TargetConfig, badges: Vec<QuickBadge>) -> Self {
        QuickConnectEntry { config, badges }
    }
}

/// 快速连接目标的展示与派生逻辑（纯函数，便于单测）。
pub enum QuickConnect {}

impl QuickConnect {
    /// 从 path 派生默认 name：取路径最后一段目录名（最小目录）。
    /// 根目录 / 空路径回退 "workspace"。
    pub fn default_name(for_path: &str) -> String {
        let trimmed = for_path.trim();
        if trimmed.is_empty() {
            return "workspace".into();
        }
        let last = Path::new(trimmed)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if last.is_empty() || last == "/" {
            return "workspace".into();
        }
        last
    }

    /// 面板副标题：`runtime @ transport`。ssh transport 显示为名字。
    pub fn subtitle(config: &TargetConfig) -> String {
        format!("{} @ {}", config.runtime.as_str(), config.transport.label())
    }

    /// 该目标是否需要 tmux 按 name attach（tmux 且 name 非空）。
    pub fn should_attach(existing_name: Option<&str>, config: &TargetConfig) -> bool {
        config.runtime == TargetRuntime::Tmux && !existing_name.unwrap_or("").trim().is_empty()
    }

    /// 展示文本（搜索用）：name + runtime/transport + path + attach identity。
    pub fn search_text(config: &TargetConfig) -> String {
        config.search_fields().join(" ").to_lowercase()
    }

    /// 工作区面板的统一查询匹配。
    pub fn matches_query(config: &TargetConfig, query: &str) -> bool {
        WorkspaceQuery::parse(query).matches(config)
    }

    /// 工作区面板的统一查询分数。
    pub fn search_score(config: &TargetConfig, query: &str) -> Option<u32> {
        WorkspaceQuery::parse(query).score(config)
    }

    /// 按查询过滤并按模糊匹配紧密度排序。
    pub fn filter_entries(entries: &[QuickConnectEntry], query: &str) -> Vec<QuickConnectEntry> {
        let parsed = WorkspaceQuery::parse(query);
        let mut matched: Vec<(usize, u32, QuickConnectEntry)> = entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                parsed
                    .score(&entry.config)
                    .map(|score| (index, score, entry.clone()))
            })
            .collect();
        matched.sort_by(
            |(left_index, left_score, _), (right_index, right_score, _)| {
                right_score
                    .cmp(left_score)
                    .then(left_index.cmp(right_index))
            },
        );
        matched.into_iter().map(|(_, _, entry)| entry).collect()
    }

    /// 目标唯一 ID：与 attach identity 一致；name/path 只在 identity 不完整
    /// 的 Project/Herdr provisional 阶段参与 key。
    pub fn unique_id(config: &TargetConfig) -> String {
        config.identity_key()
    }

    /// 计算目标应显示的标记：Recent 在前、Project 在后。
    pub fn badges(
        config: &TargetConfig,
        recents: &[TargetConfig],
        projects: &[TargetConfig],
    ) -> Vec<QuickBadge> {
        let id = Self::unique_id(config);
        let mut result = Vec::new();
        if recents.iter().any(|r| Self::unique_id(r) == id) {
            result.push(QuickBadge::Recent);
        }
        if projects.iter().any(|p| Self::unique_id(p) == id) {
            result.push(QuickBadge::Project);
        }
        result
    }

    /// 面板条目：先展示最近的前 `recent_limit` 条（最新在前），
    /// 再补 Project 中未出现的目标。按 attach identity 去重。
    pub fn entries(
        recents: &[TargetConfig],
        projects: &[TargetConfig],
        recent_limit: usize,
    ) -> Vec<QuickConnectEntry> {
        let mut seen = std::collections::HashSet::new();
        let mut result = Vec::new();
        for config in recents.iter().take(recent_limit) {
            let id = Self::unique_id(config);
            if !seen.insert(id) {
                continue;
            }
            result.push(QuickConnectEntry::new(
                config.clone(),
                Self::badges(config, recents, projects),
            ));
        }
        for config in projects {
            let id = Self::unique_id(config);
            if !seen.insert(id) {
                continue;
            }
            result.push(QuickConnectEntry::new(
                config.clone(),
                Self::badges(config, recents, projects),
            ));
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(
        name: &str,
        runtime: TargetRuntime,
        transport: TargetTransport,
        path: &str,
    ) -> TargetConfig {
        TargetConfig::new(name, runtime, transport, path)
    }

    #[test]
    fn default_name_from_path() {
        assert_eq!(QuickConnect::default_name("/a/b/c"), "c");
        assert_eq!(
            QuickConnect::default_name("~/Developer/self/muxterm"),
            "muxterm"
        );
        assert_eq!(QuickConnect::default_name(""), "workspace");
        assert_eq!(QuickConnect::default_name("/"), "workspace");
    }

    /// W20a：TargetRuntime::Herdr 可解析，subtitle 含 `herdr @`。
    #[test]
    fn herdr_runtime_roundtrip_and_subtitle() {
        assert_eq!(TargetRuntime::from_str("herdr"), Some(TargetRuntime::Herdr));
        assert_eq!(TargetRuntime::Herdr.as_str(), "herdr");
        let c = cfg("w1", TargetRuntime::Herdr, TargetTransport::Local, "w1");
        let subtitle = QuickConnect::subtitle(&c);
        assert!(
            subtitle.contains("herdr @"),
            "subtitle 必须含 `herdr @`: {subtitle}"
        );
        assert_eq!(subtitle, "herdr @ local");
    }

    #[test]
    fn unique_id_distinguishes_transport() {
        let a = cfg("srv", TargetRuntime::Tmux, TargetTransport::Local, "~/x");
        let b = cfg(
            "srv",
            TargetRuntime::Tmux,
            TargetTransport::Ssh {
                name: "ryzen".into(),
            },
            "~/x",
        );
        assert_ne!(QuickConnect::unique_id(&a), QuickConnect::unique_id(&b));
        assert_eq!(QuickConnect::unique_id(&a), a.identity_key());
    }

    #[test]
    fn workspace_query_fuzzy_matches_all_terms() {
        let config = TargetConfig::new(
            "muxterm",
            TargetRuntime::Tmux,
            TargetTransport::Ssh {
                name: "ryzen".into(),
            },
            "/home/wlz/Developer/muxterm",
        );
        assert!(QuickConnect::matches_query(&config, "mterm @tmux @ryzen"));
        assert!(QuickConnect::matches_query(&config, "@ryzen mux"));
        assert!(
            QuickConnect::matches_query(&config, "@tmux @ry"),
            "@ry 必须前缀命中 ryzen"
        );
        assert!(
            QuickConnect::matches_query(&config, "@tm mux"),
            "@tm 必须唯一前缀命中 tmux"
        );
        assert!(!QuickConnect::matches_query(&config, "mterm @shell"));
        assert!(!QuickConnect::matches_query(&config, "mterm @local"));
        assert!(!QuickConnect::matches_query(&config, "mterm @legion"));
    }

    #[test]
    fn workspace_query_host_score_keeps_alias_and_hides_runtime_only() {
        let query = WorkspaceQuery::parse("@tmux @ryzen");
        assert!(query.host_score("ryzen").is_some());
        assert!(query.host_score("RyZen").is_some());
        assert!(query.host_score("mac").is_none());
        assert!(WorkspaceQuery::parse("@ry").host_score("ryzen").is_some());
        assert!(
            WorkspaceQuery::parse("@tmux").host_score("ryzen").is_none(),
            "仅 runtime 条件时 Host 行不应挡住 Existing"
        );
        assert!(WorkspaceQuery::parse("ryz").host_score("ryzen").is_some());
        assert!(WorkspaceQuery::parse("@local")
            .host_score("ryzen")
            .is_none());
    }

    #[test]
    fn workspace_query_is_case_insensitive_and_combines_runtime_transport_filters() {
        let remote_tmux = cfg(
            "MuxTerm",
            TargetRuntime::Tmux,
            TargetTransport::Ssh {
                name: "RyZen".into(),
            },
            "/srv/muxterm",
        );
        let local_tmux = cfg(
            "MuxTerm",
            TargetRuntime::Tmux,
            TargetTransport::Local,
            "/srv/muxterm",
        );

        assert!(QuickConnect::matches_query(
            &remote_tmux,
            "MXE @TMUX @RYZEN"
        ));
        assert!(!QuickConnect::matches_query(
            &local_tmux,
            "MXE @TMUX @RYZEN"
        ));
        assert!(QuickConnect::matches_query(&local_tmux, "mxe @tmux @local"));
        assert!(!QuickConnect::matches_query(
            &remote_tmux,
            "mxe @tmux @local"
        ));
    }

    #[test]
    fn workspace_query_searches_attach_fields() {
        let mut config = cfg(
            "display-name",
            TargetRuntime::Herdr,
            TargetTransport::Local,
            "/project",
        );
        config.session = Some("agents".into());
        config.socket = Some("/tmp/agents.sock".into());
        config.workspace_id = Some("w7".into());
        assert!(QuickConnect::matches_query(&config, "agent"));
        assert!(QuickConnect::matches_query(&config, "w7"));
        assert!(QuickConnect::matches_query(&config, "sock"));
    }

    #[test]
    fn workspace_query_completion_replaces_only_current_token() {
        let candidates = WorkspaceQuery::completion_candidates(
            "project @ry",
            &["ryzen".into(), "legion".into()],
        );
        assert_eq!(candidates, vec!["@ryzen"]);
        assert_eq!(
            WorkspaceQuery::replace_current_token("project @ry", "@ryzen"),
            "project @ryzen"
        );
        assert!(WorkspaceQuery::completion_candidates("project ", &["ryzen".into()]).is_empty());
    }

    #[test]
    fn workspace_query_completion_lists_runtime_local_and_ssh_aliases() {
        assert_eq!(
            WorkspaceQuery::completion_candidates(
                "@",
                &["ryzen".into(), "RYZEN".into(), "legion".into()]
            ),
            vec![
                "@shell".to_string(),
                "@tmux".to_string(),
                "@herdr".to_string(),
                "@local".to_string(),
                "@ryzen".to_string(),
                "@legion".to_string(),
            ]
        );
        assert_eq!(
            WorkspaceQuery::completion_candidates("@tm", &["ryzen".into()]),
            vec!["@tmux".to_string()]
        );
    }

    #[test]
    fn workspace_query_filter_sorts_tighter_match_first() {
        let close = cfg(
            "muxterm",
            TargetRuntime::Tmux,
            TargetTransport::Local,
            "~/x",
        );
        let sparse = cfg(
            "m-u-x-t-e-r",
            TargetRuntime::Tmux,
            TargetTransport::Local,
            "~/x",
        );
        let entries = vec![
            QuickConnectEntry::new(sparse, vec![]),
            QuickConnectEntry::new(close, vec![]),
        ];
        let filtered = QuickConnect::filter_entries(&entries, "mxe");
        assert_eq!(filtered[0].config.name, "muxterm");
    }

    #[test]
    fn transport_backends_split_create_and_attach() {
        assert_eq!(TargetTransport::Local.create_backend(), ("local", None));
        assert_eq!(TargetTransport::Local.attach_backend(), ("tmux", None));
        let ssh = TargetTransport::Ssh {
            name: "ryzen".into(),
        };
        assert_eq!(ssh.create_backend(), ("ssh", Some("ryzen")));
        assert_eq!(ssh.attach_backend(), ("tmux-ssh", Some("ryzen")));
    }

    #[test]
    fn tmux_session_helper_keeps_ssh_alias() {
        let cfg = TargetConfig::tmux_session(
            "legion",
            TargetTransport::Ssh {
                name: "ryzen".into(),
            },
        );
        assert_eq!(cfg.name, "legion");
        assert_eq!(cfg.runtime, TargetRuntime::Tmux);
        assert_eq!(cfg.transport.attach_backend(), ("tmux-ssh", Some("ryzen")));
        assert_eq!(cfg.transport.create_backend(), ("ssh", Some("ryzen")));
    }

    #[test]
    fn badges_both_recent_and_project() {
        let a = cfg("srv", TargetRuntime::Tmux, TargetTransport::Local, "~/x");
        let badges = QuickConnect::badges(&a, std::slice::from_ref(&a), std::slice::from_ref(&a));
        assert_eq!(badges, vec![QuickBadge::Recent, QuickBadge::Project]);
    }

    #[test]
    fn entries_dedupe_and_order() {
        let r1 = cfg("r1", TargetRuntime::Shell, TargetTransport::Local, "~/r1");
        let r2 = cfg("r2", TargetRuntime::Shell, TargetTransport::Local, "~/r2");
        let p1 = cfg("p1", TargetRuntime::Tmux, TargetTransport::Local, "~/p1");
        let dup = r1.clone();
        let entries = QuickConnect::entries(&[r1, r2], &[dup, p1], 5);
        let names: Vec<_> = entries.iter().map(|e| e.config.name.as_str()).collect();
        assert_eq!(names, vec!["r1", "r2", "p1"]);
        assert_eq!(
            entries[0].badges,
            vec![QuickBadge::Recent, QuickBadge::Project]
        );
    }
}
