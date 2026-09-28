//! 远端 Herdr 子进程使用目标机器实际安装的 UTF-8 locale。

use serde_json::Value;

use crate::transport::{ChannelRequest, TargetConnection};

/// `locale -a` 在不同系统上的拼写不同；保留远端报告的原样名称。
fn choose_utf8_locale(output: &str) -> Option<String> {
    let locales: Vec<&str> = output.lines().map(str::trim).collect();
    ["C.UTF-8", "C.utf8", "en_US.UTF-8", "en_US.utf8"]
        .into_iter()
        .find_map(|preferred| {
            locales
                .iter()
                .find(|available| available.eq_ignore_ascii_case(preferred))
                .map(|available| (*available).to_string())
        })
        .or_else(|| {
            locales
                .into_iter()
                .find(|locale| {
                    let normalized = locale.to_ascii_lowercase();
                    normalized.ends_with(".utf8") || normalized.ends_with(".utf-8")
                })
                .map(str::to_string)
        })
}

pub(super) fn ssh_utf8_locale(connection: &dyn TargetConnection) -> Option<String> {
    let output = connection
        .exec_command(ChannelRequest::Exec {
            argv: vec!["locale".into(), "-a".into()],
            cwd: None,
            env: Vec::new(),
            pty: None,
        })
        .ok()?;
    (output.status == 0)
        .then(|| choose_utf8_locale(&String::from_utf8_lossy(&output.stdout)))
        .flatten()
}

pub(super) fn locale_env(locale: &str) -> Vec<(String, String)> {
    ["LANG", "LC_CTYPE", "LC_ALL"]
        .into_iter()
        .map(|key| (key.into(), locale.into()))
        .collect()
}

pub(super) fn set_process_locale(params: &mut Value, locale: Option<&str>) {
    if let Some(locale) = locale {
        params["env"] = serde_json::json!({
            "LANG": locale,
            "LC_CTYPE": locale,
            "LC_ALL": locale,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_installed_utf8_locale_without_guessing_spelling() {
        assert_eq!(
            choose_utf8_locale("C\nC.utf8\nen_US.utf8\nPOSIX\n"),
            Some("C.utf8".into())
        );
        assert_eq!(
            choose_utf8_locale("C\nzh_CN.utf8\nPOSIX\n"),
            Some("zh_CN.utf8".into())
        );
        assert_eq!(choose_utf8_locale("C\nPOSIX\n"), None);
    }

    #[test]
    fn process_env_overrides_stale_server_locale() {
        let mut params = serde_json::json!({ "workspace_id": "w1" });
        set_process_locale(&mut params, Some("C.utf8"));
        assert_eq!(params["env"]["LANG"], "C.utf8");
        assert_eq!(params["env"]["LC_CTYPE"], "C.utf8");
        assert_eq!(params["env"]["LC_ALL"], "C.utf8");
    }
}
