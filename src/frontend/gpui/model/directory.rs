//! 目录补全：路径归一化 + 异步列表请求的 generation 守卫。
//!
//! 移植自 macOS `Chrome/DirectorySuggestion.swift`。`DirectorySuggestionController`
//! 是纯逻辑：调用方在发请求前拿 `request` 快照，响应回来时交给 `apply`，
//! generation / path / transport / alias 任一不同的响应都会被丢弃。

/// 一次目录列表请求的完整标识。
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

pub struct DirectoryPathModel;

/// 当前用户的 home 目录（绝对路径）。
///
/// 拿不到环境变量时返回 `None`：调用方不能猜一个假路径交给 Core。
pub fn home_directory() -> Option<String> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// 展开开头的 `~`。
///
/// Core 的 `list_dir` 只认字面 `"~"`，而 tmux / Herdr 的 `-c` 不会再展开
/// `~/x`；本地路径必须在传给 Core 之前变成绝对路径，否则目录列不出来、
/// 新建的 workspace 也打不开。SSH 路径不要调这个函数：远端 shell 自己展开。
pub fn expand_home(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed == "~" || trimmed == "~/" {
        return home_directory().unwrap_or_else(|| trimmed.to_string());
    }
    let Some(rest) = trimmed.strip_prefix("~/") else {
        return trimmed.to_string();
    };
    match home_directory() {
        Some(home) => format!("{}/{}", home.trim_end_matches('/'), rest),
        None => trimmed.to_string(),
    }
}

impl DirectoryPathModel {
    /// 列表请求应针对的目录。
    pub fn base_directory(raw: &str) -> String {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return "~".to_string();
        }
        if trimmed == "~" || trimmed == "~/" {
            return "~".to_string();
        }
        if trimmed == "/" {
            return "/".to_string();
        }
        if let Some(rest) = trimmed.strip_prefix("~/") {
            if has_trailing_slash(rest) {
                return format!("~/{}", trimming_trailing_slashes(rest));
            }
            let parent = parent_path(rest);
            return if parent.is_empty() {
                "~".to_string()
            } else {
                format!("~/{parent}")
            };
        }
        if trimmed.starts_with('/') {
            return if has_trailing_slash(trimmed) {
                trimming_trailing_slashes(trimmed)
            } else {
                parent_path(trimmed)
            };
        }
        // 相对路径（如 "foo"）：父目录是当前目录。
        if has_trailing_slash(trimmed) {
            trimming_trailing_slashes(trimmed)
        } else {
            ".".to_string()
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
        normalized_components(trimmed).pop().unwrap_or_default()
    }

    /// 选择候选 = 进入该目录：候选**替换当前输入段**，绝不拼到完整路径上。
    /// candidate 必须是纯目录名（不含 `/`）。
    pub fn applying_selection(candidate: &str, to: &str) -> String {
        let trimmed = to.trim();
        if trimmed.is_empty() {
            return format!("{candidate}/");
        }
        match Self::base_directory(trimmed).as_str() {
            "/" => format!("/{candidate}/"),
            "~" => format!("~/{candidate}/"),
            "." => format!("{candidate}/"),
            base => format!("{base}/{candidate}/"),
        }
    }

    /// 上级目录。
    pub fn applying_go_up(to: &str) -> String {
        let trimmed = to.trim();
        if trimmed.is_empty() {
            return "~".to_string();
        }
        if trimmed == "~" || trimmed == "~/" {
            return "~".to_string();
        }
        if trimmed == "/" {
            return "/".to_string();
        }
        let without_trailing = trimming_trailing_slashes(trimmed);
        if without_trailing.is_empty() {
            return "/".to_string();
        }
        if let Some(rest) = without_trailing.strip_prefix("~/") {
            let parent = parent_path(rest);
            return if parent.is_empty() {
                "~/".to_string()
            } else {
                format!("~/{parent}/")
            };
        }
        if without_trailing.starts_with('/') {
            let parent = parent_path(&without_trailing);
            return if parent.is_empty() || parent == "/" {
                "/".to_string()
            } else {
                format!("{parent}/")
            };
        }
        let parent = parent_path(&without_trailing);
        if parent.is_empty() {
            ".".to_string()
        } else {
            format!("{parent}/")
        }
    }

    /// 归一化路径：去尾斜杠、处理 `.` / `..`；空 → `~`。
    pub fn resolved_path(raw: &str) -> String {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return "~".to_string();
        }
        if trimmed == "/" {
            return "/".to_string();
        }
        if trimmed == "~" || trimmed == "~/" {
            return "~".to_string();
        }

        let is_tilde = trimmed.starts_with("~/");
        let body = if is_tilde { &trimmed[2..] } else { trimmed };
        let mut stack: Vec<&str> = Vec::new();
        let is_absolute = body.starts_with('/');
        for component in body.split('/').filter(|part| !part.is_empty()) {
            match component {
                "." => continue,
                ".." => {
                    if let Some(last) = stack.last() {
                        if *last != ".." {
                            stack.pop();
                        } else if !is_absolute && !is_tilde {
                            stack.push("..");
                        }
                    } else if !is_absolute && !is_tilde {
                        stack.push("..");
                    }
                }
                other => stack.push(other),
            }
        }
        let joined = stack.join("/");
        if is_tilde {
            return if joined.is_empty() {
                "~".to_string()
            } else {
                format!("~/{joined}")
            };
        }
        if is_absolute {
            return format!("/{joined}");
        }
        if joined.is_empty() {
            ".".to_string()
        } else {
            joined
        }
    }
}

fn has_trailing_slash(value: &str) -> bool {
    value.ends_with('/')
}

fn trimming_trailing_slashes(value: &str) -> String {
    value.trim_end_matches('/').to_string()
}

fn parent_path(value: &str) -> String {
    let parts: Vec<&str> = value.split('/').filter(|part| !part.is_empty()).collect();
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
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

/// 目录补全控制器（纯逻辑）：当前输入、请求 generation 与候选应用。
#[derive(Debug, Clone)]
pub struct DirectorySuggestionController {
    text: String,
    is_ssh: bool,
    alias: Option<String>,
    candidates: Vec<String>,
    generation: u64,
}

impl Default for DirectorySuggestionController {
    fn default() -> Self {
        Self::new("~")
    }
}

impl DirectorySuggestionController {
    pub fn new(path: &str) -> Self {
        Self {
            text: path.to_string(),
            is_ssh: false,
            alias: None,
            candidates: Vec::new(),
            generation: 0,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_ssh(&self) -> bool {
        self.is_ssh
    }

    pub fn alias(&self) -> Option<&str> {
        self.alias.as_deref()
    }

    pub fn candidates(&self) -> &[String] {
        &self.candidates
    }

    /// 当前请求快照（path = 父目录/当前目录，不含输入前缀）。
    pub fn request(&self) -> DirectoryListingRequest {
        DirectoryListingRequest {
            generation: self.generation,
            path: DirectoryPathModel::base_directory(&self.text),
            is_ssh: self.is_ssh,
            alias: if self.is_ssh {
                self.alias.clone()
            } else {
                None
            },
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
        let trimmed = candidate.trim();
        if trimmed.is_empty() || trimmed.contains('/') {
            return self.request();
        }
        // 已进入同名字目录（尾斜杠）：重复选择保持幂等，不再次拼接。
        if self.last_entered_component().as_deref() == Some(trimmed) {
            return self.request();
        }
        let next = DirectoryPathModel::applying_selection(trimmed, &self.text);
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
        self.alias = if is_ssh {
            alias.map(str::to_string)
        } else {
            None
        };
        self.invalidate();
        self.request()
    }

    /// 应用异步响应。只有与当前请求完全一致的响应才会更新候选。
    pub fn apply(&mut self, response: &DirectoryListingResponse) -> bool {
        if response.request != self.request() {
            return false;
        }
        let prefix = DirectoryPathModel::input_prefix(&self.text);
        let mut candidates: Vec<String> = response
            .directories
            .iter()
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty() && !name.contains('/'))
            .filter(|name| prefix.is_empty() || name.starts_with(&prefix))
            .collect();
        candidates.sort();
        self.candidates = candidates;
        true
    }

    fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.candidates.clear();
    }

    fn last_entered_component(&self) -> Option<String> {
        let trimmed = self.text.trim();
        if !trimmed.ends_with('/') || trimmed == "/" || trimmed == "~/" {
            return None;
        }
        let without_trailing = trimming_trailing_slashes(trimmed);
        if without_trailing == "~" {
            return None;
        }
        without_trailing.rsplit('/').next().map(str::to_string)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对照 `DirectorySuggestionControllerTests.testListingBaseUsesParentWhileTypingPrefix`。
    #[test]
    fn listing_base_uses_parent_while_typing_prefix() {
        assert_eq!(
            DirectoryPathModel::base_directory("/Users/wlz/Developer/mu"),
            "/Users/wlz/Developer"
        );
        assert_eq!(
            DirectoryPathModel::input_prefix("/Users/wlz/Developer/mu"),
            "mu"
        );
    }

    /// 对照 `DirectorySuggestionControllerTests.testTrailingSlashListsThatDirectoryWithoutPrefix`。
    #[test]
    fn trailing_slash_lists_that_directory_without_prefix() {
        assert_eq!(
            DirectoryPathModel::base_directory("/Users/wlz/Developer/"),
            "/Users/wlz/Developer"
        );
        assert_eq!(
            DirectoryPathModel::input_prefix("/Users/wlz/Developer/"),
            ""
        );
    }

    /// 对照 `DirectorySuggestionControllerTests.testTildeAndRootListingBases`。
    #[test]
    fn tilde_and_root_listing_bases() {
        assert_eq!(DirectoryPathModel::base_directory("~/Dev"), "~");
        assert_eq!(DirectoryPathModel::input_prefix("~/Dev"), "Dev");
        assert_eq!(DirectoryPathModel::base_directory("~/Dev/"), "~/Dev");
        assert_eq!(DirectoryPathModel::base_directory("~"), "~");
        assert_eq!(DirectoryPathModel::base_directory("/"), "/");
        assert_eq!(DirectoryPathModel::base_directory("/home/"), "/home");
        assert_eq!(DirectoryPathModel::base_directory(""), "~");
    }

    /// 对照 `DirectorySuggestionControllerTests.testSelectingCandidateEntersDirectoryWithoutDuplicatingBasename`。
    #[test]
    fn selecting_a_candidate_enters_the_directory() {
        assert_eq!(
            DirectoryPathModel::applying_selection("muxterm", "/Users/wlz/Developer/"),
            "/Users/wlz/Developer/muxterm/"
        );
        assert_eq!(
            DirectoryPathModel::applying_selection("muxterm", "/Users/wlz/Developer"),
            "/Users/wlz/muxterm/",
            "没有尾斜杠时最后一段是输入前缀，候选替换它"
        );
    }

    /// 对照 `DirectorySuggestionControllerTests.testSelectingCandidateHandlesTildeRootAndEmpty`。
    #[test]
    fn selecting_a_candidate_handles_tilde_root_and_empty() {
        assert_eq!(DirectoryPathModel::applying_selection("Dev", "~"), "~/Dev/");
        assert_eq!(
            DirectoryPathModel::applying_selection("Dev", "~/"),
            "~/Dev/"
        );
        assert_eq!(
            DirectoryPathModel::applying_selection("home", "/"),
            "/home/"
        );
        assert_eq!(DirectoryPathModel::applying_selection("foo", ""), "foo/");
    }

    /// 对照 `DirectorySuggestionControllerTests.testSelectingFullPathCandidateIsIgnored`。
    #[test]
    fn selecting_a_full_path_candidate_is_ignored() {
        let mut controller = DirectorySuggestionController::new("/Users/wlz/Developer");
        let request = controller.request();
        let response = controller.select("/Users/wlz/Developer/muxterm");
        assert_eq!(controller.text(), "/Users/wlz/Developer");
        assert_eq!(response, request, "完整路径候选必须被忽略");
    }

    /// 对照 `DirectorySuggestionControllerTests.testSelectingCandidateReplacesTypedPrefixNotWholePath`。
    #[test]
    fn selecting_a_candidate_replaces_the_typed_prefix() {
        let mut controller = DirectorySuggestionController::new("/Users/wlz/Developer/mu");
        let request = controller.select("muxterm");
        assert_eq!(controller.text(), "/Users/wlz/Developer/muxterm/");
        assert_eq!(request.path, "/Users/wlz/Developer/muxterm");
        assert_eq!(request.generation, 1);
    }

    /// 对照 `DirectorySuggestionControllerTests.testRepeatedSelectionIsIdempotent`。
    #[test]
    fn repeated_selection_is_idempotent() {
        let mut controller = DirectorySuggestionController::new("/Users/wlz/Developer/");
        controller.select("muxterm");
        let before = controller.text().to_string();
        let request = controller.select("muxterm");
        assert_eq!(controller.text(), before, "重复选择不得再次拼接");
        assert_eq!(request.generation, 1, "重复选择不得触发新请求");
    }

    /// 对照 `DirectorySuggestionControllerTests.testGoUpUsesDirectorySemantics`。
    #[test]
    fn go_up_uses_directory_semantics() {
        assert_eq!(
            DirectoryPathModel::applying_go_up("/Users/wlz/Developer/muxterm/"),
            "/Users/wlz/Developer/"
        );
        assert_eq!(
            DirectoryPathModel::applying_go_up("/Users/wlz/Developer/muxterm"),
            "/Users/wlz/Developer/"
        );
        assert_eq!(DirectoryPathModel::applying_go_up("~/Dev"), "~/");
        assert_eq!(DirectoryPathModel::applying_go_up("~"), "~");
        assert_eq!(DirectoryPathModel::applying_go_up("/"), "/");
        assert_eq!(DirectoryPathModel::applying_go_up("/a/b/"), "/a/");
        assert_eq!(DirectoryPathModel::applying_go_up("~/"), "~");
    }

    /// 对照 `DirectorySuggestionControllerTests.testResolvedPathNormalizesDotsAndTrailingSlash`。
    #[test]
    fn resolved_path_normalizes_dots_and_trailing_slash() {
        assert_eq!(
            DirectoryPathModel::resolved_path("/Users/wlz/Developer/"),
            "/Users/wlz/Developer"
        );
        assert_eq!(DirectoryPathModel::resolved_path("~/Dev/"), "~/Dev");
        assert_eq!(DirectoryPathModel::resolved_path("~/a/../b"), "~/b");
        assert_eq!(DirectoryPathModel::resolved_path("/a/./b/"), "/a/b");
        assert_eq!(DirectoryPathModel::resolved_path("/"), "/");
        assert_eq!(DirectoryPathModel::resolved_path("~"), "~");
        assert_eq!(DirectoryPathModel::resolved_path(""), "~");
    }

    /// 对照 `DirectorySuggestionControllerTests.testAsyncResponsesAreGuardedByGenerationAndRequestKey`。
    #[test]
    fn async_responses_are_guarded_by_generation_and_request_key() {
        let mut controller = DirectorySuggestionController::new("/Users/wlz/Developer");
        let first = controller.request();
        let second = controller.update_input("/Users/wlz/Developer/mu");
        assert_ne!(first.generation, second.generation);

        let stale = DirectoryListingResponse {
            request: DirectoryListingRequest {
                generation: first.generation,
                path: first.path.clone(),
                is_ssh: first.is_ssh,
                alias: first.alias.clone(),
            },
            directories: vec!["muxterm".to_string(), "old".to_string()],
        };
        assert!(!controller.apply(&stale), "旧 generation 响应必须丢弃");
        assert!(controller.candidates().is_empty());

        let current = DirectoryListingResponse {
            request: second.clone(),
            directories: vec![
                "muxterm".to_string(),
                "mu-notes".to_string(),
                "other".to_string(),
                "nested/dir".to_string(),
                "  ".to_string(),
            ],
        };
        assert!(controller.apply(&current));
        assert_eq!(
            controller.candidates(),
            ["mu-notes", "muxterm"],
            "按前缀过滤、去掉含斜杠与空白项、排序"
        );
    }

    /// transport / alias 变化也会作废旧响应。
    #[test]
    fn transport_change_invalidates_pending_responses() {
        let mut controller = DirectorySuggestionController::new("~");
        let local = controller.request();
        assert!(!local.is_ssh);

        let remote = controller.set_transport(true, Some("ryzen"));
        assert!(remote.is_ssh);
        assert_eq!(remote.alias.as_deref(), Some("ryzen"));

        let late = DirectoryListingResponse {
            request: local,
            directories: vec!["dev".to_string()],
        };
        assert!(!controller.apply(&late), "本地响应不能落到 SSH 请求上");

        let current = DirectoryListingResponse {
            request: remote,
            directories: vec!["srv".to_string()],
        };
        assert!(controller.apply(&current));
        assert_eq!(controller.candidates(), ["srv"]);
    }

    /// `~` / `~/x` 必须展开成绝对路径：Core 只认字面 `~`，tmux/herdr 不展开。
    #[test]
    fn expand_home_turns_tilde_into_an_absolute_path() {
        let home = home_directory().expect("测试机必须有 HOME");
        assert_eq!(expand_home("~"), home);
        assert_eq!(expand_home("~/"), home);
        assert_eq!(expand_home("~/dev/x"), format!("{home}/dev/x"));
        assert_eq!(
            expand_home(" /opt/dev "),
            "/opt/dev",
            "绝对路径原样（去空白），不加前缀"
        );
        assert_eq!(expand_home("~someone"), "~someone", "别人的 home 不动");
        assert_eq!(expand_home(""), "");
    }
}
