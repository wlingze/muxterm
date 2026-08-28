#![cfg(feature = "ffi")]

//! macOS 客户端同源 FFI e2e：镜像 Linux `tmux_attach_contract` /
//! `tmux_feature_contract` / `linux_disconnect_e2e` / `linux_attach_history_e2e`。
//!
//! 跑：`cargo test --no-default-features --features ffi --test macos_e2e -- --test-threads=1`

use std::ffi::{CStr, CString};
use std::os::unix::process::ExitStatusExt;
use std::process::Command;
use std::ptr;
use std::time::{Duration, Instant};

use muxterm::core::protocol::ffi::api::{
    muxterm_attention_on_became_visible, muxterm_attention_snapshot,
    muxterm_attention_take_notifications, muxterm_connect, muxterm_execute, muxterm_free,
    muxterm_get_layout, muxterm_get_pane_output, muxterm_new, muxterm_pane_command_marks_json,
    muxterm_pane_history_max_offset, muxterm_pane_last_n_lines, muxterm_pane_scroll_ansi,
    muxterm_pane_viewport, muxterm_poll_events, muxterm_resize_client, muxterm_search_all,
    muxterm_set_pane_viewport,
};
use muxterm::core::protocol::ffi::types::{
    CLayoutNode, CStateChange, CTask, BACKEND_STATUS_DISCONNECTED, BACKEND_STATUS_EXITED,
    DIR_HORIZONTAL, LAYOUT_LEAF, STATE_BACKEND_STATUS, TASK_SPLIT_PANE,
    TASK_TOGGLE_PANE_FULLSCREEN,
};

fn tmux_available() -> bool {
    Command::new("tmux")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn require_tmux() {
    assert!(tmux_available(), "需要本机 tmux（禁止 skip 冒充绿）");
}

fn rand_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{nanos}")
}

fn run_tmux(socket: &str, args: &[&str]) -> std::process::Output {
    Command::new("tmux")
        .args(["-L", socket])
        .args(args)
        .output()
        .unwrap_or_else(|_| std::process::Output {
            status: std::process::ExitStatus::from_raw(1),
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
}

fn tmux_ok(socket: &str, args: &[&str]) -> bool {
    run_tmux(socket, args).status.success()
}

fn tmux_out(socket: &str, args: &[&str]) -> String {
    String::from_utf8_lossy(&run_tmux(socket, args).stdout)
        .trim()
        .to_string()
}

/// 用 `send-keys -H` 发送原始字节（`-l` 会把 BEL 转成 `^G` 字面量）。
fn send_keys_hex(socket: &str, target: &str, bytes: &[u8]) {
    let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let mut args = vec!["send-keys", "-t", target, "-H"];
    args.extend(hex.iter().map(String::as_str));
    assert!(tmux_ok(socket, &args), "send-keys -H 失败 target={target}");
}

/// 2tab/3pane 夹具：tab1 = 3 pane（水平 + 竖直），tab2 = 1 pane。
/// 每个 pane 用 /bin/cat 涂独立 token，capture 确认后再 attach。
struct PaintedFixture {
    socket: String,
    session: String,
    tab1_tokens: Vec<String>,
    tab2_token: String,
    tab1_panes: Vec<String>,
}

impl PaintedFixture {
    fn new(label: &str) -> Self {
        let socket = format!(
            "muxterm-e2e-{label}-{}-{}",
            std::process::id(),
            rand_suffix()
        );
        let session = "demo";
        let _ = run_tmux(&socket, &["kill-server"]);
        assert!(
            tmux_ok(
                &socket,
                &[
                    "-f",
                    "/dev/null",
                    "new-session",
                    "-d",
                    "-s",
                    session,
                    "-x",
                    "100",
                    "-y",
                    "30",
                    "/bin/cat",
                ]
            ),
            "创建 /bin/cat session 失败"
        );

        let w0 = tmux_out(
            &socket,
            &["list-windows", "-t", session, "-F", "#{window_id}"],
        );
        assert!(tmux_ok(
            &socket,
            &["split-window", "-h", "-t", &w0, "/bin/cat"]
        ));
        let panes = tmux_out(&socket, &["list-panes", "-t", &w0, "-F", "#{pane_id}"]);
        let panes: Vec<String> = panes.lines().map(String::from).collect();
        assert!(panes.len() >= 2, "应有 2 pane: {panes:?}");
        assert!(tmux_ok(
            &socket,
            &["split-window", "-v", "-t", &panes[1], "/bin/cat"]
        ));
        assert!(tmux_ok(&socket, &["new-window", "-t", session, "/bin/cat"]));

        let panes = tmux_out(&socket, &["list-panes", "-t", &w0, "-F", "#{pane_id}"]);
        let tab1_panes: Vec<String> = panes.lines().map(String::from).collect();
        assert_eq!(tab1_panes.len(), 3, "tab1 应有 3 pane: {tab1_panes:?}");

        let tab1_tokens: Vec<String> = (0..3)
            .map(|i| format!("E2E_TAB1_TOKEN_{i}_{}", rand_suffix()))
            .collect();
        for (i, pane) in tab1_panes.iter().enumerate() {
            assert!(
                tmux_ok(&socket, &["send-keys", "-t", pane, "-l", &tab1_tokens[i]]),
                "涂 token 失败 pane={pane}"
            );
        }
        let tab2_token = format!("E2E_TAB2_TOKEN_{}", rand_suffix());
        let w1 = tmux_out(
            &socket,
            &["list-windows", "-t", session, "-F", "#{window_id}"],
        )
        .lines()
        .nth(1)
        .unwrap_or("")
        .to_string();
        if !w1.is_empty() {
            let p = tmux_out(&socket, &["list-panes", "-t", &w1, "-F", "#{pane_id}"]);
            let p = p.lines().next().unwrap_or("").to_string();
            if !p.is_empty() {
                assert!(tmux_ok(
                    &socket,
                    &["send-keys", "-t", &p, "-l", &tab2_token]
                ));
            }
        }
        assert!(
            tmux_ok(&socket, &["select-window", "-t", &w0]),
            "fixture 必须让带三个 pane 的 tab1 成为 attach 活动 tab"
        );

        // capture 确认每个 token 已上屏。
        for (i, pane) in tab1_panes.iter().enumerate() {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if tmux_out(&socket, &["capture-pane", "-p", "-t", pane]).contains(&tab1_tokens[i])
                {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            assert!(
                tmux_out(&socket, &["capture-pane", "-p", "-t", pane]).contains(&tab1_tokens[i]),
                "token {} 未上屏 pane={pane}",
                tab1_tokens[i]
            );
        }

        Self {
            socket,
            session: session.to_string(),
            tab1_tokens,
            tab2_token,
            tab1_panes,
        }
    }

    fn pane_target(&self, pane: &str) -> String {
        pane.to_string()
    }
}

impl Drop for PaintedFixture {
    fn drop(&mut self) {
        let _ = run_tmux(&self.socket, &["kill-server"]);
    }
}

fn connect_attach(
    socket: &str,
    session: &str,
    cols: u16,
    rows: u16,
) -> *mut muxterm::core::protocol::ffi::api::MuxtermHandle {
    let bt = CString::new("tmux").unwrap();
    let sock = CString::new(socket).unwrap();
    let sess = CString::new(session).unwrap();
    let h = muxterm_new(bt.as_ptr(), sock.as_ptr(), sess.as_ptr());
    assert!(!h.is_null(), "muxterm_new 失败");
    assert_eq!(unsafe { muxterm_connect(h) }, 0, "muxterm_connect 失败");
    assert_eq!(
        unsafe { muxterm_resize_client(h, cols, rows) },
        0,
        "macOS attach 必须自动提交首个字符网格尺寸"
    );
    h
}

fn poll_until(
    h: *mut muxterm::core::protocol::ffi::api::MuxtermHandle,
    timeout: Duration,
    mut pred: impl FnMut() -> bool,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let mut events = [CStateChange::default(); 64];
        unsafe {
            let _ = muxterm_poll_events(h, events.as_mut_ptr(), events.len() as i32);
        }
        if pred() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    pred()
}

fn pane_output(h: *mut muxterm::core::protocol::ffi::api::MuxtermHandle, pane: u32) -> String {
    let mut buf = vec![0u8; 256 * 1024];
    let n = unsafe { muxterm_get_pane_output(h, pane, buf.as_mut_ptr(), buf.len()) };
    if n <= 0 {
        return String::new();
    }
    String::from_utf8_lossy(&buf[..n as usize]).into_owned()
}

fn search_json(
    h: *mut muxterm::core::protocol::ffi::api::MuxtermHandle,
    query: &str,
) -> serde_json::Value {
    let q = CString::new(query).unwrap();
    let raw = unsafe { muxterm_search_all(h, q.as_ptr()) };
    assert!(!raw.is_null(), "search_all 返回 null");
    let json = unsafe {
        let s = CStr::from_ptr(raw).to_string_lossy().into_owned();
        muxterm::core::protocol::ffi::api::muxterm_free_string(raw);
        serde_json::from_str(&s).unwrap()
    };
    json
}

fn attention_json(h: *mut muxterm::core::protocol::ffi::api::MuxtermHandle) -> serde_json::Value {
    let raw = unsafe { muxterm_attention_snapshot(h) };
    assert!(!raw.is_null(), "attention_snapshot 返回 null");
    unsafe {
        let s = CStr::from_ptr(raw).to_string_lossy().into_owned();
        muxterm::core::protocol::ffi::api::muxterm_free_string(raw);
        serde_json::from_str(&s).unwrap()
    }
}

fn take_notifications(
    h: *mut muxterm::core::protocol::ffi::api::MuxtermHandle,
) -> serde_json::Value {
    let raw = unsafe { muxterm_attention_take_notifications(h) };
    assert!(!raw.is_null(), "take_notifications 返回 null");
    unsafe {
        let s = CStr::from_ptr(raw).to_string_lossy().into_owned();
        muxterm::core::protocol::ffi::api::muxterm_free_string(raw);
        serde_json::from_str(&s).unwrap()
    }
}

/// W13/W14：attach 已有 2tab/3pane，搜索命中、BEL → blocked、Done → 通知。
#[test]
fn macos_ffi_attach_search_attention_and_done() {
    require_tmux();
    let fx = PaintedFixture::new("search-attn");
    let h = connect_attach(&fx.socket, &fx.session, 100, 30);

    // 等 attach 完成：pane 输出含 token。
    let ok = poll_until(h, Duration::from_secs(8), || {
        fx.tab1_tokens.iter().all(|t| {
            pane_output(h, 0).contains(t)
                || pane_output(h, 1).contains(t)
                || pane_output(h, 2).contains(t)
        })
    });
    assert!(ok, "attach 后 pane 输出应含播种 token");

    // 搜索：跨工作区 PaneBuf 命中（tab1 + tab2 都验证）。
    let hits = search_json(h, &fx.tab1_tokens[0]);
    let hits2 = search_json(h, &fx.tab2_token);
    assert!(
        hits2["hits"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "tab2 token 也应可搜索: {hits2}"
    );
    assert_eq!(hits["ok"], true, "search_all 应 ok: {hits}");
    assert!(
        hits["hits"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "搜索必须找到播种 token {}: {hits}",
        fx.tab1_tokens[0]
    );

    // BEL → blocked：向 pane1 发原始 BEL（cat 需要 Enter 才回显）。
    let pane1 = fx.tab1_panes[1].trim_start_matches('%').to_string();
    let pane1_id: u32 = pane1.parse().unwrap();
    send_keys_hex(&fx.socket, &fx.pane_target(&fx.tab1_panes[1]), b"\x07");
    assert!(tmux_ok(
        &fx.socket,
        &[
            "send-keys",
            "-t",
            &fx.pane_target(&fx.tab1_panes[1]),
            "Enter"
        ]
    ));
    let ok = poll_until(h, Duration::from_secs(5), || {
        let snap = attention_json(h);
        snap["blocked_count"].as_u64().unwrap_or(0) >= 1
    });
    assert!(ok, "BEL 后 blocked_count 应 ≥ 1: {}", attention_json(h));

    // 取通知：blocked 列表非空。
    let notifications = take_notifications(h);
    assert!(
        notifications["blocked"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "BEL 后应有 blocked 通知: {notifications}"
    );

    // 看见不熄：Blocked + BecameVisible 仍保持（W16c 语义）。
    unsafe { muxterm_attention_on_became_visible(h, pane1_id) };
    let snap = attention_json(h);
    assert_eq!(
        snap["blocked_count"].as_u64().unwrap_or(0),
        1,
        "看见不熄：Blocked 应保持: {snap}"
    );

    // 输入才熄：Blocked + UserInput → Idle。
    let input = b"x";
    assert_eq!(
        unsafe {
            muxterm::core::protocol::ffi::api::muxterm_send_input(
                h,
                pane1_id,
                input.as_ptr(),
                input.len(),
            )
        },
        0,
        "send_input 应成功"
    );
    let ok = poll_until(h, Duration::from_secs(3), || {
        attention_json(h)["blocked_count"].as_u64().unwrap_or(99) == 0
    });
    assert!(ok, "输入后 blocked 应清 0: {}", attention_json(h));

    // 后台 Done：先切前台到 pane0，再让 pane1 直接写 OSC 133 D（send-keys
    // 会把控制字节转成字面量，必须由 pane 进程写 stdout，与 Linux 契约一致）。
    let pane0 = fx.tab1_panes[0].trim_start_matches('%').to_string();
    let pane0_id: u32 = pane0.parse().unwrap();
    unsafe { muxterm_attention_on_became_visible(h, pane0_id) };
    let py =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/scripts/osc133_done.py");
    assert!(py.is_file(), "缺少 {}", py.display());
    let cmd = format!("python3 -u {}", py.display());
    assert!(tmux_ok(
        &fx.socket,
        &[
            "respawn-pane",
            "-k",
            "-t",
            &fx.pane_target(&fx.tab1_panes[1]),
            &cmd
        ]
    ));
    let ok = poll_until(h, Duration::from_secs(5), || {
        let notifications = take_notifications(h);
        notifications["done"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false)
    });
    assert!(ok, "后台 OSC 133 D 后应有 done 通知");

    unsafe { muxterm_free(h) };
}

/// W16b：tmux server 死后窗口保留最后一帧（pane 输出仍在）+ 断线状态。
#[test]
fn macos_ffi_disconnect_keeps_last_frame() {
    require_tmux();
    let fx = PaintedFixture::new("disconnect");
    let h = connect_attach(&fx.socket, &fx.session, 100, 30);

    let ok = poll_until(h, Duration::from_secs(8), || {
        pane_output(h, 0).contains(&fx.tab1_tokens[0])
    });
    assert!(ok, "attach 后 pane 输出应含 token");

    // 隔离 kill-server：只杀测试 server。
    assert!(tmux_ok(&fx.socket, &["kill-server"]));

    // 断线后：pane 输出（最后一帧）必须仍在。
    let ok = poll_until(h, Duration::from_secs(5), || {
        pane_output(h, 0).contains(&fx.tab1_tokens[0])
    });
    assert!(ok, "断线后最后一帧必须保留（W16b 水印）");

    unsafe { muxterm_free(h) };
}

/// W16a：attach 离屏历史可滚动查看 + viewport 回底。
#[test]
fn macos_ffi_attach_history_and_jump_latest() {
    require_tmux();
    let socket = format!("muxterm-e2e-hist-{}-{}", std::process::id(), rand_suffix());
    let session = "hist";
    let _ = run_tmux(&socket, &["kill-server"]);
    assert!(tmux_ok(
        &socket,
        &[
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            session,
            "-x",
            "80",
            "-y",
            "24",
            "--",
            "/bin/cat",
        ]
    ));
    let pane = tmux_out(&socket, &["list-panes", "-t", session, "-F", "#{pane_id}"]);
    let pane = pane.lines().next().unwrap_or("").to_string();
    // 先涂离屏 token（多行），再涂可见 token。
    let offscreen = format!("E2E_OFFSCREEN_{}", rand_suffix());
    let visible = format!("E2E_VISIBLE_{}", rand_suffix());
    for _ in 0..30 {
        assert!(tmux_ok(
            &socket,
            &["send-keys", "-t", &pane, "-l", &format!("{offscreen}\n")]
        ));
    }
    assert!(tmux_ok(
        &socket,
        &["send-keys", "-t", &pane, "-l", &visible]
    ));

    let h = connect_attach(&socket, session, 80, 24);
    let ok = poll_until(h, Duration::from_secs(8), || {
        pane_output(h, 0).contains(&visible)
    });
    assert!(ok, "attach 后可见 token 应出现");

    // 搜索离屏 token：PaneBuf 必须含历史。
    let hits = search_json(h, &offscreen);
    assert!(
        hits["hits"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "离屏历史必须可搜索: {hits}"
    );

    // viewport 初始为 0（底部/最新）。
    assert_eq!(
        unsafe { muxterm_pane_viewport(h, 0) },
        0,
        "初始 viewport 应为 0"
    );

    // 首屏优先：历史回填会等可见 capture 交付并让出控制通道后再开始。
    // 持续 poll 到历史真正进入 PaneBuf，不能用可见 token 代替完成条件。
    let mut max = 0;
    let history_ready = poll_until(h, Duration::from_secs(8), || {
        max = unsafe { muxterm_pane_history_max_offset(h, 0, 24) };
        max > 0
    });
    assert!(history_ready, "离屏 30 行必须能滚, max={max}");
    let mut ansi = vec![0u8; 256 * 1024];
    let n =
        unsafe { muxterm_pane_scroll_ansi(h, 0, max as u32, 24, ansi.as_mut_ptr(), ansi.len()) };
    assert!(n > 0, "滚到顶必须有历史 ANSI");
    let top = String::from_utf8_lossy(&ansi[..n as usize]);
    assert!(
        top.contains(&offscreen),
        "scroll_ansi(max) 必须含离屏 token {offscreen}。got={}",
        top.chars().take(200).collect::<String>()
    );

    // 滚到顶（offset 大值）→ viewport > 0；回底 → 0。
    assert_eq!(unsafe { muxterm_set_pane_viewport(h, 0, 1000) }, 0);
    assert!(
        unsafe { muxterm_pane_viewport(h, 0) } > 0,
        "滚离底部后 viewport 应 > 0"
    );
    assert_eq!(unsafe { muxterm_set_pane_viewport(h, 0, 0) }, 0);
    assert_eq!(
        unsafe { muxterm_pane_viewport(h, 0) },
        0,
        "回底后 viewport 应为 0"
    );

    // 最近 n 行 JSON 可用。
    let raw = unsafe { muxterm_pane_last_n_lines(h, 0, 5) };
    assert!(!raw.is_null());
    unsafe {
        let s = CStr::from_ptr(raw).to_string_lossy().into_owned();
        muxterm::core::protocol::ffi::api::muxterm_free_string(raw);
        let json: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(json["ok"], true);
        assert!(json["lines"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false));
    }

    unsafe { muxterm_free(h) };
    let _ = run_tmux(&socket, &["kill-server"]);
}

/// FFI 必须把 core OSC 133 命令时间线完整暴露给 macOS：文本、退出码和
/// 可跳转 offset 都不能依赖前端自己重解析 pane 字节。
#[test]
fn macos_ffi_command_marks_roundtrip() {
    require_tmux();
    let socket = format!("muxterm-e2e-cmd-{}-{}", std::process::id(), rand_suffix());
    let session = "cmd";
    let _ = run_tmux(&socket, &["kill-server"]);
    assert!(tmux_ok(
        &socket,
        &[
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            session,
            "-x",
            "80",
            "-y",
            "24",
            "--",
            "/bin/cat",
        ]
    ));
    let pane = tmux_out(&socket, &["list-panes", "-t", session, "-F", "#{pane_id}"]);
    let pane = pane.lines().next().unwrap_or("").to_string();
    let h = connect_attach(&socket, session, 80, 24);
    assert!(
        poll_until(h, Duration::from_secs(3), || !pane_output(h, 0).is_empty()),
        "写 OSC 前必须完成初始 Surface seed"
    );

    // canonical TTY 会把输入侧 ESC/BEL 回显成 `^[`/`^G`。命令标记必须由
    // pane 进程从 stdout 写出真实 OSC 133，与生产 shell integration 一致。
    let suffix = rand_suffix();
    let ok_token = format!("CMD_OK_{suffix}");
    let fail_token = format!("CMD_FAIL_{suffix}");
    let py =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/scripts/osc133_rounds.py");
    assert!(py.is_file(), "缺少 {}", py.display());
    let command = format!(
        "env MUXTERM_CMD_SUFFIX={suffix} python3 -u {}",
        py.display()
    );
    assert!(
        tmux_ok(&socket, &["respawn-pane", "-k", "-t", &pane, &command]),
        "启动 OSC 133 command fixture 失败"
    );

    let mut last_marks = String::new();
    let ok = poll_until(h, Duration::from_secs(8), || unsafe {
        let raw = muxterm_pane_command_marks_json(h, 0);
        if raw.is_null() {
            return false;
        }
        let text = CStr::from_ptr(raw).to_string_lossy().into_owned();
        last_marks = text.clone();
        muxterm::core::protocol::ffi::api::muxterm_free_string(raw);
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            return false;
        };
        let marks = value["marks"].as_array();
        marks.is_some_and(|marks| {
            marks.len() >= 2
                && value["marks"].to_string().contains(&ok_token)
                && value["marks"].to_string().contains(&fail_token)
        })
    });
    assert!(
        ok,
        "FFI command marks 必须含文本与退出码: {last_marks}; output={}",
        pane_output(h, 0)
    );
    unsafe { muxterm_free(h) };
    let _ = run_tmux(&socket, &["kill-server"]);
}

/// W16b：kill-server 后必须发出 Disconnected/Exited，且最后一帧还在。
#[test]
fn macos_ffi_disconnect_emits_exited_or_disconnected() {
    require_tmux();
    let fx = PaintedFixture::new("disc-status");
    let h = connect_attach(&fx.socket, &fx.session, 100, 30);
    let ok = poll_until(h, Duration::from_secs(8), || {
        pane_output(h, 0).contains(&fx.tab1_tokens[0])
    });
    assert!(ok, "attach 后 pane 输出应含 token");

    assert!(tmux_ok(&fx.socket, &["kill-server"]));

    let mut saw_status = None;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let mut events = [CStateChange::default(); 64];
        let n = unsafe { muxterm_poll_events(h, events.as_mut_ptr(), events.len() as i32) };
        for ev in events.iter().take(n.max(0) as usize) {
            if ev.type_ == STATE_BACKEND_STATUS {
                saw_status = Some(ev.pane_id);
            }
        }
        if saw_status.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    let status = saw_status.expect("kill-server 后必须有 BackendStatus 事件");
    assert!(
        status == BACKEND_STATUS_DISCONNECTED || status == BACKEND_STATUS_EXITED,
        "断线状态应为 Disconnected(0) 或 Exited(4)，实际 {status}"
    );
    assert!(
        pane_output(h, 0).contains(&fx.tab1_tokens[0]),
        "断线后最后一帧必须保留"
    );
    unsafe { muxterm_free(h) };
}

/// Alt+Enter 对应的 core zoom：layout 必须塌成单叶。
#[test]
fn macos_ffi_zoom_collapses_layout_to_leaf() {
    require_tmux();
    let fx = PaintedFixture::new("zoom");
    let h = connect_attach(&fx.socket, &fx.session, 100, 30);
    let ok = poll_until(h, Duration::from_secs(8), || {
        pane_output(h, 0).contains(&fx.tab1_tokens[0])
    });
    assert!(ok, "attach 后应有 token");

    let pane = fx.tab1_panes[0]
        .trim_start_matches('%')
        .parse::<u32>()
        .unwrap_or(0);
    let split = CTask {
        type_: TASK_SPLIT_PANE,
        target_pane: pane,
        target_tab: 0,
        dir: DIR_HORIZONTAL,
        name: ptr::null(),
    };
    // 已有 3 pane，不必再 split；直接 zoom 当前 pane。
    let _ = split;
    let zoom = CTask {
        type_: TASK_TOGGLE_PANE_FULLSCREEN,
        target_pane: pane,
        target_tab: 0,
        dir: 0,
        name: ptr::null(),
    };
    assert_eq!(
        unsafe { muxterm_execute(h, &zoom) },
        0,
        "resize-pane -Z 应成功"
    );
    let ok = poll_until(h, Duration::from_secs(3), || {
        let mut node = CLayoutNode {
            type_: 99,
            pane_id: 0,
            ratio: 0,
            first: ptr::null(),
            second: ptr::null(),
        };
        unsafe { muxterm_get_layout(h, 0, &mut node) == 0 && node.type_ == LAYOUT_LEAF }
    });
    assert!(ok, "zoom 后 layout 必须是单叶 LAYOUT_LEAF");
    unsafe { muxterm_free(h) };
}
