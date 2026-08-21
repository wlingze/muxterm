import XCTest
@testable import MuxtermChrome

final class StatusBarColorTests: XCTestCase {
    func testNamedAndXtermColors() {
        XCTAssertEqual(StatusBarStyleParser.color("red"), StatusBarColor(red: 205 / 255, green: 49 / 255, blue: 49 / 255))
        XCTAssertEqual(StatusBarStyleParser.color("brightwhite"), StatusBarColor(red: 1, green: 1, blue: 1))
        XCTAssertEqual(StatusBarStyleParser.color("colour0"), StatusBarColor(red: 0, green: 0, blue: 0))
        // xterm 灰度：colour232=8, colour255=238。
        XCTAssertEqual(StatusBarStyleParser.color("colour255"), StatusBarColor(red: 238 / 255, green: 238 / 255, blue: 238 / 255))
        XCTAssertEqual(StatusBarStyleParser.color("colour231"), StatusBarColor(red: 1, green: 1, blue: 1))
        XCTAssertEqual(StatusBarStyleParser.color("#abc"), StatusBarColor(red: 170 / 255, green: 187 / 255, blue: 204 / 255))
        XCTAssertEqual(StatusBarStyleParser.color("cdd6f4"), StatusBarColor(red: 205 / 255, green: 214 / 255, blue: 244 / 255))
        XCTAssertNil(StatusBarStyleParser.color("default"))
        XCTAssertNil(StatusBarStyleParser.color("bogus"))
    }

    func testXtermCubeAndGray() {
        XCTAssertEqual(StatusBarStyleParser.xterm256(16), StatusBarColor(red: 0, green: 0, blue: 0))
        XCTAssertEqual(StatusBarStyleParser.xterm256(231), StatusBarColor(red: 1, green: 1, blue: 1))
        XCTAssertEqual(StatusBarStyleParser.xterm256(232), StatusBarColor(red: 8 / 255, green: 8 / 255, blue: 8 / 255))
        XCTAssertNil(StatusBarStyleParser.xterm256(256))
    }
}

final class StatusBarModeTests: XCTestCase {
    func testDefaultsToTmux() {
        XCTAssertEqual(StatusBarMode.from(toml: nil), .tmux)
        XCTAssertEqual(StatusBarMode.from(toml: "[font]\nsize = 18"), .tmux)
    }

    func testParsesThemeMode() {
        let toml = """
        [statusbar]
        mode = "theme"
        """
        XCTAssertEqual(StatusBarMode.from(toml: toml), .theme)
    }

    func testParsesLegacyGuiModeAsTheme() {
        let toml = """
        [statusbar]
        color_mode = "gui"
        """
        XCTAssertEqual(StatusBarMode.from(toml: toml), .theme)
    }
}

final class StatusQueryTargetTests: XCTestCase {
    func testLocalKeepsSocket() {
        let target = StatusQueryTarget.resolve(
            backendType: "tmux",
            socket: "my-socket",
            sshAlias: nil
        )
        XCTAssertEqual(target.socket, "my-socket")
        XCTAssertNil(target.sshAlias)
    }

    func testSshMovesAliasOutOfSocket() {
        let target = StatusQueryTarget.resolve(
            backendType: "ssh",
            socket: "ryzen",
            sshAlias: nil
        )
        XCTAssertNil(target.socket)
        XCTAssertEqual(target.sshAlias, "ryzen")
    }

    func testSshWithExplicitAliasPreservesRemoteSocket() {
        let target = StatusQueryTarget.resolve(
            backendType: "ssh",
            socket: "muxterm-test-remote",
            sshAlias: "ryzen"
        )
        XCTAssertEqual(target.socket, "muxterm-test-remote")
        XCTAssertEqual(target.sshAlias, "ryzen")
    }
}

final class StatusBarStyleParserTests: XCTestCase {
    func testParseStyleString() {
        let style = StatusBarStyleParser.parse(style: "bg=green,fg=black,bold")
        XCTAssertEqual(style.fg, StatusBarStyleParser.color("black"))
        XCTAssertEqual(style.bg, StatusBarStyleParser.color("green"))
        XCTAssertTrue(style.bold)
        XCTAssertFalse(style.reverse)
    }

    func testParseStyleDefault() {
        XCTAssertEqual(StatusBarStyleParser.parse(style: "default"), .default)
        XCTAssertEqual(StatusBarStyleParser.parse(style: ""), .default)
    }

    func testParseInlineSegments() {
        let segments = StatusBarStyleParser.parseInline(
            text: "#[fg=colour233,bg=colour241,bold] 13/08 #[fg=colour233,bg=colour245,nobold] 13:50 ",
            base: .default
        )
        XCTAssertEqual(segments.count, 2)
        XCTAssertEqual(segments[0].text, " 13/08 ")
        XCTAssertEqual(segments[0].style.fg, StatusBarStyleParser.color("colour233"))
        XCTAssertEqual(segments[0].style.bg, StatusBarStyleParser.color("colour241"))
        XCTAssertTrue(segments[0].style.bold)
        XCTAssertEqual(segments[1].style.fg, StatusBarStyleParser.color("colour233"))
        XCTAssertEqual(segments[1].style.bg, StatusBarStyleParser.color("colour245"))
        XCTAssertFalse(segments[1].style.bold)
    }

    func testParseInlineDefaultResetsToBase() {
        let base = StatusBarTextStyle(fg: StatusBarStyleParser.color("white"), bg: StatusBarStyleParser.color("black"), bold: true)
        let segments = StatusBarStyleParser.parseInline(
            text: "A#[default]B#[fg=red]C",
            base: base
        )
        XCTAssertEqual(segments.count, 3)
        XCTAssertEqual(segments[0].style, base)
        // `#[default]` 回到传入的 base（status-style），而不是无样式。
        XCTAssertEqual(segments[1].style, base)
        XCTAssertEqual(segments[2].style.fg, StatusBarStyleParser.color("red"))
        // 未覆盖的属性（bg）保持 base。
        XCTAssertEqual(segments[2].style.bg, base.bg)
    }
}

final class StatusBarSnapshotDecodingTests: XCTestCase {
    func testDecodeSnapshotJSON() throws {
        let json = """
        {
          "ok": true,
          "status": {
            "enabled": true,
            "position": "bottom",
            "justify": "left",
            "interval": 15,
            "left": " foo ",
            "right": "#[fg=colour233,bg=colour241,bold] 13/08 ",
            "left_length": 20,
            "right_length": 50,
            "status_style": "bg=green,fg=black",
            "left_style": "default",
            "right_style": "default",
            "separator": " ",
            "window_format": " #I#[fg=colour237]:#[fg=colour250]#W#[fg=colour244]#F ",
            "window_current_format": " #I#[fg=colour250]:#[fg=colour255]#W#[fg=colour50]#F ",
            "window_style": "default",
            "window_current_style": "default",
            "windows": [
              { "window_id": 0, "index": 1, "name": "sleep", "flags": "*", "current": true, "text": " 1#[fg=colour237]:#[fg=colour250]sleep#[fg=colour244]* " }
            ],
            "error": null
          }
        }
        """
        let response = try JSONDecoder().decode(StatusBarResponse.self, from: Data(json.utf8))
        XCTAssertTrue(response.ok)
        let snapshot = try XCTUnwrap(response.status)
        XCTAssertTrue(snapshot.enabled)
        XCTAssertEqual(snapshot.position, "bottom")
        XCTAssertEqual(snapshot.windows.count, 1)
        XCTAssertEqual(snapshot.windows[0].windowId, 0)
        XCTAssertEqual(snapshot.windows[0].index, 1)
        XCTAssertTrue(snapshot.windows[0].current)
        let segments = StatusBarStyleParser.parseInline(text: snapshot.right)
        XCTAssertFalse(segments.isEmpty)
    }
}

final class StatusBarLayoutPolicyTests: XCTestCase {
    /// 左右段封顶 + 窗口列表最小宽度必须能同时放进整条 bar（含边距）。
    func testBudgetFitsInsideBar() {
        let total: CGFloat = 960
        let budget = StatusBarLayoutPolicy.budget(totalWidth: total)
        XCTAssertEqual(budget.leftMax, total * StatusBarLayoutPolicy.sideMaxFraction)
        XCTAssertEqual(budget.rightMax, total * StatusBarLayoutPolicy.sideMaxFraction)
        XCTAssertEqual(budget.windowMin, total * StatusBarLayoutPolicy.windowMinFraction)
        // 36% + 36% + 28% = 100%，左右段与窗口列表不会互相挤没。
        XCTAssertLessThanOrEqual(
            budget.leftMax + budget.rightMax + budget.windowMin,
            total
        )
    }

    /// 窄窗口（最小 480pt）下预算仍然成立，窗口列表至少可见。
    func testBudgetFitsNarrowWindow() {
        let total: CGFloat = 480
        let budget = StatusBarLayoutPolicy.budget(totalWidth: total)
        XCTAssertGreaterThan(budget.windowMin, 100)
        XCTAssertLessThanOrEqual(
            budget.leftMax + budget.rightMax + budget.windowMin,
            total
        )
    }

    /// 非正宽度不产生负预算。
    func testBudgetClampsNonPositiveWidth() {
        let budget = StatusBarLayoutPolicy.budget(totalWidth: 0)
        XCTAssertEqual(budget.leftMax, 0)
        XCTAssertEqual(budget.rightMax, 0)
        XCTAssertEqual(budget.windowMin, 0)
    }
}

final class StatusBarTabOverflowTests: XCTestCase {
    func testManyTabsOverflowInsteadOfCrushingRight() {
        let overflow = StatusBarTabOverflow.overflowCount(
            tabCount: 20,
            barWidth: 720,
            leftWidth: 80
        )
        XCTAssertGreaterThan(overflow, 0, "20 个 tab 在 720pt 下必须溢出，不得把 status-right 挤没")
    }

    func testFewTabsFitWithoutOverflow() {
        let overflow = StatusBarTabOverflow.overflowCount(
            tabCount: 2,
            barWidth: 960,
            leftWidth: 40
        )
        XCTAssertEqual(overflow, 0)
    }

    func testReservedChromeAndRightStayPositive() {
        XCTAssertGreaterThan(StatusBarTabOverflow.statusRightMinWidth, 0)
        XCTAssertGreaterThan(StatusBarTabOverflow.chromeWidth, 0)
        XCTAssertGreaterThan(StatusBarTabOverflow.fixedTabWidth, 0)
    }
}

final class StatusBarAttentionTests: XCTestCase {
    /// 平时是空的：没有「卡在我这里」的工作区就不亮红点。
    func testZeroCountIsInactive() {
        let attention = StatusBarAttention(count: 0)
        XCTAssertFalse(attention.isActive)
    }

    /// 有事才亮：blocked/done 工作区数量 > 0 时显示红点。
    func testPositiveCountIsActive() {
        XCTAssertTrue(StatusBarAttention(count: 1).isActive)
        XCTAssertTrue(StatusBarAttention(count: 3).isActive)
    }

    /// 计数只增不减的防御：负数按 0 处理，绝不出负红点。
    func testNegativeCountClampsToZero() {
        let attention = StatusBarAttention(count: -5)
        XCTAssertEqual(attention.count, 0)
        XCTAssertFalse(attention.isActive)
    }
}

final class StatusBarTabTitleTests: XCTestCase {
    func testIndexAndName() {
        XCTAssertEqual(StatusBarTabTitle.display(index: 1, name: "zsh"), "1  zsh")
        XCTAssertEqual(StatusBarTabTitle.display(index: 2, name: "  "), "2")
        XCTAssertEqual(StatusBarTabTitle.display(index: 3, name: ""), "3")
        XCTAssertFalse(StatusBarTabTitle.display(index: 1, name: "sleep").contains("#["))
    }
}

final class StatusBarFrontendSyncTests: XCTestCase {
    func testWindowIndexLookupUsesStableWindowID() throws {
        let windows = [
            StatusBarWindow(windowId: 47, index: 7, name: "seven", flags: "", current: false, text: ""),
            StatusBarWindow(windowId: 25, index: 6, name: "six", flags: "", current: true, text: ""),
            StatusBarWindow(windowId: 10, index: 1, name: "one", flags: "", current: false, text: ""),
        ]
        let snapshot = StatusBarSnapshot(
            enabled: true,
            position: "bottom",
            justify: "left",
            interval: 15,
            left: "",
            right: "",
            leftLength: 20,
            rightLength: 50,
            statusStyle: "default",
            leftStyle: "default",
            rightStyle: "default",
            separator: " ",
            windowFormat: "",
            windowCurrentFormat: "",
            windowStyle: "default",
            windowCurrentStyle: "default",
            windows: windows,
            error: nil
        )
        XCTAssertEqual(snapshot.windowsByIndex().map(\.index), [1, 6, 7])
        XCTAssertEqual(snapshot.windowID(forIndex: 6), 25)
        XCTAssertEqual(snapshot.windowID(forIndex: 7), 47)
        XCTAssertNil(snapshot.windowID(forIndex: 2))
    }

    /// 解码出的快照 current 标记可变：前端驱动高亮的前提。
    func testDecodedWindowsAreMutable() throws {
        let json = """
        {
          "ok": true,
          "status": {
            "enabled": true,
            "position": "bottom",
            "justify": "left",
            "interval": 15,
            "left": "",
            "right": "",
            "left_length": 20,
            "right_length": 50,
            "status_style": "bg=green,fg=black",
            "left_style": "default",
            "right_style": "default",
            "separator": " ",
            "window_format": " #I:#W ",
            "window_current_format": " #I:#W ",
            "window_style": "default",
            "window_current_style": "default",
            "windows": [
              { "window_id": 0, "index": 1, "name": "a", "flags": "", "current": true, "text": " 1:a " },
              { "window_id": 1, "index": 2, "name": "b", "flags": "", "current": false, "text": " 2:b " }
            ],
            "error": null
          }
        }
        """
        let response = try JSONDecoder().decode(StatusBarResponse.self, from: Data(json.utf8))
        let snapshot = try XCTUnwrap(response.status)
        XCTAssertTrue(snapshot.windows[0].current)
        XCTAssertFalse(snapshot.windows[1].current)

        // 前端切到 tab2：高亮立即移动，不依赖 tmux 查询。
        let switched = snapshot.updatingCurrentWindow(1)
        XCTAssertFalse(switched.windows[0].current)
        XCTAssertTrue(switched.windows[1].current)
    }

    /// tab 关闭后前端本地移除条目，Alt+N 不再指向幽灵 tab。
    func testRemovingWindowDropsTabEntry() throws {
        let json = """
        {
          "ok": true,
          "status": {
            "enabled": true,
            "position": "bottom",
            "justify": "left",
            "interval": 15,
            "left": "",
            "right": "",
            "left_length": 20,
            "right_length": 50,
            "status_style": "bg=green,fg=black",
            "left_style": "default",
            "right_style": "default",
            "separator": " ",
            "window_format": " #I:#W ",
            "window_current_format": " #I:#W ",
            "window_style": "default",
            "window_current_style": "default",
            "windows": [
              { "window_id": 0, "index": 1, "name": "a", "flags": "", "current": true, "text": " 1:a " },
              { "window_id": 3, "index": 2, "name": "c", "flags": "", "current": false, "text": " 2:c " }
            ],
            "error": null
          }
        }
        """
        let response = try JSONDecoder().decode(StatusBarResponse.self, from: Data(json.utf8))
        let snapshot = try XCTUnwrap(response.status)
        let after = snapshot.removingWindow(3)
        XCTAssertEqual(after.windows.map(\.windowId), [0])
    }
}
