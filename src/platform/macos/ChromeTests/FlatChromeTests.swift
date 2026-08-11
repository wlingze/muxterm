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


final class TerminalMirrorPolicyTests: XCTestCase {
    /// tmux 镜像在 feed 远端输出期间生成应答：丢弃（git lg 泄漏根因）。
    func testTmuxMirrorDropsParserResponseDuringFeed() {
        XCTAssertFalse(TerminalMirrorPolicy.shouldForwardParserResponse(
            duringRemoteOutputFeed: true,
            isTmuxMirror: true
        ))
    }

    /// tmux 镜像在 feed 之外（鼠标 / 焦点等用户驱动事件）：保持转发。
    func testTmuxMirrorForwardsOutsideFeed() {
        XCTAssertTrue(TerminalMirrorPolicy.shouldForwardParserResponse(
            duringRemoteOutputFeed: false,
            isTmuxMirror: true
        ))
    }

    /// 本地 / daemon 模式（非镜像）：查询应答必须写回 pty，始终转发。
    func testLocalTerminalAlwaysForwardsResponses() {
        XCTAssertTrue(TerminalMirrorPolicy.shouldForwardParserResponse(
            duringRemoteOutputFeed: true,
            isTmuxMirror: false
        ))
        XCTAssertTrue(TerminalMirrorPolicy.shouldForwardParserResponse(
            duringRemoteOutputFeed: false,
            isTmuxMirror: false
        ))
    }
}

final class PaneOutputFeedPolicyTests: XCTestCase {
    /// 视图早已存在：事件就是纯增量，必须喂入（与快照窗口无关）。
    func testExistingViewAlwaysFeedsEvent() {
        XCTAssertTrue(PaneOutputFeedPolicy.shouldFeedEvent(
            viewExistedBeforeEvent: true,
            seedCoveredEvent: true
        ))
        XCTAssertTrue(PaneOutputFeedPolicy.shouldFeedEvent(
            viewExistedBeforeEvent: true,
            seedCoveredEvent: false
        ))
    }

    /// 回归：视图刚创建且播种快照非空（覆盖了后端已入队事件）时，事件必须
    /// 跳过，否则同一批字节双写（输入/回显重复、状态区堆叠）。
    func testNewViewWithSeedSkipsAlreadyCoveredEvents() {
        XCTAssertFalse(PaneOutputFeedPolicy.shouldFeedEvent(
            viewExistedBeforeEvent: false,
            seedCoveredEvent: true
        ))
    }

    /// 新 pane 首批字节：快照为空，没有任何覆盖，事件必须原样喂入。
    func testNewViewWithoutSeedFeedsFirstBytes() {
        XCTAssertTrue(PaneOutputFeedPolicy.shouldFeedEvent(
            viewExistedBeforeEvent: false,
            seedCoveredEvent: false
        ))
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

    /// 非法 cell 尺寸或超界像素应返回 nil，而不是溢出。
    func testCharacterCountRejectsInvalidInput() {
        XCTAssertNil(PaneResizeMath.characterCount(pixelLength: 100, cellPixels: 0))
        XCTAssertNil(PaneResizeMath.characterCount(pixelLength: 100_000_000, cellPixels: 8))
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
        XCTAssertEqual(TerminalInputEncoding.controlByte(for: "^"), 0x1e)
        XCTAssertEqual(TerminalInputEncoding.controlByte(for: "~"), 0x1e)
        XCTAssertEqual(TerminalInputEncoding.controlByte(for: "_"), 0x1f)
        XCTAssertEqual(TerminalInputEncoding.controlByte(for: "/"), 0x1f)
        XCTAssertEqual(TerminalInputEncoding.controlByte(for: "`"), 0x00)
        XCTAssertEqual(TerminalInputEncoding.controlByte(for: "?"), 0x7f)
        XCTAssertEqual(TerminalInputEncoding.controlByte(for: "\u{03}"), 0x03)
        XCTAssertEqual(TerminalInputEncoding.backspaceByte, 0x7f)
        XCTAssertNil(TerminalInputEncoding.controlByte(for: "中"))
    }

    /// xterm C0 别名：Ctrl+2..8 对应 NUL/ESC/FS/GS/RS/US/DEL。
    func testCtrlDigitsMatchXtermC0Aliases() {
        let expected: [(String, UInt8)] = [
            ("2", 0x00), ("3", 0x1b), ("4", 0x1c), ("5", 0x1d),
            ("6", 0x1e), ("7", 0x1f), ("8", 0x7f),
        ]
        for (key, byte) in expected {
            XCTAssertEqual(TerminalInputEncoding.controlByte(for: key), byte, "Ctrl+\(key)")
        }
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

    /// OSC 12 光标色 + #rgb 简写也要按同一格式展开。
    func testOscDynamicColorCursorAndShorthand() {
        let bytes = TerminalQueryReply.oscDynamicColor(code: 12, hex: "#f00")
        let s = String(bytes: Data(bytes), encoding: .utf8)
        XCTAssertEqual(s, "\u{1b}]12;rgb:ffff/0000/0000\u{1b}\\")
    }

    /// data() 应保持字节数组逐字节不变，供 sendInput 直接使用。
    func testDataPreservesReplyBytes() {
        let bytes = TerminalQueryReply.oscDynamicColor(code: 10, hex: "123456")
        XCTAssertEqual(TerminalQueryReply.data(bytes), Data(bytes))
    }
}
