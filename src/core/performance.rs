//! 独立于 UI / Core poll 的诊断线程。计时是 wall time，不冒充 CPU 归因。
//! macOS 超限时由 sample 抓全进程线程栈（也包含 Swift），不持有 Core 锁。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::Once;
use std::time::{Duration, Instant};

static ENABLED: AtomicBool = AtomicBool::new(false);
static START: Once = Once::new();
const COOLDOWN: Duration = Duration::from_secs(30);

pub(crate) struct Stage {
    name: &'static str,
    calls: AtomicU64,
    total_us: AtomicU64,
    max_us: AtomicU64,
    active: AtomicU64,
}

impl Stage {
    const fn new(name: &'static str) -> Self {
        Self {
            name,
            calls: AtomicU64::new(0),
            total_us: AtomicU64::new(0),
            max_us: AtomicU64::new(0),
            active: AtomicU64::new(0),
        }
    }

    pub(crate) fn enter(&'static self) -> Guard {
        let start = ENABLED.load(Relaxed).then(|| {
            self.active.fetch_add(1, Relaxed);
            Instant::now()
        });
        Guard { stage: self, start }
    }

    fn drain(&self) -> (u64, u64, u64, u64) {
        (
            self.calls.swap(0, Relaxed),
            self.total_us.swap(0, Relaxed),
            self.max_us.swap(0, Relaxed),
            self.active.load(Relaxed),
        )
    }
}

pub(crate) struct Guard {
    stage: &'static Stage,
    start: Option<Instant>,
}
impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(start) = self.start {
            let micros = start.elapsed().as_micros().min(u64::MAX as u128) as u64;
            self.stage.calls.fetch_add(1, Relaxed);
            self.stage.total_us.fetch_add(micros, Relaxed);
            self.stage.max_us.fetch_max(micros, Relaxed);
            self.stage.active.fetch_sub(1, Relaxed);
        }
    }
}

pub(crate) static POLL: Stage = Stage::new("ffi.poll");
pub(crate) static RUNTIME: Stage = Stage::new("runtime.refresh");
pub(crate) static INDEX: Stage = Stage::new("index.feed");
pub(crate) static ATTENTION: Stage = Stage::new("activity.batch");
pub(crate) static SCREEN: Stage = Stage::new("activity.screen_rules");
pub(crate) static PROCESS: Stage = Stage::new("process.observe");
pub(crate) static SEND: Stage = Stage::new("tmux.send");
pub(crate) static INPUT: Stage = Stage::new("runtime.input");
static STAGES: &[&Stage] = &[
    &POLL, &RUNTIME, &INDEX, &ATTENTION, &SCREEN, &PROCESS, &SEND, &INPUT,
];

/// 写一条控制命令。高频 send-keys -H（鼠标）只记次数，避免刷 debug 日志。
pub(crate) fn note_send() {
    SEND.calls.fetch_add(1, Relaxed);
}

pub(crate) fn log_control_send(target: &'static str, raw: &str) {
    note_send();
    if raw.contains("send-keys") && raw.contains("-H") {
        tracing::trace!(target, "send: {:?}", raw);
    } else {
        tracing::debug!(target, "send: {:?}", raw);
    }
}

fn cpu_percent(previous: Duration, current: Duration, wall: Duration) -> Option<f64> {
    if wall.is_zero() {
        return None;
    }
    Some(current.checked_sub(previous)?.as_secs_f64() / wall.as_secs_f64() * 100.0)
}

fn should_capture(cpu: f64, now: Duration, last: Option<Duration>) -> bool {
    cpu > 100.0 && last.is_none_or(|last| now.saturating_sub(last) >= COOLDOWN)
}

#[cfg(unix)]
fn process_cpu() -> Option<Duration> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // RUSAGE_SELF 只测本进程，不把 ps/sample/Agent 子进程算成 Muxterm CPU。
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return None;
    }
    let usage = unsafe { usage.assume_init() };
    let micros = |time: libc::timeval| -> Option<u64> {
        u64::try_from(time.tv_sec)
            .ok()?
            .checked_mul(1_000_000)?
            .checked_add(u64::try_from(time.tv_usec).ok()?)
    };
    Some(Duration::from_micros(
        micros(usage.ru_utime)?.checked_add(micros(usage.ru_stime)?)?,
    ))
}

pub(crate) fn start() {
    #[cfg(unix)]
    START.call_once(|| {
        if !tracing::enabled!(target: "muxterm::performance", tracing::Level::DEBUG) {
            return;
        }
        match std::thread::Builder::new()
            .name("muxterm-performance".into())
            .spawn(monitor)
        {
            Ok(_) => {
                ENABLED.store(true, Relaxed);
            }
            Err(error) => tracing::warn!(%error, "performance monitor unavailable"),
        }
    });
}

#[cfg(unix)]
fn monitor() {
    tracing::debug!(
        log_threshold_cpu_percent = 90,
        sample_threshold_cpu_percent = 100,
        interval_ms = 2000,
        cooldown_secs = 30,
        "performance monitor started (100% = one core)"
    );
    let epoch = Instant::now();
    let mut previous = process_cpu().map(|cpu| (Instant::now(), cpu));
    let mut last_capture = None;
    loop {
        std::thread::sleep(Duration::from_secs(2));
        let now = Instant::now();
        let current = process_cpu();
        let cpu = previous.zip(current).and_then(|((then, before), after)| {
            cpu_percent(before, after, now.duration_since(then))
        });
        previous = current.map(|cpu| (now, cpu));
        // Activity Monitor 的 90%+ 是一核；sample 仍要 100% 才抓栈，避免 sample 自己搅局。
        let log_high = cpu.is_some_and(|cpu| cpu >= 90.0);
        let sample_high = cpu.is_some_and(|cpu| cpu > 100.0);
        for stage in STAGES {
            let (calls, total_us, max_us, active) = stage.drain();
            if log_high || max_us >= 100_000 || (stage.name == "tmux.send" && calls > 20) {
                tracing::debug!(
                    stage = stage.name,
                    calls,
                    total_us,
                    max_us,
                    active,
                    "performance stage wall time (nested stages overlap)"
                );
            }
        }
        if let Some(cpu) = cpu.filter(|_| log_high) {
            tracing::debug!(cpu_percent = cpu, "performance CPU threshold exceeded");
            if sample_high && should_capture(cpu, epoch.elapsed(), last_capture) {
                last_capture = Some(epoch.elapsed());
                let _ = capture();
            }
        }
    }
}

/// 日志保留有限的调用树和叶子栈摘要；完整原始报告是临时诊断附件。
fn report_excerpt(report: &str) -> String {
    let mut out = String::new();
    if let Some((_, tree)) = report.split_once("Call graph:") {
        out.push_str("Call graph (first 100 lines):\n");
        for line in tree.lines().take(100).take_while(|line| {
            !line.starts_with("Total number in stack")
                && !line.starts_with("Sort by top of stack")
                && !line.starts_with("Binary Images:")
        }) {
            if out.len() + line.len() > 24 * 1024 {
                break;
            }
            out.push_str(line);
            out.push('\n');
        }
    }
    if let Some((_, top)) = report.split_once("Sort by top of stack") {
        out.push_str("Top of stack (includes waiting threads; not CPU percentages):\n");
        for line in top
            .lines()
            .take(40)
            .take_while(|line| !line.starts_with("Binary Images:"))
        {
            if out.len() + line.len() > 32 * 1024 {
                break;
            }
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

#[cfg(target_os = "macos")]
fn capture() -> std::io::Result<()> {
    use std::io::Read;
    use std::os::unix::fs::DirBuilderExt;
    use std::process::{Command, Stdio};
    use std::sync::OnceLock;
    use std::time::{SystemTime, UNIX_EPOCH};
    static DIRECTORY: OnceLock<std::path::PathBuf> = OnceLock::new();
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let result = (|| -> std::io::Result<()> {
        let dir = DIRECTORY.get_or_init(|| {
            std::env::temp_dir().join(format!(
                "muxterm-profile-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ))
        });
        if !dir.exists() {
            std::fs::DirBuilder::new().mode(0o700).create(dir)?;
        }
        let sequence = SEQUENCE.fetch_add(1, Relaxed);
        let path = dir.join(format!("sample-{sequence}.txt"));
        // 只保留本监控实例最近三份文件，不碰用户文件或任何会话。
        if sequence >= 3 {
            let _ = std::fs::remove_file(dir.join(format!("sample-{}.txt", sequence - 3)));
        }
        let mut child = Command::new("/usr/bin/sample")
            .args([
                std::process::id().to_string(),
                "1".into(),
                "10".into(),
                "-file".into(),
            ])
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(15);
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "sample exceeded 15s deadline",
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        if !status.success() {
            return Err(std::io::Error::other(format!("sample failed: {status}")));
        }
        let mut bytes = Vec::new();
        std::fs::File::open(&path)?
            .take(2 * 1024 * 1024)
            .read_to_end(&mut bytes)?;
        let report = report_excerpt(&String::from_utf8_lossy(&bytes));
        tracing::debug!(path = %path.display(), %report, "performance stack sample completed");
        Ok(())
    })();
    if let Err(error) = &result {
        tracing::debug!(%error, "performance stack sample failed; stage metrics remain available");
    }
    result
}

#[cfg(not(target_os = "macos"))]
fn capture() -> std::io::Result<()> {
    tracing::debug!(
        "performance stack sampling unavailable on this platform; see stage wall times"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cpu_is_per_core_not_normalized_and_rejects_invalid_deltas() {
        assert_eq!(
            cpu_percent(
                Duration::from_secs(1),
                Duration::from_secs(4),
                Duration::from_secs(2)
            ),
            Some(150.0)
        );
        assert_eq!(
            cpu_percent(
                Duration::from_secs(2),
                Duration::from_secs(1),
                Duration::from_secs(2)
            ),
            None
        );
        assert_eq!(
            cpu_percent(Duration::ZERO, Duration::ZERO, Duration::ZERO),
            None
        );
    }
    #[test]
    fn capture_has_strict_threshold_and_cooldown() {
        assert!(!should_capture(100.0, Duration::ZERO, None));
        assert!(should_capture(101.0, Duration::ZERO, None));
        assert!(!should_capture(
            200.0,
            Duration::from_secs(29),
            Some(Duration::ZERO)
        ));
        assert!(should_capture(
            101.0,
            Duration::from_secs(30),
            Some(Duration::ZERO)
        ));
        assert!(!should_capture(90.0, Duration::ZERO, None));
    }
    #[test]
    fn excerpt_keeps_stacks_bounded_and_omits_binary_images() {
        let report = format!(
            "Call graph:\n{}Sort by top of stack\n busy 12\nBinary Images:\nsecret-path",
            "frame\n".repeat(200)
        );
        let excerpt = report_excerpt(&report);
        assert!(excerpt.contains("busy 12"));
        assert!(excerpt.lines().count() < 150);
        assert!(!excerpt.contains("secret-path"));
        assert!(
            !report_excerpt("Call graph:\n frame\nBinary Images:\nsecret-path")
                .contains("secret-path")
        );
    }

    #[test]
    fn stage_records_completed_and_in_flight_work_without_locks() {
        static STAGE: Stage = Stage::new("test");
        STAGE.active.store(1, Relaxed);
        let guard = Guard {
            stage: &STAGE,
            start: Some(Instant::now()),
        };
        assert_eq!(STAGE.drain().3, 1);
        drop(guard);
        let (calls, total, max, active) = STAGE.drain();
        assert_eq!(calls, 1);
        assert_eq!(total, max);
        assert_eq!(active, 0);
        assert_eq!(STAGE.drain(), (0, 0, 0, 0));
    }
    #[test]
    #[cfg(target_os = "macos")]
    #[ignore = "manual real sampler smoke test"]
    fn sample_current_process_without_ui_or_core_locks() {
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_test_writer()
            .finish();
        tracing::subscriber::with_default(subscriber, capture)
            .expect("real sample must produce a report");
    }
}
