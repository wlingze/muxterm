#![allow(dead_code)]
//! W18：loopback SSH + 远端隔离 tmux 夹具。断言与本地 `/bin/cat` 套件对齐。
//!
//! sshd 由测试自己拉起（随机端口），**禁止**连用户 22 / 默认 tmux。
//! 远端 tmux 一律 `-L muxterm-test-*`；Drop 先 `kill-server`（带同一 `-L`）再杀 sshd。

use std::time::Duration;

use super::sshd_test_support::{loopback_sshd_available, LoopbackSshd};
use super::tmux_test_support::tmux_available;

pub const SSH_TIMEOUT: Duration = Duration::from_secs(20);

/// 隔离 sshd + 远端 `/bin/cat` session。
pub struct RemoteCat {
    pub sshd: LoopbackSshd,
    pub socket: String,
    pub session: String,
    pub pane: u32,
    pub token: String,
}

impl Drop for RemoteCat {
    fn drop(&mut self) {
        let _ = self.sshd.remote_tmux(&self.socket, "kill-server");
    }
}

impl RemoteCat {
    pub fn pane_target(&self) -> String {
        format!("%{}", self.pane)
    }

    pub fn apply_ssh_config_env(&self) {
        self.sshd.apply_ssh_config_env();
    }

    pub fn send_keys_line(&self, text: &str) {
        let target = self.pane_target();
        let (ok, _, err) = self
            .sshd
            .remote_tmux(&self.socket, &format!("send-keys -t {target} -l {text}"));
        assert!(ok, "远端 send-keys -l 失败: {err}");
        let (ok, _, err) = self
            .sshd
            .remote_tmux(&self.socket, &format!("send-keys -t {target} Enter"));
        assert!(ok, "远端 send-keys Enter 失败: {err}");
    }

    pub fn send_bel(&self) {
        let target = self.pane_target();
        let (ok, _, err) = self
            .sshd
            .remote_tmux(&self.socket, &format!("send-keys -t {target} -H 07"));
        assert!(ok, "远端 BEL -H 07 失败: {err}");
        let (ok, _, err) = self
            .sshd
            .remote_tmux(&self.socket, &format!("send-keys -t {target} Enter"));
        assert!(ok, "远端 BEL Enter 失败: {err}");
    }

    pub fn detach_clients(&self) {
        let (ok, _, err) = self
            .sshd
            .remote_tmux(&self.socket, &format!("detach-client -s {}", self.session));
        assert!(ok, "远端 detach-client 失败: {err}");
    }

    pub fn has_session(&self) -> bool {
        self.sshd
            .remote_tmux(&self.socket, &format!("has-session -t {}", self.session))
            .0
    }

    pub fn capture(&self) -> String {
        self.sshd
            .remote_tmux(
                &self.socket,
                &format!("capture-pane -p -t {}", self.pane_target()),
            )
            .1
    }

    pub fn capture_history(&self) -> String {
        self.sshd
            .remote_tmux(
                &self.socket,
                &format!("capture-pane -p -S - -t {}", self.pane_target()),
            )
            .1
    }

    pub fn wait_capture_contains(&self, needle: &str, timeout: Duration) {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if self.capture().contains(needle) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("远端 capture-pane 未出现 {needle}。got={}", self.capture());
    }
}

pub fn ssh_tmux_available() -> bool {
    tmux_available() && loopback_sshd_available()
}

/// 远端 80×24 `/bin/cat`，attach 前已涂 token。
pub fn build_remote_one_pane(label: &str) -> RemoteCat {
    assert!(ssh_tmux_available(), "需要 tmux + 可启动的 sshd");
    let sshd = LoopbackSshd::start(label).expect("启动隔离 loopback sshd");
    let socket = format!(
        "muxterm-test-ssh-{}-{}",
        label,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(1)
    );
    let session = format!("ssh-{label}");
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(1);
    let token = format!("SSH_LIVE_{suffix}");

    let (ok, _, err) = sshd.remote_tmux(
        &socket,
        &format!("-f /dev/null new-session -d -s {session} -x 80 -y 24 -- /bin/cat"),
    );
    assert!(ok, "远端 new-session /bin/cat 失败: {err}");

    let (ok, list, err) = sshd.remote_tmux(
        &socket,
        &format!("list-panes -t {session} -F '#{{pane_id}}'"),
    );
    assert!(ok, "远端 list-panes 失败: {err}");
    let pane = list
        .lines()
        .find_map(|l| l.trim().trim_start_matches('%').parse().ok())
        .expect("远端应有 pane id");

    let fx = RemoteCat {
        sshd,
        socket,
        session,
        pane,
        token: token.clone(),
    };
    fx.send_keys_line(&token);
    fx.wait_capture_contains(&token, Duration::from_secs(5));
    fx
}

/// 远端离屏历史：可见屏无 token，`-S -` 有 token。与本地 `build_offscreen_history` 同结构。
pub fn build_remote_offscreen_history(label: &str) -> (RemoteCat, String) {
    let mut fx = build_remote_one_pane(label);
    let suffix = fx.token.trim_start_matches("SSH_LIVE_").to_string();
    let hist_token = format!("SSH_HIST_OFFSCREEN_{suffix}");
    let tail_mark = format!("SSH_HIST_TAIL_{suffix}");
    fx.token = hist_token.clone();

    let (ok, _, err) = fx.sshd.remote_tmux(
        &fx.socket,
        &format!("set-option -t {} history-limit 2000", fx.session),
    );
    assert!(ok, "远端 history-limit 失败: {err}");

    fx.send_keys_line(&hist_token);
    for i in 1..=40 {
        fx.send_keys_line(&format!("pad-{i:02}"));
    }
    fx.send_keys_line(&tail_mark);

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if fx.capture_history().contains(&hist_token) && fx.capture().contains(&tail_mark) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let visible = fx.capture();
    assert!(
        !visible.contains(&hist_token),
        "夹具失败：离屏 token 还在可见屏。visible={visible:?}"
    );
    assert!(
        visible.contains(&tail_mark),
        "夹具失败：可见屏应有尾标 {tail_mark}。visible={visible:?}"
    );
    (fx, tail_mark)
}
