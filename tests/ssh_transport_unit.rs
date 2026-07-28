//! 底层 SshProcessTransport 生命周期测试：分阶段验证 spawn/read/exit/cleanup。
//!
//! 用共享 sshd + 简单远端命令（echo marker），硬超时 15s。
//! 每个阶段打印明确标记，定位卡在哪个环节。

#![cfg(feature = "ffi")]

mod support;

use muxterm::core::transport::ssh::{build_ssh_command, SshProcessTransport};
use muxterm::core::transport::{PtySize, Transport, TransportSignal};
use std::time::{Duration, Instant};
use support::sshd_test_support::*;
use support::tmux_test_support::*;

fn bounded<T>(secs: u64, label: &str, f: impl FnOnce() -> T + Send) -> T
where
    T: Send,
{
    run_with_timeout(Duration::from_secs(secs), label, f)
}

fn sshd_ready() -> bool {
    sshd_available()
}

fn ssh_env(label: &str) -> SshTestEnv {
    SshTestEnv::setup(label).expect("SSH 测试环境创建失败")
}

/// 阶段化 echo marker 测试：每步打印标记，定位卡在哪。
#[test]
#[ignore]
fn ssh_transport_echo_marker_staged() {
    bounded(20, "echo-staged", || {
        assert!(sshd_ready(), "需要共享 sshd");
        let env = ssh_env("echo-staged");

        let config_path = &env.ssh_config_path;
        assert!(
            config_path.exists(),
            "config 不存在: {}",
            config_path.display()
        );

        let (program, args) = build_ssh_command(
            &env.alias,
            "echo STAGED_MARKER_42",
            Some(&config_path.to_string_lossy()),
        );
        eprintln!("[stage1] argv: {program} {:?}", args);
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

        // ── Stage 1: spawn ──
        let mut transport = SshProcessTransport::new();
        transport
            .spawn_exec(&program, &arg_refs, PtySize::new(80, 24))
            .expect("SSH spawn 失败");
        let pid = transport
            .try_wait()
            .ok()
            .flatten()
            .map(|c| format!("pid exited code={c}"))
            .unwrap_or_else(|| "running".to_string());
        eprintln!("[stage1] spawn ok, child={pid}");

        // ── Stage 2: collect output until marker found or EOF ──
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut all_output = Vec::new();
        let mut got_marker = false;
        let mut got_eof = false;
        let mut child_exited = false;

        while Instant::now() < deadline {
            match transport.read() {
                Ok(Some(data)) => {
                    all_output.extend_from_slice(&data);
                    if String::from_utf8_lossy(&all_output).contains("STAGED_MARKER_42") {
                        got_marker = true;
                        eprintln!("[stage2] marker found in output");
                    }
                }
                Ok(None) => {
                    // No data yet — check if child exited
                    if let Ok(Some(code)) = transport.try_wait() {
                        child_exited = true;
                        eprintln!("[stage2] child exited code={code}");
                        // Drain remaining
                        while let Ok(Some(d)) = transport.read() {
                            all_output.extend_from_slice(&d);
                        }
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => {
                    got_eof = true;
                    eprintln!("[stage2] read Err (EOF): {e}");
                    // Drain any remaining
                    break;
                }
            }
        }

        // ── Stage 3: verify child exit ──
        if !child_exited {
            if let Ok(Some(code)) = transport.try_wait() {
                child_exited = true;
                eprintln!("[stage3] child exited code={code}");
            } else {
                eprintln!("[stage3] child still running, killing");
            }
        }

        // ── Stage 4: explicit cleanup — kill + drop master ──
        let _ = transport.kill(TransportSignal::Term);
        eprintln!("[stage4] kill sent");

        // Drop transport (drops master → reader thread gets EOF → channel closes)
        drop(transport);
        eprintln!("[stage4] transport dropped (master released)");

        // ── Assertions ──
        let text = String::from_utf8_lossy(&all_output);
        eprintln!(
            "[result] output={text:?} marker={got_marker} eof={got_eof} exited={child_exited}"
        );

        assert!(
            got_marker || text.contains("STAGED_MARKER_42"),
            "应读到 marker: text={text:?}"
        );
        assert!(
            child_exited || got_eof,
            "子进程应退出或 EOF: exited={child_exited} eof={got_eof}"
        );
    });
}

/// 不存在 host 应快速失败（不需要 sshd）。
#[test]
fn ssh_transport_nonexistent_host_fails() {
    bounded(15, "bad-host", || {
        let (program, args) = build_ssh_command("nonexistent-host-xyz-999", "echo hi", None);
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

        let mut transport = SshProcessTransport::new();
        let _ = transport.spawn_exec(&program, &arg_refs, PtySize::new(80, 24));

        let deadline = Instant::now() + Duration::from_secs(15);
        let mut exited = false;
        while Instant::now() < deadline {
            match transport.read() {
                Ok(Some(_)) => {}
                Ok(None) => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => {
                    exited = true;
                    break;
                }
            }
            if let Ok(Some(_)) = transport.try_wait() {
                exited = true;
                break;
            }
        }
        let _ = transport.kill(TransportSignal::Term);
        drop(transport);
        assert!(exited, "不存在 host 的 ssh 应在超时前退出/报错");
    });
}
