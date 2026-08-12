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
        XCTAssertEqual(KeyBindings.action(for: KeyChord(command: true, key: "d")), .splitVertical)
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(command: true, shift: true, key: "d")),
            .splitHorizontal
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
        // Cmd-P → QuickConnect（Recent + Project 面板）。
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(command: true, key: "p")),
            .quickConnect
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

final class ScreenTextTests: XCTestCase {
    /// AX 文本必须只反映当前屏幕：行尾去空白、去掉末尾空行，
    /// 不能累积 feed 历史（输入/状态区的中间帧会像堆叠一样残留）。
    func testScreenTextReflectsCurrentScreenOnly() {
        var grid: [[Character]] = [
            ["A", " ", "B", " "],
            [" ", " ", " ", " "],
            ["C", "D", " ", " "],
            [" ", " ", " ", " "],
        ]
        let lines = ScreenText.lines(cols: 4, rows: 4) { x, y in
            grid[y][x]
        }
        XCTAssertEqual(lines, ["A B", "", "CD"])
        // 修改屏幕后重新提取：只反映新屏幕
        grid[0] = ["X", "Y", "Z", " "]
        let lines2 = ScreenText.lines(cols: 4, rows: 4) { x, y in
            grid[y][x]
        }
        XCTAssertEqual(lines2, ["XYZ", "", "CD"])
        XCTAssertFalse(lines2.contains { $0.contains("A B") }, "旧屏幕不得残留")
    }

    func testScreenTextHandlesZeroDims() {
        XCTAssertEqual(ScreenText.lines(cols: 0, rows: 10) { _, _ in " " }, [])
        XCTAssertEqual(ScreenText.lines(cols: 10, rows: 0) { _, _ in " " }, [])
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

final class QuickConnectModelTests: XCTestCase {
    func testDefaultNameUsesPathBasename() {
        XCTAssertEqual(QuickConnect.defaultName(for: "/home/wlz/Developer/self/muxterm"), "muxterm")
        XCTAssertEqual(QuickConnect.defaultName(for: "/Users/me/project"), "project")
    }

    func testDefaultNameFallbackForEmptyOrRoot() {
        XCTAssertEqual(QuickConnect.defaultName(for: ""), "workspace")
        XCTAssertEqual(QuickConnect.defaultName(for: "  "), "workspace")
        XCTAssertEqual(QuickConnect.defaultName(for: "/"), "workspace")
    }

    func testSubtitleShowsRuntimeAtTransport() {
        let local = TargetConfig(name: "m", runtime: .tmux, transport: .local, path: "/x")
        XCTAssertEqual(QuickConnect.subtitle(for: local), "tmux @ local")

        let ssh = TargetConfig(name: "m", runtime: .shell, transport: .ssh(name: "ryzen"), path: "/x")
        XCTAssertEqual(QuickConnect.subtitle(for: ssh), "shell @ ryzen")
    }

    func testShouldAttachTmuxWithNameOnly() {
        let tmux = TargetConfig(name: "sess", runtime: .tmux, transport: .local, path: "/x")
        XCTAssertTrue(QuickConnect.shouldAttach(existingName: "sess", config: tmux))

        // shell runtime 不应 attach（由程序决定）
        let shell = TargetConfig(name: "sess", runtime: .shell, transport: .local, path: "/x")
        XCTAssertFalse(QuickConnect.shouldAttach(existingName: "sess", config: shell))
    }

    func testUniqueIDUsesNameAndTransport() {
        let local = TargetConfig(name: "m", runtime: .tmux, transport: .local, path: "/x")
        XCTAssertEqual(QuickConnect.uniqueID(for: local), "m@local")

        let ssh = TargetConfig(name: "m", runtime: .tmux, transport: .ssh(name: "ryzen"), path: "/x")
        XCTAssertEqual(QuickConnect.uniqueID(for: ssh), "m@ryzen")
    }

    func testBadgesShowRecentAndProjectIndependently() {
        let config = TargetConfig(name: "m", runtime: .tmux, transport: .local, path: "/x")
        let recent = TargetConfig(name: "m", runtime: .tmux, transport: .local, path: "/x")
        let project = TargetConfig(name: "m", runtime: .tmux, transport: .local, path: "/x")

        // 只 recent
        XCTAssertEqual(QuickConnect.badges(for: config, recents: [recent], projects: []), [.recent])
        // 只 project
        XCTAssertEqual(QuickConnect.badges(for: config, recents: [], projects: [project]), [.project])
        // 两者都有
        XCTAssertEqual(
            QuickConnect.badges(for: config, recents: [recent], projects: [project]),
            [.recent, .project]
        )
    }

    func testBadgesMatchByUniqueIDNotFullEquality() {
        // transport 相同但 name 不同的不匹配；transport 不同也不匹配。
        let config = TargetConfig(name: "m", runtime: .tmux, transport: .local, path: "/x")
        let otherPath = TargetConfig(name: "m", runtime: .tmux, transport: .local, path: "/other")
        XCTAssertEqual(QuickConnect.badges(for: config, recents: [otherPath], projects: []), [.recent])

        let ssh = TargetConfig(name: "m", runtime: .tmux, transport: .ssh(name: "ryzen"), path: "/x")
        XCTAssertEqual(QuickConnect.badges(for: config, recents: [ssh], projects: []), [])
    }
}

final class QuickConnectStoreTests: XCTestCase {
    private func cfg(_ name: String, _ path: String, runtime: TargetRuntime = .tmux, transport: TargetTransport = .local) -> TargetConfig {
        TargetConfig(name: name, runtime: runtime, transport: transport, path: path)
    }

    func testRecordRecentDedupesAndMovesToFront() {
        let store = QuickConnectStore()
        store.recordRecent(cfg("b", "/x/b"))
        store.recordRecent(cfg("a", "/x/a"))
        store.recordRecent(cfg("b", "/x/b")) // 去重，b 移到最前
        XCTAssertEqual(store.recents.map { $0.name }, ["b", "a"])
    }

    func testRecordRecentDedupesByUniqueIDNotPath() {
        let store = QuickConnectStore()
        store.recordRecent(cfg("m", "/x/one"))
        store.recordRecent(cfg("m", "/x/two")) // 同名同 transport：按 ID 去重，保留最新 path
        XCTAssertEqual(store.recents.count, 1)
        XCTAssertEqual(store.recents.first?.path, "/x/two")

        store.recordRecent(cfg("m", "/x/three", transport: .ssh(name: "ryzen")))
        XCTAssertEqual(store.recents.count, 2) // 不同 transport：不同 ID，都保留
    }

    func testRecentsBounded() {
        let store = QuickConnectStore()
        for i in 0..<(QuickConnectStore.maxRecent + 10) {
            store.recordRecent(cfg("p\(i)", "/x/p\(i)"))
        }
        XCTAssertEqual(store.recents.count, QuickConnectStore.maxRecent)
        // 最新在最前
        XCTAssertEqual(store.recents.first?.name, "p\(QuickConnectStore.maxRecent + 9)")
    }

    func testUpsertProjectByUniqueName() {
        let store = QuickConnectStore()
        XCTAssertTrue(store.upsertProject(cfg("proj", "/x/proj")))
        XCTAssertFalse(store.upsertProject(cfg("proj", "/x/proj2"))) // 同名更新
        XCTAssertEqual(store.projects.count, 1)
        XCTAssertEqual(store.projects.first?.path, "/x/proj2")
    }

    func testUpsertProjectKeepsSeparateTransports() {
        let store = QuickConnectStore()
        XCTAssertTrue(store.upsertProject(cfg("m", "/x/local")))
        XCTAssertTrue(store.upsertProject(cfg("m", "/x/remote", transport: .ssh(name: "ryzen"))))
        XCTAssertEqual(store.projects.count, 2)
    }

    func testEncodeDecodeRoundTrip() {
        let store = QuickConnectStore()
        store.recordRecent(cfg("recent", "/x/r", transport: .ssh(name: "ryzen")))
        store.upsertProject(cfg("proj", "/x/p", runtime: .shell))
        let data = store.encode()
        let store2 = QuickConnectStore()
        store2.decode(data)
        XCTAssertEqual(store2.recents, store.recents)
        XCTAssertEqual(store2.projects, store.projects)
        XCTAssertEqual(store2.recents.first?.transport, .ssh(name: "ryzen"))
        XCTAssertEqual(store2.projects.first?.runtime, .shell)
    }
}

final class KeyBindingsConfigTests: XCTestCase {
    func testParseBasicBinding() {
        let toml = """
        [[keybindings]]
        key = "d"
        mods = ["command"]
        action = "new_pane_vertical"
        """
        let map = KeyBindingsConfig.parse(toml: toml)
        XCTAssertEqual(map[KeyChord(command: true, key: "d")], .splitVertical)
    }

    func testParseMultiModAndSuperAlias() {
        let toml = """
        [[keybindings]]
        key = "D"
        mods = ["super", "shift"]
        action = "new_pane"
        """
        let map = KeyBindingsConfig.parse(toml: toml)
        XCTAssertEqual(map[KeyChord(command: true, shift: true, key: "d")], .splitHorizontal)
    }

    func testParseSwitchTabAndQuickConnect() {
        let toml = """
        [[keybindings]]
        key = "1"
        mods = ["command"]
        action = "switch_tab_3"

        [[keybindings]]
        key = "p"
        mods = ["command"]
        action = "quick_connect"
        """
        let map = KeyBindingsConfig.parse(toml: toml)
        XCTAssertEqual(map[KeyChord(command: true, key: "1")], .switchTab(3))
        XCTAssertEqual(map[KeyChord(command: true, key: "p")], .quickConnect)
    }

    func testUnknownActionIgnored() {
        let toml = """
        [[keybindings]]
        key = "x"
        mods = ["command"]
        action = "nonsense"
        """
        XCTAssertTrue(KeyBindingsConfig.parse(toml: toml).isEmpty)
    }

    func testCustomTakesPrecedenceOverDefault() {
        let toml = """
        [[keybindings]]
        key = "d"
        mods = ["command"]
        action = "new_pane"
        """
        let custom = KeyBindingsConfig.parse(toml: toml)
        // 默认 Cmd-D = splitVertical；自定义覆盖为 splitHorizontal
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(command: true, key: "d"), custom: custom),
            .splitHorizontal
        )
        // 未覆盖的键仍走默认
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(command: true, key: "t"), custom: custom),
            .newTab
        )
    }
}
