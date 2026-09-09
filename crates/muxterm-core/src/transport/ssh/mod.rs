//! Compatibility path for the transport-owned SSH PTY implementation.

pub mod provider;

pub use muxterm_transport::ssh::{
    build_ssh_command, LaunchedProcess, ProcessLauncher, SshProcessTransport, SystemLauncher,
};
