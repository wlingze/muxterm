import XCTest
@testable import MuxtermChrome

final class FlatChromeTests: XCTestCase {
    func testFlatMetricsMatchArchitectureBudget() {
        XCTAssertLessThanOrEqual(FlatChrome.eventPollInterval, 0.02)
        XCTAssertLessThanOrEqual(FlatChrome.tabBarHeight, 24)
        XCTAssertLessThanOrEqual(FlatChrome.statusBarHeight, 20)
        XCTAssertEqual(FlatChrome.splitDividerThickness, 1)
        XCTAssertEqual(FlatChrome.activePaneBorderWidth, 1)
        XCTAssertFalse(FlatChrome.tabBarHeight > 24, "tab 栏不得高于 ARCHITECTURE 预算")
    }

    func testStatusTextKeepsUITestTokens() {
        let text = FlatChrome.statusText(
            status: "connected",
            tabCount: 2,
            paneCount: 3,
            activePane: 12,
            tabsLabel: "tabs",
            panesLabel: "panes",
            paneLabel: "pane"
        )
        XCTAssertTrue(text.contains("connected"))
        XCTAssertTrue(text.contains("tabs: 2"))
        XCTAssertTrue(text.contains("panes: 3"))
        XCTAssertTrue(text.contains("pane: @12"))
        // 去掉冗余「tab: name |」层级
        XCTAssertFalse(text.contains("tab:"))
        XCTAssertFalse(text.contains("|"))
    }
}

final class KeyBindingsTests: XCTestCase {
    func testCmdBracketPaneSwitchMapping() {
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(command: true, key: "[")),
            .prevPane
        )
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(command: true, key: "]")),
            .nextPane
        )
    }

    func testCmdBracketIgnoresShift() {
        XCTAssertNil(KeyBindings.action(for: KeyChord(command: true, shift: true, key: "[")))
        XCTAssertNil(KeyBindings.action(for: KeyChord(command: true, shift: true, key: "]")))
    }

    func testAltBracketAlsoSwitchesPane() {
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(option: true, key: "[")),
            .prevPane
        )
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(option: true, key: "]")),
            .nextPane
        )
    }

    func testCommonCmdShortcutsUnchanged() {
        XCTAssertEqual(KeyBindings.action(for: KeyChord(command: true, key: "t")), .newTab)
        XCTAssertEqual(KeyBindings.action(for: KeyChord(command: true, key: "d")), .splitHorizontal)
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(command: true, shift: true, key: "d")),
            .splitVertical
        )
        XCTAssertEqual(KeyBindings.action(for: KeyChord(command: true, key: "w")), .closeWindow)
        XCTAssertEqual(KeyBindings.action(for: KeyChord(command: true, key: "2")), .switchTab(2))
        XCTAssertNil(
            KeyBindings.action(for: KeyChord(control: true, key: "d")),
            "Ctrl+D 必须留给当前 pane 的前台进程处理 EOF"
        )
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(command: true, shift: true, key: "p")),
            .commandPalette
        )
    }
}


final class PaneOutputCursorTests: XCTestCase {
    func testInitialSnapshotAlreadyContainingEventDoesNotFeedEventTwice() {
        var cursor = PaneOutputCursor()
        let snapshot = Data("prompt% ls\r\n".utf8)
        let event = Data("prompt% ls\r\n".utf8)

        XCTAssertEqual(cursor.initial(snapshot: snapshot), snapshot)
        XCTAssertEqual(cursor.incremental(event: event, snapshot: snapshot), Data())
    }

    func testLaterIncrementalEventIsFedExactlyOnce() {
        var cursor = PaneOutputCursor()
        let snapshot = Data("prompt% ".utf8)
        let event = Data("echo UNIQUE_INPUT\r\nUNIQUE_INPUT\r\n".utf8)

        XCTAssertEqual(cursor.initial(snapshot: snapshot), snapshot)
        XCTAssertEqual(cursor.incremental(event: event, snapshot: snapshot + event), event)
    }

    func testBoundedBufferTrimResetsCursorAndRefeedsTail() {
        var cursor = PaneOutputCursor()
        let old = Data(String(repeating: "x", count: 100).utf8)
        XCTAssertEqual(cursor.initial(snapshot: old), old)

        // 有界缓冲被裁剪后，snapshot 变短：应重置并重放当前尾部，而不是丢弃。
        let trimmed = Data(String(repeating: "y", count: 20).utf8)
        XCTAssertEqual(cursor.incremental(event: trimmed, snapshot: trimmed), trimmed)
    }
}

final class PaneOutputCursorRealLogTests: XCTestCase {
    /// 真实 `ls -la` 回显（a.log 提取）：空格必须逐字节通过增量游标，
    /// 不能变成 `ls-la`；backspace(0x08)、ESC 光标回退也要保留。
    func testLsLaSpacesSurviveIncrementalCursor() {
        var cursor = PaneOutputCursor()
        // 真实回显内容： l s 空格... 空格 [19D
        let snapshot = Data([
            0x08, 0x6c, 0x1b, 0x5b, 0x33, 0x39, 0x6d, 0x73,  // l ESC[39m s
            0x1b, 0x5b, 0x33, 0x39, 0x6d, 0x20,               // ESC[39m ' '
        ])
        let event = Data([0x20, 0x20, 0x20, 0x1b, 0x5b, 0x31, 0x39, 0x44]) // spaces + ESC[19D

        let initial = cursor.initial(snapshot: snapshot)
        XCTAssertEqual(initial, snapshot)
        let inc = cursor.incremental(event: event, snapshot: snapshot + event)
        XCTAssertEqual(inc, event, "增量回显必须逐字节透传，空格不能丢")
        XCTAssertTrue(inc.contains(0x20))
    }

    /// 真实 codex 提示符（a.log 提取）：UTF-8 的 ❯ 符号 + 模式切换 ESC 必须保留，
    /// 不产生 replacement 字符（UTF-8 三字节序列不被 cursor 截断）。
    func testCodexUtf8PromptSurvivesCursorAcrossChunkBoundary() {
        var cursor = PaneOutputCursor()
        // ❯ = 0xE2 0x9D 0xAF；故意把一个多字节 UTF-8 字符拆在两个事件中间
        let first = Data([0x1b, 0x5b, 0x33, 0x35, 0x6d, 0xe2, 0x9d]) // ESC[35m + ❯ 前两字节
        let second = Data([0xaf, 0x1b, 0x5b, 0x39, 0x6d])          // ❯ 末字节 + ESC[39m

        let snapshot = first + second
        let initial = cursor.initial(snapshot: snapshot)
        XCTAssertEqual(initial, snapshot, "initial 必须把整个 snapshot 逐字节透传")
        // 验证 ❯ 的 UTF-8 三字节 (0xE2 0x9D 0xAF) 在透传结果中连续出现，未被截断
        let utf8Triple = Data([0xe2, 0x9d, 0xaf])
        var found = false
        if initial.count >= 3 {
            for i in 0...(initial.count - 3) {
                if initial.subdata(in: i..<(i + 3)) == utf8Triple {
                    found = true
                    break
                }
            }
        }
        XCTAssertTrue(found, "❯ 的 UTF-8 三字节必须完整透传")
    }
}

final class PaneLayoutProjectionTests: XCTestCase {
    func testLayoutMustContainExactlyCurrentTabPanes() {
        XCTAssertTrue(
            PaneLayoutProjection.accepts(treePaneIDs: [11, 13, 12], paneIDs: [11, 12, 13])
        )
        XCTAssertFalse(
            PaneLayoutProjection.accepts(treePaneIDs: [11, 12, 13], paneIDs: [21])
        )
        XCTAssertFalse(
            PaneLayoutProjection.accepts(treePaneIDs: [11, 12], paneIDs: [11, 12, 13])
        )
        XCTAssertFalse(PaneLayoutProjection.accepts(treePaneIDs: [0], paneIDs: [11]))
        XCTAssertTrue(PaneLayoutProjection.accepts(treePaneIDs: [0], paneIDs: [0]))
    }

    func testOnlyStructuralEventsRebuildLayout() {
        for type: UInt32 in [1, 2, 3, 4, 5, 6] {
            XCTAssertTrue(StateEventPolicy.requiresLayoutReload(type), "event=\(type)")
        }
        for type: UInt32 in [0, 7, 8, 9, 10, 99] {
            XCTAssertFalse(StateEventPolicy.requiresLayoutReload(type), "event=\(type)")
        }
        XCTAssertTrue(StateEventPolicy.changesActivePane(7))
        XCTAssertFalse(StateEventPolicy.changesActivePane(3))
    }
}

final class PaneResizeMathTests: XCTestCase {
    func testRatioIsBounded() {
        XCTAssertEqual(PaneResizeMath.clampedRatio(-1), 0.05, accuracy: 0.0001)
        XCTAssertEqual(PaneResizeMath.clampedRatio(2), 0.95, accuracy: 0.0001)
    }

    func testDragKeepsDividerLengthOutOfCollapsedRange() {
        let left = PaneResizeMath.ratioAfterDrag(
            startRatio: 0.5, delta: -10_000, totalLength: 1_000, dividerLength: 6
        )
        let right = PaneResizeMath.ratioAfterDrag(
            startRatio: 0.5, delta: 10_000, totalLength: 1_000, dividerLength: 6
        )
        XCTAssertEqual(left, 0.05, accuracy: 0.0001)
        XCTAssertEqual(right, 0.95, accuracy: 0.0001)
    }

    func testPixelLengthMapsToCharacterCells() {
        XCTAssertEqual(PaneResizeMath.characterCount(pixelLength: 480, cellPixels: 8), 60)
        XCTAssertNil(PaneResizeMath.characterCount(pixelLength: 8, cellPixels: 8))
    }
}

final class TerminalInputEncodingTests: XCTestCase {
    func testCtrlLettersBecomeTerminalControlBytes() {
        let expected: [(String, UInt8)] = [
            ("a", 0x01), ("c", 0x03), ("e", 0x05),
            ("l", 0x0c), ("n", 0x0e), ("p", 0x10), ("r", 0x12),
        ]
        for (key, byte) in expected {
            XCTAssertEqual(TerminalInputEncoding.controlByte(for: key), byte, "Ctrl+\(key)")
        }
    }

    func testControlPunctuationAndAlreadyEncodedBytes() {
        XCTAssertEqual(TerminalInputEncoding.controlByte(for: " "), 0x00)
        XCTAssertEqual(TerminalInputEncoding.controlByte(for: "["), 0x1b)
        XCTAssertEqual(TerminalInputEncoding.controlByte(for: "\\"), 0x1c)
        XCTAssertEqual(TerminalInputEncoding.controlByte(for: "]"), 0x1d)
        XCTAssertEqual(TerminalInputEncoding.controlByte(for: "?"), 0x7f)
        XCTAssertEqual(TerminalInputEncoding.controlByte(for: "\u{03}"), 0x03)
        XCTAssertEqual(TerminalInputEncoding.backspaceByte, 0x7f)
        XCTAssertNil(TerminalInputEncoding.controlByte(for: "中"))
    }
}

final class TerminalQueryReplyTests: XCTestCase {
    /// OSC 10 前景色查询回复必须是 `ESC ] 10 ; rgb:RRRR/GGGG/BBBB ESC \`。
    /// 引导字节（ESC 0x1b、ST ESC\）必须逐字节保留，否则 shell 把 `10;rgb:` 当命令。
    func testOscDynamicColorForeground() {
        let bytes = TerminalQueryReply.oscDynamicColor(code: 10, hex: "000000")
        XCTAssertEqual(
            bytes,
            [0x1b, 0x5d, 0x31, 0x30, 0x3b, 0x72, 0x67, 0x62, 0x3a,
             0x30, 0x30, 0x30, 0x30, 0x2f, 0x30, 0x30, 0x30, 0x30, 0x2f,
             0x30, 0x30, 0x30, 0x30,
             0x1b, 0x5c] // ESC \
        )
    }

    /// OSC 11 背景色查询回复。
    func testOscDynamicColorBackground() {
        let bytes = TerminalQueryReply.oscDynamicColor(code: 11, hex: "ffffff")
        let s = String(bytes: Data(bytes), encoding: .utf8)
        XCTAssertTrue(s?.hasPrefix("\u{1b}]11;rgb:") ?? false, "OSC 11 应以 ESC]11;rgb: 开头: \(String(describing: s))")
        XCTAssertTrue(s?.hasSuffix("\u{1b}\\") ?? false, "应以 ESC\\ (ST) 结尾")
    }

    /// CSI 设备属性（Primary DA）查询回复：`ESC [ ? 65 ; ... c`。
    func testCsiDeviceAttributes() {
        let bytes = TerminalQueryReply.csiDeviceAttributes(attrs: [65, 4, 1, 2, 6, 21, 22, 17, 28])
        let s = String(bytes: Data(bytes), encoding: .utf8)
        XCTAssertTrue(s?.hasPrefix("\u{1b}[?65;4;1;2;6;21;22;17;28c") ?? false,
                      "DA 回复应为 ESC[?65;...c: \(String(describing: s))")
    }

    /// xterm rgb 转换：6 位 hex → RRRR/GGGG/BBBB（每分量 4 位）。
    func testXtermRgbExpandsTo4DigitComponents() {
        XCTAssertEqual(TerminalQueryReply.xtermRgb(fromHex: "000000"), "0000/0000/0000")
        XCTAssertEqual(TerminalQueryReply.xtermRgb(fromHex: "ff0000"), "ffff/0000/0000")
        XCTAssertEqual(TerminalQueryReply.xtermRgb(fromHex: "ffffff"), "ffff/ffff/ffff")
    }
}
