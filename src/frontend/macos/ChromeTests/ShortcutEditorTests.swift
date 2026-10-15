import XCTest
@testable import MuxtermChrome

final class ShortcutEditorTests: XCTestCase {
    func testBindAddsChordAndDisableClearsEverything() throws {
        let rows = ShortcutEditor.rows(
            defaults: [(
                action: "new_tab",
                title: "New Tab",
                chords: [ShortcutChord(key: "t", modifiers: ["command"])]
            )],
            overrides: []
        )
        let bound = ShortcutEditor.bind(
            rows,
            action: "new_tab",
            chord: ShortcutChord(key: "n", modifiers: ["command"])
        )
        let next = try XCTUnwrap(bound)
        // core 语义：非空 override 是追加，内置 Cmd-T 仍在。
        XCTAssertEqual(
            next[0].effective,
            [
                ShortcutChord(key: "n", modifiers: ["command"]),
                ShortcutChord(key: "t", modifiers: ["command"]),
            ]
        )
        XCTAssertTrue(next[0].customized)
        XCTAssertFalse(next[0].isDisabled)

        let disabled = ShortcutEditor.disable(next, action: "new_tab")
        XCTAssertTrue(disabled[0].effective.isEmpty)
        XCTAssertTrue(disabled[0].isDisabled)
        let saved = ShortcutEditor.overrideValues(from: disabled)
        XCTAssertEqual(saved.count, 1)
        XCTAssertEqual(saved[0]["action"] as? String, "new_tab")
        XCTAssertEqual((saved[0]["bindings"] as? [[String: Any]])?.count, 0)

        let reset = ShortcutEditor.reset(disabled, action: "new_tab")
        XCTAssertFalse(reset[0].customized)
        XCTAssertEqual(reset[0].effective, [ShortcutChord(key: "t", modifiers: ["command"])])
        XCTAssertTrue(ShortcutEditor.overrideValues(from: reset).isEmpty)
    }

    /// 用户配置里 Linux 时代写下的 Alt override（40 条，全部非空）不能把
    /// macOS 内置的 Cmd 键一起压掉：否则 Cmd-1 / Cmd-[ / Cmd-] 全失效。
    func testPresetStyleOverridesKeepBuiltinChords() {
        let binding1: [String: Any] = ["key": "1", "modifiers": ["alt"]]
        let bindingPrev: [String: Any] = ["key": "[", "modifiers": ["alt"]]
        let bindingNext: [String: Any] = ["key": "]", "modifiers": ["alt"]]
        let item1: [String: Any] = ["action": "switch_tab_1", "bindings": [binding1]]
        let itemPrev: [String: Any] = ["action": "switch_pane_prev", "bindings": [bindingPrev]]
        let itemNext: [String: Any] = ["action": "switch_pane_next", "bindings": [bindingNext]]
        let values: [String: Any] = [
            "shortcuts": [
                "preset": "qwerty",
                "primary_key": "auto",
                "overrides": [item1, itemPrev, itemNext],
            ],
        ]
        let custom = KeyBindingsConfig.parse(values: values)
        let suppressed = KeyBindingsConfig.suppressedActions(in: values)
        XCTAssertTrue(suppressed.isEmpty, "非空 override 不是禁用")
        for chord in [
            KeyChord(command: true, key: "1"),
            KeyChord(command: true, key: "["),
            KeyChord(command: true, key: "]"),
        ] {
            XCTAssertNotNil(
                KeyBindings.action(for: chord, custom: custom, suppressed: suppressed),
                "内置键必须仍然生效: \(chord.key)"
            )
        }
    }

    /// 空 bindings 才是真禁用，连内置键一起压掉。
    func testEmptyOverrideSuppressesBuiltinChord() {
        let empty: [[String: Any]] = []
        let item: [String: Any] = ["action": "switch_pane_next", "bindings": empty]
        let values: [String: Any] = [
            "shortcuts": [
                "overrides": [item],
            ],
        ]
        let custom = KeyBindingsConfig.parse(values: values)
        let suppressed = KeyBindingsConfig.suppressedActions(in: values)
        XCTAssertEqual(suppressed, [.nextPane])
        XCTAssertNil(
            KeyBindings.action(
                for: KeyChord(command: true, key: "]"),
                custom: custom,
                suppressed: suppressed
            )
        )
        XCTAssertNotNil(
            KeyBindings.action(
                for: KeyChord(command: true, key: "["),
                custom: custom,
                suppressed: suppressed
            ),
            "只禁用被显式关闭的那一个动作"
        )
    }

    func testBindRejectsChordUsedByAnotherAction() {
        let rows = ShortcutEditor.rows(
            defaults: [
                ("new_tab", "New Tab", [ShortcutChord(key: "t", modifiers: ["command"])]),
                ("close_pane", "Close Pane", [ShortcutChord(key: "w", modifiers: ["command"])]),
            ],
            overrides: []
        )
        XCTAssertNil(ShortcutEditor.bind(
            rows,
            action: "new_tab",
            chord: ShortcutChord(key: "w", modifiers: ["command"])
        ))
    }

    func testCatalogUsesQwertyPositionForColemakFullscreen() throws {
        let rows = ShortcutCatalog.defaults()
        let fullscreen = try XCTUnwrap(rows.first { $0.action == "toggle_window_fullscreen" })
        XCTAssertTrue(fullscreen.chords.contains(ShortcutChord(key: "f", modifiers: ["command", "shift"])))
        XCTAssertTrue(fullscreen.chords.contains(ShortcutChord(key: "e", modifiers: ["command", "shift"])))
        XCTAssertTrue(fullscreen.chords.contains(ShortcutChord(key: "\r", modifiers: ["command", "shift"])))
        let quickConnect = try XCTUnwrap(rows.first { $0.action == "quick_connect" })
        XCTAssertTrue(quickConnect.chords.contains(ShortcutChord(key: "p", modifiers: ["command"])))
        XCTAssertFalse(quickConnect.chords.contains(ShortcutChord(key: "r", modifiers: ["command"])))
    }

    func testOverrideDisablesBuiltinChord() {
        let custom = KeyBindingsConfig.parse(values: [
            "keybindings": [
                ["key": "q", "mods": ["command"], "action": "quick_connect"],
            ],
            "shortcuts": [
                "overrides": [
                    ["action": "quick_connect", "bindings": [] as [[String: Any]]],
                ],
            ],
        ])
        XCTAssertNil(custom[KeyChord(command: true, key: "q")])
        let suppressed = KeyBindingsConfig.suppressedActions(in: [
            "shortcuts": [
                "overrides": [
                    ["action": "quick_connect", "bindings": [] as [[String: Any]]],
                ],
            ],
        ])
        XCTAssertNil(
            KeyBindings.action(
                for: KeyChord(command: true, key: "p"),
                custom: custom,
                suppressed: suppressed
            )
        )
    }

    /// 同一个 action 出现多条 override 是合法配置（旧 [[keybindings]] 迁移
    /// 出来就是这样），不能让编辑页崩掉。
    func testDuplicateOverrideEntriesDoNotCrash() {
        let rows = ShortcutEditor.rows(
            defaults: [(
                action: "new_pane_vertical",
                title: "New Pane Vertical",
                chords: [ShortcutChord(key: "v", modifiers: ["command"])]
            )],
            overrides: [
                ("new_pane_vertical", [ShortcutChord(key: "v", modifiers: ["command"])]),
                ("new_pane_vertical", [ShortcutChord(key: "d", modifiers: ["command"])]),
            ]
        )
        XCTAssertEqual(rows.count, 1)
        XCTAssertTrue(rows[0].customized)
        // 绑定了 Cmd-D，默认也有 Cmd-V；非空 override 是追加，所以两个都在。
        XCTAssertEqual(
            rows[0].effective,
            [
                ShortcutChord(key: "d", modifiers: ["command"]),
                ShortcutChord(key: "v", modifiers: ["command"]),
            ]
        )
    }
}
