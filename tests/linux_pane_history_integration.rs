//! PaneView 历史/快照生命周期 GTK 测试。
//!
//! 独立测试二进制：这些用例创建/销毁 VTE 窗口，若与 AppWindow 重用例
//! 同进程顺序执行，连续建窗销毁会触发 VTE/GL double-free（SIGSEGV；
//! 见 docs/HERDR-RUNTIME-STABILITY.md §GL）。CI 按 `--jobs 1` 串行跑
//! 独立二进制，进程隔离后互不干扰。
#![cfg(feature = "gtk")]

mod support;

use gtk4::prelude::*;
use gtk4::Widget;

use muxterm::platform::linux::pane_view::PaneView;
use muxterm::platform::linux::quickconnect::font::FontSettings;

use support::linux_gtk::*;

/// 历史必须等首次 seed 后再写入 scrollback；权威 Snapshot reset 后按
/// generation 重放；新一轮 attach 清掉旧 generation 保留。
fn assert_history_waits_for_surface_seed(view: &PaneView) {
    view.prepend_history(b"HIST_BEFORE_SEED\npad-01");
    assert!(!view.is_seeded());
    assert_eq!(
        view.render_trace().feeds,
        0,
        "未播种 Surface 不得提前回放历史"
    );

    view.seed_snapshot(b"TAIL_VISIBLE\r\n", 80, 24);
    pump_main_loop(40);
    assert_eq!(view.render_trace().feeds, 1, "历史应在 seed 后恰好回放一次");
    let history = view.buffer_text();
    assert!(history.contains("HIST_BEFORE_SEED"), "{history}");
    let visible = view.visible_text();
    assert!(visible.contains("TAIL_VISIBLE"), "{visible}");
    assert!(!visible.contains("HIST_BEFORE_SEED"), "{visible}");

    view.seed_snapshot(b"TAIL_AFTER_RESET\r\n", 80, 24);
    pump_main_loop(40);
    // 同一 generation 内第二次权威 Snapshot 只替换当前屏：scrollback 里
    // 已应用的历史必须保留，不得再次重放翻倍。
    let replayed = view.buffer_text();
    assert!(replayed.contains("HIST_BEFORE_SEED"), "{replayed}");
    let visible = view.visible_text();
    assert!(visible.contains("TAIL_AFTER_RESET"), "{visible}");

    // 新一轮 attach：旧 generation 的保留历史必须清掉，权威 Snapshot
    // reset 后不得重放旧历史（否则 reattach 会重复历史/无界增长）。
    view.begin_attach_generation();
    view.seed_snapshot(b"TAIL_AFTER_REATTACH\r\n", 80, 24);
    pump_main_loop(40);
    let after_reattach = view.buffer_text();
    assert!(
        !after_reattach.contains("HIST_BEFORE_SEED"),
        "{after_reattach}"
    );
    let visible = view.visible_text();
    assert!(visible.contains("TAIL_AFTER_REATTACH"), "{visible}");
    assert!(
        !visible.contains("HIST_BEFORE_SEED"),
        "新 attach generation 不得重放旧历史"
    );
}

/// 复现 reattach 失败链：快照网格大于 VTE 可见行数时，seed 后光标必须
/// 锚定在 buffer 末尾，否则 shell 的 prompt 重绘（resize 触发）会从错误
/// 位置 `ESC[J` 清掉刚 seed 的历史 token。
fn assert_snapshot_larger_than_surface_survives_prompt_redraw(view: &PaneView) {
    // 40 行快照（含历史 token），保证大于任意常见 VTE 可见行数。
    let mut snapshot = Vec::new();
    for i in 0..35 {
        snapshot.extend_from_slice(format!("line-{i:02}\r\n").as_bytes());
    }
    snapshot.extend_from_slice(b"HIST_TOKEN_IN_SNAPSHOT\r\n");
    for _ in 0..4 {
        snapshot.extend_from_slice(b"\r\n");
    }
    snapshot.extend_from_slice(b"\x1b[40;3H");
    view.seed_snapshot(&snapshot, 42, 20);
    pump_main_loop(40);
    assert!(
        view.buffer_text().contains("HIST_TOKEN_IN_SNAPSHOT"),
        "快照内容必须进入 VTE buffer"
    );

    // shell 因 resize 重绘 prompt：\r\r ESC M ESC M ESC[J + 新 prompt。
    view.feed_output(b"\r\r\x1bM\x1bM\x1b[0m\x1b[27m\x1b[24m\x1b[Jwlz@ryzen prompt\r\n\x1b[35m\xe2\x9d\xaf\x1b[39m ");
    pump_main_loop(40);
    view.flush_pending_feed();
    pump_main_loop(40);
    let buffer = view.buffer_text();
    assert!(
        buffer.contains("HIST_TOKEN_IN_SNAPSHOT"),
        "prompt 重绘不得清掉快照历史: {buffer}"
    );
}

/// 同一个窗口/同一个 PaneView 顺序跑历史与快照场景。
///
/// 同进程连续创建/销毁 VTE 窗口会触发 GLib/VTE double-free 或分配溢出
/// （SIGABRT），因此所有轻量窗口断言必须共享一个窗口实例。
fn assert_pane_history_and_snapshot_lifecycle() {
    let view = PaneView::new(7, &load_theme(), &FontSettings::default(), true, 1_000);
    let widget = view.widget();
    let window = gtk4::Window::builder()
        .title("pane-history-snapshot")
        .default_width(338)
        .default_height(440)
        .build();
    window.set_child(Some(&widget));
    window.present();
    gtk4::test_widget_wait_for_draw(&window);
    pump_main_loop(40);

    assert_history_waits_for_surface_seed(&view);
    assert_snapshot_larger_than_surface_survives_prompt_redraw(&view);

    window.set_child(None::<&Widget>);
    window.destroy();
    pump_main_loop(40);
}

#[test]
fn gtk_z_pane_history_and_snapshot_lifecycle() {
    if skip_no_display() {
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        assert_pane_history_and_snapshot_lifecycle();
    });
}
