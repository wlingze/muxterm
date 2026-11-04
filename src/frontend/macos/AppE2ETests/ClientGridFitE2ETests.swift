import AppKit
import XCTest

@testable import MuxtermAppLib

/// client 格子（`refresh-client -C`）必须是可见终端区的格子。
///
/// 用户报告：切工作区/切 tab 后画面在两种字距上重复，窗口尺寸也会变。
/// 日志里的 `refresh-client -C` 出现过 422x44 / 210x44，而同一时刻窗口只有
/// 2048pt 宽（186 列）：Runtime 把 pane 排到窗口外，每个 pane 的模型都比
/// host 宽，字被裁在右边界。容器一旦大于窗口，client 格子必须被夹回窗口。
final class ClientGridFitE2ETests: XCTestCase {
  private func send(_ app: MainWindowController, _ event: NSEvent) {
    _ = app.testRouteMonitoredKeyEvent(event)
    NSApp.sendEvent(event)
    AppE2E.pump(120)
    app.testPollOnce()
  }

  /// 容器被撑大（缓存树重挂/侧栏动画）时，client 格子不能超过窗口内容区。
  func testClientGridNeverExceedsWindowContent() throws {
    let painted = PaintedWorkspace(label: "clientfit-stretch")
    let app = try AppE2E.attachWindow(socket: painted.socket, session: painted.session)
    defer { app.testShutdown() }
    XCTAssertTrue(app.waitReady(minTabs: 2, minLeaves: 3))
    app.window?.setFrame(
      AppE2E.fixedWindowFrame(width: 1200, height: 800),
      display: true
    )
    app.window?.makeKeyAndOrderFront(nil)
    AppE2E.pump(400)
    app.testPollOnce()

    let windowContent = app.testWindowContentViewSize()
    XCTAssertGreaterThan(windowContent.width, 0)
    let baseline = try XCTUnwrap(app.testClientGridForStretchedLayout(width: windowContent.width))
    XCTAssertNil(
      app.testClientGridForStretchedLayout(width: 4642),
      "4642pt 的容器（210/422 列那次事故）不是当前窗口的终端区，不能写出 client"
    )
    XCTAssertNil(
      app.testClientGridForStretchedLayout(width: 2310),
      "2310pt 的容器同样不属于当前窗口"
    )
    XCTAssertNil(
      app.testClientGridForStretchedLayout(width: 200),
      "比窗口窄出侧栏上限的容器也不能写 client"
    )
    // 更窄的容器（侧栏展开）保持自己的列数，不能被夹成窗口列数。
    let sidebar = try XCTUnwrap(app.testClientGridForStretchedLayout(width: 900))
    XCTAssertLessThan(sidebar.cols, baseline.cols)

    // 撑大只读：paneLayout 不能被测试留在被撑大的尺寸上。
    AppE2E.pump(120)
    XCTAssertEqual(
      Int(app.testPaneLayoutFrame().width),
      Int(app.testTerminalClientContentSize().width),
      "测试后 paneLayout 必须回到真实宽度"
    )
  }

  /// 切 tab / 切工作区之后，client 列数必须等于单 pane tab 的 pane 列数。
  ///
  /// 事故里 client 是 422 列而 pane 只有 211 列（每个 pane 都比 host 宽），
  /// 这个断言直接把「client 与可见 pane 不一致」钉死。
  func testClientGridMatchesVisiblePaneGridAcrossSwitches() throws {
    let first = PaintedWorkspace(label: "clientfit-switch")
    let second = OnePaneCat(label: "clientfit-second")
    let app = try AppE2E.attachWindow(socket: first.socket, session: first.session)
    defer { app.testShutdown() }
    XCTAssertTrue(app.waitReady(minTabs: 2, minLeaves: 3))
    app.window?.setFrame(
      AppE2E.fixedWindowFrame(width: 1200, height: 800),
      display: true
    )
    app.window?.makeKeyAndOrderFront(nil)
    AppE2E.pump(400)
    app.testPollOnce()

    func assertSinglePaneTabMatchesClient(_ label: String) {
      let leaves = app.testLayoutLeafIDs()
      guard leaves.count == 1, let pane = leaves.first else { return }
      let grid = app.testPaneGrid(pane)
      // 不硬编码字符宽度：CI 无 config.toml，字体回落到 Core 默认 12pt。
      let client = app.testClientGrid()
      let expectedCols = client.map { Int($0.cols) }
      XCTAssertEqual(
        grid.cols,
        expectedCols,
        "\(label): 单 pane tab 的格子必须等于 client 格子"
          + "（pane=\(grid.cols) client=\(String(describing: expectedCols))）"
      )
      XCTAssertEqual(
        grid.rows,
        client.map { Int($0.rows) },
        "\(label): 行数同样必须等于 client 格子（pane=\(grid.rows)）"
      )
    }

    for tab in app.testTabIDs() {
      app.testSwitchTab(tab)
      AppE2E.pump(300)
      app.testPollOnce()
      assertSinglePaneTabMatchesClient("tab-\(tab)")
    }

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
    AppE2E.pump(300)
    app.testPollOnce()
    assertSinglePaneTabMatchesClient("workspace-second")

    for key in ["1", "2", "1"] {
      let code: UInt16 = key == "1" ? 18 : 19
      let event = try XCTUnwrap(
        app.testMakeKeyEvent(key: key, keyCode: code, command: true, control: true)
      )
      send(app, event)
      AppE2E.pump(300)
      assertSinglePaneTabMatchesClient("workspace-\(key)")
    }
  }
}
