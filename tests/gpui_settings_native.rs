//! 原生多窗口与视觉验收（主线程）。不启动 Core/PTY，不访问用户配置。
//!
//! **必须显式开启**：`MUXTERM_NATIVE_UI_TESTS=1 cargo test --test gpui_settings_native
//! --features gpui -- [--snapshots /tmp/muxterm-ui-review]`
//!
//! 原因：这个测试会真的打开两个窗口。以前它默认执行并调用 `cx.activate(true)`，
//! 等于在用户机器上抢焦点、反复呈现图层、循环 `screencapture`，把整台机器拖卡。
//! 现在的规则：
//! - 默认跳过，不开窗口（纯逻辑断言在 `tests/support/gpui_settings.rs` 里已覆盖）；
//! - 绝不调用 `activate`，不抢前台；
//! - 只在最后呈现一次并截图，不循环刷新。
#![allow(unexpected_cfgs)]

#[cfg(target_os = "macos")]
fn main() {
    let enabled = std::env::var("MUXTERM_NATIVE_UI_TESTS").as_deref() == Ok("1");
    if !enabled {
        println!("native Settings: skipped (set MUXTERM_NATIVE_UI_TESTS=1 to open real windows)");
        return;
    }
    run();
}

#[cfg(target_os = "macos")]
fn run() {
    use gpui::{px, size, AppContext, Application, Bounds, Context, WindowBounds, WindowOptions};
    use muxterm::frontend::{
        gpui::{model::panel::PanelTab, state::GpuiApp},
        view_store::ViewStore,
    };
    let args: Vec<_> = std::env::args().collect();
    let snapshots = args
        .iter()
        .position(|arg| arg == "--snapshots")
        .and_then(|index| args.get(index + 1))
        .cloned();
    if let Some(path) = &snapshots {
        std::fs::create_dir_all(path).unwrap();
    }
    Application::new().run(move|cx|{
        let bounds=Bounds::centered(None,size(px(1200.0),px(800.0)),cx);
        let handle=cx.open_window(WindowOptions{window_bounds:Some(WindowBounds::Windowed(bounds)),..Default::default()},|window,cx|cx.new(|cx:&mut Context<GpuiApp>|{
            let mut store=ViewStore::default();
            store.replace_topology(muxterm::frontend::utils::corebridge::ClientWorkspace{id:"preview".into(),name:"Preview".into(),runtime:"shell".into(),active:true,resolved_target:None},vec![muxterm::frontend::utils::corebridge::ClientTab{id:1,name:"Shell".into(),is_active:true}],vec![(1,vec![muxterm::frontend::utils::corebridge::ClientPane{id:1,cols:80,rows:24,is_active:true,title:"Preview".into()}])]);
            let mut app=GpuiApp::new_headless(store,cx);
            window.focus(&app.focus_handle());
            let projects=serde_json::json!([
                {"id":"timepulse","name":"timepulse","path":"~/Project/self/timepulse","runtime":{"id":"herdr"},"transport":{"id":"ssh","target":"ryzen"}},
                {"id":"muxterm","name":"muxterm","path":"~/Developer/self/muxterm","runtime":{"id":"tmux"},"transport":{"id":"local"}},
                {"id":"legion","name":"legion","path":"~/Developer/work/legion","runtime":{"id":"herdr"},"transport":{"id":"ssh","target":"ryzen"}},
                {"id":"ubuntu","name":"ubuntu-home","path":"/home/ubuntu","runtime":{"id":"tmux"},"transport":{"id":"ssh","target":"home"}}
            ]);
            let values=serde_json::json!({"projects":projects,"shortcuts":{"preset":"colemak","primary_key":"auto","overrides":[]},"font":{"family":"Menlo","size":18,"line_spacing":100,"column_spacing":100,"fallback":[]},"theme":{"name":"system","light":"white","dark":"black"}});
            app.set_picker_data_for_tests(projects,vec!["ryzen".into(),"home".into()]);
            app.open_settings(&muxterm::config::ConfigDocument::manifest_json(),&values,cx);
            app.settings_select("projects",cx);
            app.detach_settings_window(cx).unwrap();
            app
        })).unwrap();
        // 不 activate：测试永远不抢用户前台。
        cx.spawn(async move|cx|{
            for (category,file) in [("projects","projects.png"),("shortcuts","shortcuts.png"),("appearance","appearance.png")] {
                handle.update(cx,|app,_,cx|{app.settings_select(category,cx);assert_eq!(app.settings_selected().unwrap().id,category);}).expect("select settings category");
                gpui::Timer::after(std::time::Duration::from_millis(600)).await;
                cx.update(|cx|assert_eq!(cx.windows().len(),2,"one main window and one settings window")).unwrap();
                if let Some(path)=&snapshots { capture(&format!("{path}/{file}"),true); }
            }
            handle.update(cx,|app,window,cx|{app.settings_cancel(cx);app.show_panel(PanelTab::Workspaces,cx);assert!(app.panel_open());window.refresh();}).expect("show panel");
            gpui::Timer::after(std::time::Duration::from_millis(800)).await;
            cx.update(|cx|assert_eq!(cx.windows().len(),1,"closing Settings must keep the main window")).unwrap();
            if let Some(path)=&snapshots { capture(&format!("{path}/quick-panel.png"),false); }
            let _=cx.update(|cx|cx.quit());
        }).detach();
    });
    println!(
        "native Settings: Projects, Shortcuts, Appearance and multi-window lifecycle verified"
    );
}

#[cfg(target_os = "macos")]
fn capture(path: &str, settings: bool) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let windows: *mut Object = msg_send![app, windows];
        let count: usize = msg_send![windows, count];
        for index in 0..count {
            let window: *mut Object = msg_send![windows,objectAtIndex:index];
            let title: *mut Object = msg_send![window, title];
            let bytes: *const std::os::raw::c_char = msg_send![title, UTF8String];
            let is_settings = std::ffi::CStr::from_ptr(bytes)
                .to_string_lossy()
                .ends_with(" — Settings");
            if is_settings != settings {
                continue;
            }
            // 非活动测试窗口可能暂停 display link；显式呈现已脏的帧再截图。
            let view: *mut Object = msg_send![window, contentView];
            let layer: *mut Object = msg_send![view, layer];
            let _: () = msg_send![layer, setNeedsDisplay];
            let _: () = msg_send![layer, displayIfNeeded];
            let id: isize = msg_send![window, windowNumber];
            let status = std::process::Command::new("/usr/sbin/screencapture")
                .args(["-x", "-o", "-l", &id.to_string(), path])
                .status()
                .unwrap();
            assert!(status.success(), "failed to capture test window {id}");
            return;
        }
        panic!("test window not found");
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("native Settings: not applicable on this platform");
}
