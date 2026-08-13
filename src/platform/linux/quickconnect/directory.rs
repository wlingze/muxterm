//! 目录输入 / 候选选择的纯路径模型（与 macOS DirectorySuggestion 一致）。
//!
//! - 输入最后一段是补全前缀，列表请求针对父目录；
//! - 尾部 `/` 表示已确定进入该目录；
//! - 选择候选只替换当前输入段，绝不重复拼接 basename；
//! - `~`、`/`、`.`、`..` 与空输入按目录语义归一化。

/// 目录路径的纯函数模型。
pub enum DirectoryPathModel {}

impl DirectoryPathModel {
    /// 列表请求应针对的目录。
    pub fn base_directory(raw: &str) -> String {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return "~".into();
        }
        if trimmed == "~" || trimmed == "~/" {
            return "~".into();
        }
        if trimmed == "/" {
            return "/".into();
        }
        if let Some(rest) = trimmed.strip_prefix("~/") {
            if has_trailing_slash(rest) {
                return format!("~/{}", trim_trailing_slashes(rest));
            }
            let parent = parent_path(rest);
            return if parent.is_empty() {
                "~".into()
            } else {
                format!("~/{parent}")
            };
        }
        if trimmed.starts_with('/') {
            return if has_trailing_slash(trimmed) {
                trim_trailing_slashes(trimmed)
            } else {
                parent_path(trimmed)
            };
        }
        // 相对路径：父目录是当前目录
        if has_trailing_slash(trimmed) {
            trim_trailing_slashes(trimmed)
        } else {
            ".".into()
        }
    }

    /// 当前输入的最后一节（补全过滤前缀）。
    pub fn input_prefix(raw: &str) -> String {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed == "~" || trimmed == "/" {
            return String::new();
        }
        if has_trailing_slash(trimmed) {
            return String::new();
        }
        normalized_components(trimmed)
            .last()
            .cloned()
            .unwrap_or_default()
    }

    /// 选择候选 = 进入该目录：候选替换当前输入段。candidate 必须是纯目录名。
    pub fn applying_selection(candidate: &str, raw: &str) -> String {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return format!("{candidate}/");
        }
        let base = Self::base_directory(trimmed);
        match base.as_str() {
            "/" => format!("/{candidate}/"),
            "~" => format!("~/{candidate}/"),
            "." => format!("{candidate}/"),
            _ => format!("{base}/{candidate}/"),
        }
    }

    /// 上级目录。
    pub fn applying_go_up(raw: &str) -> String {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed == "~" || trimmed == "~/" {
            return "~".into();
        }
        if trimmed == "/" {
            return "/".into();
        }
        let without_trailing = trim_trailing_slashes(trimmed);
        if without_trailing.is_empty() {
            return "/".into();
        }
        if let Some(rest) = without_trailing.strip_prefix("~/") {
            let parent = parent_path(rest);
            return if parent.is_empty() { "~/".into() } else { format!("~/{parent}/") };
        }
        if without_trailing.starts_with('/') {
            let parent = parent_path(&without_trailing);
            return if parent.is_empty() || parent == "/" {
                "/".into()
            } else {
                format!("{parent}/")
            };
        }
        let parent = parent_path(&without_trailing);
        if parent.is_empty() {
            ".".into()
        } else {
            format!("{parent}/")
        }
    }

    /// 归一化路径：去尾斜杠、处理 `.` / `..`；空 → `~`。
    pub fn resolved_path(raw: &str) -> String {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed == "~" || trimmed == "~/" {
            return "~".into();
        }
        if trimmed == "/" {
            return "/".into();
        }
        let is_tilde = trimmed.starts_with("~/");
        let body = if is_tilde { &trimmed[2..] } else { trimmed };
        let is_absolute = body.starts_with('/');
        let mut stack: Vec<String> = Vec::new();
        for component in body.split('/') {
            match component {
                "" | "." => continue,
                ".." => {
                    if let Some(last) = stack.last() {
                        if last != ".." {
                            stack.pop();
                            continue;
                        }
                    }
                    if !is_absolute && !is_tilde {
                        stack.push("..".into());
                    }
                }
                c => stack.push(c.to_string()),
            }
        }
        let joined = stack.join("/");
        if is_tilde {
            if joined.is_empty() {
                "~".into()
            } else {
                format!("~/{joined}")
            }
        } else if is_absolute {
            format!("/{joined}")
        } else if joined.is_empty() {
            ".".into()
        } else {
            joined
        }
    }
}

fn has_trailing_slash(value: &str) -> bool {
    value.ends_with('/')
}

fn trim_trailing_slashes(value: &str) -> String {
    let mut result = value.to_string();
    while result.ends_with('/') {
        result.pop();
    }
    result
}

fn parent_path(value: &str) -> String {
    let parts: Vec<&str> = value.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() <= 1 {
        return String::new();
    }
    let joined = parts[..parts.len() - 1].join("/");
    if value.starts_with('/') {
        format!("/{joined}")
    } else {
        joined
    }
}

fn normalized_components(value: &str) -> Vec<String> {
    value
        .split('/')
        .filter(|p| !p.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// 一次目录列表请求的完整标识：generation + 请求 key。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryListingRequest {
    pub generation: u64,
    pub path: String,
    pub is_ssh: bool,
    pub alias: Option<String>,
}

/// 异步目录列表响应。只有与当前请求完全一致的响应才允许应用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryListingResponse {
    pub request: DirectoryListingRequest,
    pub directories: Vec<String>,
}

/// 目录补全控制器：管理当前输入、请求 generation 与候选应用。
#[derive(Debug, Clone)]
pub struct DirectorySuggestionController {
    pub text: String,
    pub is_ssh: bool,
    pub alias: Option<String>,
    pub candidates: Vec<String>,
    generation: u64,
}

impl Default for DirectorySuggestionController {
    fn default() -> Self {
        Self::new("~")
    }
}

impl DirectorySuggestionController {
    pub fn new(path: &str) -> Self {
        DirectorySuggestionController {
            text: path.to_string(),
            is_ssh: false,
            alias: None,
            candidates: Vec::new(),
            generation: 0,
        }
    }

    /// 当前请求快照（path = 父目录/当前目录，不含输入前缀）。
    pub fn request(&self) -> DirectoryListingRequest {
        DirectoryListingRequest {
            generation: self.generation,
            path: DirectoryPathModel::base_directory(&self.text),
            is_ssh: self.is_ssh,
            alias: if self.is_ssh { self.alias.clone() } else { None },
        }
    }

    /// 输入变化：更新文本、作废旧候选与旧请求，返回新请求。
    pub fn update_input(&mut self, new_text: &str) -> DirectoryListingRequest {
        self.text = new_text.trim().to_string();
        self.invalidate();
        self.request()
    }

    /// 选择候选：仅接受纯目录名；进入该目录并返回新请求。
    pub fn select(&mut self, candidate: &str) -> DirectoryListingRequest {
        let trimmed = candidate.trim().to_string();
        if trimmed.is_empty() || trimmed.contains('/') {
            return self.request();
        }
        if self.last_entered_component(&self.text) == Some(trimmed.clone()) {
            return self.request();
        }
        let next = DirectoryPathModel::applying_selection(&trimmed, &self.text);
        if next == self.text {
            return self.request();
        }
        self.text = next;
        self.invalidate();
        self.request()
    }

    /// 上级目录。
    pub fn go_up(&mut self) -> DirectoryListingRequest {
        self.text = DirectoryPathModel::applying_go_up(&self.text);
        self.invalidate();
        self.request()
    }

    /// transport / SSH alias 变化：作废旧候选，返回新请求。
    pub fn set_transport(&mut self, is_ssh: bool, alias: Option<&str>) -> DirectoryListingRequest {
        self.is_ssh = is_ssh;
        self.alias = if is_ssh { alias.map(|s| s.to_string()) } else { None };
        self.invalidate();
        self.request()
    }

    /// 应用异步响应。只有与当前请求完全一致的响应才会更新候选。
    pub fn apply(&mut self, response: &DirectoryListingResponse) -> bool {
        if response.request != self.request() {
            return false;
        }
        let prefix = DirectoryPathModel::input_prefix(&self.text);
        self.candidates = response
            .directories
            .iter()
            .map(|d| d.trim().to_string())
            .filter(|d| !d.is_empty() && !d.contains('/'))
            .filter(|d| prefix.is_empty() || d.starts_with(&prefix))
            .collect();
        self.candidates.sort();
        true
    }

    fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.candidates.clear();
    }

    fn last_entered_component(&self, raw: &str) -> Option<String> {
        let trimmed = raw.trim();
        if !trimmed.ends_with('/') || trimmed == "/" || trimmed == "~/" {
            return None;
        }
        let without_trailing = trim_trailing_slashes(trimmed);
        if without_trailing == "~" {
            return None;
        }
        without_trailing
            .split('/')
            .last()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_directory_uses_parent_for_incomplete_input() {
        assert_eq!(DirectoryPathModel::base_directory("~/Devel"), "~");
        assert_eq!(DirectoryPathModel::base_directory("~/Developer/self"), "~/Developer");
        assert_eq!(DirectoryPathModel::base_directory("~/Developer/self/"), "~/Developer/self");
        assert_eq!(DirectoryPathModel::base_directory("/usr/li"), "/usr");
        assert_eq!(DirectoryPathModel::base_directory("/"), "/");
        assert_eq!(DirectoryPathModel::base_directory(""), "~");
    }

    #[test]
    fn input_prefix_last_component() {
        assert_eq!(DirectoryPathModel::input_prefix("~/Devel"), "Devel");
        assert_eq!(DirectoryPathModel::input_prefix("~/Developer/self/"), "");
        assert_eq!(DirectoryPathModel::input_prefix("/"), "");
    }

    #[test]
    fn selection_replaces_input_segment() {
        assert_eq!(DirectoryPathModel::applying_selection("muxterm", "~/Devel"), "~/muxterm/");
        assert_eq!(DirectoryPathModel::applying_selection("etc", "/u"), "/etc/");
    }

    #[test]
    fn go_up_navigates() {
        assert_eq!(DirectoryPathModel::applying_go_up("~/Developer/self/muxterm/"), "~/Developer/self/");
        assert_eq!(DirectoryPathModel::applying_go_up("~/"), "~");
        assert_eq!(DirectoryPathModel::applying_go_up("/usr"), "/");
    }

    #[test]
    fn resolved_path_normalizes_dots() {
        assert_eq!(DirectoryPathModel::resolved_path("~/a/./b/../c"), "~/a/c");
        assert_eq!(DirectoryPathModel::resolved_path("/a/../b"), "/b");
        assert_eq!(DirectoryPathModel::resolved_path(""), "~");
        assert_eq!(DirectoryPathModel::resolved_path("a/../../b"), "../b");
    }

    #[test]
    fn controller_discards_stale_responses() {
        let mut c = DirectorySuggestionController::new("~/");
        let req1 = c.request();
        c.update_input("~/Devel");
        let stale = DirectoryListingResponse { request: req1, directories: vec!["old".into()] };
        assert!(!c.apply(&stale));
        assert!(c.candidates.is_empty());
    }

    #[test]
    fn controller_filters_by_prefix() {
        let mut c = DirectorySuggestionController::new("~/Devel");
        let req = c.request();
        let resp = DirectoryListingResponse {
            request: req,
            directories: vec!["Documents".into(), "Developer".into(), "Downloads".into()],
        };
        assert!(c.apply(&resp));
        assert_eq!(c.candidates, vec!["Developer"]);
    }

    #[test]
    fn controller_select_idempotent() {
        let mut c = DirectorySuggestionController::new("~/Devel/");
        c.select("Devel");
        assert_eq!(c.text, "~/Devel/");
        c.select("foo");
        assert_eq!(c.text, "~/Devel/foo/");
    }
}
