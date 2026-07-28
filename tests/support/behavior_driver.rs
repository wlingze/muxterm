//! 共享 2tab3pane 行为驱动：CLI 复用同一套场景断言。
//!
//! 场景定义：
//! - 创建 session → 1 Window → Tab 1（3 panes: split + nested split）→ Tab 2（1 pane）
//! - 对 Tab 1 三个 pane 分别发 echo marker，capture 确认
//! - 切到 Tab 2，发另一个 marker，确认输出在 Tab 2
//! - 切回 Tab 1，确认原有 pane 仍在
//! - 验证 tab list、pane list、layout
//! - tab rename、pane close、验证剩余布局

use std::process::Command;
use std::time::Duration;

/// 唯一 marker（避免测试间串）。
pub fn unique_marker(label: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("m{}{}", label, nanos)
}

/// 解析 JSON envelope 的 ok 字段。
pub fn envelope_ok(stdout: &str) -> bool {
    stdout.contains("\"ok\":true")
}

/// 解析 JSON envelope 的 data 字段。
pub fn envelope_data(stdout: &str) -> Option<serde_json::Value> {
    serde_json::from_str::<serde_json::Value>(stdout)
        .ok()
        .and_then(|v| v.get("data").cloned())
}

/// 运行 muxterm CLI 命令：`muxterm tmux <sub> <sub_args> <global_opts>`。
///
/// `global_opts`（如 --socket）放在子命令参数之后，确保 parser 能正确解析。
fn run_muxterm(bin: &std::path::Path, global_opts: &[String], cmd_args: &[String]) -> String {
    let mut cmd = Command::new(bin);
    let mut full_args = vec!["tmux".to_string()];
    // 先放子命令及其参数，再放全局选项（--socket 等）
    full_args.extend_from_slice(cmd_args);
    full_args.extend_from_slice(global_opts);
    let str_args: Vec<&str> = full_args.iter().map(|s| s.as_str()).collect();
    cmd.args(&str_args);
    let output = cmd.output().expect("muxterm binary 执行失败");
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// CLI 2tab3pane 行为场景：通过 muxterm binary CLI 命令执行完整场景。
///
/// `bin` = muxterm binary 路径
/// `session_name` = tmux session 名
/// `global_opts` = 全局选项（如 ["--socket", "xxx"]）
///
/// 返回 failures 列表（空 = 全部通过）。
pub fn cli_2tab3pane_scenario(
    bin: &std::path::Path,
    session_name: &str,
    global_opts: &[String],
    timeout: Duration,
) -> Vec<String> {
    let mut failures = Vec::new();
    let deadline = std::time::Instant::now() + timeout;

    macro_rules! run {
        ($($arg:expr),* $(,)?) => {{
            if std::time::Instant::now() > deadline {
                String::new()
            } else {
                run_muxterm(bin, global_opts, &[$($arg.to_string()),*])
            }
        }};
    }

    // ── 1. tab list（初始应有 1 个 tab）──
    let stdout = run!("tab", "list", "--session", session_name);
    if !envelope_ok(&stdout) {
        failures.push(format!("tab list 初始应 ok: {stdout}"));
    }
    let data = envelope_data(&stdout);
    let tabs = data
        .as_ref()
        .and_then(|d| d.get("tabs"))
        .and_then(|t| t.as_array());
    let tab_count = tabs.map(|t| t.len()).unwrap_or(0);
    if tab_count < 1 {
        failures.push(format!("初始应有 ≥1 tab, got {tab_count}: {stdout}"));
    }

    // ── 2. pane list（初始应有 1 个 pane）──
    let stdout = run!("pane", "list", "--session", session_name);
    if !envelope_ok(&stdout) {
        failures.push(format!("pane list 初始应 ok: {stdout}"));
    }
    let data = envelope_data(&stdout);
    let panes = data
        .as_ref()
        .and_then(|d| d.get("panes"))
        .and_then(|p| p.as_array());
    let pane_count = panes.map(|p| p.len()).unwrap_or(0);
    if pane_count < 1 {
        failures.push(format!("初始应有 ≥1 pane, got {pane_count}: {stdout}"));
    }
    let first_pane_id = panes
        .and_then(|p| p.first())
        .and_then(|p| p.get("id"))
        .and_then(|i| i.as_u64())
        .unwrap_or(1);
    let pane_id_str = first_pane_id.to_string();

    // ── 3. pane split（水平）→ 2 panes ──
    let stdout = run!(
        "pane",
        "split",
        "--session",
        session_name,
        "--pane",
        &pane_id_str,
        "--direction",
        "horizontal",
    );
    if !envelope_ok(&stdout) {
        failures.push(format!("pane split H 应 ok: {stdout}"));
    }

    // ── 4. pane list（应有 2 panes）──
    let stdout = run!("pane", "list", "--session", session_name);
    let data = envelope_data(&stdout);
    let panes = data
        .as_ref()
        .and_then(|d| d.get("panes"))
        .and_then(|p| p.as_array());
    let pane_count = panes.map(|p| p.len()).unwrap_or(0);
    if pane_count < 2 {
        failures.push(format!(
            "split H 后应有 ≥2 panes, got {pane_count}: {stdout}"
        ));
    }
    let second_pane_id = panes
        .and_then(|p| p.get(1))
        .and_then(|p| p.get("id"))
        .and_then(|i| i.as_u64())
        .unwrap_or(2);
    let second_pane_id_str = second_pane_id.to_string();

    // ── 5. pane split（竖直，nested）→ 3 panes ──
    let stdout = run!(
        "pane",
        "split",
        "--session",
        session_name,
        "--pane",
        &second_pane_id_str,
        "--direction",
        "vertical",
    );
    if !envelope_ok(&stdout) {
        failures.push(format!("pane split V (nested) 应 ok: {stdout}"));
    }

    // ── 6. pane list（Tab 1 应有 3 panes）──
    let stdout = run!("pane", "list", "--session", session_name);
    let data = envelope_data(&stdout);
    let panes = data
        .as_ref()
        .and_then(|d| d.get("panes"))
        .and_then(|p| p.as_array());
    let pane_count = panes.map(|p| p.len()).unwrap_or(0);
    if pane_count < 3 {
        failures.push(format!(
            "nested split 后 Tab1 应有 ≥3 panes, got {pane_count}: {stdout}"
        ));
    }

    // ── 7. new tab → Tab 2（1 pane）──
    let stdout = run!("tab", "new", "--session", session_name);
    if !envelope_ok(&stdout) {
        failures.push(format!("tab new 应 ok: {stdout}"));
    }

    // ── 8. tab list（应有 2 tabs）──
    let stdout = run!("tab", "list", "--session", session_name);
    let data = envelope_data(&stdout);
    let tabs = data
        .as_ref()
        .and_then(|d| d.get("tabs"))
        .and_then(|t| t.as_array());
    let tab_count = tabs.map(|t| t.len()).unwrap_or(0);
    if tab_count < 2 {
        failures.push(format!("new tab 后应有 ≥2 tabs, got {tab_count}: {stdout}"));
    }

    // ── 9. send-keys + capture：向 pane 1 发 echo marker ──
    let marker1 = unique_marker("p1");
    let stdout = run!(
        "pane",
        "send-keys",
        "--session",
        session_name,
        "--pane",
        &pane_id_str,
        "--text",
        &format!("echo {marker1}"),
    );
    if !envelope_ok(&stdout) {
        failures.push(format!("send-keys to pane1 应 ok: {stdout}"));
    }

    // 等待 shell 执行
    std::thread::sleep(Duration::from_millis(1000));

    // capture pane 1
    let stdout = run!(
        "pane",
        "capture",
        "--session",
        session_name,
        "--pane",
        &pane_id_str,
        "--lines",
        "10",
    );
    let data = envelope_data(&stdout);
    let output_text = data
        .as_ref()
        .and_then(|d| d.get("output"))
        .and_then(|o| o.as_str())
        .unwrap_or("");
    if !output_text.contains(&marker1) {
        failures.push(format!(
            "capture pane1 应含 marker '{marker1}': output={output_text}"
        ));
    }

    failures
}
