#ifndef muxterm_h
#define muxterm_h

#include <stdint.h>
#include <stddef.h>

// ── C 友好类型 ──

struct CStateChange {
    uint32_t type_;        // 0=PaneOutput, 1=TabAdded, 2=TabClosed, ...
    uint32_t pane_id;
    uint32_t tab_id;
    uint32_t window_id;
    const uint8_t* data;
    size_t data_len;
    const char* name;
};

struct CTask {
    uint32_t type_;        // 0=SplitPane, 1=NewTab, 2=SwitchTab, ... 9=Detach
    uint32_t target_pane;
    uint32_t target_tab;
    uint32_t dir;          // 0=horizontal, 1=vertical
    const char* name;
};

struct CTab {
    uint32_t id;
    const char* name;
    uint8_t is_active;
};

struct CPane {
    uint32_t id;
    uint16_t cols;
    uint16_t rows;
    uint8_t is_active;
};

struct CLayoutNode {
    uint32_t type_;        // 0=leaf, 1=split_h, 2=split_v
    uint32_t pane_id;
    uint32_t ratio;        // 0-1000
    const struct CLayoutNode* first;
    const struct CLayoutNode* second;
};

// ── 常量（与 src/core/ffi/types.rs 对齐；Rust 侧未导出符号，用宏定义）──
#define STATE_PANE_OUTPUT         0u
#define STATE_TAB_ADDED           1u
#define STATE_TAB_CLOSED          2u
#define STATE_LAYOUT_CHANGED      3u
#define STATE_PANE_ADDED          4u
#define STATE_PANE_CLOSED         5u
#define STATE_ACTIVE_TAB_CHANGED  6u
#define STATE_ACTIVE_PANE_CHANGED 7u
#define STATE_TAB_RENAMED         8u
#define STATE_PANE_RESIZED        9u
#define STATE_BACKEND_STATUS      10u
#define STATE_STATUS_SUBSCRIPTION 11u
#define STATE_WORKSPACE_RENAMED   12u
#define STATE_POOL_CHANGED        13u
#define STATE_OTHER               99u

#define TASK_SPLIT_PANE  0u
#define TASK_NEW_TAB     1u
#define TASK_SWITCH_TAB  2u
#define TASK_CLOSE_PANE  3u
#define TASK_CLOSE_TAB   4u
#define TASK_NEXT_PANE   5u
#define TASK_PREV_PANE   6u
#define TASK_SHUTDOWN    7u
#define TASK_SWITCH_PANE 8u
#define TASK_DETACH      9u
#define TASK_TOGGLE_PANE_FULLSCREEN 10u
#define TASK_MOVE_WINDOW 11u
#define TASK_BREAK_PANE 12u
#define TASK_REFRESH_TABS 13u
#define TASK_RENAME_TAB 14u
#define TASK_RENAME_WORKSPACE 15u

#define DIR_HORIZONTAL 0u
#define DIR_VERTICAL   1u

#define LAYOUT_LEAF    0u
#define LAYOUT_SPLIT_H 1u
#define LAYOUT_SPLIT_V 2u

// ── 生命周期（W7：MuxtermHandle = WorkspacePool）──
struct MuxtermHandle;
// deprecated 转发：建池并打开一个工作区（macOS 暂用）。
struct MuxtermHandle* muxterm_new(const char* backend_type, const char* socket, const char* session);
// deprecated 转发：一步建连（macOS 暂用）。
struct MuxtermHandle* muxterm_new_connect(
    const char* backend_type, const char* socket, const char* session,
    const char* ssh_alias, const char* start_directory);
void muxterm_free(struct MuxtermHandle* h);
int muxterm_connect(struct MuxtermHandle* h);
int muxterm_shutdown(struct MuxtermHandle* h);
int muxterm_detach(struct MuxtermHandle* h);
int muxterm_init_logging(const char* log_file, const char* level);

// ── Workspace 池（W7 新接口）──
int muxterm_workspace_open(
    struct MuxtermHandle* h,
    const char* transport, const char* alias, const char* session,
    const char* runtime, const char* path, const char* socket);
char* muxterm_workspace_list(struct MuxtermHandle* h);
int muxterm_workspace_activate(struct MuxtermHandle* h, const char* id);
int muxterm_workspace_close(struct MuxtermHandle* h, const char* id);

// ── 命令执行 ──
int muxterm_execute(struct MuxtermHandle* h, const struct CTask* task);
int muxterm_send_input(struct MuxtermHandle* h, uint32_t pane_id, const uint8_t* data, size_t len);
int muxterm_send_input_quiet(struct MuxtermHandle* h, uint32_t pane_id, const uint8_t* data, size_t len);
int muxterm_report_pane_colours(struct MuxtermHandle* h, uint32_t pane_id, const char* fg_hex, const char* bg_hex);
int muxterm_report_all_pane_colours(struct MuxtermHandle* h, const char* fg_hex, const char* bg_hex);
int muxterm_resize_pane(struct MuxtermHandle* h, uint32_t pane_id, uint16_t cols, uint16_t rows);
int muxterm_resize_client(struct MuxtermHandle* h, uint16_t cols, uint16_t rows);
int muxterm_status_subscription_active(struct MuxtermHandle* h);
int muxterm_resize_pane_axis(struct MuxtermHandle* h, uint32_t pane_id, uint32_t axis, uint16_t size);

// ── 事件轮询 ──
int muxterm_poll_events(struct MuxtermHandle* h, struct CStateChange* out, int max_count);

// ── 状态查询 ──
int muxterm_get_tabs(struct MuxtermHandle* h, struct CTab* out, int max_count);
int muxterm_get_panes(struct MuxtermHandle* h, uint32_t tab_id, struct CPane* out, int max_count);
int muxterm_get_pane_output(struct MuxtermHandle* h, uint32_t pane_id, uint8_t* buf, size_t buf_len);
int muxterm_get_layout(struct MuxtermHandle* h, uint32_t tab_id, struct CLayoutNode* out);

// ── 搜索 / 注意力 / 历史（W14/W16 跨平台契约）──
char* muxterm_search_all(struct MuxtermHandle* h, const char* query);
char* muxterm_attention_snapshot(struct MuxtermHandle* h);
char* muxterm_attention_take_notifications(struct MuxtermHandle* h);
int muxterm_attention_on_became_visible(struct MuxtermHandle* h, uint32_t pane_id);
int muxterm_attention_set_process_name(struct MuxtermHandle* h, uint32_t pane_id, const char* name);
int muxterm_attention_mute(struct MuxtermHandle* h, uint32_t pane_id, uint64_t seconds);
int muxterm_pane_scroll_ansi(struct MuxtermHandle* h, uint32_t pane_id, uint32_t offset, uint32_t rows, uint8_t* buf, size_t buf_len);
int muxterm_pane_visible_ansi(struct MuxtermHandle* h, uint32_t pane_id, uint8_t* buf, size_t buf_len);
int muxterm_pane_surface_seed_ansi(struct MuxtermHandle* h, uint32_t pane_id, uint8_t* buf, size_t buf_len);
int muxterm_pane_viewport(struct MuxtermHandle* h, uint32_t pane_id);
int muxterm_set_pane_viewport(struct MuxtermHandle* h, uint32_t pane_id, uint32_t offset);
int muxterm_pane_history_max_offset(struct MuxtermHandle* h, uint32_t pane_id, uint32_t rows);
char* muxterm_pane_command_marks_json(struct MuxtermHandle* h, uint32_t pane_id);
int64_t muxterm_pane_latest_line_seq(struct MuxtermHandle* h, uint32_t pane_id);
int muxterm_pane_viewport_for_seq(struct MuxtermHandle* h, uint32_t pane_id, uint64_t seq);
char* muxterm_pane_last_n_lines(struct MuxtermHandle* h, uint32_t pane_id, uint32_t n);

// ── 无状态 discovery（由 core 读取 SSH config / 查询 tmux）──
// 返回 malloc 风格的 JSON 字符串，调用 muxterm_free_string 释放。
char* muxterm_discover_ssh_hosts_json(const char* config_path);
char* muxterm_list_dir_json(
    const char* backend_type,
    const char* target,
    const char* config_path,
    const char* path,
    uint32_t timeout_ms
);
char* muxterm_discover_workspaces_json(
    const char* backend_type,
    const char* target,
    const char* socket,
    const char* config_path,
    uint32_t timeout_ms
);
// deprecated 别名（W7 改名）。
char* muxterm_discover_tmux_sessions_json(
    const char* backend_type,
    const char* target,
    const char* socket,
    const char* config_path,
    uint32_t timeout_ms
);
char* muxterm_status_snapshot_json(
    const char* backend_type,
    const char* target,
    const char* socket,
    const char* session
);
char* muxterm_workspace_create(
    const char* backend_type,
    const char* target,
    const char* socket,
    const char* config_path,
    const char* session,
    const char* directory,
    uint32_t timeout_ms
);
// deprecated 别名（W7 改名）。
char* muxterm_create_tmux_session_json(
    const char* backend_type,
    const char* target,
    const char* socket,
    const char* config_path,
    const char* session,
    const char* directory,
    uint32_t timeout_ms
);
void muxterm_free_string(char* value);

#endif /* muxterm_h */
