//! SSH config discovery for transport targets.

use std::path::{Path, PathBuf};

/// SSH Host 条目（从 `~/.ssh/config` 读取）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SshHostEntry {
    /// Host 别名（如 "myserver"）。
    pub alias: String,
    /// HostName（如 "server.example.com"）。
    pub hostname: String,
    /// Port（默认 22）。
    pub port: u16,
    /// User。
    pub user: String,
}

#[derive(Debug, Clone, Default)]
struct SshConfigBlock {
    patterns: Vec<String>,
    options: Vec<(String, String)>,
}

/// 解析 OpenSSH config 中用于连接发现的字段。
///
/// 这里只解析 Host/HostName/User/Port。认证、ProxyJump、Include 等连接行为
/// 仍然完全交给系统 `ssh`，因此 Muxterm 不会复制或替换用户的 SSH 配置。
pub fn parse_ssh_config(text: &str) -> Vec<SshHostEntry> {
    let mut global = SshConfigBlock::default();
    let mut blocks = Vec::new();
    let mut current: Option<SshConfigBlock> = None;

    for raw_line in text.lines() {
        let line = strip_config_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, raw_value)) = split_config_line(line) else {
            continue;
        };
        let key = key.to_ascii_lowercase();
        if key == "host" {
            if let Some(block) = current.take() {
                blocks.push(block);
            }
            current = Some(SshConfigBlock {
                patterns: split_config_words(raw_value),
                options: Vec::new(),
            });
        } else if let Some(block) = current.as_mut() {
            block.options.push((key, unquote_config_value(raw_value)));
        } else {
            global.options.push((key, unquote_config_value(raw_value)));
        }
    }
    if let Some(block) = current {
        blocks.push(block);
    }

    let mut aliases = Vec::new();
    for block in &blocks {
        for pattern in &block.patterns {
            if !pattern.starts_with('!') && !has_glob(pattern) && !aliases.contains(pattern) {
                aliases.push(pattern.clone());
            }
        }
    }

    aliases
        .into_iter()
        .map(|alias| {
            let hostname = ssh_config_value(&alias, "hostname", &global, &blocks)
                .unwrap_or_else(|| alias.clone());
            let user = ssh_config_value(&alias, "user", &global, &blocks).unwrap_or_default();
            let port = ssh_config_value(&alias, "port", &global, &blocks)
                .and_then(|value| value.parse().ok())
                .filter(|port: &u16| *port > 0)
                .unwrap_or(22);
            SshHostEntry {
                alias,
                hostname,
                port,
                user,
            }
        })
        .collect()
}

/// 列出用户现有 SSH 配置中的 Host alias。
///
/// `path` 仅用于测试或显式配置；未传入时优先使用 `MUXTERM_SSH_CONFIG_PATH`，
/// 否则读取默认的 `~/.ssh/config`。不存在配置文件视为没有可发现的主机。
pub fn list_ssh_hosts(path: Option<&Path>) -> anyhow::Result<Vec<SshHostEntry>> {
    let path = path
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("MUXTERM_SSH_CONFIG_PATH").map(PathBuf::from))
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".ssh").join("config"))
        });
    let Some(path) = path else {
        return Ok(Vec::new());
    };
    let mut visited = Vec::new();
    match load_ssh_config(&path, &mut visited) {
        Ok(text) => Ok(parse_ssh_config(&text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}

fn load_ssh_config(path: &Path, visited: &mut Vec<PathBuf>) -> std::io::Result<String> {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if visited.contains(&path) {
        return Ok(String::new());
    }
    visited.push(path.clone());

    let text = std::fs::read_to_string(&path)?;
    let mut expanded = String::new();
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    for raw_line in text.lines() {
        let line = strip_config_comment(raw_line).trim();
        let is_include =
            split_config_line(line).is_some_and(|(key, _)| key.eq_ignore_ascii_case("include"));
        if !is_include {
            expanded.push_str(raw_line);
            expanded.push('\n');
            continue;
        }

        let Some((_, raw_patterns)) = split_config_line(line) else {
            continue;
        };
        for pattern in split_config_words(raw_patterns) {
            for include_path in expand_include_pattern(&pattern, base_dir) {
                match load_ssh_config(&include_path, visited) {
                    Ok(included) => expanded.push_str(&included),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
        }
    }
    Ok(expanded)
}

fn expand_include_pattern(pattern: &str, base_dir: &Path) -> Vec<PathBuf> {
    let expanded = if let Some(rest) = pattern.strip_prefix("~/") {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(rest))
            .unwrap_or_else(|| PathBuf::from(pattern))
    } else {
        let path = PathBuf::from(pattern);
        if path.is_absolute() {
            path
        } else {
            base_dir.join(path)
        }
    };
    if !has_glob(&expanded.to_string_lossy()) {
        return if expanded.is_file() {
            vec![expanded]
        } else {
            Vec::new()
        };
    }

    let Some(parent) = expanded.parent() else {
        return Vec::new();
    };
    let Some(file_pattern) = expanded.file_name().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = match std::fs::read_dir(parent) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| glob_matches(file_pattern, name))
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    paths.sort();
    paths
}

fn split_config_line(line: &str) -> Option<(&str, &str)> {
    let mut fields = line.splitn(2, char::is_whitespace);
    let key = fields.next()?.trim();
    let value = fields.next()?.trim();
    (!key.is_empty() && !value.is_empty()).then_some((key, value))
}

fn strip_config_comment(line: &str) -> &str {
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '\'' | '"' => {
                if quote == Some(ch) {
                    quote = None;
                } else if quote.is_none() {
                    quote = Some(ch);
                }
            }
            '#' if quote.is_none()
                && line[..index]
                    .chars()
                    .next_back()
                    .is_none_or(char::is_whitespace) =>
            {
                return &line[..index];
            }
            _ => {}
        }
    }
    line
}

fn split_config_words(value: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    for ch in value.chars() {
        if escaped {
            word.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '\'' | '"' if quote == Some(ch) => quote = None,
            '\'' | '"' if quote.is_none() => quote = Some(ch),
            c if c.is_whitespace() && quote.is_none() => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            c => word.push(c),
        }
    }
    if escaped {
        word.push('\\');
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

fn unquote_config_value(value: &str) -> String {
    split_config_words(value).join(" ")
}

fn ssh_config_value(
    alias: &str,
    key: &str,
    global: &SshConfigBlock,
    blocks: &[SshConfigBlock],
) -> Option<String> {
    global
        .options
        .iter()
        .find(|(option, _)| option == key)
        .map(|(_, value)| value.clone())
        .or_else(|| {
            blocks
                .iter()
                .filter(|block| ssh_block_matches(alias, &block.patterns))
                .flat_map(|block| block.options.iter())
                .find(|(option, _)| option == key)
                .map(|(_, value)| value.clone())
        })
}

fn ssh_block_matches(alias: &str, patterns: &[String]) -> bool {
    let mut has_positive = false;
    let mut positive_match = false;
    for pattern in patterns {
        if let Some(pattern) = pattern.strip_prefix('!') {
            if glob_matches(pattern, alias) {
                return false;
            }
        } else {
            has_positive = true;
            positive_match |= glob_matches(pattern, alias);
        }
    }
    !has_positive || positive_match
}

fn has_glob(value: &str) -> bool {
    value.contains(['*', '?'])
}

fn glob_matches(pattern: &str, value: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let value: Vec<char> = value.chars().collect();
    let mut dp = vec![vec![false; value.len() + 1]; pattern.len() + 1];
    dp[0][0] = true;
    for i in 0..pattern.len() {
        for j in 0..=value.len() {
            if !dp[i][j] {
                continue;
            }
            if pattern[i] == '*' {
                dp[i + 1][j] = true;
                if j < value.len() {
                    dp[i][j + 1] = true;
                }
            } else if j < value.len() && (pattern[i] == '?' || pattern[i] == value[j]) {
                dp[i + 1][j + 1] = true;
            }
        }
    }
    dp[pattern.len()][value.len()]
}
