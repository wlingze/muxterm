use anyhow::{Context, Result};
use std::collections::BTreeMap;

use crate::core::config_service::schema::{ProjectDocument, ProjectRuntime, ProjectTransport};
pub fn import_legacy_projects(raw: &str) -> Result<Vec<ProjectDocument>> {
    let root: toml::Value = raw.parse().context("旧 QuickConnect TOML 解析失败")?;
    let Some(items) = root.get("projects").and_then(toml::Value::as_array) else {
        return Ok(Vec::new());
    };
    items.iter().filter_map(import_legacy_project).collect()
}

fn import_legacy_project(value: &toml::Value) -> Option<Result<ProjectDocument>> {
    let table = value.as_table()?;
    let get_string = |name: &str| {
        table
            .get(name)
            .and_then(toml::Value::as_str)
            .map(str::to_string)
    };
    let name = get_string("name")?;
    let path = get_string("path").unwrap_or_else(|| "~".into());

    let runtime_table = table.get("runtime").and_then(toml::Value::as_table);
    let runtime_id = table
        .get("runtime")
        .and_then(toml::Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            runtime_table
                .and_then(|item| item.get("id"))
                .and_then(toml::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "tmux".into());
    let session = table
        .get("session")
        .and_then(toml::Value::as_str)
        .or_else(|| {
            runtime_table
                .and_then(|item| item.get("session"))
                .and_then(toml::Value::as_str)
        })
        .map(str::to_string);
    let socket = table
        .get("socket")
        .and_then(toml::Value::as_str)
        .or_else(|| {
            runtime_table
                .and_then(|item| item.get("socket"))
                .and_then(toml::Value::as_str)
        })
        .map(str::to_string);

    let transport_table = table.get("transport").and_then(toml::Value::as_table);
    let transport_id = table
        .get("transport")
        .and_then(toml::Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            transport_table
                .and_then(|item| item.get("id"))
                .and_then(toml::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "local".into());
    let target = table
        .get("transport_name")
        .and_then(toml::Value::as_str)
        .or_else(|| table.get("target").and_then(toml::Value::as_str))
        .or_else(|| {
            transport_table
                .and_then(|item| item.get("target"))
                .and_then(toml::Value::as_str)
        })
        .unwrap_or_default()
        .to_string();

    let command = table
        .get("command")
        .and_then(toml::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(toml::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let env = table
        .get("env")
        .and_then(toml::Value::as_table)
        .map(|items| {
            items
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();

    let transport = ProjectTransport {
        id: transport_id,
        target,
        options: BTreeMap::new(),
    };
    let project_id = get_string("id").unwrap_or_else(|| format!("{}@{}", name, transport.id));
    Some(Ok(ProjectDocument {
        id: project_id,
        name,
        path,
        runtime: ProjectRuntime {
            id: runtime_id,
            options: BTreeMap::new(),
            session,
            socket,
        },
        transport,
        command,
        env,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_quickconnect_import_keeps_runtime_fields() {
        let raw = r#"
            [[projects]]
            id = "demo"
            name = "Demo"
            path = "~/demo"
            runtime = "tmux"
            session = "demo-session"
            socket = "muxterm-test"
            command = ["zsh", "-l"]
            env = { RUST_LOG = "info" }
            transport = "local"
        "#;
        let projects = import_legacy_projects(raw).unwrap();
        assert_eq!(projects.len(), 1);
        let project = &projects[0];
        assert_eq!(project.runtime.session.as_deref(), Some("demo-session"));
        assert_eq!(project.runtime.socket.as_deref(), Some("muxterm-test"));
        assert_eq!(project.command, vec!["zsh", "-l"]);
        assert_eq!(project.env.get("RUST_LOG").map(String::as_str), Some("info"));
    }
}
