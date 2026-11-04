//! Core `WorkspaceId::replica_id()` 的前端对照。
//!
//! 移植自 `chrome/WorkspaceReplicaID.swift`。Attention / Agents / Commands
//! 都按这个键聚合；Catalog 的 `as_str()`（`ssh/ryzen/session/tmux/path`）
//! 不能拿来匹配，所以必须在前端精确复刻 Core 的构造规则。

/// 无最小目录时的兜底名字。
pub const FALLBACK_NAME: &str = "workspace";
/// 空 transport 显示为本地。
pub const LOCAL_TRANSPORT: &str = "local";

/// 取路径的最后一段作为工作区名字。
///
/// 语义对齐 Swift 的 `URL(fileURLWithPath:).lastPathComponent`：忽略末尾的
/// 分隔符，`""`、`"  "`、`"/"` 都返回 [`FALLBACK_NAME`]。
///
/// 已知差异：Swift 会对 `.` 和 `..` 做标准化，这里按字面返回最后一段，
/// 因为 gpui 前端拿到的路径都已经由 Core 规范化过。
pub fn default_name(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return FALLBACK_NAME.to_string();
    }
    let last = trimmed
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default();
    if last.is_empty() || last == "/" {
        return FALLBACK_NAME.to_string();
    }
    last.to_string()
}

/// 用一个工作区身份构造 replica id。
///
/// 规则：
/// - session 与 path 都非空：`<name>:<path>@<transport>`
/// - 否则：`<name>@<transport>`
/// - name 在 session 为空时取 path 的最后一段
/// - transport 为空时记为 `local`
pub fn replica_id_from_parts(session: Option<&str>, path: &str, transport: &str) -> String {
    let trimmed_path = path.trim();
    let trimmed_session = session.unwrap_or_default().trim();
    let name = if trimmed_session.is_empty() {
        default_name(trimmed_path)
    } else {
        trimmed_session.to_string()
    };
    let trimmed_transport = transport.trim();
    let transport_label = if trimmed_transport.is_empty() {
        LOCAL_TRANSPORT
    } else {
        trimmed_transport
    };
    if !trimmed_session.is_empty() && !trimmed_path.is_empty() {
        format!("{name}:{trimmed_path}@{transport_label}")
    } else {
        format!("{name}@{transport_label}")
    }
}

/// 目标身份：`TargetConfig` 里参与 replica id 的字段。
#[derive(Debug, Clone, Copy)]
pub struct TargetIdentity<'a> {
    pub session: Option<&'a str>,
    pub path: &'a str,
    pub transport: &'a str,
    /// Herdr 的 Core replica_id 用 workspace id，不是 project 目录。
    pub workspace_id: Option<&'a str>,
    /// 是否 Herdr runtime。Herdr 时用 `workspace_id`（`wN`）替换 path。
    pub herdr: bool,
}

impl TargetIdentity<'_> {
    /// 构造 replica id。
    pub fn replica_id(&self) -> String {
        let identity_path = if self.herdr {
            let workspace_id = self.workspace_id.unwrap_or_default().trim();
            if workspace_id.is_empty() {
                self.path
            } else {
                workspace_id
            }
        } else {
            self.path
        };
        replica_id_from_parts(self.session, identity_path, self.transport)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对照 `WorkspaceReplicaIDTests.testSshSessionKeepsProjectPathLikeCoreReplicaId`。
    #[test]
    fn ssh_session_keeps_project_path() {
        assert_eq!(
            replica_id_from_parts(Some("muxterm"), "~/Developer/self/muxterm", "ryzen"),
            "muxterm:~/Developer/self/muxterm@ryzen"
        );
    }

    /// 对照 `testSharedSessionNamesStayDistinctByPath`。
    #[test]
    fn shared_session_names_stay_distinct_by_path() {
        assert_ne!(
            replica_id_from_parts(Some("shared"), "one", "local"),
            replica_id_from_parts(Some("shared"), "two", "local")
        );
    }

    /// 对照 `testEmptySessionFallsBackToPathBasename`。
    #[test]
    fn empty_session_falls_back_to_path_basename() {
        assert_eq!(
            replica_id_from_parts(None, "/tmp/foo", "local"),
            "foo@local"
        );
    }

    /// 对照 `testHerdrReplicaUsesWorkspaceIDNotProjectPath`。
    #[test]
    fn herdr_replica_uses_workspace_id_not_project_path() {
        let identity = TargetIdentity {
            session: Some("default"),
            path: "~/Developer/Yak/yakit",
            transport: "local",
            workspace_id: Some("w9"),
            herdr: true,
        };
        let id = identity.replica_id();
        assert_eq!(id, "default:w9@local");
        assert!(!id.contains("Yak"), "Herdr 不能用项目目录参与 identity");
    }

    /// 对照 `testTargetConfigUsesSessionPathAndSshAlias`：transport label 是
    /// ssh 别名，不是 `ssh/<alias>/`。
    #[test]
    fn tmux_target_uses_session_path_and_ssh_alias() {
        let identity = TargetIdentity {
            session: Some("muxterm"),
            path: "~/Developer/self/muxterm",
            transport: "ryzen",
            workspace_id: None,
            herdr: false,
        };
        let id = identity.replica_id();
        assert_eq!(id, "muxterm:~/Developer/self/muxterm@ryzen");
        assert!(!id.contains("ssh/ryzen/"));
    }

    /// 对照 `QuickConnect.defaultName` 的五条断言。
    #[test]
    fn default_name_takes_last_path_component() {
        assert_eq!(default_name("/home/wlz/Developer/self/muxterm"), "muxterm");
        assert_eq!(default_name("/Users/me/project"), "project");
        assert_eq!(default_name(""), FALLBACK_NAME);
        assert_eq!(default_name("  "), FALLBACK_NAME);
        assert_eq!(default_name("/"), FALLBACK_NAME);
    }

    #[test]
    fn default_name_ignores_trailing_separators() {
        assert_eq!(default_name("/tmp/foo/"), "foo");
        assert_eq!(default_name("/tmp/foo//"), "foo");
        assert_eq!(default_name("foo"), "foo");
    }

    #[test]
    fn blank_transport_becomes_local() {
        assert_eq!(replica_id_from_parts(None, "/tmp/foo", ""), "foo@local");
        assert_eq!(replica_id_from_parts(None, "/tmp/foo", "  "), "foo@local");
    }

    #[test]
    fn empty_path_with_session_still_produces_a_name() {
        assert_eq!(
            replica_id_from_parts(Some("muxterm"), "", "local"),
            "muxterm@local"
        );
        assert_eq!(
            replica_id_from_parts(Some("muxterm"), "   ", "local"),
            "muxterm@local"
        );
    }

    #[test]
    fn session_and_path_are_trimmed_before_use() {
        assert_eq!(
            replica_id_from_parts(Some("  muxterm  "), "  /tmp/foo  ", "  local  "),
            "muxterm:/tmp/foo@local"
        );
    }

    #[test]
    fn herdr_without_workspace_id_falls_back_to_path() {
        let identity = TargetIdentity {
            session: Some("default"),
            path: "/tmp/yakit",
            transport: "local",
            workspace_id: None,
            herdr: true,
        };
        assert_eq!(identity.replica_id(), "default:/tmp/yakit@local");
    }
}
