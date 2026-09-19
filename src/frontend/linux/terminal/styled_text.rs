//! VTE 自有屏幕的 HTML 导出转为带 SGR 的行；只用于一次历史插入后的原屏恢复。
//! 不维护第二份终端网格，也不读取 Core Index。

pub(super) fn html_to_ansi(html: &str) -> String {
    let mut out = String::new();
    let mut styles: Vec<(String, String)> = Vec::new();
    let mut rest = html;
    while !rest.is_empty() {
        if let Some(tag) = rest.strip_prefix('<') {
            let Some(end) = tag.find('>') else { break };
            let tag = &tag[..end];
            let name = tag.split_whitespace().next().unwrap_or_default();
            if let Some(closing) = name.strip_prefix('/') {
                if let Some(index) = styles.iter().rposition(|(name, _)| name == closing) {
                    styles.truncate(index);
                    out.push_str("\x1b[0m");
                    for (_, style) in &styles {
                        out.push_str(style);
                    }
                }
            } else {
                let mut style = match name {
                    "b" => "\x1b[1m".to_string(),
                    "i" => "\x1b[3m".to_string(),
                    "u" => "\x1b[4m".to_string(),
                    "strike" => "\x1b[9m".to_string(),
                    "blink" => "\x1b[5m".to_string(),
                    _ => String::new(),
                };
                for (attribute, code) in [("color=\"#", 38), ("background-color:#", 48)] {
                    if let Some(value) = tag.split_once(attribute).map(|(_, v)| v) {
                        if let Some(hex) = value.get(..6) {
                            if let Ok(rgb) = u32::from_str_radix(hex, 16) {
                                style.push_str(&format!(
                                    "\x1b[{code};2;{};{};{}m",
                                    rgb >> 16,
                                    (rgb >> 8) & 255,
                                    rgb & 255
                                ));
                            }
                        }
                    }
                }
                if tag.contains("text-decoration-line:overline") {
                    style.push_str("\x1b[53m");
                }
                if !style.is_empty() {
                    out.push_str(&style);
                    styles.push((name.into(), style));
                }
            }
            rest = &rest[end + 2..];
        } else if rest.starts_with('&') {
            if let Some(end) = rest.find(';') {
                let entity = &rest[1..end];
                let ch = match entity {
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "amp" => Some('&'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    _ => entity
                        .strip_prefix("#x")
                        .and_then(|v| u32::from_str_radix(v, 16).ok())
                        .or_else(|| entity.strip_prefix('#').and_then(|v| v.parse().ok()))
                        .and_then(char::from_u32),
                };
                if let Some(ch) = ch {
                    out.push(ch);
                    rest = &rest[end + 1..];
                    continue;
                }
            }
            out.push('&');
            rest = &rest[1..];
        } else {
            let end = rest.find(['<', '&']).unwrap_or(rest.len());
            out.push_str(&rest[..end]);
            rest = &rest[end..];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_nested_colors_and_escaped_text() {
        let ansi = html_to_ansi("<pre><font color=\"#FF0080\"><b>中&lt;&amp;</b>x</font>y</pre>");
        assert_eq!(
            ansi,
            "\x1b[38;2;255;0;128m\x1b[1m中<&\x1b[0m\x1b[38;2;255;0;128mx\x1b[0my"
        );
    }
}
