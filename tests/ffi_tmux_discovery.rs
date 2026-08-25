//! FFI discovery must honor the caller's isolated tmux socket.

#![cfg(feature = "ffi")]

mod support;

use std::ffi::{CStr, CString};
use std::ptr;

use muxterm::core::protocol::ffi::{muxterm_discover_tmux_sessions_json, muxterm_free_string};
use support::tmux_test_support::{create_session, kill_server, unique_socket};

#[test]
fn ffi_tmux_discovery_returns_sessions_from_explicit_socket() {
    let socket = unique_socket("ffi-discovery");
    let session = "muxterm-test-ffi-discovery";
    create_session(&socket, session, 80, 24);

    let transport = CString::new("local").unwrap();
    let socket_c = CString::new(socket.clone()).unwrap();
    let response = muxterm_discover_tmux_sessions_json(
        transport.as_ptr(),
        ptr::null(),
        socket_c.as_ptr(),
        ptr::null(),
        2_000,
    );
    assert!(!response.is_null(), "FFI discovery must return JSON");
    let json = unsafe { CStr::from_ptr(response) }
        .to_str()
        .expect("FFI discovery JSON must be UTF-8")
        .to_owned();
    unsafe { muxterm_free_string(response) };
    kill_server(&socket);

    let value: serde_json::Value = serde_json::from_str(&json).expect("valid discovery JSON");
    assert_eq!(
        value.get("ok").and_then(serde_json::Value::as_bool),
        Some(true)
    );
    let sessions = value
        .get("sessions")
        .and_then(serde_json::Value::as_array)
        .expect("FFI response must contain sessions");
    assert!(
        sessions.iter().any(|entry| {
            entry.get("name").and_then(serde_json::Value::as_str) == Some(session)
                && entry.get("windows").is_some()
                && entry.get("created").is_some()
        }),
        "explicit socket session missing from FFI response: {value}"
    );
}
