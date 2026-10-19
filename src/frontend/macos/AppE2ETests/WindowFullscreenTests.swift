import AppKit
import XCTest
import MuxtermChrome
@testable import MuxtermAppLib

final class WindowFullscreenTests: XCTestCase {
    func testKeyboardPaletteAndStatusBarAllToggleNativeWindowNotPane() throws {
        AppE2E.ensureApp()
        let bridge = try CoreBridge(backendType: "local")
        let app = MainWindowController(bridge: bridge)
        defer { app.testShutdown() }
        let window = try XCTUnwrap(app.window)
        let before = app.testLayoutLeafIDs()
        var toggles = 0
        app.performWindowFullscreenToggle = { target in
            XCTAssertTrue(target === window)
            toggles += 1
        }
        XCTAssertTrue(window.collectionBehavior.contains(.fullScreenPrimary))
        for (key, code) in [("f", UInt16(3)), ("\r", UInt16(36))] {
            let event = try XCTUnwrap(app.testMakeKeyEvent(
                key: key, keyCode: code, command: true, shift: true
            ))
            XCTAssertTrue(app.testDispatchKeyEvent(event))
        }
        let colemakE = try XCTUnwrap(app.testMakeKeyEvent(
            key: "e", keyCode: 14, command: true, shift: true
        ))
        XCTAssertFalse(app.testDispatchKeyEvent(colemakE))
        XCTAssertEqual(toggles, 2)
        app.testOpenCommandPalette()
        let title = MuxtermI18n.shared.trDynamic("window.fullscreen.enter", fallback: "Enter Window Full Screen")
        XCTAssertTrue(app.testPaletteTitles().contains(title))
        app.testSelectPaletteTitle(title)
        XCTAssertEqual(toggles, 3)
        let button = try XCTUnwrap(app.content.statusBar.subviews.first {
            $0.accessibilityIdentifier() == "muxterm.windowFullscreen"
        } as? NSButton)
        button.performClick(nil)
        XCTAssertEqual(toggles, 4)
        XCTAssertEqual(app.testLayoutLeafIDs(), before, "窗口全屏入口不能修改 pane zoom")
        app.windowDidEnterFullScreen(Notification(name: NSWindow.didEnterFullScreenNotification, object: window))
        XCTAssertEqual(button.accessibilityValue() as? String, "fullscreen")
        app.windowDidExitFullScreen(Notification(name: NSWindow.didExitFullScreenNotification, object: window))
        XCTAssertEqual(button.accessibilityValue() as? String, "windowed")
        var zooms = 0
        app.performWindowZoom = { _ in zooms += 1 }
        let zoom = try XCTUnwrap(app.testMakeKeyEvent(
            key: "t", keyCode: 17, command: true, shift: true
        ))
        XCTAssertTrue(app.testDispatchKeyEvent(zoom))
        XCTAssertEqual(zooms, 1)
        XCTAssertEqual(toggles, 4, "桌面最大化不能顺手触发全屏")
    }

    func testDesktopZoomFillsTheVisibleFrameWithoutFullscreen() {
        let small = NSRect(x: 40, y: 40, width: 800, height: 500)
        let visible = NSRect(x: 0, y: 40, width: 1440, height: 860)
        let maximized = WindowFrameZoom.next(current: small, saved: nil, visible: visible)
        XCTAssertEqual(maximized.frame, visible)
        XCTAssertEqual(maximized.saved, small)
        let restored = WindowFrameZoom.next(current: maximized.frame, saved: maximized.saved, visible: visible)
        XCTAssertEqual(restored.frame, small)
        XCTAssertNil(restored.saved)
    }

    func testFullscreenAndPaneZoomHaveIndependentShortcutActions() {
        XCTAssertEqual(KeyBindings.action(for: .init(command: true, key: "\r")), .togglePaneFullscreen)
        XCTAssertEqual(KeyBindings.action(for: .init(option: true, key: "\r")), .togglePaneFullscreen)
        XCTAssertEqual(KeyBindings.action(for: .init(command: true, shift: true, key: "f")), .toggleWindowFullscreen)
        XCTAssertEqual(KeyBindings.action(for: .init(command: true, shift: true, key: "t")), .toggleWindowZoom)
        XCTAssertNil(KeyBindings.action(for: .init(command: true, shift: true, key: "e")))
        XCTAssertNil(KeyBindings.action(for: .init(command: true, shift: true, key: "g")))
        XCTAssertEqual(KeyBindings.action(for: .init(command: true, key: ",")), .openSettings)
        XCTAssertEqual(KeyBindingsConfig.action(from: "toggle_window_fullscreen"), .toggleWindowFullscreen)
        let rows = ShortcutCatalog.defaults()
        XCTAssertTrue(rows.contains { $0.action == "toggle_window_fullscreen" })
        XCTAssertTrue(rows.contains { $0.action == "toggle_pane_fullscreen" })
    }
}
