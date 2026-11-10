//! 快速连接（Workspaces tab）的目录、查询与排序规则。
//!
//! 移植自 macOS `Chrome/QuickConnectModel.swift`。全是纯函数：目标配置从哪里来、
//! 怎么排序、attach identity 怎么算，都不碰 UI，也不读 Core。

use crate::frontend::gpui::model::target::{TargetRuntime, TargetTransport};

/// 一个可连接的目标。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TargetConfig {
    pub name: String,
    pub runtime: TargetRuntime,
    pub transport: TargetTransport,
    /// 用户项目目录；Herdr 的 workspace id 必须独立保存在 `workspace_id`。
    pub path: String,
    /// Runtime namespace：Herdr named session；tmux Existing 为 session 名。
    pub session: Option<String>,
    /// Target-side socket。SSH Herdr 保存远端路径，不保存本地临时 forward。
    pub socket: Option<String>,
    /// Herdr workspace id（`wN`）；tmux/shell 为 `None`。
    pub workspace_id: Option<String>,
    /// `config.toml` 里的稳定 Project id。连接指纹不是项目 id。
    pub persisted_id: Option<String>,
}

impl TargetConfig {
    pub fn new(
        name: impl Into<String>,
        runtime: TargetRuntime,
        transport: TargetTransport,
        path: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            runtime,
            transport,
            path: path.into(),
            ..Self::default()
        }
    }

    /// 搜索字段：展示名、runtime、传输/SSH alias、路径以及 attach identity。
    pub fn search_fields(&self) -> Vec<String> {
        let transport = match &self.transport {
            TargetTransport::Local => "local".to_string(),
            TargetTransport::Ssh { name } => format!("ssh {name}"),
        };
        vec![
            self.name.clone(),
            self.runtime.label().to_lowercase(),
            transport,
            self.path.clone(),
            self.session.clone().unwrap_or_default(),
            self.socket.clone().unwrap_or_default(),
            self.workspace_id.clone().unwrap_or_default(),
        ]
    }
}

/// Workspaces tab 的查询规则。
///
/// 普通词使用大小写不敏感的子序列匹配；`@tmux` / `@herdr` / `@shell` 过滤
/// runtime（`@tm` 这类唯一前缀也可以），`@local` 过滤本地传输，其他 `@xxx`
/// 过滤 SSH alias（精确、前缀或模糊）。所有普通词和 `@` 条件都必须同时满足。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceQuery {
    terms: Vec<String>,
    runtime_filters: Vec<TargetRuntime>,
    local_only: bool,
    ssh_alias_filters: Vec<String>,
}

impl WorkspaceQuery {
    pub fn new(raw: &str) -> Self {
        let mut query = Self::default();
        for token in raw.split_whitespace() {
            let Some(rest) = token.strip_prefix('@') else {
                query.terms.push(token.to_lowercase());
                continue;
            };
            let filter = rest.to_lowercase();
            if filter.is_empty() {
                continue;
            }
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

    pub fn matches(&self, config: &TargetConfig) -> bool {
        self.score(config).is_some()
    }

    /// 越高表示匹配越紧密；只用于排序，不改变匹配结果。
    pub fn score(&self, config: &TargetConfig) -> Option<i64> {
        if self
            .runtime_filters
            .iter()
            .any(|runtime| *runtime != config.runtime)
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
            if !ssh_alias_matches(name, alias) {
                return None;
            }
        }

        let mut score = 10_000i64;
        for term in &self.terms {
            // 保持 Swift 的全字段子串/模糊搜索；名字命中额外加权，不缩小范围。
            let best = config
                .search_fields()
                .iter()
                .filter_map(|field| field_score(field, term))
                .max()?;
            score += best;
            if config.name.to_lowercase().contains(term) {
                score += 2_000;
            }
        }
        score += self.runtime_filters.len() as i64 * 2_000;
        if self.local_only || !self.ssh_alias_filters.is_empty() {
            score += 2_000;
        }
        Some(score)
    }

    /// 返回当前 `@` token 的候选；结果带 `@`，可直接替换输入 token。
    pub fn completion_candidates(raw: &str, ssh_aliases: &[String]) -> Vec<String> {
        let Some(token) = current_token(raw) else {
            return Vec::new();
        };
        let Some(prefix) = token.strip_prefix('@') else {
            return Vec::new();
        };
        let prefix = prefix.to_lowercase();
        let mut candidates = vec![
            "@shell".to_string(),
            "@tmux".to_string(),
            "@herdr".to_string(),
            "@local".to_string(),
        ];
        for alias in ssh_aliases {
            let alias = alias.trim();
            if alias.is_empty() {
                continue;
            }
            let candidate = format!("@{alias}");
            if !candidates
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&candidate))
            {
                candidates.push(candidate);
            }
        }
        candidates
            .into_iter()
            .filter(|candidate| {
                let value = candidate[1..].to_lowercase();
                prefix.is_empty() || fuzzy_gap_score(&value, &prefix).is_some()
            })
            .collect()
    }

    /// 用补全候选替换最后一个 token，并保留前面的查询条件。
    pub fn replace_current_token(raw: &str, replacement: &str) -> String {
        let Some(separator) = raw.char_indices().rev().find(|(_, ch)| ch.is_whitespace()) else {
            return replacement.to_string();
        };
        let split = separator.0 + separator.1.len_utf8();
        format!("{}{replacement}", &raw[..split])
    }
}

fn current_token(raw: &str) -> Option<&str> {
    if raw.chars().last().is_some_and(char::is_whitespace) {
        return None;
    }
    raw.split_whitespace().last()
}

/// `@lo` / `@loc` 这类唯一前缀也算 `@local`。
fn unique_local_prefix(filter: &str) -> bool {
    filter.chars().count() >= 2 && "local".starts_with(filter)
}

fn ssh_alias_matches(alias: &str, filter: &str) -> bool {
    let alias = alias.to_lowercase();
    let want = filter.to_lowercase();
    if want.is_empty() {
        return true;
    }
    if alias == want || alias.starts_with(&want) {
        return true;
    }
    fuzzy_gap_score(&alias, &want).is_some()
}

fn field_score(field: &str, query: &str) -> Option<i64> {
    let field = field.to_lowercase();
    let query = query.to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    let chars = field.chars().count() as i64;
    if field.contains(&query) {
        return Some((2_000 - chars).max(0));
    }
    let gaps = fuzzy_gap_score(&field, &query)?;
    Some((1_000 - gaps - chars / 4).max(0))
}

/// 返回匹配字符间的 gap 数。
fn fuzzy_gap_score(candidate: &str, query: &str) -> Option<i64> {
    let candidate: Vec<char> = candidate.chars().collect();
    let mut previous: Option<usize> = None;
    let mut gaps = 0i64;
    let mut cursor = 0usize;
    for wanted in query.chars() {
        let mut found = None;
        while cursor < candidate.len() {
            let position = cursor;
            cursor += 1;
            if candidate[position] == wanted {
                found = Some(position);
                break;
            }
        }
        let position = found?;
        if let Some(previous) = previous {
            gaps += position as i64 - previous as i64 - 1;
        }
        previous = Some(position);
    }
    Some(gaps)
}

/// 面板条目上的标记。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickBadge {
    Recent,
    Project,
}

/// 面板条目：配置 + 标记。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickConnectEntry {
    pub config: TargetConfig,
    pub badges: Vec<QuickBadge>,
}

/// 快速连接目标的展示与派生逻辑（纯函数）。
pub struct QuickConnect;

impl QuickConnect {
    /// 从 path 派生默认 name：取路径最后一段目录名。
    /// 根目录 / 空路径回退到 `workspace`。
    pub fn default_name(path: &str) -> String {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return "workspace".to_string();
        }
        let last = last_path_component(trimmed);
        if last.is_empty() || last == "/" {
            return "workspace".to_string();
        }
        last
    }

    /// 面板副标题（小字）：`runtime @ transport`。
    pub fn subtitle(config: &TargetConfig) -> String {
        format!("{} @ {}", config.runtime.label(), config.transport.label())
    }

    /// 该目标是否需要 tmux 按 name attach（tmux 且 name 非空）。
    pub fn should_attach(_existing_name: Option<&str>, config: &TargetConfig) -> bool {
        config.runtime == TargetRuntime::Tmux && !config.name.is_empty()
    }

    /// 展示文本（搜索用）。
    pub fn search_text(config: &TargetConfig) -> String {
        config.search_fields().join(" ").to_lowercase()
    }

    pub fn matches_query(query: &str, config: &TargetConfig) -> bool {
        WorkspaceQuery::new(query).matches(config)
    }

    pub fn search_score(query: &str, config: &TargetConfig) -> Option<i64> {
        WorkspaceQuery::new(query).score(config)
    }

    /// 过滤 + 排序；分数相同保持原顺序。
    pub fn filter_entries(entries: &[QuickConnectEntry], query: &str) -> Vec<QuickConnectEntry> {
        let query = WorkspaceQuery::new(query);
        let mut scored: Vec<(usize, i64, &QuickConnectEntry)> = entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                query
                    .score(&entry.config)
                    .map(|score| (index, score, entry))
            })
            .collect();
        scored.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        scored
            .into_iter()
            .map(|(_, _, entry)| entry.clone())
            .collect()
    }

    /// 目标的唯一 ID（= attach identity）。
    ///
    /// name/path 只在 Herdr identity 尚不完整的 provisional 阶段参与 key。
    pub fn unique_id(config: &TargetConfig) -> String {
        let mut components: Vec<String> = vec![config.runtime.label().to_lowercase()];
        components.extend(transport_identity(&config.transport));
        match config.runtime {
            TargetRuntime::Shell => components.push(if config.path.is_empty() {
                config.name.clone()
            } else {
                config.path.clone()
            }),
            TargetRuntime::Tmux => {
                components.push(non_empty(&config.session).unwrap_or(config.name.clone()));
                components.push(config.socket.clone().unwrap_or_default());
            }
            TargetRuntime::Herdr => {
                match (
                    non_empty(&config.session),
                    non_empty(&config.socket),
                    non_empty(&config.workspace_id),
                ) {
                    (Some(session), Some(socket), Some(workspace_id)) => {
                        components.push(session);
                        components.push(socket);
                        components.push(workspace_id);
                    }
                    _ => {
                        components.clear();
                        components.push("herdr-provisional".to_string());
                        components.extend(transport_identity(&config.transport));
                        components.push(config.name.clone());
                        components.push(config.path.clone());
                    }
                }
            }
        }
        components
            .iter()
            .map(|component| format!("{}:{}", component.len(), component))
            .collect::<Vec<_>>()
            .join("|")
    }

    /// 同一 canonical identity 从 Existing 与 Project 两条入口复用时，
    /// 只用非空 Project 元数据补全展示字段，attach identity 保留 `resolved`。
    pub fn merging_project_metadata(
        resolved: &TargetConfig,
        requested: &TargetConfig,
    ) -> TargetConfig {
        if requested.path.is_empty() {
            return resolved.clone();
        }
        let mut merged = resolved.clone();
        merged.path = requested.path.clone();
        if !requested.name.is_empty() {
            merged.name = requested.name.clone();
        }
        merged
    }

    /// 计算目标应显示哪些标记；顺序固定 Recent 在前、Project 在后。
    pub fn badges(
        config: &TargetConfig,
        recents: &[TargetConfig],
        projects: &[TargetConfig],
    ) -> Vec<QuickBadge> {
        let id = Self::unique_id(config);
        let mut badges = Vec::new();
        if recents
            .iter()
            .any(|candidate| Self::unique_id(candidate) == id)
        {
            badges.push(QuickBadge::Recent);
        }
        if projects
            .iter()
            .any(|candidate| Self::unique_id(candidate) == id)
        {
            badges.push(QuickBadge::Project);
        }
        badges
    }

    /// 面板条目：先最近的前 `recent_limit` 条（最新在前），再补 Project 里
    /// 未出现的目标。按 attach identity 去重。
    pub fn entries(
        recents: &[TargetConfig],
        projects: &[TargetConfig],
        recent_limit: usize,
    ) -> Vec<QuickConnectEntry> {
        let mut seen = std::collections::HashSet::new();
        let mut result = Vec::new();
        for config in recents.iter().take(recent_limit) {
            if !seen.insert(Self::unique_id(config)) {
                continue;
            }
            result.push(QuickConnectEntry {
                config: config.clone(),
                badges: Self::badges(config, recents, projects),
            });
        }
        for config in projects {
            if !seen.insert(Self::unique_id(config)) {
                continue;
            }
            result.push(QuickConnectEntry {
                config: config.clone(),
                badges: Self::badges(config, recents, projects),
            });
        }
        result
    }
}

fn non_empty(value: &Option<String>) -> Option<String> {
    value.as_ref().filter(|value| !value.is_empty()).cloned()
}

/// Local 的 target 是空串；SSH target 是 Host alias。kind 与 target 分开编码，
/// 避免名为 `local` 的 SSH alias 与本机 identity 碰撞。
fn transport_identity(transport: &TargetTransport) -> Vec<String> {
    match transport {
        TargetTransport::Local => vec!["local".to_string(), String::new()],
        TargetTransport::Ssh { name } => vec!["ssh".to_string(), name.clone()],
    }
}

/// 路径最后一段；尾随 `/` 不算（`/a/b/` → `b`）。
fn last_path_component(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_string();
    }
    trimmed.rsplit('/').next().unwrap_or_default().to_string()
}

/// 打开列表的一行（Zed 式：按主机会话分组）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceLauncherRow {
    ConnectSsh,
    Host(String),
    Recent(TargetConfig),
    OpenFolder { host: String },
}

/// 使用次数高的在前；次数相同保持原来的先后。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePickerRank {
    pub id: String,
    pub count: i64,
    pub order: i64,
}

pub struct WorkspacePickerOrder;

impl WorkspacePickerOrder {
    pub fn sorted_ids(ranks: &[WorkspacePickerRank]) -> Vec<String> {
        let mut sorted: Vec<&WorkspacePickerRank> = ranks.iter().collect();
        sorted.sort_by(|left, right| {
            right
                .count
                .cmp(&left.count)
                .then_with(|| left.order.cmp(&right.order))
        });
        sorted.into_iter().map(|rank| rank.id.clone()).collect()
    }
}

pub struct WorkspaceLauncher;

impl WorkspaceLauncher {
    pub fn rows(recents: &[TargetConfig], excluding_ids: &[String]) -> Vec<WorkspaceLauncherRow> {
        let mut hosts: Vec<String> = Vec::new();
        let mut grouped: Vec<(String, Vec<TargetConfig>)> = Vec::new();
        for config in recents {
            let id = QuickConnect::unique_id(config);
            if excluding_ids.contains(&id) {
                continue;
            }
            let host = config.transport.label().to_string();
            if !hosts.contains(&host) {
                hosts.push(host.clone());
                grouped.push((host.clone(), Vec::new()));
            }
            if let Some((_, configs)) = grouped.iter_mut().find(|(name, _)| *name == host) {
                configs.push(config.clone());
            }
        }
        if !hosts.iter().any(|host| host == "local") {
            hosts.push("local".to_string());
        }
        let mut rows = vec![WorkspaceLauncherRow::ConnectSsh];
        for host in hosts {
            rows.push(WorkspaceLauncherRow::Host(host.clone()));
            if let Some((_, configs)) = grouped.iter().find(|(name, _)| *name == host) {
                for config in configs {
                    rows.push(WorkspaceLauncherRow::Recent(config.clone()));
                }
            }
            rows.push(WorkspaceLauncherRow::OpenFolder { host });
        }
        rows
    }

    /// 临时打开一个目录。SSH 用 Herdr，本机用 tmux；两者都不写 Project。
    pub fn temporary_target(path: &str, transport: &TargetTransport) -> TargetConfig {
        let trimmed = path.trim();
        let base = last_path_component(trimmed);
        let name = if base.is_empty() || base == "/" {
            transport.label().to_string()
        } else {
            base
        };
        let runtime = if transport.is_ssh() {
            TargetRuntime::Herdr
        } else {
            TargetRuntime::Tmux
        };
        TargetConfig {
            name,
            runtime,
            transport: transport.clone(),
            path: if trimmed.is_empty() {
                "~".to_string()
            } else {
                trimmed.to_string()
            },
            ..TargetConfig::default()
        }
    }
}

/// SSH 主机补全的一行。已知 alias 与手工输入的主机都可确认。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshHostSuggestion {
    Known(String),
    Typed(String),
}

impl SshHostSuggestion {
    pub fn host(&self) -> &str {
        match self {
            Self::Known(host) | Self::Typed(host) => host,
        }
    }

    pub fn is_typed(&self) -> bool {
        matches!(self, Self::Typed(_))
    }
}

pub struct SshHostSuggestions;

impl SshHostSuggestions {
    pub fn rows(aliases: &[String], query: &str) -> Vec<SshHostSuggestion> {
        let query = query.trim();
        let aliases = unique_aliases(aliases);
        let known: Vec<SshHostSuggestion> = aliases
            .iter()
            .filter(|alias| matches_alias(alias, query))
            .map(|alias| SshHostSuggestion::Known(alias.clone()))
            .collect();
        let mut rows = known;
        let exact = aliases
            .iter()
            .any(|alias| alias.eq_ignore_ascii_case(query));
        if !query.is_empty() && !exact {
            rows.push(SshHostSuggestion::Typed(query.to_string()));
        }
        rows
    }

    /// Tab 补全：优先采用当前选中项，否则只在唯一前缀（或唯一公共前缀）时补全。
    pub fn tab_completion(
        aliases: &[String],
        query: &str,
        selected: Option<&str>,
    ) -> Option<String> {
        let query = query.trim();
        let aliases = unique_aliases(aliases);
        let selected = selected.map(str::trim).unwrap_or_default();
        if !selected.is_empty() && !selected.eq_ignore_ascii_case(query) {
            if let Some(canonical) = aliases
                .iter()
                .find(|alias| alias.eq_ignore_ascii_case(selected))
            {
                if query.is_empty() || matches_alias(canonical, query) {
                    return Some(canonical.clone());
                }
            }
        }
        let lowered = query.to_lowercase();
        let prefixes: Vec<&String> = aliases
            .iter()
            .filter(|alias| !lowered.is_empty() && alias.to_lowercase().starts_with(&lowered))
            .collect();
        if prefixes.len() == 1 && !prefixes[0].eq_ignore_ascii_case(query) {
            return Some(prefixes[0].clone());
        }
        if prefixes.len() > 1 {
            if let Some(shared) = shared_prefix(&prefixes) {
                if shared.chars().count() > query.chars().count() {
                    return Some(shared);
                }
            }
        }
        None
    }
}

fn unique_aliases(aliases: &[String]) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();
    for alias in aliases {
        let alias = alias.trim();
        if alias.is_empty() {
            continue;
        }
        if result
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(alias))
        {
            continue;
        }
        result.push(alias.to_string());
    }
    result
}

fn matches_alias(alias: &str, query: &str) -> bool {
    query.is_empty() || alias.to_lowercase().contains(&query.to_lowercase())
}

/// 所有 alias 的公共前缀；保留第一个 alias 的原始大小写。
fn shared_prefix(values: &[&String]) -> Option<String> {
    let first = values.first()?;
    let mut prefix = first.to_lowercase();
    for value in values.iter().skip(1) {
        let lowered = value.to_lowercase();
        while !prefix.is_empty() && !lowered.starts_with(&prefix) {
            prefix.pop();
        }
    }
    if prefix.is_empty() {
        return None;
    }
    Some(first.chars().take(prefix.chars().count()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(path: &str) -> TargetConfig {
        TargetConfig::new(
            QuickConnect::default_name(path),
            TargetRuntime::Shell,
            TargetTransport::Local,
            path,
        )
    }

    fn tmux(workspace: &str, session: &str, path: &str) -> TargetConfig {
        TargetConfig {
            name: workspace.to_string(),
            runtime: TargetRuntime::Tmux,
            transport: TargetTransport::Local,
            path: path.to_string(),
            session: Some(session.to_string()),
            socket: Some("/tmp/tmux-501/default".to_string()),
            ..TargetConfig::default()
        }
    }

    fn ssh(name: &str, host: &str) -> TargetConfig {
        TargetConfig {
            name: name.to_string(),
            runtime: TargetRuntime::Herdr,
            transport: TargetTransport::Ssh {
                name: host.to_string(),
            },
            path: format!("/srv/{name}"),
            session: Some("named".to_string()),
            socket: Some("/run/herdr.sock".to_string()),
            workspace_id: Some("w3".to_string()),
            ..TargetConfig::default()
        }
    }

    /// 对照 `UnifiedPanelExistingConnectionsE2ETests.testConnectedWorkspaceSearchCoversPathRuntimeAliasSessionAndIdentity`。
    /// 普通查询仍涵盖 runtime/socket；名字真正匹配的 muxterm 排在泛 tmux 命中前面。
    #[test]
    fn query_keeps_runtime_and_socket_matches_and_prefers_names() {
        let muxterm = tmux("muxterm", "muxterm", "~/Developer/muxterm");
        let other = tmux("ubuntu-home", "ubuntu", "~");
        let query = WorkspaceQuery::new("mux");
        assert!(query.matches(&other), "tmux 仍属于普通搜索字段");
        assert!(query.score(&muxterm).unwrap() > query.score(&other).unwrap());
        assert!(WorkspaceQuery::new("501/default").matches(&other));
        assert!(WorkspaceQuery::new("@local @tmux muxterm").matches(&muxterm));
        assert!(!WorkspaceQuery::new("@local @tmux muxterm").matches(&other));
    }

    /// 对照 `QuickConnectE2ETests.testStartupLocalWorkspaceAppearsThroughFixedShellsItem`。
    #[test]
    fn default_name_uses_the_last_directory() {
        assert_eq!(
            QuickConnect::default_name("/Users/dev/Developer/self/muxterm"),
            "muxterm"
        );
        assert_eq!(QuickConnect::default_name("/Users/dev/project/"), "project");
        assert_eq!(QuickConnect::default_name("/"), "workspace");
        assert_eq!(QuickConnect::default_name(""), "workspace");
        assert_eq!(QuickConnect::default_name("   "), "workspace");
        assert_eq!(QuickConnect::default_name("relative/repo"), "repo");
    }

    /// 副标题是 `runtime @ transport`。
    #[test]
    fn subtitle_names_runtime_and_transport() {
        assert_eq!(QuickConnect::subtitle(&local("/tmp/x")), "shell @ local");
        assert_eq!(
            QuickConnect::subtitle(&ssh("api", "ryzen")),
            "herdr @ ryzen"
        );
    }

    /// 只有 tmux 且 name 非空才按 name attach。
    #[test]
    fn should_attach_only_for_named_tmux() {
        let mut config = tmux("api", "api", "/srv/api");
        assert!(QuickConnect::should_attach(None, &config));
        config.name.clear();
        assert!(!QuickConnect::should_attach(None, &config));
        let herdr = ssh("api", "ryzen");
        assert!(!QuickConnect::should_attach(None, &herdr));
    }

    /// 对照 `UnifiedPanelExistingConnectionsE2ETests.testConnectedWorkspaceSearchCoversPathRuntimeAliasSessionAndIdentity`。
    #[test]
    fn search_covers_path_runtime_alias_session_and_identity() {
        let config = ssh("api", "ryzen");
        assert!(QuickConnect::matches_query("api", &config));
        assert!(QuickConnect::matches_query("/srv/api", &config));
        assert!(QuickConnect::matches_query("herdr", &config));
        assert!(QuickConnect::matches_query("ryzen", &config));
        assert!(QuickConnect::matches_query("named", &config));
        assert!(QuickConnect::matches_query("w3", &config));
        assert!(!QuickConnect::matches_query("missing", &config));
    }

    /// `@runtime` / `@local` / `@alias` 过滤可以组合，且必须同时满足。
    #[test]
    fn query_filters_combine_with_and_semantics() {
        let config = ssh("api", "ryzen");
        assert!(QuickConnect::matches_query("@herdr", &config));
        assert!(QuickConnect::matches_query("@her", &config), "唯一前缀也认");
        assert!(!QuickConnect::matches_query("@tmux", &config));
        assert!(!QuickConnect::matches_query("@local", &config));
        assert!(QuickConnect::matches_query("@ryzen", &config));
        assert!(QuickConnect::matches_query("@ryz", &config), "alias 前缀");
        assert!(QuickConnect::matches_query("@ryz @herdr api", &config));
        assert!(!QuickConnect::matches_query("@ryz @tmux", &config));
        assert!(
            QuickConnect::matches_query("@lo", &local("/tmp/x")),
            "@lo 是 @local 的唯一前缀"
        );
    }

    /// 子序列匹配与分数：包含命中比模糊命中分数高，runtime 过滤再加权。
    #[test]
    fn scores_prefer_substrings_then_filters() {
        let config = local("/Users/dev/muxterm");
        let contains = QuickConnect::search_score("muxterm", &config).expect("contains");
        let fuzzy = QuickConnect::search_score("mxtrm", &config).expect("subsequence");
        assert!(contains > fuzzy, "{contains} 应该高于 {fuzzy}");

        let filtered = QuickConnect::search_score("@shell muxterm", &config).expect("filtered");
        assert_eq!(filtered, contains + 2_000, "runtime 过滤加分");
    }

    /// 空查询匹配所有目标，也是最弱的分数。
    #[test]
    fn empty_query_matches_everything() {
        let query = WorkspaceQuery::new("");
        assert!(query.is_empty());
        assert_eq!(query.score(&local("/tmp/x")), Some(10_000));
    }

    /// 补全候选：带 `@`、按前缀模糊过滤、去重。
    #[test]
    fn completion_candidates_filter_by_prefix() {
        let aliases = vec!["ryzen".to_string(), "Ryzen".to_string(), "".to_string()];
        let all = WorkspaceQuery::completion_candidates("api @", &aliases);
        assert_eq!(all, ["@shell", "@tmux", "@herdr", "@local", "@ryzen"]);

        let filtered = WorkspaceQuery::completion_candidates("api @tm", &[]);
        assert_eq!(filtered, ["@tmux"], "只留匹配前缀的 runtime");

        let local = WorkspaceQuery::completion_candidates("@loc", &[]);
        assert_eq!(local, ["@local"]);

        assert!(
            WorkspaceQuery::completion_candidates("api", &[]).is_empty(),
            "没有 @ token 就没有候选"
        );
        assert!(
            WorkspaceQuery::completion_candidates("api @ ", &[]).is_empty(),
            "刚敲完空格还没开始 token"
        );
    }

    /// 补全只替换最后一个 token。
    #[test]
    fn completion_replaces_the_last_token_only() {
        assert_eq!(
            WorkspaceQuery::replace_current_token("api @tm", "@tmux"),
            "api @tmux"
        );
        assert_eq!(
            WorkspaceQuery::replace_current_token("@tm", "@tmux"),
            "@tmux"
        );
        assert_eq!(
            WorkspaceQuery::replace_current_token("api  @tm", "@tmux"),
            "api  @tmux",
            "分隔空白原样保留"
        );
    }

    /// attach identity：session/socket 不同就是不同目标。
    #[test]
    fn unique_id_tracks_the_attach_identity() {
        let first = tmux("api", "api-one", "/srv/api");
        let second = tmux("api", "api-two", "/srv/api");
        assert_ne!(
            QuickConnect::unique_id(&first),
            QuickConnect::unique_id(&second),
            "session 变了 identity 就变"
        );

        let mut same_session_other_socket = first.clone();
        same_session_other_socket.socket = Some("/tmp/other".to_string());
        assert_ne!(
            QuickConnect::unique_id(&first),
            QuickConnect::unique_id(&same_session_other_socket)
        );

        assert_eq!(
            QuickConnect::unique_id(&first),
            QuickConnect::unique_id(&first.clone())
        );
    }

    /// 本地与 SSH 的同名目标不能碰撞，长度前缀挡住分隔符碰撞。
    #[test]
    fn unique_id_separates_transports_and_escapes_delimiters() {
        let mut local_target = tmux("api", "api", "/srv/api");
        let remote = TargetConfig {
            transport: TargetTransport::Ssh {
                name: "local".to_string(),
            },
            ..local_target.clone()
        };
        assert_ne!(
            QuickConnect::unique_id(&local_target),
            QuickConnect::unique_id(&remote),
            "名为 local 的 SSH alias 不等于本机"
        );

        // 分隔符碰撞：("a|b", "c") 与 ("a", "b|c") 必须不同。
        local_target.session = Some("a|b".to_string());
        local_target.socket = Some("c".to_string());
        let mut other = local_target.clone();
        other.session = Some("a".to_string());
        other.socket = Some("b|c".to_string());
        assert_ne!(
            QuickConnect::unique_id(&local_target),
            QuickConnect::unique_id(&other),
            "长度前缀要挡住分隔符碰撞"
        );
    }

    /// Herdr 身份完整时用 session/socket/workspace；缺一项就是 provisional。
    #[test]
    fn herdr_identity_falls_back_to_provisional() {
        let complete = ssh("api", "ryzen");
        assert_eq!(
            QuickConnect::unique_id(&complete),
            "5:herdr|3:ssh|5:ryzen|5:named|15:/run/herdr.sock|2:w3"
        );

        let mut provisional = complete.clone();
        provisional.workspace_id = Some(String::new());
        let id = QuickConnect::unique_id(&provisional);
        assert!(
            id.starts_with("17:herdr-provisional|3:ssh|5:ryzen|"),
            "{id}"
        );
    }

    /// 对照 `UnifiedPanelExistingConnectionsE2ETests.testHerdrExistingRowKeepsIdentityAndUsesAttachOnlyCallback`。
    #[test]
    fn merging_project_metadata_keeps_the_resolved_identity() {
        let resolved = ssh("api", "ryzen");
        let requested = TargetConfig {
            path: "/srv/api-v2".to_string(),
            name: "api-v2".to_string(),
            ..ssh("ignored", "ryzen")
        };
        let merged = QuickConnect::merging_project_metadata(&resolved, &requested);
        assert_eq!(merged.path, "/srv/api-v2");
        assert_eq!(merged.name, "api-v2");
        assert_eq!(
            QuickConnect::unique_id(&merged),
            QuickConnect::unique_id(&resolved),
            "attach identity 保留 resolved 的值"
        );

        let without_path = TargetConfig {
            path: String::new(),
            name: "ignored".to_string(),
            ..requested
        };
        let merged = QuickConnect::merging_project_metadata(&resolved, &without_path);
        assert_eq!(merged.name, "api", "没有 path 就完全用 resolved");
        assert_eq!(merged.path, resolved.path);
    }

    /// Recent 与 Project 标记顺序固定，且按 identity 匹配。
    #[test]
    fn badges_put_recent_before_project() {
        let config = tmux("api", "api", "/srv/api");
        let recents = vec![config.clone()];
        let projects = vec![config.clone()];
        assert_eq!(
            QuickConnect::badges(&config, &recents, &projects),
            [QuickBadge::Recent, QuickBadge::Project]
        );
        assert_eq!(
            QuickConnect::badges(&config, &[], &projects),
            [QuickBadge::Project]
        );

        let renamed = TargetConfig {
            name: "renamed".to_string(),
            ..config.clone()
        };
        assert_eq!(
            QuickConnect::badges(&renamed, &recents, &projects),
            [QuickBadge::Recent, QuickBadge::Project],
            "name 不参与 attach identity"
        );
    }

    /// 对照 `UnifiedPanelLauncherE2ETests.testRootListsOnlyOpenProjectsAndWorkspacesByFrequency`。
    #[test]
    fn entries_take_the_recent_window_then_projects_without_duplicates() {
        let first = tmux("api", "api", "/srv/api");
        let second = tmux("web", "web", "/srv/web");
        let third = tmux("db", "db", "/srv/db");
        let entries = QuickConnect::entries(
            &[first.clone(), second.clone(), third],
            &[second.clone(), first.clone()],
            2,
        );
        let names: Vec<&str> = entries
            .iter()
            .map(|entry| entry.config.name.as_str())
            .collect();
        assert_eq!(names, ["api", "web"], "最近窗口 2 条，Project 不再重复");
        assert_eq!(
            entries[0].badges,
            [QuickBadge::Recent, QuickBadge::Project],
            "两条入口都有的目标带两个标记"
        );
    }

    /// 搜索过滤后按分数排序，同分保持原顺序。
    #[test]
    fn filtering_sorts_by_score_and_keeps_ties_in_order() {
        let api = QuickConnectEntry {
            config: tmux("api", "api", "/srv/api"),
            badges: Vec::new(),
        };
        let api_extra = QuickConnectEntry {
            config: tmux("api-extra", "api-extra", "/srv/api-extra"),
            badges: Vec::new(),
        };
        let entries = vec![api_extra.clone(), api.clone()];
        let filtered = QuickConnect::filter_entries(&entries, "api");
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].config.name, "api", "短字段分数更高，排前面");
    }

    /// 打开列表：先连接 SSH，再按主机分组，最后每台机器一条「打开目录」。
    #[test]
    fn launcher_rows_group_by_host_and_always_offer_local() {
        let recents = vec![
            ssh("api", "ryzen"),
            local("/srv/local-one"),
            ssh("web", "ryzen"),
        ];
        let rows = WorkspaceLauncher::rows(&recents, &[]);
        let shape: Vec<String> = rows
            .iter()
            .map(|row| match row {
                WorkspaceLauncherRow::ConnectSsh => "connect".to_string(),
                WorkspaceLauncherRow::Host(host) => format!("host:{host}"),
                WorkspaceLauncherRow::Recent(config) => format!("recent:{}", config.name),
                WorkspaceLauncherRow::OpenFolder { host } => format!("open:{host}"),
            })
            .collect();
        assert_eq!(
            shape,
            [
                "connect",
                "host:ryzen",
                "recent:api",
                "recent:web",
                "open:ryzen",
                "host:local",
                "recent:local-one",
                "open:local",
            ],
            "同一台机器上的 recents 归到它自己的 host 段下"
        );

        let excluded = vec![QuickConnect::unique_id(&recents[0])];
        let rows = WorkspaceLauncher::rows(&recents, &excluded);
        assert!(
            !rows.iter().any(|row| matches!(
                row,
                WorkspaceLauncherRow::Recent(config) if config.name == "api"
            )),
            "被排除的目标不出现"
        );
    }

    /// 临时目录：SSH 走 Herdr，本机走 tmux，空路径回退 `~`。
    #[test]
    fn temporary_targets_pick_the_runtime_from_the_transport() {
        let local_target =
            WorkspaceLauncher::temporary_target("/srv/api/", &TargetTransport::Local);
        assert_eq!(local_target.runtime, TargetRuntime::Tmux);
        assert_eq!(local_target.name, "api");
        assert_eq!(local_target.path, "/srv/api/");

        let remote = WorkspaceLauncher::temporary_target(
            "/srv/api",
            &TargetTransport::Ssh {
                name: "ryzen".into(),
            },
        );
        assert_eq!(remote.runtime, TargetRuntime::Herdr);

        let empty = WorkspaceLauncher::temporary_target("   ", &TargetTransport::Local);
        assert_eq!(empty.path, "~");
        assert_eq!(empty.name, "local", "没有目录段就用 transport 标签");
    }

    /// 使用次数高的在前，同次数保持原顺序。
    #[test]
    fn picker_order_sorts_by_count_then_insertion() {
        let ranks = vec![
            WorkspacePickerRank {
                id: "a".into(),
                count: 3,
                order: 2,
            },
            WorkspacePickerRank {
                id: "b".into(),
                count: 5,
                order: 3,
            },
            WorkspacePickerRank {
                id: "c".into(),
                count: 3,
                order: 1,
            },
        ];
        assert_eq!(WorkspacePickerOrder::sorted_ids(&ranks), ["b", "c", "a"]);
    }

    /// SSH 建议：已知 alias 命中在前，手工输入作为可确认的额外一行。
    #[test]
    fn ssh_suggestions_offer_known_aliases_then_the_typed_host() {
        let aliases = vec!["ryzen".to_string(), "Ryzen".to_string(), "box".to_string()];
        let rows = SshHostSuggestions::rows(&aliases, "ryz");
        assert_eq!(
            rows,
            [
                SshHostSuggestion::Known("ryzen".to_string()),
                SshHostSuggestion::Typed("ryz".to_string())
            ]
        );

        let exact = SshHostSuggestions::rows(&aliases, "ryzen");
        assert_eq!(exact, [SshHostSuggestion::Known("ryzen".to_string())]);

        let empty = SshHostSuggestions::rows(&aliases, "");
        assert_eq!(empty.len(), 2, "空查询列出全部已知 alias，不产生 typed");
        assert!(empty.iter().all(|row| !row.is_typed()));
    }

    /// Tab 补全：选中项优先，其次唯一前缀，再其次唯一公共前缀。
    #[test]
    fn ssh_tab_completion_prefers_selection_then_unique_prefix() {
        let aliases = vec![
            "ryzen".to_string(),
            "ryzen-b".to_string(),
            "box".to_string(),
        ];
        assert_eq!(
            SshHostSuggestions::tab_completion(&aliases, "ryz", Some("ryzen-b")),
            Some("ryzen-b".to_string()),
            "选中项本身匹配查询时优先采用"
        );
        assert_eq!(
            SshHostSuggestions::tab_completion(&aliases, "ryz", Some("box")),
            Some("ryzen".to_string()),
            "选中项不匹配查询就退回前缀补全"
        );
        assert_eq!(
            SshHostSuggestions::tab_completion(&aliases, "r", None),
            Some("ryzen".to_string()),
            "两个前缀时有唯一公共前缀就补到那里"
        );
        assert_eq!(
            SshHostSuggestions::tab_completion(&aliases, "bo", None),
            Some("box".to_string())
        );
        assert_eq!(
            SshHostSuggestions::tab_completion(&aliases, "ryzen", None),
            None,
            "已经完整就不再补"
        );
        assert_eq!(
            SshHostSuggestions::tab_completion(&aliases, "", None),
            None,
            "空查询不猜"
        );

        let two = vec!["alpha".to_string(), "beta".to_string()];
        assert_eq!(
            SshHostSuggestions::tab_completion(&two, "a", None),
            Some("alpha".to_string())
        );
        assert_eq!(
            SshHostSuggestions::tab_completion(&two, "b", None),
            Some("beta".to_string())
        );
    }
}
