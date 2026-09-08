//! TUI mirror response policy.
//!
//! The TUI owns the decision about whether parser-generated terminal replies
//! should be sent back through the public input surface. Core only provides
//! terminal parsing; it does not decide how a frontend mirrors a remote pane.

/// Whether parser-generated terminal replies should be sent to the backend.
///
/// A tmux mirror must discard replies while feeding remote output: writing
/// them through `send_input` makes the pane echo the reply as shell input.
/// A direct PTY frontend forwards replies both during and outside a feed.
pub fn should_forward_parser_response(
    during_remote_output_feed: bool,
    is_tmux_mirror: bool,
) -> bool {
    !is_tmux_mirror || !during_remote_output_feed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmux_mirror_drops_parser_response_during_feed() {
        assert!(!should_forward_parser_response(true, true));
    }

    #[test]
    fn tmux_mirror_forwards_outside_feed() {
        assert!(should_forward_parser_response(false, true));
    }

    #[test]
    fn direct_pty_forwards_responses() {
        assert!(should_forward_parser_response(true, false));
        assert!(should_forward_parser_response(false, false));
    }
}
