import AppKit
import XCTest

@testable import MuxtermAppLib

/// 用户报告：用键盘切 tab / 工作区时，主窗口的尺寸和位置会莫名其妙地变。
///
/// 主窗口 frame 只该由用户拖拽、缩放或显式 zoom/全屏改变；任何切换动作都
/// 不能动它。这些用例走真实快捷键路径（local monitor + NSApp.sendEvent）。
final class WindowFrameStabilityE2ETests: XCTestCase {
  private func describe(_ frame: NSRect?) -> String {
    guard let frame else { return "nil" }
    return
      "(\(Int(frame.origin.x)),\(Int(frame.origin.y)) \(Int(frame.width))x\(Int(frame.height)))"
  }

  private func send(_ app: MainWindowController, _ event: NSEvent) {
    _ = app.testRouteMonitoredKeyEvent(event)
    NSApp.sendEvent(event)
    AppE2E.pump(120)
    app.testPollOnce()
  }

  /// 图片横幅的首选宽度曾经高于窗口保持当前尺寸的优先级，布局求解会把
  /// 紧凑主窗口重新拉宽。
  func testCompactWindowKeepsItsRequestedFrame() throws {
    let painted = PaintedWorkspace(label: "frame-requested")
    let app = try AppE2E.attachWindow(socket: painted.socket, session: painted.session)
    defer { app.testShutdown() }
    XCTAssertTrue(app.waitReady(minTabs: 2, minLeaves: 3))

    let requested = AppE2E.fixedWindowFrame(width: 560, height: 380)
    app.window?.setFrame(requested, display: true)
    let started = Date()
    while Date().timeIntervalSince(started) < 1 {
      AppE2E.pump(20)
      app.testPollOnce()
      XCTAssertEqual(
        app.window?.frame,
        requested,
        "布局求解不能改写紧凑主窗口：actual=\(describe(app.window?.frame))"
      )
    }
  }

  /// 紧凑窗口 + 多 pane 的 tab：窗口小的时候切 tab 也不能被布局顶大。
  func testCompactWindowKeepsItsFrameWhenSwitchingTabs() throws {
    let painted = PaintedWorkspace(label: "frame-compact")
    let app = try AppE2E.attachWindow(socket: painted.socket, session: painted.session)
    defer { app.testShutdown() }
    XCTAssertTrue(app.waitReady(minTabs: 2, minLeaves: 3))
    let requested = AppE2E.fixedWindowFrame(width: 560, height: 380)
    app.window?.setFrame(requested, display: true)
    app.window?.makeKeyAndOrderFront(nil)
    AppE2E.pump(250)
    app.testPollOnce()
    XCTAssertEqual(app.window?.frame, requested)

    for key in ["2", "1"] {
      let code: UInt16 = key == "2" ? 19 : 18
      let event = try XCTUnwrap(app.testMakeKeyEvent(key: key, keyCode: code, command: true))
      let before = app.testActiveTabID()
      send(app, event)
      AppE2E.pump(250)
      XCTAssertNotEqual(app.testActiveTabID(), before, "Cmd-\(key) 必须真正切换 tab")
      XCTAssertEqual(
        app.window?.frame,
        requested,
        "紧凑窗口切 tab 不能改 frame：actual=\(describe(app.window?.frame))"
      )
    }
  }

  /// 展开侧栏后反复切 tab / 工作区。侧栏最小宽度参与窗口 fitting-size，
  /// 这是最容易把主窗口整体顶动的布局。
  func testOpenSidebarAndKeyboardSwitchingKeepTheMainWindowFrame() throws {
    let first = PaintedWorkspace(label: "frame-sidebar-first")
    let second = OnePaneCat(label: "frame-sidebar-second")
    let app = try AppE2E.attachWindow(socket: first.socket, session: first.session)
    defer { app.testShutdown() }
    XCTAssertTrue(app.waitReady(minTabs: 2, minLeaves: 3))

    let secondBridge = try CoreBridge(
      backendType: "tmux",
      socket: second.socket,
      session: second.session
    )
    app.testActivateWorkspaceBridge(secondBridge, session: second.session)
    XCTAssertTrue(
      AppE2E.wait(timeout: AppE2E.attachTimeout) {
        app.testPollOnce()
        return app.testActiveWorkspaceSession() == second.session
      })

    let back = try XCTUnwrap(
      app.testMakeKeyEvent(key: "1", keyCode: 18, command: true, control: true)
    )
    send(app, back)
    XCTAssertTrue(
      AppE2E.wait(timeout: AppE2E.attachTimeout) {
        app.testPollOnce()
        return app.testActiveWorkspaceSession() == first.session
      })
    app.setWorkspaceSidebarOpenForTest(true)
    let requested = AppE2E.fixedWindowFrame(width: 760, height: 420)
    app.window?.setFrame(requested, display: true)
    app.window?.makeKeyAndOrderFront(nil)
    AppE2E.pump(300)
    app.testPollOnce()
    XCTAssertEqual(app.window?.frame, requested)
    let initialTab = app.testActiveTabID()

    let tabEvent = try XCTUnwrap(app.testMakeKeyEvent(key: "2", keyCode: 19, command: true))
    send(app, tabEvent)
    AppE2E.pump(250)
    XCTAssertNotEqual(app.testActiveTabID(), initialTab, "Cmd-2 必须真正切到另一个 tab")
    XCTAssertEqual(app.window?.frame, requested, "展开侧栏后切 tab 不能改 frame")

    let workspaceEvent = try XCTUnwrap(
      app.testMakeKeyEvent(key: "2", keyCode: 19, command: true, control: true)
    )
    send(app, workspaceEvent)
    XCTAssertTrue(
      AppE2E.wait(timeout: AppE2E.attachTimeout) {
        app.testPollOnce()
        return app.testActiveWorkspaceSession() == second.session
      })
    AppE2E.pump(250)
    XCTAssertEqual(app.window?.frame, requested, "展开侧栏后切工作区不能改 frame")
  }

  /// Cmd-1 / Cmd-2 在同一 workspace 的两个 tab 之间切。
  func testKeyboardTabSwitchKeepsTheMainWindowFrame() throws {
    let painted = PaintedWorkspace(label: "frame-tab")
    let app = try AppE2E.attachWindow(socket: painted.socket, session: painted.session)
    defer { app.testShutdown() }
    XCTAssertTrue(app.waitReady(minTabs: 2, minLeaves: 3))
    app.window?.makeKeyAndOrderFront(nil)
    AppE2E.pump(200)

    let before = try XCTUnwrap(app.window?.frame)
    let initialTab = app.testActiveTabID()
    let event = try XCTUnwrap(app.testMakeKeyEvent(key: "2", keyCode: 19, command: true))
    send(app, event)
    AppE2E.pump(200)
    XCTAssertNotEqual(app.testActiveTabID(), initialTab)
    XCTAssertEqual(
      app.window?.frame,
      before,
      "Cmd-2 切 tab 不能改窗口 frame：actual=\(describe(app.window?.frame))"
    )
  }

  /// Cmd-Ctrl-1 / Cmd-Ctrl-2 在两个 workspace 之间切。
  func testKeyboardWorkspaceSwitchKeepsTheMainWindowFrame() throws {
    let first = OnePaneCat(label: "frame-ws-first")
    let second = OnePaneCat(label: "frame-ws-second")
    let app = try AppE2E.attachWindow(socket: first.socket, session: first.session)
    defer { app.testShutdown() }
    XCTAssertTrue(app.waitReady(minLeaves: 1))
    app.window?.makeKeyAndOrderFront(nil)
    AppE2E.pump(200)

    let secondBridge = try CoreBridge(
      backendType: "tmux",
      socket: second.socket,
      session: second.session
    )
    app.testActivateWorkspaceBridge(secondBridge, session: second.session)
    XCTAssertTrue(
      AppE2E.wait(timeout: AppE2E.attachTimeout) {
        app.testPollOnce()
        return app.testActiveWorkspaceSession() == second.session
      })
    AppE2E.pump(200)
    let before = try XCTUnwrap(app.window?.frame)

    let switches: [(key: String, code: UInt16, session: String)] = [
      ("1", 18, first.session),
      ("2", 19, second.session),
      ("1", 18, first.session),
    ]
    for item in switches {
      let event = try XCTUnwrap(
        app.testMakeKeyEvent(key: item.key, keyCode: item.code, command: true, control: true)
      )
      send(app, event)
      XCTAssertTrue(
        AppE2E.wait(timeout: AppE2E.attachTimeout) {
          app.testPollOnce()
          return app.testActiveWorkspaceSession() == item.session
        },
        "Cmd-Ctrl-\(item.key) 应切到 \(item.session)"
      )
      AppE2E.pump(200)
      XCTAssertEqual(
        app.window?.frame,
        before,
        "Cmd-Ctrl-\(item.key) 切工作区不能改窗口 frame：actual=\(describe(app.window?.frame))"
      )
    }
  }
}
