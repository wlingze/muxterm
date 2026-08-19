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

    func testLastSeenTargetClearsWhenCoreSeqIsStale() {
        XCTAssertNil(
            LastSeenNavigation.targetOffset(
                latest: 200,
                seen: 100,
                rawOffset: -1
            ),
            "旧 seq 被淘汰后不能沿用缓存 offset"
        )
    }

    func testLastSeenTargetRequiresNewerLine() {
        XCTAssertNil(
            LastSeenNavigation.targetOffset(
                latest: 100,
                seen: 100,
                rawOffset: 12
            )
        )
        XCTAssertEqual(
            LastSeenNavigation.targetOffset(
                latest: 101,
                seen: 100,
                rawOffset: 12
            ),
            12
        )
    }
}

final class KeyBindingsTests: XCTestCase {
    func testAltZeroSwitchesToLastTab() {
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(option: true, key: "0")),
            .switchLastTab
        )
    }

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

    func testCommandTimelineShortcutsUseOnlyCommandOptionArrows() {
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(command: true, option: true, key: "up")),
            .previousCommand
        )
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(command: true, option: true, key: "down")),
            .nextCommand
        )
        XCTAssertNil(KeyBindings.action(for: KeyChord(command: true, key: "up")))
        XCTAssertNil(KeyBindings.action(for: KeyChord(option: true, key: "down")))
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

    func testCmdFontZoomShortcuts() {
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(command: true, key: "=")),
            .increaseFontSize
        )
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(command: true, shift: true, key: "=")),
            .increaseFontSize
        )
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(command: true, key: "+")),
            .increaseFontSize
        )
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(command: true, key: "-")),
            .decreaseFontSize
        )
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(command: true, key: "0")),
            .resetFontSize
        )
    }

    func testCmdEnterTogglesPaneFullscreen() {
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(command: true, key: "\r")),
            .togglePaneFullscreen
        )
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(command: true, key: "\n")),
            .togglePaneFullscreen
        )
    }

    func testAltEnterTogglesPaneFullscreen() {
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(option: true, key: "\r")),
            .togglePaneFullscreen
        )
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(option: true, key: "\n")),
            .togglePaneFullscreen
        )
        XCTAssertNil(
            KeyBindings.action(for: KeyChord(command: true, option: true, key: "\r"))
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

    /// feed 里即使有终端查询（OSC 10/11、CSI DA 等）也一律丢弃：tmux 自己
    /// 代答查询（颜色来自 `refresh-client -r` 上报），前端回写会被 pane
    /// 回显成 git lg 字面乱码。
    func testTmuxMirrorDropsParserResponseEvenWhenFeedContainsQuery() {
        XCTAssertFalse(TerminalMirrorPolicy.shouldForwardParserResponse(
            duringRemoteOutputFeed: true,
            isTmuxMirror: true
        ))
    }

    /// tmux 镜像：SwiftTerm 解析器应答一律丢弃（不区分 feed 内外）。
    /// 用户按键走另一条 send(source: TerminalView) 通道。
    func testTmuxMirrorDropsParserResponseOutsideFeed() {
        XCTAssertFalse(TerminalMirrorPolicy.shouldForwardParserResponse(
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

final class MuxtermTerminalColorsTests: XCTestCase {
    func testDefaultTerminalColorsAreDarkOnLight() {
        let fg = Int(MuxtermTerminalColors.lightForegroundHex, radix: 16) ?? 0
        let bg = Int(MuxtermTerminalColors.lightBackgroundHex, radix: 16) ?? 0
        let fgLuminance = ((fg >> 16) & 0xff) + ((fg >> 8) & 0xff) + (fg & 0xff)
        let bgLuminance = ((bg >> 16) & 0xff) + ((bg >> 8) & 0xff) + (bg & 0xff)
        // 默认浅色：背景必须比前景亮。
        XCTAssertGreaterThan(bgLuminance, fgLuminance)
        XCTAssertGreaterThan(bgLuminance, 600)
        XCTAssertLessThan(fgLuminance, 200)
    }

    func testThemePaletteAndConfigParsing() {
        XCTAssertEqual(
            MuxtermTerminalColors.palette(forThemeName: nil).fg,
            MuxtermTerminalColors.lightForegroundHex
        )
        let dark = MuxtermTerminalColors.palette(forThemeName: "dark")
        XCTAssertEqual(dark.fg, MuxtermTerminalColors.foregroundHex)
        XCTAssertEqual(dark.bg, MuxtermTerminalColors.backgroundHex)
        let toml = """
        [theme]
        name = "dark"

        [[keybindings]]
        key = "p"
        """
        XCTAssertEqual(MuxtermTerminalColors.themeName(from: toml), "dark")
        XCTAssertNil(MuxtermTerminalColors.themeName(from: "[[keybindings]]\nkey = \"p\""))
    }
}

final class MuxtermThemeTests: XCTestCase {
    func testThemeParsingDefaultsToLight() {
        XCTAssertEqual(MuxtermTheme.from(name: nil), .light)
        XCTAssertEqual(MuxtermTheme.from(name: "Light"), .light)
        XCTAssertEqual(MuxtermTheme.from(name: "dark"), .dark)
        XCTAssertEqual(MuxtermTheme.from(name: "bogus"), .light)
    }

    func testLightThemeIsDefaultPalette() {
        XCTAssertEqual(
            MuxtermTheme.light.palette.fg,
            MuxtermTerminalColors.lightForegroundHex
        )
        XCTAssertEqual(
            MuxtermTheme.dark.palette.bg,
            MuxtermTerminalColors.backgroundHex
        )
        XCTAssertEqual(MuxtermPalette.light.ansi.count, 16)
        XCTAssertEqual(MuxtermPalette.dark.ansi.count, 16)
        XCTAssertEqual(MuxtermPalette.light.cursor, "dc8a78")
        XCTAssertEqual(MuxtermPalette.dark.cursor, "f5e0dc")
        XCTAssertNotEqual(MuxtermPalette.light.ansi[0], MuxtermPalette.dark.ansi[0])
        XCTAssertEqual(MuxtermTerminalColors.activePalette.bg, MuxtermPalette.light.bg)
    }
}

final class MuxtermTerminalFontTests: XCTestCase {
    func testDefaultsFollowAlacrittyStyle18ptMenlo() {
        XCTAssertEqual(MuxtermTerminalFont.defaultFamily, "Menlo")
        XCTAssertEqual(MuxtermTerminalFont.defaultSize, 18)
    }

    func testParseFontSection() {
        let toml = """
        [font]
        family = "JetBrains Mono"
        size = 15.5
        """
        let s = MuxtermTerminalFont.settings(from: toml)
        XCTAssertEqual(s.family, "JetBrains Mono")
        XCTAssertEqual(s.size, 15.5)
    }

    func testFontSizeClampedAndZoomed() {
        XCTAssertEqual(MuxtermTerminalFont.zoomed(18, direction: 1), 19)
        XCTAssertEqual(MuxtermTerminalFont.zoomed(18, direction: -1), 17)
        XCTAssertEqual(MuxtermTerminalFont.zoomed(MuxtermTerminalFont.maxSize, direction: 1), MuxtermTerminalFont.maxSize)
        XCTAssertEqual(MuxtermTerminalFont.zoomed(MuxtermTerminalFont.minSize, direction: -1), MuxtermTerminalFont.minSize)
    }
}

final class MuxtermConfigTests: XCTestCase {
    func testPoolMaxSlotsDefaultAndParse() {
        XCTAssertEqual(MuxtermConfig.poolMaxSlots(from: nil), MuxtermConfig.defaultPoolMaxSlots)
        XCTAssertEqual(MuxtermConfig.poolMaxSlots(from: "[pool]\nmax_slots = 8\n"), 8)
        XCTAssertEqual(MuxtermConfig.poolMaxSlots(from: "[pool]\nmax_slots = 0\n"), MuxtermConfig.defaultPoolMaxSlots)
        XCTAssertEqual(MuxtermConfig.poolMaxSlots(from: "[theme]\nname = \"dark\""), MuxtermConfig.defaultPoolMaxSlots)
    }
}

final class TerminalQueryDetectorTests: XCTestCase {
    private func bytes(_ s: String) -> [UInt8] {
        Array(s.utf8)
    }

    func testDetectsOSCDynamicColorQueries() {
        // codex 启动查询：ESC ] 10 ; ? BEL ESC ] 11 ; ? BEL
        let raw = "\u{1b}]10;?\u{7}\u{1b}]11;?\u{7}"
        let kinds = TerminalQueryDetector.queries(in: bytes(raw))
        XCTAssertEqual(kinds, [.oscDynamicColor(10), .oscDynamicColor(11)])
        XCTAssertTrue(TerminalQueryDetector.containsQuery(in: bytes(raw)))
    }

    func testDetectsOSCQueryWithSTTerminator() {
        let raw = "\u{1b}]12;?\u{1b}\\"
        XCTAssertEqual(
            TerminalQueryDetector.queries(in: bytes(raw)),
            [.oscDynamicColor(12)]
        )
    }

    func testDetectsCSIDeviceAttributes() {
        // ESC [ c 和 ESC [ ? 65 ; ... c（codex 的 DA 查询）
        let raw = "\u{1b}[c\u{1b}[?65;4;1;2;6;21;22;17;28c"
        let kinds = TerminalQueryDetector.queries(in: bytes(raw))
        XCTAssertTrue(kinds.contains(.csiDeviceAttributes))
    }

    func testDetectsCSIDeviceStatus() {
        XCTAssertEqual(
            TerminalQueryDetector.queries(in: bytes("\u{1b}[6n")),
            [.csiDeviceStatus]
        )
    }

    func testDetectsKittyKeyboardQueries() {
        // kitty keyboard：CSI ? u 与 CSI > 4 ; 0 u（codex 的扩展键盘查询）
        let q = "\u{1b}[?u"
        let greater = "\u{1b}[>4;0u"
        XCTAssertTrue(TerminalQueryDetector.containsQuery(in: bytes(q)))
        XCTAssertTrue(TerminalQueryDetector.containsQuery(in: bytes(greater)))
    }

    func testIgnoresPlainOutputWithoutQueries() {
        let raw = "hello world\r\n\u{1b}[31mred\u{1b}[0m"
        XCTAssertFalse(TerminalQueryDetector.containsQuery(in: bytes(raw)))
        XCTAssertTrue(TerminalQueryDetector.queries(in: bytes(raw)).isEmpty)
    }

    func testIgnoresOSCColorSetNotQuery() {
        // OSC 10/11 设置颜色（不是查询）：不触发应答放行
        let raw = "\u{1b}]10;#ffffff\u{7}\u{1b}]11;#000000\u{7}"
        XCTAssertFalse(TerminalQueryDetector.containsQuery(in: bytes(raw)))
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

    func testLiveFeedContinuesWhileReadingHistory() {
        XCTAssertTrue(PaneOutputFeedPolicy.shouldFeedLive(viewport: 0))
        XCTAssertTrue(
            PaneOutputFeedPolicy.shouldFeedLive(viewport: 12),
            "native scrollback 位置不能成为丢弃 live 增量的门禁"
        )
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

    /// Snapshot seed 覆盖的是同一批已经入队的事件；逐事件判断时必须全部跳过，
    /// 不能把同一批终端字节再次喂给 SwiftTerm。
    func testSeededBatchDoesNotReplayAnyEvent() {
        let events: [[UInt8]] = [
            [0x1b, 0x5b, 0x32, 0x30, 0x32, 0x36, 0x68], // CSI ?2026h 的片段
            [0x0f, 0x08, 0x0d], // SO / BS / CR
        ]
        var fed = [[UInt8]]()
        for event in events {
            if PaneOutputFeedPolicy.shouldFeedEvent(
                viewExistedBeforeEvent: false,
                seedCoveredEvent: true
            ) {
                fed.append(event)
            }
        }
        XCTAssertTrue(fed.isEmpty)
    }

    /// 空 snapshot 时首个事件建立 view 后，后续事件仍是增量；批次结束后下一批
    /// 也不能把旧 seed 状态带过去。
    func testEmptySeedAndBatchBoundaryKeepIncrementalEvents() {
        let firstBatch = [
            [UInt8]("first".utf8),
            [0x1b, 0x5b, 0x32, 0x4b, 0x0d],
        ]
        var viewExisted = false
        var fed = [[UInt8]]()
        for event in firstBatch {
            if PaneOutputFeedPolicy.shouldFeedEvent(
                viewExistedBeforeEvent: viewExisted,
                seedCoveredEvent: false
            ) {
                fed.append(event)
            }
            viewExisted = true
        }

        // New poll batch: the view remains alive, so this is a pure increment.
        let secondBatch = [UInt8]("second".utf8)
        XCTAssertTrue(PaneOutputFeedPolicy.shouldFeedEvent(
            viewExistedBeforeEvent: true,
            seedCoveredEvent: false
        ))
        fed.append(secondBatch)

        XCTAssertEqual(
            fed.flatMap { $0 },
            firstBatch.flatMap { $0 } + secondBatch
        )
    }

    /// policy 只决定事件是否喂入，不解释终端控制序列；CR/CSI/SO/ESC 的顺序和
    /// 每个字节都必须原样穿过 policy 层。
    func testPolicyDoesNotMergeOrDropTerminalFrameBytes() {
        let frame: [[UInt8]] = [
            [0x1b, 0x5b, 0x32, 0x4b], // CSI 2K
            [0x0f, 0x1b, 0x5b, 0x31, 0x41], // SO + CSI 1A
            [0x0d, 0x0a, 0x68, 0x74, 0x6f, 0x70], // CRLF + text
        ]
        let fed = frame.filter { _ in
            PaneOutputFeedPolicy.shouldFeedEvent(
                viewExistedBeforeEvent: true,
                seedCoveredEvent: true
            )
        }
        XCTAssertEqual(fed, frame)
        XCTAssertEqual(fed.flatMap { $0 }, frame.flatMap { $0 })
    }
}

final class PanePaintPolicyTests: XCTestCase {
    func testFirstPaintPrefersVisibleGridOverRawHistory() {
        var raw = Data()
        for i in 0..<200 {
            raw.append(contentsOf: Array("line-\(i)\r\n".utf8))
        }
        let visible = Data("\u{1b}[H\u{1b}[2JVISIBLE-TAIL".utf8)
        let painted = PanePaintPolicy.firstPaint(visible: visible, raw: raw, rows: 24)
        let text = String(data: painted, encoding: .utf8) ?? ""
        XCTAssertTrue(text.contains("VISIBLE-TAIL"))
        XCTAssertFalse(text.contains("line-0"), "不得重放 200 行历史。got=\(text.prefix(80))")
    }

    func testFirstPaintHistoryDumpKeepsOnlyLastScreen() {
        var raw = Data()
        for i in 0..<200 {
            raw.append(contentsOf: Array("line-\(i)\r\n".utf8))
        }
        let painted = PanePaintPolicy.firstPaint(visible: Data(), raw: raw, rows: 24)
        let text = String(data: painted, encoding: .utf8) ?? ""
        XCTAssertTrue(text.contains("line-199"), "末屏应含最后一行。got=\(text.suffix(80))")
        XCTAssertFalse(text.contains("line-0"), "末屏不得含最早行（iTerm2 也不会重放）。got=\(text.prefix(80))")
        XCTAssertLessThan(painted.count, raw.count / 2, "首屏字节必须远小于整段 history")
    }

    func testLiveCupStormKeepsOnlyLastFrame() {
        var raw = Data()
        for i in 0..<20 {
            raw.append(contentsOf: Array("\u{1b}[H\u{1b}[2Jframe-\(i)".utf8))
        }
        let painted = PanePaintPolicy.live(raw)
        let text = String(data: painted, encoding: .utf8) ?? ""
        XCTAssertTrue(text.contains("frame-19"))
        XCTAssertFalse(text.contains("frame-0"))
        XCTAssertFalse(text.contains("frame-18"))
    }

    func testLooksLikeHistoryDumpForCaptureReplay() {
        var raw = Data()
        for i in 0..<200 {
            raw.append(contentsOf: Array("line-\(i)\r\n".utf8))
        }
        XCTAssertTrue(
            PanePaintPolicy.looksLikeHistoryDump(raw, rows: 24),
            "200 行 capture 必须当成历史录像"
        )
        var frames = Data()
        for i in 0..<8 {
            frames.append(contentsOf: Array("\u{1b}[H\u{1b}[2Jframe-\(i)".utf8))
        }
        XCTAssertFalse(
            PanePaintPolicy.looksLikeHistoryDump(frames, rows: 24),
            "CUP 风暴是 live TUI，不能当历史丢掉中间帧以外的处理路径"
        )
    }

    func testPaintOfSeededViewKeepsLiveStream() {
        var raw = Data()
        for i in 0..<200 {
            raw.append(contentsOf: Array("line-\(i)\r\n".utf8))
        }
        let visible = Data("\u{1b}[H\u{1b}[2JVISIBLE-TAIL".utf8)
        let painted = PanePaintPolicy.paint(
            seeded: true,
            visible: visible,
            incoming: raw,
            rows: 24
        )
        let text = String(data: painted, encoding: .utf8) ?? ""
        XCTAssertTrue(text.contains("line-0"), "已播种后不得丢掉 live 开头。got=\(text.prefix(80))")
        XCTAssertTrue(text.contains("line-199"), "live 末行必须在。got=\(text.suffix(80))")
        XCTAssertFalse(text.contains("VISIBLE-TAIL"), "已播种后不得用可见网格整屏替换")
    }

    func testLiveStreamingTextIsNotTrimmed() {
        var raw = Data()
        for i in 0..<80 {
            raw.append(contentsOf: Array("https://github.com/example/repo-\(i)\r\n".utf8))
        }
        let painted = PanePaintPolicy.live(raw, visibleRows: 24)
        let text = String(data: painted, encoding: .utf8) ?? ""
        XCTAssertTrue(text.contains("repo-0"), "Codex 刷出的地址不能被 live 裁掉")
        XCTAssertTrue(text.contains("repo-79"))
    }
}

final class PaneHistoryScrollPolicyTests: XCTestCase {
    func testNativeScrollbackOwnsTrackpadAndPageKeys() {
        XCTAssertFalse(
            PaneHistoryScrollPolicy.stealsLiveTrackpad,
            "1124：拦触控板再 RIS 历史 dump 会把 Cursor/htop/echo 弄死"
        )
        XCTAssertFalse(
            PaneHistoryScrollPolicy.stealsLivePageKeys,
            "PageUp 必须留给 htop/Cursor，不能改 viewport"
        )
        XCTAssertFalse(
            PaneHistoryScrollPolicy.shouldReplaceLiveScreen(isSearchJump: false),
            "非搜索跳转不得整屏替换 live"
        )
        XCTAssertFalse(
            PaneHistoryScrollPolicy.shouldReplaceLiveScreen(isSearchJump: true),
            "搜索跳转也必须只移动 native viewport，不能喂历史帧"
        )
    }

    func testWheelUpIncreasesOffsetUntilMax() {
        XCTAssertEqual(
            PaneHistoryScrollPolicy.nextOffset(current: 0, deltaLines: 3, maxOffset: 40),
            3
        )
        XCTAssertEqual(
            PaneHistoryScrollPolicy.nextOffset(current: 38, deltaLines: 10, maxOffset: 40),
            40
        )
        XCTAssertEqual(
            PaneHistoryScrollPolicy.nextOffset(current: 5, deltaLines: -8, maxOffset: 40),
            0
        )
    }

    func testPreciseTrackpadAccumulatesPartialCells() {
        var acc: CGFloat = 0
        XCTAssertEqual(
            PaneHistoryScrollPolicy.lines(
                deltaY: 10,
                precise: true,
                cellHeight: 16,
                accumulator: &acc
            ),
            0
        )
        XCTAssertEqual(acc, 10)
        XCTAssertEqual(
            PaneHistoryScrollPolicy.lines(
                deltaY: 10,
                precise: true,
                cellHeight: 16,
                accumulator: &acc
            ),
            1
        )
        XCTAssertEqual(acc, 4)
    }

    func testPreciseTrackpadHandlesNegativePixelsAndClampsToWholeCells() {
        var acc: CGFloat = 0
        XCTAssertEqual(
            PaneHistoryScrollPolicy.lines(
                deltaY: -8,
                precise: true,
                cellHeight: 16,
                accumulator: &acc
            ),
            0
        )
        XCTAssertEqual(acc, -8)
        XCTAssertEqual(
            PaneHistoryScrollPolicy.lines(
                deltaY: -8,
                precise: true,
                cellHeight: 16,
                accumulator: &acc
            ),
            -1
        )
        XCTAssertEqual(acc, 0)
        XCTAssertEqual(
            PaneHistoryScrollPolicy.nextOffset(current: 0, deltaLines: -1, maxOffset: 40),
            0
        )
        XCTAssertEqual(
            PaneHistoryScrollPolicy.nextOffset(current: 40, deltaLines: 1, maxOffset: 40),
            40
        )
    }
}

final class ColorContrastTests: XCTestCase {
    func testBlackOnWhiteIsUnchanged() {
        let fg = ColorContrast.ensureReadable(fg: "000000", bg: "ffffff")
        XCTAssertEqual(fg, "000000")
        XCTAssertGreaterThan(ColorContrast.contrastRatio(fg: fg, bg: "ffffff"), 10)
    }

    func testBlackOnBlackIsLightened() {
        let fg = ColorContrast.ensureReadable(fg: "000000", bg: "000000")
        XCTAssertNotEqual(fg, "000000", "黑底黑字必须把前景往白推")
        XCTAssertGreaterThan(
            ColorContrast.contrastRatio(fg: fg, bg: "000000"),
            ColorContrast.minimumRatio - 0.05,
            "调整后必须可读。fg=\(fg)"
        )
        let lum = ColorContrast.parse(fg).map { ColorContrast.relativeLuminance($0) } ?? 0
        XCTAssertGreaterThan(lum, 0.05, "前景应明显变亮。fg=\(fg)")
        let rgb = ColorContrast.ensureReadable(
            fg: ColorContrast.RGB(r: 0, g: 0, b: 0),
            bg: ColorContrast.RGB(r: 0, g: 0, b: 0)
        )
        XCTAssertGreaterThan(rgb.r + rgb.g + rgb.b, 0.3)
    }

    func testWhiteOnWhiteIsDarkened() {
        let fg = ColorContrast.ensureReadable(fg: "ffffff", bg: "ffffff")
        XCTAssertNotEqual(fg, "ffffff")
        XCTAssertGreaterThan(
            ColorContrast.contrastRatio(fg: fg, bg: "ffffff"),
            ColorContrast.minimumRatio - 0.05,
            "白底白字必须把前景往黑推。fg=\(fg)"
        )
    }

    func testOscColorsFollowThemeWithoutGrayHack() {
        let osc = ColorContrast.oscColors(fg: "000000", bg: "ffffff")
        XCTAssertEqual(osc.fg, "000000", "浅色 OSC 10 必须是黑，不能报灰污染 tmux session")
        XCTAssertEqual(osc.bg, "ffffff", "浅色背景保持白")
        let dark = ColorContrast.oscColors(fg: "cdd6f4", bg: "1e1e2e")
        XCTAssertEqual(dark.fg, "cdd6f4")
        XCTAssertEqual(dark.bg, "1e1e2e")
    }

    func testLightPaletteContrastedKeepsBlackOnWhite() {
        XCTAssertEqual(MuxtermPalette.light.contrasted().fg, MuxtermPalette.light.fg)
        XCTAssertEqual(MuxtermPalette.light.contrasted().bg, MuxtermPalette.light.bg)
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
        // zoom：单叶覆盖多 pane 快照。
        XCTAssertTrue(
            PaneLayoutProjection.accepts(treePaneIDs: [11], paneIDs: [11, 12, 13])
        )
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

    func testBackgroundTabEventsDoNotReloadCurrentUI() {
        // 当前 tab=2：tab2 的 layout/pane 事件应重建。
        XCTAssertTrue(
            StateEventPolicy.shouldReloadUI(type: 3, tabId: 2, activeTabId: 2)
        )
        XCTAssertTrue(
            StateEventPolicy.shouldReloadUI(type: 4, tabId: 2, activeTabId: 2)
        )
        // 后台 tab=5 的 layout/pane 事件不重建前台（htop 不被其它 tab 刷新干扰）。
        XCTAssertFalse(
            StateEventPolicy.shouldReloadUI(type: 3, tabId: 5, activeTabId: 2)
        )
        XCTAssertFalse(
            StateEventPolicy.shouldReloadUI(type: 4, tabId: 5, activeTabId: 2)
        )
        XCTAssertFalse(
            StateEventPolicy.shouldReloadUI(type: 5, tabId: 5, activeTabId: 2)
        )
        // tab add/close、active tab changed 无条件重建。
        XCTAssertTrue(
            StateEventPolicy.shouldReloadUI(type: 1, tabId: 5, activeTabId: 2)
        )
        XCTAssertTrue(
            StateEventPolicy.shouldReloadUI(type: 6, tabId: 5, activeTabId: 2)
        )
    }
}

final class PaneFullscreenPolicyTests: XCTestCase {
    func testNoFullscreenKeepsNil() {
        XCTAssertEqual(
            PaneFullscreenPolicy.resolvedFullscreenId(fullscreenPaneId: nil, paneIDs: [1, 2]),
            nil
        )
    }

    func testFullscreenResolvesTargetPane() {
        XCTAssertEqual(
            PaneFullscreenPolicy.resolvedFullscreenId(fullscreenPaneId: 2, paneIDs: [1, 2]),
            2
        )
    }

    func testFullscreenIgnoredWhenPaneMissing() {
        XCTAssertEqual(
            PaneFullscreenPolicy.resolvedFullscreenId(fullscreenPaneId: 9, paneIDs: [1]),
            nil
        )
    }
}

final class EventBatchPlanTests: XCTestCase {
    func testStructuralEventDefersOutputs() {
        // 1 = tab add（结构事件），7 = active pane changed，99 = 普通输出。
        XCTAssertTrue(EventBatchPlan.hasStructuralEvent(
            types: [1, 7],
            requiresLayoutReload: StateEventPolicy.requiresLayoutReload
        ))
        XCTAssertTrue(EventBatchPlan.hasStructuralEvent(
            types: [99, 1, 99],
            requiresLayoutReload: StateEventPolicy.requiresLayoutReload
        ))
    }

    func testOutputOnlyBatchFeedsImmediately() {
        XCTAssertFalse(EventBatchPlan.hasStructuralEvent(
            types: [7, 99, 99],
            requiresLayoutReload: StateEventPolicy.requiresLayoutReload
        ))
        XCTAssertFalse(EventBatchPlan.hasStructuralEvent(
            types: [],
            requiresLayoutReload: StateEventPolicy.requiresLayoutReload
        ))
    }
}

final class TabSwitchGateTests: XCTestCase {
    private let t0 = Date(timeIntervalSince1970: 1_000)

    func testRequestBlocksUntilConfirmation() {
        var gate = TabSwitchGate(timeout: 1.5)
        gate.request(tab: 3, now: t0)
        XCTAssertEqual(gate.pendingTab, 3)
        XCTAssertFalse(gate.isReleased(now: t0))

        gate.onTabChanged(to: 3)
        XCTAssertTrue(gate.isReleased(now: t0))
        XCTAssertNil(gate.pendingTab)
    }

    func testTabChangedToDifferentTabKeepsWaiting() {
        var gate = TabSwitchGate(timeout: 1.5)
        gate.request(tab: 4, now: t0)
        gate.onTabChanged(to: 8)
        XCTAssertFalse(gate.isReleased(now: t0))
    }

    func testExternalCloseReleasesImmediately() {
        var gate = TabSwitchGate(timeout: 1.5)
        gate.request(tab: 2, now: t0)
        gate.onTabClosed(2)
        XCTAssertTrue(gate.isReleased(now: t0))
        XCTAssertNil(gate.pendingTab)
    }

    func testSnapshotMissingReleases() {
        var gate = TabSwitchGate(timeout: 1.5)
        gate.request(tab: 5, now: t0)
        gate.onSnapshot(tabs: [1, 5])
        XCTAssertFalse(gate.isReleased(now: t0))
        gate.onSnapshot(tabs: [1])
        XCTAssertTrue(gate.isReleased(now: t0))
        XCTAssertNil(gate.pendingTab)
    }

    func testTimeoutReleases() {
        var gate = TabSwitchGate(timeout: 1.5)
        gate.request(tab: 7, now: t0)
        XCTAssertFalse(gate.isReleased(now: t0.addingTimeInterval(1.0)))
        XCTAssertTrue(gate.isReleased(now: t0.addingTimeInterval(1.6)))
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

    func testEntriesRecentLimitAndProjectFill() {
        let recents = (0..<8).map { i in
            TargetConfig(name: "r\(i)", runtime: .tmux, transport: .local, path: "/x/r\(i)")
        }
        let projects = [
            TargetConfig(name: "p1", runtime: .tmux, transport: .local, path: "/x/p1"),
            TargetConfig(name: "r0", runtime: .tmux, transport: .local, path: "/x/r0"),
        ]
        let entries = QuickConnect.entries(recents: recents, projects: projects)

        // 只取前 5 条 recent（r0..r4），再补 project 独有的 p1。
        XCTAssertEqual(entries.map { $0.config.name }, ["r0", "r1", "r2", "r3", "r4", "p1"])
        // r0 同时在 recent + project → 两个标记。
        XCTAssertEqual(entries[0].badges, [.recent, .project])
        // p1 只 project。
        XCTAssertEqual(entries[5].badges, [.project])
    }

    func testEntriesDedupesByUniqueIDAcrossTransports() {
        let recents = [
            TargetConfig(name: "m", runtime: .tmux, transport: .local, path: "/x/l"),
            TargetConfig(name: "m", runtime: .tmux, transport: .ssh(name: "ryzen"), path: "/x/r"),
        ]
        let projects: [TargetConfig] = []
        let entries = QuickConnect.entries(recents: recents, projects: projects)
        XCTAssertEqual(entries.count, 2) // local 与 ryzen 是不同目标
        XCTAssertEqual(entries.map { $0.config.transport.label }, ["local", "ryzen"])
    }

    func testSshAttachBackendDoesNotPutAliasInSocket() {
        let ssh = TargetTransport.ssh(name: "ryzen").attachBackend
        XCTAssertEqual(ssh.type, "ssh")
        XCTAssertNil(ssh.socket, "Host alias 不得塞进 socket，否则远端 tmux -L ryzen")
        XCTAssertEqual(ssh.sshAlias, "ryzen")

        let local = TargetTransport.local.attachBackend
        XCTAssertEqual(local.type, "tmux")
        XCTAssertNil(local.socket)
        XCTAssertNil(local.sshAlias)
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
        // recents 不落盘：decode 后为空，运行时由连接池 replaceRecents 注入。
        XCTAssertTrue(store2.recents.isEmpty)
        XCTAssertEqual(store2.projects, store.projects)
        XCTAssertEqual(store2.projects.first?.runtime, .shell)
    }

    func testEncodePersistsProjectsOnly() {
        let store = QuickConnectStore()
        store.recordRecent(cfg("recent", "/x/r", transport: .ssh(name: "ryzen")))
        store.upsertProject(cfg("proj", "/x/p", runtime: .shell, transport: .ssh(name: "ryzen")))
        let text = String(data: store.encode(), encoding: .utf8) ?? ""
        XCTAssertTrue(text.contains("[[projects]]"))
        XCTAssertFalse(text.contains("[[recents]]"))
        XCTAssertTrue(text.contains("transport_name = \"ryzen\""))
        XCTAssertFalse(text.contains("{"))
    }

    func testDecodeHandWrittenToml() {
        let toml = """
        # 手写示例
        [[projects]]
        name = "local"
        runtime = "shell"
        transport = "local"
        path = "/tmp/project"
        """
        let store = QuickConnectStore()
        store.decode(Data(toml.utf8))
        XCTAssertTrue(store.recents.isEmpty)
        XCTAssertEqual(store.projects.count, 1)
        XCTAssertEqual(store.projects.first?.runtime, .shell)
        XCTAssertEqual(store.projects.first?.path, "/tmp/project")
    }

    func testDecodeKeepsHyphenatedProjectNames() {
        let toml = """
        [[projects]]
        name = "archmini-home"
        runtime = "tmux"
        transport = "ssh"
        transport_name = "archmini"
        path = "~"

        [[projects]]
        name = "pc-home"
        runtime = "tmux"
        transport = "ssh"
        transport_name = "pc"
        path = "~"

        [[projects]]
        name = "ubuntu-home"
        runtime = "tmux"
        transport = "ssh"
        transport_name = "cd"
        path = "/home/ubuntu"
        """
        let store = QuickConnectStore()
        store.decode(Data(toml.utf8))
        XCTAssertEqual(
            store.projects.map(\.name),
            ["archmini-home", "pc-home", "ubuntu-home"]
        )
    }

    func testReplaceRecentsStaysInMemoryOnly() {
        let store = QuickConnectStore()
        store.replaceRecents([
            cfg("a", "/x/a"),
            cfg("b", "/x/b"),
        ])
        XCTAssertEqual(store.recents.map(\.name), ["a", "b"])
        let text = String(data: store.encode(), encoding: .utf8) ?? ""
        XCTAssertFalse(text.contains("[[recents]]"))
    }

    func testTomlEscapesQuotesAndBackslashes() {
        let store = QuickConnectStore()
        store.upsertProject(cfg("a\"b\\c", "/x/pa\"th\\n", transport: .ssh(name: "r\"y")))
        let data = store.encode()
        let store2 = QuickConnectStore()
        store2.decode(data)
        XCTAssertEqual(store2.projects, store.projects)
        XCTAssertEqual(store2.projects.first?.name, "a\"b\\c")
        XCTAssertEqual(store2.projects.first?.path, "/x/pa\"th\\n")
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

        [[keybindings]]
        key = "0"
        mods = ["option"]
        action = "switch_tab_last"
        """
        let map = KeyBindingsConfig.parse(toml: toml)
        XCTAssertEqual(map[KeyChord(command: true, key: "1")], .switchTab(3))
        XCTAssertEqual(map[KeyChord(command: true, key: "p")], .quickConnect)
        XCTAssertEqual(map[KeyChord(option: true, key: "0")], .switchLastTab)
    }

    func testParseFontZoomBindings() {
        let toml = """
        [[keybindings]]
        key = "="
        mods = ["command"]
        action = "increase_font_size"

        [[keybindings]]
        key = "-"
        mods = ["command"]
        action = "decrease_font_size"

        [[keybindings]]
        key = "0"
        mods = ["command"]
        action = "reset_font_size"
        """
        let map = KeyBindingsConfig.parse(toml: toml)
        XCTAssertEqual(map[KeyChord(command: true, key: "=")], .increaseFontSize)
        XCTAssertEqual(map[KeyChord(command: true, key: "-")], .decreaseFontSize)
        XCTAssertEqual(map[KeyChord(command: true, key: "0")], .resetFontSize)
    }

    func testParsePaneFullscreenBinding() {
        let toml = """
        [[keybindings]]
        key = "\\r"
        mods = ["command"]
        action = "toggle_pane_fullscreen"
        """
        let map = KeyBindingsConfig.parse(toml: toml)
        XCTAssertEqual(map[KeyChord(command: true, key: "\r")], .togglePaneFullscreen)
    }

    func testParseCommandTimelineBindings() {
        let toml = """
        [[keybindings]]
        key = "up"
        mods = ["command", "option"]
        action = "previous_command"

        [[keybindings]]
        key = "down"
        mods = ["command", "option"]
        action = "next_command"
        """
        let map = KeyBindingsConfig.parse(toml: toml)
        XCTAssertEqual(
            map[KeyChord(command: true, option: true, key: "up")],
            .previousCommand
        )
        XCTAssertEqual(
            map[KeyChord(command: true, option: true, key: "down")],
            .nextCommand
        )
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
