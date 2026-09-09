//! `#[no_mangle] extern "C"` 导出函数。
//!
//! 对外全部同步；具体实现按领域位于 `functions/`。
pub use super::functions::activity::{
    muxterm_activity_snapshot_json, muxterm_activity_take_events_json,
};
pub use super::functions::attention::{
    muxterm_attention_acknowledge, muxterm_attention_configure_json, muxterm_attention_mute,
    muxterm_attention_on_became_visible, muxterm_attention_set_process_name,
    muxterm_attention_snapshot, muxterm_attention_take_notifications,
    muxterm_workspace_attention_acknowledge, muxterm_workspace_attention_mute,
    muxterm_workspace_attention_on_became_visible, muxterm_workspace_attention_set_process_name,
};
pub use super::functions::catalog::{
    muxterm_candidates_json, muxterm_open_json, muxterm_workspace_open_target_json,
    muxterm_workspace_worktree_create_json,
};
pub use super::functions::config::{
    muxterm_config_begin_json, muxterm_config_cancel_json, muxterm_config_commit_json,
    muxterm_config_describe_json, muxterm_config_events_json, muxterm_config_patch_json,
    muxterm_config_reload_json, muxterm_config_validate_json,
};
pub use super::functions::events::{muxterm_poll_events, muxterm_poll_workspace_events};
pub use super::functions::handle::{
    muxterm_catalog_new, muxterm_free, muxterm_free_string, muxterm_init_logging, muxterm_new,
    muxterm_new_connect, muxterm_new_connect_sized,
};
pub use super::functions::runtime::{
    muxterm_connect, muxterm_detach, muxterm_runtime_list_json, muxterm_shutdown,
    muxterm_status_subscription_active, muxterm_traffic_down, muxterm_traffic_up,
    muxterm_workspace_herdr_probe_json,
};
pub use super::functions::search::muxterm_search_all;
pub use super::functions::snapshot::{
    muxterm_pane_command_marks_json, muxterm_pane_history_max_offset, muxterm_pane_last_n_lines,
    muxterm_pane_latest_line_seq, muxterm_pane_scroll_ansi, muxterm_pane_surface_seed_ansi,
    muxterm_pane_viewport, muxterm_pane_viewport_for_seq, muxterm_set_pane_viewport,
    muxterm_workspace_pane_command_marks_json, muxterm_workspace_pane_history_max_offset,
    muxterm_workspace_pane_last_n_lines, muxterm_workspace_pane_latest_line_seq,
    muxterm_workspace_pane_scroll_ansi, muxterm_workspace_pane_surface_seed_ansi,
    muxterm_workspace_pane_viewport, muxterm_workspace_pane_viewport_for_seq,
    muxterm_workspace_set_pane_viewport, muxterm_workspace_take_pane_reply,
};
pub use super::functions::support::MuxtermHandle;
pub use super::functions::task::{
    muxterm_execute, muxterm_execute_json, muxterm_execute_workspace,
    muxterm_report_all_pane_colours, muxterm_report_pane_colours, muxterm_resize_client,
    muxterm_resize_pane, muxterm_resize_pane_axis, muxterm_send_input, muxterm_send_input_quiet,
    muxterm_workspace_resize_client, muxterm_workspace_resize_pane,
    muxterm_workspace_resize_pane_axis, muxterm_workspace_send_input,
    muxterm_workspace_send_input_quiet,
};
pub use super::functions::transport::{
    muxterm_discover_sessions_json, muxterm_discover_ssh_hosts_json,
    muxterm_discover_ssh_tmux_panes_json, muxterm_discover_targets_json,
    muxterm_discover_tmux_sessions_json, muxterm_discover_workspaces_json, muxterm_list_dir_json,
    muxterm_status_snapshot_json, muxterm_transport_list_json,
};
pub use super::functions::workspace::{
    muxterm_create_tmux_session_json, muxterm_get_layout, muxterm_get_pane_output,
    muxterm_get_panes, muxterm_get_tabs, muxterm_workspace_activate, muxterm_workspace_close,
    muxterm_workspace_create, muxterm_workspace_get_layout, muxterm_workspace_get_pane_output,
    muxterm_workspace_get_panes, muxterm_workspace_get_tabs, muxterm_workspace_list,
    muxterm_workspace_open,
};
