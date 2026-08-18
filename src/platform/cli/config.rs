//! `muxterm config` adapter.
//!
//! All parsing, validation, patching and persistence is delegated to the Core
//! `SettingsService`; this module only translates command-line arguments and
//! formats the result.

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::path::PathBuf;

use super::format::OutputFormat;
use crate::core::config_service::{
    ConfigDocument, JsonPatchOperation, ProjectDocument, ProjectRuntime, ProjectTransport,
    SettingsService, ShortcutBinding, ShortcutOverride,
};

pub fn run(args: &[String], format: OutputFormat) -> Result<()> {
    let Some(command) = args.first().map(String::as_str) else {
        print_usage();
        return Ok(());
    };
    if matches!(command, "-h" | "--help" | "help") {
        print_usage();
        return Ok(());
    }
    match command {
        "path" => {
            let service = SettingsService::default_user_or_memory();
            emit(
                serde_json::json!({"ok": true, "path": service.path()}),
                format,
            )
        }
        "show" => show(&args[1..], format),
        "schema" => schema(&args[1..], format),
        "validate" => validate(&args[1..], format),
        "doctor" => doctor(format),
        "get" => get(&args[1..], format),
        "set" => set(&args[1..], format),
        "unset" => unset(&args[1..], format),
        "project" => project(&args[1..], format),
        "shortcut" => shortcut(&args[1..], format),
        other => Err(anyhow!("未知 config 子命令: {other}")),
    }
}

fn show(args: &[String], format: OutputFormat) -> Result<()> {
    let service = SettingsService::default_user_or_memory();
    let resolved = args.iter().any(|arg| arg == "--resolved");
    let snapshot = service.snapshot();
    let value = if resolved {
        snapshot.values
    } else {
        snapshot.raw
    };
    emit(
        serde_json::json!({"ok": true, "revision": snapshot.revision, "values": value}),
        format,
    )
}

fn schema(args: &[String], format: OutputFormat) -> Result<()> {
    let mut value = serde_json::json!({"schema": ConfigDocument::schema_json()});
    if args.iter().any(|arg| arg == "--manifest") {
        value["manifest"] = ConfigDocument::manifest_json();
    }
    emit(serde_json::json!({"ok": true, "data": value}), format)
}

fn validate(args: &[String], format: OutputFormat) -> Result<()> {
    let path = args.first().map(PathBuf::from);
    let service = match path {
        Some(path) => SettingsService::open(path),
        None => SettingsService::default_user(),
    }?;
    service.document().validate()?;
    emit(
        serde_json::json!({"ok": true, "valid": true, "path": service.path()}),
        format,
    )
}

fn doctor(format: OutputFormat) -> Result<()> {
    let service = SettingsService::default_user_or_memory();
    let document = service.document();
    let mut checks = Vec::new();
    checks.push(serde_json::json!({"id":"path","ok":service.path().parent().is_some()}));
    checks.push(serde_json::json!({"id":"schema","ok":ConfigDocument::schema_json().is_object()}));
    checks.push(serde_json::json!({"id":"validation","ok":document.validate().is_ok()}));
    checks.push(serde_json::json!({"id":"themes","ok":!document.config.theme.name.is_empty()}));
    emit(
        serde_json::json!({"ok": checks.iter().all(|item| item["ok"] == true), "checks": checks}),
        format,
    )
}

fn get(args: &[String], format: OutputFormat) -> Result<()> {
    let path = args
        .first()
        .ok_or_else(|| anyhow!("config get 需要 PATH"))?;
    let service = SettingsService::default_user_or_memory();
    let snapshot = service.snapshot();
    let value = pointer(&snapshot.values, &dotted_pointer(path)?)?;
    emit(
        serde_json::json!({"ok": true, "path": path, "value": value}),
        format,
    )
}

fn set(args: &[String], format: OutputFormat) -> Result<()> {
    let path = args
        .first()
        .ok_or_else(|| anyhow!("config set 需要 PATH"))?;
    let raw_value = args
        .get(1)
        .ok_or_else(|| anyhow!("config set 需要 VALUE"))?;
    let force_string = args.iter().any(|arg| arg == "--string");
    let value = parse_value(raw_value, force_string)?;
    let mut service = SettingsService::default_user_or_memory();
    let snapshot = service.snapshot();
    let json_pointer = dotted_pointer(path)?;
    let exists = pointer(&snapshot.values, &json_pointer).is_ok();
    let transaction = service.begin();
    service.patch(
        &transaction,
        &[JsonPatchOperation {
            op: if exists { "replace" } else { "add" }.into(),
            path: json_pointer,
            value: Some(value),
        }],
    )?;
    let revision = service.commit(&transaction)?;
    emit(
        serde_json::json!({"ok": true, "revision": revision}),
        format,
    )
}

fn unset(args: &[String], format: OutputFormat) -> Result<()> {
    let path = args
        .first()
        .ok_or_else(|| anyhow!("config unset 需要 PATH"))?;
    let mut service = SettingsService::default_user_or_memory();
    let transaction = service.begin();
    service.patch(
        &transaction,
        &[JsonPatchOperation {
            op: "remove".into(),
            path: dotted_pointer(path)?,
            value: None,
        }],
    )?;
    let revision = service.commit(&transaction)?;
    emit(
        serde_json::json!({"ok": true, "revision": revision}),
        format,
    )
}

fn project(args: &[String], format: OutputFormat) -> Result<()> {
    let action = args.first().map(String::as_str).unwrap_or("list");
    let mut service = SettingsService::default_user_or_memory();
    match action {
        "list" => emit(
            serde_json::json!({"ok": true, "projects": service.snapshot().values["projects"]}),
            format,
        ),
        "add" => {
            let document = project_from_args(&args[1..])?;
            let transaction = service.begin();
            service.patch(
                &transaction,
                &[JsonPatchOperation {
                    op: "add".into(),
                    path: "/projects/-".into(),
                    value: Some(serde_json::to_value(document)?),
                }],
            )?;
            let revision = service.commit(&transaction)?;
            emit(
                serde_json::json!({"ok": true, "revision": revision}),
                format,
            )
        }
        "remove" => {
            let id = args
                .get(1)
                .ok_or_else(|| anyhow!("config project remove 需要 ID"))?;
            let index = project_index(&service.snapshot().values, id)?;
            let transaction = service.begin();
            service.patch(
                &transaction,
                &[JsonPatchOperation {
                    op: "remove".into(),
                    path: format!("/projects/{index}"),
                    value: None,
                }],
            )?;
            let revision = service.commit(&transaction)?;
            emit(
                serde_json::json!({"ok": true, "revision": revision}),
                format,
            )
        }
        "edit" => {
            let id = args
                .get(1)
                .ok_or_else(|| anyhow!("config project edit 需要 ID"))?;
            let index = project_index(&service.snapshot().values, id)?;
            let transaction = service.begin();
            for (field, value) in project_field_patches(&args[2..])? {
                service.patch(
                    &transaction,
                    &[JsonPatchOperation {
                        op: "replace".into(),
                        path: format!("/projects/{index}/{field}"),
                        value: Some(value),
                    }],
                )?;
            }
            let revision = service.commit(&transaction)?;
            emit(
                serde_json::json!({"ok": true, "revision": revision}),
                format,
            )
        }
        other => Err(anyhow!("未知 project 子命令: {other}")),
    }
}

fn shortcut(args: &[String], format: OutputFormat) -> Result<()> {
    let action = args.first().map(String::as_str).unwrap_or("list");
    let mut service = SettingsService::default_user_or_memory();
    match action {
        "list" => emit(
            serde_json::json!({"ok": true, "shortcuts": service.snapshot().values["shortcuts"]}),
            format,
        ),
        "preset" => {
            let preset = args
                .get(1)
                .ok_or_else(|| anyhow!("config shortcut preset 需要 qwerty 或 colemak"))?;
            let transaction = service.begin();
            service.patch(
                &transaction,
                &[JsonPatchOperation {
                    op: "replace".into(),
                    path: "/shortcuts/preset".into(),
                    value: Some(Value::String(preset.clone())),
                }],
            )?;
            let revision = service.commit(&transaction)?;
            emit(
                serde_json::json!({"ok": true, "revision": revision}),
                format,
            )
        }
        "bind" => {
            let action_id = args
                .get(1)
                .ok_or_else(|| anyhow!("config shortcut bind 需要 ACTION"))?;
            let chord = args
                .get(2)
                .ok_or_else(|| anyhow!("config shortcut bind 需要 CHORD"))?;
            let binding = parse_chord(chord)?;
            let value = serde_json::to_value(ShortcutOverride {
                action: action_id.clone(),
                bindings: vec![binding],
            })?;
            let existing = service.snapshot().values["shortcuts"]["overrides"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let index = existing
                .iter()
                .position(|item| item["action"] == *action_id);
            let transaction = service.begin();
            let operation = if let Some(index) = index {
                JsonPatchOperation {
                    op: "replace".into(),
                    path: format!("/shortcuts/overrides/{index}"),
                    value: Some(value),
                }
            } else {
                JsonPatchOperation {
                    op: "add".into(),
                    path: "/shortcuts/overrides/-".into(),
                    value: Some(value),
                }
            };
            service.patch(&transaction, &[operation])?;
            let revision = service.commit(&transaction)?;
            emit(
                serde_json::json!({"ok": true, "revision": revision}),
                format,
            )
        }
        "unbind" => {
            let action_id = args
                .get(1)
                .ok_or_else(|| anyhow!("config shortcut unbind 需要 ACTION"))?;
            let existing = service.snapshot().values["shortcuts"]["overrides"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let index = existing
                .iter()
                .position(|item| item["action"] == *action_id)
                .ok_or_else(|| anyhow!("找不到 shortcut action: {action_id}"))?;
            let transaction = service.begin();
            service.patch(
                &transaction,
                &[JsonPatchOperation {
                    op: "replace".into(),
                    path: format!("/shortcuts/overrides/{index}/bindings"),
                    value: Some(Value::Array(Vec::new())),
                }],
            )?;
            let revision = service.commit(&transaction)?;
            emit(
                serde_json::json!({"ok": true, "revision": revision}),
                format,
            )
        }
        "reset" => {
            let transaction = service.begin();
            service.patch(
                &transaction,
                &[JsonPatchOperation {
                    op: "replace".into(),
                    path: "/shortcuts/overrides".into(),
                    value: Some(Value::Array(Vec::new())),
                }],
            )?;
            let revision = service.commit(&transaction)?;
            emit(
                serde_json::json!({"ok": true, "revision": revision}),
                format,
            )
        }
        other => Err(anyhow!("未知 shortcut 子命令: {other}")),
    }
}

fn project_from_args(args: &[String]) -> Result<ProjectDocument> {
    let required =
        |flag: &str| flag_value(args, flag).ok_or_else(|| anyhow!("project add 需要 {flag}"));
    Ok(ProjectDocument {
        id: required("--id")?,
        name: required("--name")?,
        path: required("--path")?,
        runtime: ProjectRuntime {
            id: required("--runtime")?,
            options: Default::default(),
            session: flag_value(args, "--session"),
            socket: flag_value(args, "--socket"),
        },
        transport: ProjectTransport {
            id: required("--transport")?,
            target: flag_value(args, "--target").unwrap_or_default(),
            options: Default::default(),
        },
        command: args
            .iter()
            .enumerate()
            .find_map(|(index, arg)| {
                (arg == "--command")
                    .then(|| args.get(index + 1).map(String::as_str))
                    .flatten()
            })
            .map(|raw| parse_value(raw, false))
            .transpose()?
            .and_then(|value| {
                value.as_array().map(|items| {
                    items
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
            })
            .unwrap_or_default(),
        env: args
            .iter()
            .enumerate()
            .find_map(|(index, arg)| {
                (arg == "--env")
                    .then(|| args.get(index + 1).map(String::as_str))
                    .flatten()
            })
            .map(|raw| parse_value(raw, false))
            .transpose()?
            .and_then(|value| value.as_object().cloned())
            .map(|map| {
                map.iter()
                    .filter_map(|(key, value)| {
                        value.as_str().map(|value| (key.clone(), value.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn project_field_patches(args: &[String]) -> Result<Vec<(String, Value)>> {
    let mut result = Vec::new();
    for (flag, field) in [("--name", "name"), ("--path", "path")] {
        if let Some(value) = flag_value(args, flag) {
            result.push((field.into(), Value::String(value)));
        }
    }
    if let Some(value) = flag_value(args, "--runtime") {
        result.push(("runtime/id".into(), Value::String(value)));
    }
    if let Some(value) = flag_value(args, "--transport") {
        result.push(("transport/id".into(), Value::String(value)));
    }
    if let Some(value) = flag_value(args, "--target") {
        result.push(("transport/target".into(), Value::String(value)));
    }
    if let Some(value) = flag_value(args, "--session") {
        result.push(("runtime/session".into(), Value::String(value)));
    }
    if let Some(value) = flag_value(args, "--socket") {
        result.push(("runtime/socket".into(), Value::String(value)));
    }
    if result.is_empty() {
        return Err(anyhow!("project edit 至少需要一个字段"));
    }
    Ok(result)
}

fn project_index(values: &Value, id: &str) -> Result<usize> {
    values["projects"]
        .as_array()
        .and_then(|items| items.iter().position(|item| item["id"] == id))
        .ok_or_else(|| anyhow!("找不到 project: {id}"))
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter().enumerate().find_map(|(index, arg)| {
        arg.strip_prefix(&format!("{flag}="))
            .map(str::to_string)
            .or_else(|| {
                (arg == flag)
                    .then(|| args.get(index + 1).cloned())
                    .flatten()
            })
    })
}

fn parse_chord(chord: &str) -> Result<ShortcutBinding> {
    let mut parts = chord
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty());
    let key = parts
        .next_back()
        .ok_or_else(|| anyhow!("快捷键 chord 不能为空"))?
        .to_string();
    let modifiers = parts.map(str::to_ascii_lowercase).collect();
    Ok(ShortcutBinding { key, modifiers })
}

fn parse_value(raw: &str, force_string: bool) -> Result<Value> {
    if force_string {
        return Ok(Value::String(raw.to_string()));
    }
    let scalar = if raw.eq_ignore_ascii_case("true") {
        Some(Value::Bool(true))
    } else if raw.eq_ignore_ascii_case("false") {
        Some(Value::Bool(false))
    } else if let Ok(value) = raw.parse::<i64>() {
        Some(Value::from(value))
    } else if let Ok(value) = raw.parse::<f64>() {
        Some(Value::from(value))
    } else {
        None
    };
    if let Some(value) = scalar {
        return Ok(value);
    }
    // Arrays, inline tables and quoted strings use TOML's own scalar parser;
    // wrapping the value in a temporary key avoids `Value::from_str` treating
    // a bare scalar as a document-level string.
    let wrapped = format!("value = {raw}");
    if let Ok(document) = wrapped.parse::<toml::Table>() {
        if let Some(value) = document.get("value") {
            return serde_json::to_value(value).context("转换 TOML value 失败");
        }
    }
    Ok(Value::String(raw.to_string()))
}

fn dotted_pointer(path: &str) -> Result<String> {
    if path.trim().is_empty() {
        return Err(anyhow!("配置路径不能为空"));
    }
    Ok(format!(
        "/{}",
        path.split('.')
            .map(|part| part.replace('~', "~0").replace('/', "~1"))
            .collect::<Vec<_>>()
            .join("/")
    ))
}

fn pointer<'a>(value: &'a Value, path: &str) -> Result<&'a Value> {
    let mut current = value;
    for token in path.trim_start_matches('/').split('/') {
        let token = token.replace("~1", "/").replace("~0", "~");
        current = current
            .get(&token)
            .ok_or_else(|| anyhow!("配置路径不存在: {path}"))?;
    }
    Ok(current)
}


/// Map a configuration command failure to the documented CLI exit code:
/// 2 = argument/validation, 3 = revision/merge conflict, 4 = I/O or migration.
pub fn exit_code(error: &anyhow::Error) -> i32 {
    let text = error.to_string().to_lowercase();
    if text.contains("revision") || text.contains("合并") || text.contains("冲突") {
        3
    } else if text.contains("i/o")
        || text.contains("读取配置")
        || text.contains("迁移")
        || text.contains("写入")
    {
        4
    } else {
        2
    }
}

fn emit(value: Value, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&value)?),
        OutputFormat::Text => println!("{}", text_value(&value)),
    }
    Ok(())
}

fn print_usage() {
    println!(
        "muxterm config <path|show|schema|validate|doctor|get|set|unset|project|shortcut>\n\n\
         subcommands:\n\
         \x20 path                     print the resolved config path\n\
         \x20 show [--resolved]        show raw (or resolved) config values\n\
         \x20 schema [--manifest]      print JSON Schema (and settings manifest)\n\
         \x20 validate [PATH]          validate a config file\n\
         \x20 doctor                   run config health checks\n\
         \x20 get PATH                 read one dotted path\n\
         \x20 set PATH VALUE [--string]\n\
         \x20                          write one dotted path (auto add/replace)\n\
         \x20 unset PATH               remove one dotted path\n\
         \x20 project list|add|edit|remove\n\
         \x20 shortcut list|bind|unbind|preset"
    );
}

fn text_value(value: &Value) -> String {
    match value {
        Value::Object(map) => map
            .iter()
            .map(|(key, value)| format!("{key} = {}", text_value(value)))
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Array(items) => items.iter().map(text_value).collect::<Vec<_>>().join("\n"),
        Value::String(value) => value.clone(),
        other => other.to_string(),
    }
}

trait DefaultUserSettings {
    fn default_user_or_memory() -> Self;
}

impl DefaultUserSettings for SettingsService {
    fn default_user_or_memory() -> Self {
        match SettingsService::default_user() {
            Ok(mut service) => {
                if let Err(error) = service.migrate_legacy_quickconnect() {
                    eprintln!("warning: QuickConnect 迁移未完成: {error}");
                }
                service
            }
            Err(error) => {
                eprintln!("warning: 使用内存默认配置: {error}");
                let path = crate::core::config::Config::user_config_path()
                    .unwrap_or_else(|| PathBuf::from("config.toml"));
                SettingsService::in_memory_default(path)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotted_pointer_escapes_slash_and_tilde() {
        assert_eq!(dotted_pointer("extensions.a/b").unwrap(), "/extensions/a~1b");
        assert_eq!(dotted_pointer("extensions.~").unwrap(), "/extensions/~0");
    }

    #[test]
    fn parse_value_toml_literal_forms() {
        assert_eq!(parse_value("true", false).unwrap(), Value::Bool(true));
        assert_eq!(parse_value("15", false).unwrap(), Value::from(15));
        assert_eq!(
            parse_value("\"hello\"", false).unwrap(),
            Value::String("hello".into())
        );
        assert_eq!(
            parse_value(r#"["a","b"]"#, false).unwrap(),
            serde_json::json!(["a", "b"])
        );
        assert_eq!(
            parse_value("hello", true).unwrap(),
            Value::String("hello".into())
        );
    }

    #[test]
    fn parse_chord_splits_modifiers_and_key() {
        let binding = parse_chord("ctrl+shift+p").unwrap();
        assert_eq!(binding.key, "p");
        assert_eq!(binding.modifiers, vec!["ctrl", "shift"]);
    }

    #[test]
    fn exit_code_classifies_errors() {
        assert_eq!(exit_code(&anyhow::anyhow!("字体配置校验失败")), 2);
        assert_eq!(exit_code(&anyhow::anyhow!("配置存在并发修改冲突")), 3);
        assert_eq!(exit_code(&anyhow::anyhow!("读取配置失败: /tmp/x")), 4);
    }
}
