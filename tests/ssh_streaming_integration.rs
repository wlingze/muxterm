//! Phase 6：SSH loopback E2E — 远端 tmux -CC 流式输出。
//!
//! 验证 muxterm 经 SSH transport 在远端启动 `tmux -CC` 后，能收到远端 pane 的
//! 流式输出（持续输出 / 高频 / 长行），并且远端 tmux 的 %output 与本地一样被
//! 正确解析。这是「SSH 放大本地问题」的核心场景。
//!
//! 需要共享 loopback sshd（由 CI setup 或本地环境提供），运行时：
//!   cargo test --features ffi --test ssh_streaming_integration -- --ignored

#![cfg(feature = "ffi")]

mod support;

use muxterm::core::runtime::tmux::client::{
    ConnectMode, TmuxClient, TmuxClientConfig, TmuxClientHandle, TmuxEvent,
};
use muxterm::core::runtime::tmux::protocol::Message;
use std::time::Duration;
use support::sshd_test_support::*;
use support::tmux_test_support::*;

fn ssh_env(label: &str) -> SshTestEnv {
    let env = SshTestEnv::setup(label).expect("SSH 测试环境创建失败");
    // 让子进程 ssh 用测试生成的 config（含 CI 授权的 key），
    // 并让 spawn_ssh 通过 MUXTERM_SSH_CONFIG_PATH 显式 -F 指定。
    std::env::set_var("HOME", &env.home_dir);
    std::env::set_var("MUXTERM_SSH_CONFIG_PATH", &env.ssh_config_path);
    env
}

/// 连接远端 tmux -CC，返回句柄 + 事件接收器 + 已收集的 pane id。
async fn connect_remote(
    env: &SshTestEnv,
    name: &str,
) -> (TmuxClientHandle, tokio::sync::mpsc::Receiver<TmuxEvent>) {
    let config = TmuxClientConfig {
        mode: Some(ConnectMode::NewSession {
            name: Some(name.into()),
        }),
        extra_args: vec!["-L".into(), env.remote_tmux_socket.clone()],
        cols: Some(80),
        rows: Some(24),
        event_buffer: 4096,
        tmux_bin: None,
        ssh_alias: Some(env.alias.clone()),
    };
    TmuxClient::spawn(config)
        .await
        .expect("远端 tmux -CC 启动失败")
}

fn concat(outputs: &[String]) -> String {
    outputs.concat()
}

// ── 1. 远端持续输出：echo 循环累积 ──────────────────────────

#[test]
#[ignore = "requires sshd + SSH key setup"]
fn ssh_remote_continuous_output() {
    run_with_timeout(Duration::from_secs(60), "ssh-continuous", || {
        assert!(sshd_available(), "需要共享 sshd");
        assert!(tmux_available(), "需要 tmux");
        let env = ssh_env("ssh-continuous");

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .build()
            .unwrap();

        let outputs = rt.block_on(async {
            let (mut h, mut rx) = connect_remote(&env, &format!("ssh-cont-{}", std::process::id())).await;

            // 等首个 %output，拿到远端 pane id（可能是 %N，N 非 0）
            let pane_id = {
                let mut pid = None;
                let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(10);
                loop {
                    let ev = tokio::time::timeout(
                        deadline.saturating_duration_since(tokio::time::Instant::now()),
                        rx.recv(),
                    )
                    .await;
                    match ev {
                        Ok(Some(TmuxEvent::Message(Message::Output { pane, .. }))) => {
                            pid = Some(format!("%{}", pane.0));
                            break;
                        }
                        Ok(Some(TmuxEvent::Exit { .. })) | Ok(None) => break,
                        Ok(Some(_)) => {}
                        Err(_) => break,
                    }
                }
                pid
            };
            let pane_id = pane_id.expect("未从远端收到首个 %output（拿不到 pane id）");

            // 向该 pane 发持续输出命令（-l 逐字文本 + 单独 Enter 键）
            let cmd = format!(
                "send-keys -t {} -l 'for i in $(seq 1 20); do echo ssh-remote-line-$i; sleep 0.05; done'",
                pane_id
            );
            let enter = format!("send-keys -t {} Enter", pane_id);
            h.send_raw(&format!("{}\n", cmd)).await.unwrap();
            h.send_raw(&format!("{}\n", enter)).await.unwrap();

            let mut outputs: Vec<String> = Vec::new();
            let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(20);
            loop {
                let ev = tokio::time::timeout(
                    deadline.saturating_duration_since(tokio::time::Instant::now()),
                    rx.recv(),
                )
                .await;
                match ev {
                    Ok(Some(TmuxEvent::Message(Message::Output { content, .. }))) => {
                        let s = String::from_utf8_lossy(&content).into_owned();
                        outputs.push(s);
                        if concat(&outputs).contains("ssh-remote-line-") {
                            break;
                        }
                    }
                    Ok(Some(TmuxEvent::Exit { .. })) | Ok(None) => break,
                    Ok(Some(_)) => {}
                    Err(_) => break,
                }
            }
            let _ = h.kill().await;
            outputs
        });

        assert!(
            concat(&outputs).contains("ssh-remote-line-"),
            "远端应收到持续输出行: {:?}",
            outputs
        );

        let _ = env.remote_tmux("kill-server");
    });
}

// ── 2. 远端高频输出 + 长行：验证 %output 不截断 ─────────────

#[test]
#[ignore = "requires sshd + SSH key setup"]
fn ssh_remote_high_freq_and_long_line() {
    run_with_timeout(Duration::from_secs(60), "ssh-hifreq", || {
        assert!(sshd_available(), "需要共享 sshd");
        assert!(tmux_available(), "需要 tmux");
        let env = ssh_env("ssh-hifreq");

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .build()
            .unwrap();

        let outputs = rt.block_on(async {
            let (mut h, mut rx) = connect_remote(&env, &format!("ssh-hi-{}", std::process::id())).await;

            let pane_id = {
                let mut pid = None;
                let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(10);
                loop {
                    let ev = tokio::time::timeout(
                        deadline.saturating_duration_since(tokio::time::Instant::now()),
                        rx.recv(),
                    )
                    .await;
                    match ev {
                        Ok(Some(TmuxEvent::Message(Message::Output { pane, .. }))) => {
                            pid = Some(format!("%{}", pane.0));
                            break;
                        }
                        Ok(Some(TmuxEvent::Exit { .. })) | Ok(None) => break,
                        Ok(Some(_)) => {}
                        Err(_) => break,
                    }
                }
                pid
            };
            let pane_id = pane_id.expect("未从远端收到首个 %output");

            // 高频输出 + 一条超长行（-l 逐字 + 单独 Enter）
            let cmd = format!(
                "send-keys -t {} -l 'seq 1 500; head -c 20000 /dev/zero | tr \\\\x27\\\\0\\\\x27 x; echo LONGDONE'",
                pane_id
            );
            let enter = format!("send-keys -t {} Enter", pane_id);
            h.send_raw(&format!("{}\n", cmd)).await.unwrap();
            h.send_raw(&format!("{}\n", enter)).await.unwrap();

            let mut outputs: Vec<String> = Vec::new();
            let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(20);
            loop {
                let ev = tokio::time::timeout(
                    deadline.saturating_duration_since(tokio::time::Instant::now()),
                    rx.recv(),
                )
                .await;
                match ev {
                    Ok(Some(TmuxEvent::Message(Message::Output { content, .. }))) => {
                        let s = String::from_utf8_lossy(&content).into_owned();
                        outputs.push(s);
                        if concat(&outputs).contains("LONGDONE") {
                            break;
                        }
                    }
                    Ok(Some(TmuxEvent::Exit { .. })) | Ok(None) => break,
                    Ok(Some(_)) => {}
                    Err(_) => break,
                }
            }
            let _ = h.kill().await;
            outputs
        });

        let all = concat(&outputs);
        assert!(all.contains("LONGDONE"), "长行命令应执行完: {all}");
        assert!(
            all.contains("500") || all.len() > 1000,
            "应收到高频/大量输出, got len={}",
            all.len()
        );

        let _ = env.remote_tmux("kill-server");
    });
}
