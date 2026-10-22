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

use std::path::{Path, PathBuf};
use std::sync::{Mutex, Once};

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
static ACTIVE_LOG_FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// 当前进程实际写入的日志文件。未落盘时为 `None`。
pub fn active_log_file() -> Option<PathBuf> {
    ACTIVE_LOG_FILE.lock().ok().and_then(|slot| slot.clone())
}

/// debug 模式的默认日志文件：`<目录>/muxterm-YYYY-MM-DD.log`。
///
/// 同一天继续追加，换一天写新文件，避免后一次启动覆盖前一天的日志。
/// `log_dir` 为空时用系统临时目录下的 `muxterm-logs`。
pub fn default_debug_log_file(log_dir: Option<&str>) -> PathBuf {
    let (year, month, day) = local_calendar_date();
    debug_log_file_for_date(log_dir, year, month, day)
}

/// 指定公历日期的日志路径。测试用它保证不同日期不会落到同一个文件。
pub fn debug_log_file_for_date(log_dir: Option<&str>, year: i32, month: u32, day: u32) -> PathBuf {
    let dir = log_dir
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("muxterm-logs"));
    dir.join(format!("muxterm-{year:04}-{month:02}-{day:02}.log"))
}

fn local_calendar_date() -> (i32, u32, u32) {
    #[cfg(unix)]
    {
        unsafe {
            let now = libc::time(std::ptr::null_mut());
            let mut tm = std::mem::zeroed::<libc::tm>();
            if !libc::localtime_r(&now, &mut tm).is_null() {
                return (
                    i32::from(tm.tm_year) + 1900,
                    u32::try_from(tm.tm_mon).unwrap_or(0) + 1,
                    u32::try_from(tm.tm_mday).unwrap_or(1),
                );
            }
            utc_calendar_date(now)
        }
    }
    #[cfg(not(unix))]
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0);
        utc_calendar_date(now)
    }
}

fn utc_calendar_date(unix_secs: i64) -> (i32, u32, u32) {
    let days = unix_secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (
        i32::try_from(year).unwrap_or(1970),
        u32::try_from(month).unwrap_or(1),
        u32::try_from(day).unwrap_or(1),
    )
}

/// 从配置原文里读 `[debug]`。配置整体无效时也不要挡住启动，视为未开启。
pub fn debug_settings_from_toml(raw: &str) -> (bool, String) {
    let Ok(value) = raw.parse::<toml::Value>() else {
        return (false, String::new());
    };
    let Some(debug) = value.get("debug").and_then(toml::Value::as_table) else {
        return (false, String::new());
    };
    let enabled = debug
        .get("enabled")
        .and_then(toml::Value::as_bool)
        .unwrap_or(false);
    let log_dir = debug
        .get("log_dir")
        .and_then(toml::Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    (enabled, log_dir)
}

/// 应用启动时的日志决定。
///
/// 优先级：CLI `--log-file` > CLI `--debug` > 配置 `[debug]` > 不落盘。
/// `--debug` 不再要求调用方再给一个文件；缺省写到临时目录。
pub fn resolve_launch_log(
    cli_debug: bool,
    cli_file: Option<&Path>,
    config_toml: Option<&str>,
) -> LoggingConfig {
    if let Some(file) = cli_file.filter(|path| !path.as_os_str().is_empty()) {
        return LoggingConfig {
            level: if cli_debug {
                "debug".into()
            } else {
                "info".into()
            },
            file: Some(file.to_path_buf()),
        };
    }
    if cli_debug {
        return LoggingConfig {
            level: "debug".into(),
            file: Some(default_debug_log_file(None)),
        };
    }
    if let Some(raw) = config_toml {
        let (enabled, log_dir) = debug_settings_from_toml(raw);
        if enabled {
            return LoggingConfig {
                level: "debug".into(),
                file: Some(default_debug_log_file(Some(log_dir.as_str()))),
            };
        }
    }
    LoggingConfig {
        level: "info".into(),
        file: None,
    }
}

pub fn build_version() -> &'static str {
    env!("MUXTERM_BUILD_VERSION")
}

pub fn build_commit() -> &'static str {
    env!("MUXTERM_GIT_COMMIT")
}

/// 构造应用日志过滤器。
///
/// vte 0.13 通过 `log` 记录尚未实现的 OSC 扩展（例如 shell integration 的
/// OSC 133/1337）。这些序列已经由 `TerminalState` 的注意力扫描器消费，vte
/// 的 debug 记录不代表应用错误；保留 vte 的 info/warn/error，避免掩盖真正的
/// 库故障，只把这类 parser 噪声从 debug 日志中排除。
fn build_filter(level: &str) -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::new(level).add_directive(
        "vte=info"
            .parse()
            .expect("固定的 vte 日志 directive 必须有效"),
    )
}

/// 初始化全局 tracing subscriber（进程内只生效一次）。
pub fn init_logging(config: LoggingConfig) -> anyhow::Result<()> {
    let mut result = Ok(());
    LOGGING_INIT.call_once(|| {
        result = init_logging_inner(config);
    });
    result
}

fn init_logging_inner(config: LoggingConfig) -> anyhow::Result<()> {
    let filter = build_filter(&config.level);
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact();

    match config.file {
        Some(path) => {
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        anyhow::anyhow!("创建日志目录失败 {}: {e}", parent.display())
                    })?;
                }
            }
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|e| anyhow::anyhow!("打开日志文件失败 {}: {e}", path.display()))?;
            if let Ok(mut slot) = ACTIVE_LOG_FILE.lock() {
                *slot = Some(path.clone());
            }
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
    crate::performance::start();
    Ok(())
}

/// Swift / FFI 把 UI 诊断写进与 `--log-file` 相同的 tracing 流。
pub fn log_message(level: &str, message: &str) {
    match level {
        "error" => tracing::error!(target: "muxterm::ui", "{message}"),
        "warn" => tracing::warn!(target: "muxterm::ui", "{message}"),
        "info" => tracing::info!(target: "muxterm::ui", "{message}"),
        _ => tracing::debug!(target: "muxterm::ui", "{message}"),
    }
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

    #[test]
    fn dated_log_files_do_not_share_a_path() {
        let first = debug_log_file_for_date(Some("/tmp/muxterm-logs"), 2026, 10, 22);
        let second = debug_log_file_for_date(Some("/tmp/muxterm-logs"), 2026, 10, 23);
        assert_ne!(first, second);
        assert_eq!(
            first,
            PathBuf::from("/tmp/muxterm-logs/muxterm-2026-10-22.log")
        );
        assert_eq!(
            second,
            PathBuf::from("/tmp/muxterm-logs/muxterm-2026-10-23.log")
        );
    }

    #[test]
    fn log_message_does_not_panic_without_subscriber() {
        log_message("debug", "workspace activation begin");
        log_message("error", "clipboard write failed");
        log_message("info", "ok");
    }

    #[test]
    fn launch_log_uses_config_debug_file_without_a_terminal() {
        let (enabled, dir) = debug_settings_from_toml(
            "[debug]\nenabled = true\nlog_dir = \"/tmp/muxterm-custom\"\n",
        );
        assert!(enabled);
        assert_eq!(dir, "/tmp/muxterm-custom");
        let cfg = resolve_launch_log(
            false,
            None,
            Some("[debug]\nenabled = true\nlog_dir = \"/tmp/muxterm-custom\"\n"),
        );
        assert_eq!(cfg.level, "debug");
        assert_eq!(
            cfg.file.as_deref(),
            Some(default_debug_log_file(Some("/tmp/muxterm-custom")).as_path())
        );
        let today = default_debug_log_file(Some("/tmp/muxterm-custom"));
        let name = today
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        assert!(
            name.starts_with("muxterm-") && name.ends_with(".log") && name != "muxterm.log",
            "默认日志必须带日期，避免互相覆盖: {name}"
        );

        let cli = resolve_launch_log(true, None, Some("[debug]\nenabled = false\n"));
        assert_eq!(cli.level, "debug");
        assert_eq!(
            cli.file.as_deref(),
            Some(default_debug_log_file(None).as_path())
        );

        let file = PathBuf::from("/tmp/explicit.log");
        let explicit = resolve_launch_log(false, Some(&file), Some("[debug]\nenabled = true\n"));
        assert_eq!(explicit.level, "info");
        assert_eq!(explicit.file.as_deref(), Some(file.as_path()));

        let off = resolve_launch_log(false, None, Some("font = {}\n"));
        assert!(off.file.is_none());
        assert_eq!(off.level, "info");
    }

    #[test]
    fn debug_filter_quiets_vte_parser_diagnostics() {
        let filter = build_filter("debug");
        let rendered = filter.to_string();
        let directives: Vec<&str> = rendered.split(',').collect();
        assert!(directives.contains(&"debug"));
        assert!(
            directives.contains(&"vte=info"),
            "vte 的 unhandled OSC debug 不能污染应用 debug 日志: {directives:?}"
        );
    }
}
