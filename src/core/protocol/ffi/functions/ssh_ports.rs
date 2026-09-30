//! SSH application port discovery and explicit local forwarding.

use std::ffi::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};

use super::support::{cstr_opt, json_error, json_string, parse_workspace_id, MuxtermHandle};

/// Return application ports detected from this SSH workspace's pane output.
///
/// # Safety
/// `handle`, when non-null, must point to a live Muxterm handle. `workspace_id`,
/// when non-null, must point to a NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_ssh_ports_json(
    handle: *mut MuxtermHandle,
    workspace_id: *const c_char,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return json_error("handle is null");
        }
        let Some(workspace_id) = cstr_opt(workspace_id) else {
            return json_error("workspace id is empty");
        };
        let workspace_id = parse_workspace_id(&workspace_id);
        if let Err(error) = (*handle).refresh_ssh_ports(&workspace_id) {
            return json_error(error);
        }
        match (*handle).ssh_port_snapshot(&workspace_id) {
            Ok(listing) => json_string(serde_json::json!({
                "ok": true,
                "ports": listing.ports,
                "scan_pending": listing.scan_pending,
                "scan_error": listing.scan_error,
                "remote_host": listing.remote_host,
                "remote_host_error": listing.remote_host_error,
            })),
            Err(error) => json_error(error),
        }
    }))
    .unwrap_or_else(|_| json_error("SSH port snapshot panic"))
}

/// Start an explicit local loopback forward. OpenSSH startup is asynchronous.
///
/// # Safety
/// `handle`, when non-null, must point to a live Muxterm handle. `workspace_id`,
/// when non-null, must point to a NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_ssh_port_forward(
    handle: *mut MuxtermHandle,
    workspace_id: *const c_char,
    remote_port: u32,
) -> *mut c_char {
    ssh_port_forward_json(handle, workspace_id, remote_port, false)
}

/// Start an SSH port forward with optional LAN access (0.0.0.0 bind).
///
/// # Safety
/// `handle`, when non-null, must point to a live Muxterm handle. `workspace_id`,
/// when non-null, must point to a NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_ssh_port_forward_with_access(
    handle: *mut MuxtermHandle,
    workspace_id: *const c_char,
    remote_port: u32,
    allow_lan: bool,
) -> *mut c_char {
    ssh_port_forward_json(handle, workspace_id, remote_port, allow_lan)
}

unsafe fn ssh_port_forward_json(
    handle: *mut MuxtermHandle,
    workspace_id: *const c_char,
    remote_port: u32,
    allow_lan: bool,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return json_error("handle is null");
        }
        let Some(workspace_id) = cstr_opt(workspace_id) else {
            return json_error("workspace id is empty");
        };
        let Ok(remote_port) = u16::try_from(remote_port) else {
            return json_error("remote port is outside the valid range");
        };
        let workspace_id = parse_workspace_id(&workspace_id);
        match (*handle).forward_ssh_port(&workspace_id, remote_port, allow_lan) {
            Ok(()) => json_string(serde_json::json!({ "ok": true, "pending": true })),
            Err(error) => json_error(error),
        }
    }))
    .unwrap_or_else(|_| json_error("SSH port forward panic"))
}

/// Ignore a discovered port for the lifetime of this workspace.
///
/// # Safety
/// `handle`, when non-null, must point to a live Muxterm handle. `workspace_id`,
/// when non-null, must point to a NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_ssh_port_ignore(
    handle: *mut MuxtermHandle,
    workspace_id: *const c_char,
    remote_port: u32,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return -1;
        }
        let Some(workspace_id) = cstr_opt(workspace_id) else {
            return -1;
        };
        let Ok(remote_port) = u16::try_from(remote_port) else {
            return -1;
        };
        let workspace_id = parse_workspace_id(&workspace_id);
        i32::from((*handle).ignore_ssh_port(&workspace_id, remote_port))
    }))
    .unwrap_or(-1)
}

/// Stop an active local port forward.
///
/// # Safety
/// `handle`, when non-null, must point to a live Muxterm handle. `workspace_id`,
/// when non-null, must point to a NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_ssh_port_stop(
    handle: *mut MuxtermHandle,
    workspace_id: *const c_char,
    remote_port: u32,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return -1;
        }
        let Some(workspace_id) = cstr_opt(workspace_id) else {
            return -1;
        };
        let Ok(remote_port) = u16::try_from(remote_port) else {
            return -1;
        };
        let workspace_id = parse_workspace_id(&workspace_id);
        i32::from((*handle).stop_ssh_port_forward(&workspace_id, remote_port))
    }))
    .unwrap_or(-1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ffi::{muxterm_catalog_new, muxterm_free, muxterm_free_string};
    use crate::protocol::WorkspaceId;
    use crate::runtime::MockRuntime;
    use crate::transport::{
        ByteChannel, ChannelRequest, TargetConnection, TransportError, TransportResult,
    };
    use crate::workspace::Workspace;
    use std::ffi::{CStr, CString};
    use std::sync::{mpsc, Arc, Mutex};

    struct ScanConnection {
        alias: String,
        host: Result<String, String>,
        ports: Mutex<mpsc::Receiver<Result<Vec<u16>, String>>>,
    }

    impl TargetConnection for ScanConnection {
        fn transport_id(&self) -> &str {
            "ssh"
        }
        fn target(&self) -> &str {
            &self.alias
        }
        fn open_channel(&self, _: ChannelRequest) -> TransportResult<Box<dyn ByteChannel>> {
            Err(TransportError::message("scan fixture has no byte channels"))
        }
        fn probe(&self) -> TransportResult<()> {
            Ok(())
        }
        fn tcp_browser_host(&self) -> TransportResult<String> {
            self.host.clone().map_err(TransportError::message)
        }
        fn list_tcp_listener_ports(&self) -> TransportResult<Vec<u16>> {
            self.ports
                .lock()
                .unwrap()
                .recv()
                .unwrap_or_else(|_| Err("scan fixture closed".into()))
                .map_err(TransportError::message)
        }
    }

    struct Fixture {
        handle: *mut MuxtermHandle,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                handle: muxterm_catalog_new(),
            }
        }

        fn add(
            &mut self,
            alias: &str,
            host: Result<&str, &str>,
        ) -> (WorkspaceId, mpsc::Sender<Result<Vec<u16>, String>>) {
            let (sender, receiver) = mpsc::channel();
            let connection = Arc::new(ScanConnection {
                alias: alias.into(),
                host: host.map(str::to_string).map_err(str::to_string),
                ports: Mutex::new(receiver),
            });
            let id = WorkspaceId::new("ssh", Some(alias), "ports", "mock", "/tmp");
            unsafe {
                (*self.handle)
                    .connections
                    .acquire("ssh", alias, || Ok(connection))
                    .unwrap();
                (*self.handle).pool.insert_connected(Workspace::new(
                    id.clone(),
                    alias.into(),
                    Box::new(MockRuntime::new()),
                ));
            }
            (id, sender)
        }

        fn listing(&self, id: &WorkspaceId) -> serde_json::Value {
            let id = CString::new(id.to_string()).unwrap();
            unsafe {
                let result = muxterm_workspace_ssh_ports_json(self.handle, id.as_ptr());
                assert!(!result.is_null());
                let value = serde_json::from_slice(CStr::from_ptr(result).to_bytes()).unwrap();
                muxterm_free_string(result);
                value
            }
        }

        fn wait_listing(
            &self,
            id: &WorkspaceId,
            predicate: impl Fn(&serde_json::Value) -> bool,
        ) -> serde_json::Value {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            loop {
                let value = self.listing(id);
                if predicate(&value) {
                    return value;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "SSH listing did not settle: {value}"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            unsafe { muxterm_free(self.handle) }
        }
    }

    #[test]
    fn ssh_port_browser_address_arrives_before_a_slow_listener_scan() {
        let mut fixture = Fixture::new();
        let (id, ports) = fixture.add("port-test-a", Ok("192.0.2.42"));
        let listing = fixture.wait_listing(&id, |value| value["remote_host"] == "192.0.2.42");
        assert_eq!(listing["ok"], true);
        assert_eq!(listing["scan_pending"], true);
        assert_eq!(listing["remote_host_error"], serde_json::Value::Null);
        ports.send(Ok(vec![22, 3001])).unwrap();
        let listing = fixture.wait_listing(&id, |value| value["scan_pending"] == false);
        assert_eq!(listing["ports"][1]["remote_port"], 3001);
        assert_eq!(listing["remote_host"], "192.0.2.42");
    }

    #[test]
    fn ssh_port_browser_resolution_failure_does_not_hide_forwardable_ports() {
        let mut fixture = Fixture::new();
        let (id, ports) = fixture.add("port-test-b", Err("ssh -G failed"));
        ports.send(Ok(vec![3001])).unwrap();
        let listing = fixture.wait_listing(&id, |value| value["scan_pending"] == false);
        assert_eq!(listing["remote_host"], serde_json::Value::Null);
        assert_eq!(
            listing["remote_host_error"],
            "Transport 操作失败: ssh -G failed"
        );
        assert_eq!(listing["ports"][0]["remote_port"], 3001);
        assert_eq!(listing["scan_error"], serde_json::Value::Null);
    }

    #[test]
    fn ssh_port_listener_failure_keeps_the_remote_browser_address() {
        let mut fixture = Fixture::new();
        let (id, ports) = fixture.add("port-test-c", Ok("2001:db8::42"));
        ports.send(Err("ss and lsof failed".into())).unwrap();
        let listing = fixture.wait_listing(&id, |value| value["scan_pending"] == false);
        assert_eq!(listing["remote_host"], "2001:db8::42");
        assert_eq!(
            listing["scan_error"],
            "Transport 操作失败: ss and lsof failed"
        );
    }

    #[test]
    fn ssh_port_browser_addresses_are_scoped_to_workspace_and_removed_on_close() {
        let mut fixture = Fixture::new();
        let (first, first_ports) = fixture.add("port-test-first", Ok("192.0.2.42"));
        let (second, second_ports) = fixture.add("port-test-second", Ok("198.51.100.47"));
        first_ports.send(Ok(vec![3001])).unwrap();
        second_ports.send(Ok(vec![8080])).unwrap();
        assert_eq!(
            fixture.wait_listing(&first, |value| value["scan_pending"] == false)["remote_host"],
            "192.0.2.42"
        );
        assert_eq!(
            fixture.wait_listing(&second, |value| value["scan_pending"] == false)["remote_host"],
            "198.51.100.47"
        );
        unsafe {
            assert!((*fixture.handle).close_workspace(&first));
            assert!(!(*fixture.handle).ssh_browser_hosts.contains_key(&first));
            assert!(!(*fixture.handle).ssh_machine_ports.contains_key(&first));
        }
        assert_eq!(fixture.listing(&second)["remote_host"], "198.51.100.47");
    }
}
