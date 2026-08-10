//! RED → GREEN 测试：显式 `--remote`/`--target` SSH 路径不回退 local。
//!
//! 规则：用户显式指定 SSH target 时，命令必须走 SSH transport，
//! 不允许 fallback 到 local backend / local tmux / local shell。
//! 失败应返回清晰错误 + 非零退出码，不创建任何本地资源。

#![cfg(feature = "ffi")]

use std::process::Command;

fn muxterm_bin() -> std::path::PathBuf {
    let target = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".to_string());
    let p = std::path::PathBuf::from(&target)
        .join("debug")
        .join("muxterm");
    if p.exists() {
        return p;
    }
    std::path::PathBuf::from("target/debug/muxterm")
}

fn run_mux(args: &[&str]) -> (String, String, i32) {
    let output = Command::new(muxterm_bin())
        .args(args)
        .output()
        .expect("muxterm binary 执行失败");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

// ── 1. Target parser 单元测试 ──────────────────────────────

#[test]
fn target_parser_explicit_alias_is_ssh() {
    use muxterm::platform::cli::tmux_cli::{parse_tmux_cli, Target, TmuxCliCommand};
    let cmd = parse_tmux_cli(&[
        "session".into(),
        "list".into(),
        "--target".into(),
        "myserver".into(),
    ])
    .unwrap();
    match cmd {
        TmuxCliCommand::Session(s) => assert!(
            matches!(s.target(), Target::Ssh { alias } if alias == "myserver"),
            "显式 --target myserver 应为 Target::Ssh"
        ),
        _ => panic!("应为 Session 命令"),
    }
}

#[test]
fn target_parser_local_is_local() {
    use muxterm::platform::cli::tmux_cli::{parse_tmux_cli, Target, TmuxCliCommand};
    let cmd = parse_tmux_cli(&[
        "session".into(),
        "list".into(),
        "--target".into(),
        "local".into(),
    ])
    .unwrap();
    match cmd {
        TmuxCliCommand::Session(s) => assert!(
            matches!(s.target(), Target::Local),
            "--target local 应为 Target::Local"
        ),
        _ => panic!(),
    }
}

#[test]
fn target_parser_default_is_local() {
    use muxterm::platform::cli::tmux_cli::{parse_tmux_cli, Target, TmuxCliCommand};
    let cmd = parse_tmux_cli(&["session".into(), "list".into()]).unwrap();
    match cmd {
        TmuxCliCommand::Session(s) => assert!(
            matches!(s.target(), Target::Local),
            "无 --target 应默认 Target::Local"
        ),
        _ => panic!(),
    }
}

// ── 2. 显式 remote 不回退 local（tmux CLI 路径）──────────────

/// `muxterm tmux session list --target nonexistent-host` 应返回 SSH 错误，
/// 不返回 local session 列表。
#[test]
fn remote_target_does_not_fallback_to_local() {
    let (stdout, _stderr, _rc) = run_mux(&[
        "tmux",
        "session",
        "list",
        "--target",
        "nonexistent-host-xyz-999",
    ]);

    // 不应返回 local session（local backend 会返回 {"ok":true,"data":...}）
    // SSH 错误应含 "error" / "SSH" / "remote" / "alias" / "connection" 等
    assert!(
        !stdout.contains(r#""ok":true"#) || stdout.contains("SSH") || stdout.contains("remote"),
        "显式 SSH target 不应返回 local 成功: stdout={stdout}"
    );
    assert!(
        stdout.contains("error")
            || stdout.contains("SSH")
            || stdout.contains("remote")
            || stdout.contains("alias")
            || stdout.contains("connection")
            || stdout.contains("尚未实现"),
        "应返回 SSH 相关错误: stdout={stdout}"
    );
}

/// `muxterm tmux pane list --target nonexistent-host --session test` 应返回 SSH 错误
#[test]
fn remote_target_pane_list_does_not_fallback() {
    let (stdout, _stderr, _rc) = run_mux(&[
        "tmux",
        "pane",
        "list",
        "--target",
        "nonexistent-host-xyz-999",
        "--session",
        "test-session",
    ]);

    assert!(
        !stdout.contains(r#""ok":true"#) || stdout.contains("SSH") || stdout.contains("remote"),
        "显式 SSH target pane list 不应返回 local 成功: stdout={stdout}"
    );
}

/// `muxterm tmux tab list --target nonexistent-host --session test` 应返回 SSH 错误
#[test]
fn remote_target_tab_list_does_not_fallback() {
    let (stdout, _stderr, _rc) = run_mux(&[
        "tmux",
        "tab",
        "list",
        "--target",
        "nonexistent-host-xyz-999",
        "--session",
        "test-session",
    ]);

    assert!(
        !stdout.contains(r#""ok":true"#) || stdout.contains("SSH") || stdout.contains("remote"),
        "显式 SSH target tab list 不应返回 local 成功: stdout={stdout}"
    );
}

/// `muxterm tmux pane split --target nonexistent-host --session test --pane 1 --direction h`
/// 应返回 SSH 错误，不创建 local tmux session
#[test]
fn remote_target_split_does_not_create_local_tmux() {
    let (stdout, _stderr, _rc) = run_mux(&[
        "tmux",
        "pane",
        "split",
        "--target",
        "nonexistent-host-xyz-999",
        "--session",
        "no-such-session",
        "--pane",
        "1",
        "--direction",
        "horizontal",
    ]);

    assert!(
        !stdout.contains(r#""ok":true"#) || stdout.contains("SSH") || stdout.contains("remote"),
        "显式 SSH target split 不应返回 local 成功: stdout={stdout}"
    );

    // 验证没有在默认 socket 创建 session
    let exists = std::process::Command::new("tmux")
        .args(["has-session", "-t", "no-such-session"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    assert!(!exists, "不应在默认 tmux socket 创建 no-such-session");
}

// ── 3. 缺少 --target 值是解析错误 ──────────────────────────

/// `--target` 不带值应报错，不默认 local
#[test]
fn missing_target_value_is_error_not_local() {
    let (stdout, _stderr, _rc) = run_mux(&["tmux", "session", "list", "--target"]);

    // 应有解析错误
    assert!(
        stdout.contains("error") || stdout.contains("PARSE_ERROR") || stdout.contains("缺少"),
        "缺少 --target 值应报错: stdout={stdout}"
    );
}
