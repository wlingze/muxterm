//! config.toml 编辑（LINUX-PLAN §12 C4.1）。
//!
//! 用 `toml_edit` 保注释与未知键，只改指定 dotted key（如 `font.size`、
//! `scrollback.lines`、`attention.debounce_ms`）。

use anyhow::{Context, Result};

/// 设置 dotted key 的值；保留注释与未知键。
pub fn set_dotted_key(toml: &str, dotted: &str, value: toml_edit::Item) -> Result<String> {
    let mut doc: toml_edit::DocumentMut = toml.parse().context("config.toml 不是合法 TOML")?;
    let parts: Vec<&str> = dotted.split('.').collect();
    if parts.is_empty() || parts.iter().any(|p| p.is_empty()) {
        anyhow::bail!("dotted key 格式非法: {dotted}");
    }
    let mut table = doc.as_table_mut();
    for (i, part) in parts.iter().enumerate() {
        if i + 1 == parts.len() {
            table.insert(part, value.clone());
        } else {
            if !table.contains_key(part) {
                table.insert(part, toml_edit::Item::Table(toml_edit::Table::new()));
            }
            let next = table
                .get_mut(part)
                .and_then(|v| v.as_table_mut())
                .context("dotted key 中间段不是表")?;
            table = next;
        }
    }
    Ok(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_font_size_preserves_comments_and_unknown_keys() {
        let toml = r##"
# 字体配置
[font]
family = "Monospace"
size = 12.0

[foo]
bar = 1
"##;
        let out = set_dotted_key(toml, "font.size", toml_edit::value(14.0)).unwrap();
        assert!(out.contains("# 字体配置"), "注释应保留: {out}");
        assert!(out.contains("[foo]"), "未知表应保留: {out}");
        assert!(out.contains("bar = 1"), "未知键应保留: {out}");
        let parsed: toml::Value = toml::from_str(&out).unwrap();
        assert_eq!(parsed["font"]["size"].as_float(), Some(14.0));
        assert_eq!(parsed["font"]["family"].as_str(), Some("Monospace"));
    }

    #[test]
    fn set_scrollback_lines() {
        let out = set_dotted_key("", "scrollback.lines", toml_edit::value(10_000i64)).unwrap();
        let parsed: toml::Value = toml::from_str(&out).unwrap();
        assert_eq!(parsed["scrollback"]["lines"].as_integer(), Some(10_000));
    }

    #[test]
    fn set_attention_debounce() {
        let out = set_dotted_key("", "attention.debounce_ms", toml_edit::value(100i64)).unwrap();
        let parsed: toml::Value = toml::from_str(&out).unwrap();
        assert_eq!(parsed["attention"]["debounce_ms"].as_integer(), Some(100));
    }

    #[test]
    fn invalid_dotted_key_rejected() {
        assert!(set_dotted_key("", "", toml_edit::value(1i64)).is_err());
        assert!(set_dotted_key("", "a..b", toml_edit::value(1i64)).is_err());
    }

    #[test]
    fn invalid_toml_rejected() {
        assert!(set_dotted_key("not [valid", "a.b", toml_edit::value(1i64)).is_err());
    }
}
