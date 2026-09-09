//! Built-in RuntimeProvider registration.

use super::RuntimeProvider;

/// Construct the built-in RuntimeProvider list in stable UI order.
pub fn with_builtins() -> Vec<Box<dyn RuntimeProvider>> {
    vec![
        Box::new(super::tmux::provider::TmuxDriver),
        Box::new(super::herdr::provider::HerdrDriver),
        Box::new(super::shell::provider::ShellDriver),
    ]
}
