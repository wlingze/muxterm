//! 统一的 tracing 初始化（CLI 与 FFI / macOS .app 共用）。
//!
//! - CLI：`muxterm --debug [--log-file PATH]`
//! - macOS .app：Swift 解析 `--debug` / `--log-file` 后调用
//!   `muxterm_init_logging`，日志落到文件而不是 LaunchServices 的 stderr。
//!
//! 环境变量兜底（优先级低于 CLI 参数）：
//! - `MUXTERM_LOG`：日志级别（`trace`/`debug`/`info`/`warn`/`error`）
//! - `MUXTERM_LOG_FILE`：日志文件路径；不设则写 stderr
//!
//! 全局 subscriber 只能初始化一次，重复 `try_init` 会返回
//! `AlreadyInitialized`，这里视为成功，避免 FFI 与 CLI 双层初始化 panic。

use std::path::PathBuf;
use std::sync::Once;

/// 日志初始化参数。
#[derive(Debug, Clone)]
pub struct LoggingConfig {
    /// 日志级别：`trace` / `debug` / `info` / `warn` / `error`。
    pub level: String,
    /// 输出文件；`None` 时写 stderr。
    pub file: Option<PathBuf>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
            file: None,
        }
    }
}

/// 从环境变量读日志级别；未设置返回默认 `info`。
pub fn level_from_env() -> String {
    std::env::var("MUXTERM_LOG")
        .map(|v| v.trim().to_ascii_lowercase())
        .unwrap_or_else(|_| "info".into())
}

/// 从环境变量读日志文件路径；未设置返回 `None`（写 stderr）。
pub fn file_from_env() -> Option<PathBuf> {
    std::env::var("MUXTERM_LOG_FILE")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
}

/// 合并 CLI 参数与环境变量：CLI 显式给的优先，缺省用环境变量兜底。
///
/// - `cli_level`：CLI 解析出的级别（如 `--debug` 时为 `debug`）；`None` 表示未指定。
/// - `cli_file`：CLI 的 `--log-file`；`None` 表示未指定。
pub fn resolve_config(cli_level: Option<String>, cli_file: Option<PathBuf>) -> LoggingConfig {
    let level = cli_level.unwrap_or_else(level_from_env);
    let file = cli_file.or_else(file_from_env);
    LoggingConfig { level, file }
}

static LOGGING_INIT: Once = Once::new();

/// 初始化全局 tracing subscriber（进程内只生效一次）。
pub fn init_logging(config: LoggingConfig) -> anyhow::Result<()> {
    let mut result = Ok(());
    LOGGING_INIT.call_once(|| {
        result = init_logging_inner(config);
    });
    result
}

fn init_logging_inner(config: LoggingConfig) -> anyhow::Result<()> {
    let filter = tracing_subscriber::EnvFilter::new(&config.level);
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact();

    match config.file {
        Some(path) => {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|e| anyhow::anyhow!("打开日志文件失败 {}: {e}", path.display()))?;
            builder
                .with_writer(std::sync::Mutex::new(file))
                .try_init()
                .map_err(|e| anyhow::anyhow!("初始化 tracing 失败: {e}"))?;
        }
        None => {
            builder
                .with_writer(std::io::stderr)
                .try_init()
                .map_err(|e| anyhow::anyhow!("初始化 tracing 失败: {e}"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_twice_does_not_panic() {
        // 第一个成功；第二个 AlreadyInitialized 视为成功。
        let first = init_logging(LoggingConfig::default());
        assert!(first.is_ok());
        let second = init_logging(LoggingConfig {
            level: "debug".into(),
            file: None,
        });
        assert!(second.is_ok());
    }

    #[test]
    fn resolve_config_merges_cli_and_env_without_race() {
        // 先隔离环境变量，避免并行测试互相污染。
        unsafe {
            std::env::remove_var("MUXTERM_LOG");
            std::env::remove_var("MUXTERM_LOG_FILE");
        }
        // CLI 未给时，默认 info/stderr。
        let cfg_default = resolve_config(None, None);
        assert_eq!(cfg_default.level, "info");
        assert_eq!(cfg_default.file, None);

        // 设置环境变量后，CLI 未给则回退环境变量。
        unsafe {
            std::env::set_var("MUXTERM_LOG", "warn");
            std::env::set_var("MUXTERM_LOG_FILE", "/tmp/env.log");
        }
        let cfg_env = resolve_config(None, None);
        assert_eq!(cfg_env.level, "warn");
        assert_eq!(cfg_env.file, Some(PathBuf::from("/tmp/env.log")));

        // CLI 显式指定优先于环境变量。
        let cfg_cli = resolve_config(Some("debug".into()), Some(PathBuf::from("/tmp/cli.log")));
        assert_eq!(cfg_cli.level, "debug");
        assert_eq!(cfg_cli.file, Some(PathBuf::from("/tmp/cli.log")));

        // CLI 只给级别、文件走环境变量。
        let cfg_mixed = resolve_config(Some("debug".into()), None);
        assert_eq!(cfg_mixed.level, "debug");
        assert_eq!(cfg_mixed.file, Some(PathBuf::from("/tmp/env.log")));

        // 清理。
        unsafe {
            std::env::remove_var("MUXTERM_LOG");
            std::env::remove_var("MUXTERM_LOG_FILE");
        }
    }
}
