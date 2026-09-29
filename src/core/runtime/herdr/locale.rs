//! 本地及远端 Herdr 子进程使用目标机器实际安装的 UTF-8 locale。

use serde_json::Value;

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
    fn process_env_overrides_stale_server_locale() {
        let mut params = serde_json::json!({ "workspace_id": "w1" });
        set_process_locale(&mut params, Some("C.utf8"));
        assert_eq!(params["env"]["LANG"], "C.utf8");
        assert_eq!(params["env"]["LC_CTYPE"], "C.utf8");
        assert_eq!(params["env"]["LC_ALL"], "C.utf8");
    }
}
