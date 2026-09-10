//! Background target discovery and reconnect orchestration for the Linux UI.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk4::prelude::*;

use crate::linux::quickconnect::existing::{ExistingEntry, ExistingTransport};
use crate::linux::quickconnect::model::TargetTransport;
use crate::linux::quickconnect_panel::{ExistingPanelState, PanelItem};
use crate::ssh_probe::{classify_ssh_probe, ssh_probe_args, SshReach};

use super::{
    ClientRuntimeCapability, ExistingProbeMsg, ExistingSshProbeResult, FfiClient, UiState,
};

/// SSH 可达性缓存 TTL（W15d：面板打开时后台探测，TTL 内复用）。
const SSH_PROBE_TTL: Duration = Duration::from_secs(30);

/// 后台探测一个 SSH 别名（`ssh -o BatchMode=yes -o ConnectTimeout=2 <alias> true`）。
pub(super) fn spawn_ssh_probe(s: &mut UiState, alias: String) {
    let (tx, rx) = std::sync::mpsc::channel::<(String, SshReach)>();
    s.pending_ssh_probes.push_back(rx);
    std::thread::spawn(move || {
        let args = ssh_probe_args(&alias, 2);
        let status = std::process::Command::new("ssh")
            .args(&args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let reach = match status {
            Ok(st) => classify_ssh_probe(st.code()),
            Err(_) => SshReach::Err,
        };
        let _ = tx.send((alias, reach));
    });
}

/// 面板打开时收集 SSH 灯：TTL 内用缓存，否则 Unknown 并后台探测。
pub(super) fn collect_ssh_reach(
    s: &mut UiState,
    workspaces: &[PanelItem],
) -> HashMap<String, SshReach> {
    let now = Instant::now();
    let mut out = HashMap::new();
    let mut seen = Vec::new();
    for item in workspaces {
        if let PanelItem::Target(entry, _) = item {
            if let TargetTransport::Ssh { name } = &entry.draft.transport {
                if seen.contains(name) {
                    continue;
                }
                seen.push(name.clone());
                let fresh = s
                    .ssh_reach_cache
                    .get(name.as_str())
                    .is_some_and(|(_, at)| now.duration_since(*at) < SSH_PROBE_TTL);
                if fresh {
                    out.insert(name.clone(), s.ssh_reach_cache[name.as_str()].0);
                } else {
                    out.insert(name.clone(), SshReach::Unknown);
                    spawn_ssh_probe(s, name.clone());
                }
            }
        }
    }
    out
}

/// 收编后台 SSH 探测结果（16ms poll 与 test_poll_once 共用）。
pub(super) fn drain_ssh_probes(state: &Rc<RefCell<UiState>>) {
    let mut done = false;
    while !done {
        let result = {
            let mut s = state.borrow_mut();
            let Some(rx) = s.pending_ssh_probes.front() else {
                break;
            };
            match rx.try_recv() {
                Ok(r) => {
                    s.pending_ssh_probes.pop_front();
                    Some(r)
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    done = true;
                    None
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    s.pending_ssh_probes.pop_front();
                    None
                }
            }
        };
        if let Some((alias, reach)) = result {
            state
                .borrow_mut()
                .ssh_reach_cache
                .insert(alias, (reach, Instant::now()));
        }
    }
}

pub(super) fn existing_entries(
    candidates: Vec<crate::ffi_client::ExistingCandidate>,
) -> Vec<ExistingEntry> {
    candidates
        .into_iter()
        .filter_map(|candidate| match ExistingEntry::from_candidate(candidate) {
            Ok(entry) => Some(entry),
            Err(error) => {
                tracing::warn!(
                    target = "muxterm::linux",
                    %error,
                    "ignoring unsupported Existing candidate"
                );
                None
            }
        })
        .collect()
}

pub(super) fn merge_existing_entries(ex: &mut ExistingPanelState, entries: Vec<ExistingEntry>) {
    for e in entries {
        match &e.transport {
            ExistingTransport::Ssh { name } => {
                if !ex.hosts.contains(name) {
                    ex.hosts.push(name.clone());
                }
                let bucket = ex.remote.entry(name.clone()).or_default();
                if !bucket.contains(&e) {
                    bucket.push(e);
                }
            }
            ExistingTransport::Local => {
                if !ex.locals.contains(&e) {
                    ex.locals.push(e);
                }
            }
        }
    }
}

pub(super) fn append_unique_existing_entries(
    target: &mut Vec<ExistingEntry>,
    entries: Vec<ExistingEntry>,
) {
    for entry in entries {
        if !target.contains(&entry) {
            target.push(entry);
        }
    }
}

/// C7/C9：已有的连接探测。先 `discover_existing("local")` 立刻推表，
/// 再按 SSH host 最多 4 路并发。禁止等 `all` 串完才刷新（archmini 上 cd/mac 会冻 Loading）。
pub(super) fn spawn_local_existing_probe(s: &mut UiState) {
    s.existing.borrow_mut().probe_inflight = true;
    // Catalog 的通用 local discovery 默认只查默认 tmux server。启动配置或
    // 已打开 workspace 可能使用显式 `-L` socket；把这些已知 socket 作为只读
    // discovery 的附加输入，否则 Existing 搜索会漏掉当前用户可链接的会话。
    let local_tmux_sockets: Vec<String> = {
        let mut sockets = Vec::new();
        if let Some(socket) = s
            .default_socket
            .as_deref()
            .filter(|socket| !socket.is_empty())
        {
            sockets.push(socket.to_string());
        }
        for (id, socket) in &s.workspace_sockets {
            if id.transport != "ssh" {
                if let Some(socket) = socket.as_deref().filter(|socket| !socket.is_empty()) {
                    if !sockets.iter().any(|known| known == socket) {
                        sockets.push(socket.to_string());
                    }
                }
            }
        }
        sockets
    };
    let (tx, rx) = std::sync::mpsc::channel::<ExistingProbeMsg>();
    s.pending_local_probe.push_back(rx);
    std::thread::spawn(move || {
        tracing::debug!(target = "muxterm::linux", "existing probe: local start");
        let mut local =
            existing_entries(FfiClient::discover_existing("local", None, None).unwrap_or_default());
        for socket in local_tmux_sockets {
            let entries = FfiClient::discover_tmux_sessions("local", None, Some(&socket))
                .unwrap_or_default()
                .into_iter()
                .map(|session| {
                    ExistingEntry::tmux(
                        session.name,
                        ExistingTransport::Local,
                        Some(socket.clone()),
                    )
                })
                .collect();
            append_unique_existing_entries(&mut local, entries);
        }
        tracing::debug!(
            target = "muxterm::linux",
            n = local.len(),
            "existing probe: local done"
        );
        let _ = tx.send(ExistingProbeMsg::Rows(local));

        let aliases: Vec<String> = FfiClient::discover_ssh_hosts()
            .unwrap_or_default()
            .into_iter()
            .map(|h| h.alias)
            .collect();
        let _ = tx.send(ExistingProbeMsg::Aliases(aliases.clone()));
        tracing::debug!(
            target = "muxterm::linux",
            hosts = ?aliases,
            "existing probe: ssh hosts"
        );
        for chunk in aliases.chunks(4) {
            std::thread::scope(|scope| {
                let handles: Vec<_> = chunk
                    .iter()
                    .map(|alias| {
                        let alias = alias.clone();
                        scope.spawn(move || {
                            tracing::debug!(
                                target = "muxterm::linux",
                                alias = %alias,
                                "existing probe: ssh start"
                            );
                            let entries = existing_entries(
                                FfiClient::discover_existing("ssh", Some(&alias), None)
                                    .unwrap_or_default(),
                            );
                            tracing::debug!(
                                target = "muxterm::linux",
                                alias = %alias,
                                n = entries.len(),
                                "existing probe: ssh done"
                            );
                            entries
                        })
                    })
                    .collect();
                for handle in handles {
                    if let Ok(entries) = handle.join() {
                        if !entries.is_empty() {
                            let _ = tx.send(ExistingProbeMsg::Rows(entries));
                        }
                    }
                }
            });
        }
        let _ = tx.send(ExistingProbeMsg::Done);
    });
}

/// 收编已有连接探测结果（16ms poll 与 test_poll_once 共用）。
pub(super) fn drain_local_existing(state: &Rc<RefCell<UiState>>) {
    let mut wait = false;
    while !wait {
        let msg = {
            let mut s = state.borrow_mut();
            let Some(rx) = s.pending_local_probe.front() else {
                break;
            };
            match rx.try_recv() {
                Ok(msg @ ExistingProbeMsg::Aliases(_)) | Ok(msg @ ExistingProbeMsg::Rows(_)) => {
                    Some(msg)
                }
                Ok(ExistingProbeMsg::Done) => {
                    s.pending_local_probe.pop_front();
                    Some(ExistingProbeMsg::Done)
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    wait = true;
                    None
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    s.pending_local_probe.pop_front();
                    Some(ExistingProbeMsg::Done)
                }
            }
        };
        match msg {
            Some(ExistingProbeMsg::Aliases(aliases)) => {
                let s = state.borrow();
                let mut ex = s.existing.borrow_mut();
                ex.ssh_aliases = aliases;
                drop(ex);
                crate::linux::quickconnect_panel::refresh_current();
            }
            Some(ExistingProbeMsg::Rows(entries)) => {
                let n = entries.len();
                let s = state.borrow();
                let mut ex = s.existing.borrow_mut();
                merge_existing_entries(&mut ex, entries);
                drop(ex);
                tracing::debug!(
                    target = "muxterm::linux",
                    n,
                    "existing probe: ui rows applied"
                );
                crate::linux::quickconnect_panel::refresh_current();
            }
            Some(ExistingProbeMsg::Done) => {
                state.borrow().existing.borrow_mut().probe_inflight = false;
                tracing::debug!(target = "muxterm::linux", "existing probe: ui done");
                crate::linux::quickconnect_panel::refresh_current();
            }
            None => {}
        }
    }
}

/// W20：SSH 已有的连接探测（tmux + Herdr），后台线程，最多 4 路并发。
pub(super) fn spawn_existing_ssh_probe(state: &Rc<RefCell<UiState>>) {
    {
        let mut s = state.borrow_mut();
        if s.existing_ssh_probing {
            return;
        }
        s.existing_ssh_probing = true;
        s.existing.borrow_mut().probe_inflight = true;
    }
    let aliases: Vec<String> = FfiClient::discover_ssh_hosts()
        .unwrap_or_default()
        .into_iter()
        .map(|h| h.alias)
        .collect();
    {
        let s = state.borrow();
        s.existing.borrow_mut().ssh_aliases = aliases.clone();
    }
    let (tx, rx) = std::sync::mpsc::channel::<Vec<(String, Vec<ExistingEntry>)>>();
    state.borrow_mut().pending_existing_ssh.push_back(rx);
    std::thread::spawn(move || {
        // 最多 4 路并发：慢 host 不能把整表拖到串行 10s 级。
        let results: ExistingSshProbeResult = aliases
            .chunks(4)
            .flat_map(|chunk| {
                std::thread::scope(|scope| {
                    let handles: Vec<_> = chunk
                        .iter()
                        .map(|alias| {
                            scope.spawn(move || {
                                let entries = existing_entries(
                                    FfiClient::discover_existing("ssh", Some(alias), None)
                                        .unwrap_or_default(),
                                );
                                (alias.clone(), entries)
                            })
                        })
                        .collect();
                    handles
                        .into_iter()
                        .map(|h| h.join().unwrap_or_default())
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let _ = tx.send(results);
    });
}

/// 收编 SSH 已有连接探测结果（16ms poll 与 test_poll_once 共用）。
pub(super) fn drain_existing_ssh(state: &Rc<RefCell<UiState>>) {
    let mut done = false;
    while !done {
        let result = {
            let mut s = state.borrow_mut();
            let Some(rx) = s.pending_existing_ssh.front() else {
                break;
            };
            match rx.try_recv() {
                Ok(r) => {
                    s.pending_existing_ssh.pop_front();
                    s.existing_ssh_probing = false;
                    s.existing.borrow_mut().probe_inflight = false;
                    Some(r)
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    done = true;
                    None
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    s.pending_existing_ssh.pop_front();
                    s.existing_ssh_probing = false;
                    s.existing.borrow_mut().probe_inflight = false;
                    None
                }
            }
        };
        if let Some(results) = result {
            let mut hosts = Vec::new();
            let mut remote = std::collections::HashMap::new();
            for (alias, entries) in results {
                if !entries.is_empty() {
                    hosts.push(alias.clone());
                    remote.insert(alias, entries);
                }
            }
            {
                let s = state.borrow();
                let mut ex = s.existing.borrow_mut();
                ex.hosts = hosts;
                ex.remote = remote;
            }
            crate::linux::quickconnect_panel::refresh_current();
        }
    }
}

/// W17a：tmux 控制 client 掉线后自动重连。
pub(super) fn maybe_schedule_reconnect(state: &Rc<RefCell<UiState>>) {
    let should_retry = {
        let mut s = state.borrow_mut();
        if s.reconnecting
            || s.runtime_status == muxterm_core::protocol::ffi::types::BACKEND_STATUS_CONNECTED
        {
            return;
        }
        let now = Instant::now();
        if s.reconnect_retry_at.is_some_and(|at| now < at) {
            return;
        }
        let id = s.active_ws_id();
        if !s.workspace_supports(&id.as_str(), ClientRuntimeCapability::SharedClientResize) {
            return;
        }
        s.reconnecting = true;
        true
    };
    if !should_retry {
        return;
    }
    let result = state.borrow().event_pump.client().reconnect();
    let mut s = state.borrow_mut();
    s.reconnecting = false;
    match result {
        Ok(()) => {
            s.reconnect_attempts = 0;
            s.reconnect_retry_at = None;
            s.overlay.disconnect.set_visible(false);
            drop(s);
            handle_reconnect_success(state);
        }
        Err(error) => {
            s.reconnect_attempts = s.reconnect_attempts.saturating_add(1);
            let delay = Duration::from_secs(1u64 << s.reconnect_attempts.min(3));
            s.reconnect_retry_at = Some(Instant::now() + delay);
            tracing::warn!(
                target = "muxterm::linux",
                "reconnect failed (attempt {}): {error}; retry in {delay:?}",
                s.reconnect_attempts
            );
        }
    }
}

/// 重连成功：换 Runtime、隐藏水印；断线期间的 BEL 重新推导成 Blocked。
pub(super) fn handle_reconnect_success(state: &Rc<RefCell<UiState>>) {
    let mut s = state.borrow_mut();
    s.reconnect_attempts = 0;
    s.reconnect_retry_at = None;
    s.overlay.disconnect.set_visible(false);
}
