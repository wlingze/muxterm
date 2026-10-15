import Foundation

/// 一条可保存的快捷键。修饰键顺序固定，方便比较和显示。
public struct ShortcutChord: Equatable, Sendable {
    public var key: String
    public var modifiers: [String]

    public init(key: String, modifiers: [String]) {
        self.key = key == "\n" ? "\r" : key
        self.modifiers = Self.canonical(modifiers)
    }

    public init(keyChord: KeyChord) {
        var modifiers: [String] = []
        if keyChord.control { modifiers.append("control") }
        if keyChord.option { modifiers.append("option") }
        if keyChord.shift { modifiers.append("shift") }
        if keyChord.command { modifiers.append("command") }
        self.init(key: keyChord.key, modifiers: modifiers)
    }

    public var symbolLabel: String {
        var label = ""
        if modifiers.contains("control") { label += "⌃" }
        if modifiers.contains("option") { label += "⌥" }
        if modifiers.contains("shift") { label += "⇧" }
        if modifiers.contains("command") { label += "⌘" }
        label += Self.keyLabel(key)
        return label
    }

    private static func canonical(_ modifiers: [String]) -> [String] {
        let normalized = Set(modifiers.map { modifier -> String in
            switch modifier {
            case "alt": return "option"
            case "super": return "command"
            default: return modifier
            }
        })
        return ["control", "option", "shift", "command"].filter { normalized.contains($0) }
    }

    private static func keyLabel(_ key: String) -> String {
        switch key {
        case "\r", "\n", "return", "enter": return "↩"
        case "left": return "←"
        case "right": return "→"
        case "up": return "↑"
        case "down": return "↓"
        case "space": return "Space"
        default: return key.uppercased()
        }
    }
}

/// 设置页里的一行快捷键。`customized == false` 时显示内置默认。
public struct ShortcutEditorRow: Equatable, Sendable {
    public var action: String
    public var title: String
    public var defaults: [ShortcutChord]
    public var bindings: [ShortcutChord]
    public var customized: Bool

    /// 实际会触发的按键。非空 override 是**追加**（core 语义），所以内置键
    /// 仍在；空 bindings 才是禁用，内置键也被压掉。
    public var effective: [ShortcutChord] {
        guard customized else { return defaults }
        guard !bindings.isEmpty else { return [] }
        var all = bindings
        for chord in defaults where !all.contains(chord) {
            all.append(chord)
        }
        return all
    }

    /// 是否是显式禁用（空 bindings）。
    public var isDisabled: Bool { customized && bindings.isEmpty }
}

/// 把内置键位和 `shortcuts.overrides` 合成设置页可编辑的列表。
public enum ShortcutEditor {
    public static func rows(
        defaults: [(action: String, title: String, chords: [ShortcutChord])],
        overrides: [(action: String, bindings: [ShortcutChord])]
    ) -> [ShortcutEditorRow] {
        // 同一个 action 可以有多条 override（旧 [[keybindings]] 迁移出来就是
        // 这样），用 uniqueKeysWithValues 建字典会直接崩；后写入的为准。
        var overrideByAction: [String: [ShortcutChord]] = [:]
        for item in overrides {
            overrideByAction[item.action] = item.bindings
        }
        return defaults.map { item in
            if let bindings = overrideByAction[item.action] {
                return ShortcutEditorRow(
                    action: item.action,
                    title: item.title,
                    defaults: item.chords,
                    bindings: bindings,
                    customized: true
                )
            }
            return ShortcutEditorRow(
                action: item.action,
                title: item.title,
                defaults: item.chords,
                bindings: item.chords,
                customized: false
            )
        }
    }

    /// 把某个动作改成一条新快捷键。和其它动作冲突时返回 nil，不改列表。
    public static func bind(
        _ rows: [ShortcutEditorRow],
        action: String,
        chord: ShortcutChord
    ) -> [ShortcutEditorRow]? {
        let conflict = rows.contains { row in
            row.action != action && row.effective.contains(chord)
        }
        guard !conflict else { return nil }
        return rows.map { row in
            guard row.action == action else { return row }
            var next = row
            next.bindings = [chord]
            next.customized = true
            return next
        }
    }

    public static func disable(_ rows: [ShortcutEditorRow], action: String) -> [ShortcutEditorRow] {
        rows.map { row in
            guard row.action == action else { return row }
            var next = row
            next.bindings = []
            next.customized = true
            return next
        }
    }

    public static func reset(_ rows: [ShortcutEditorRow], action: String) -> [ShortcutEditorRow] {
        rows.map { row in
            guard row.action == action else { return row }
            var next = row
            next.bindings = row.defaults
            next.customized = false
            return next
        }
    }

    /// 只写出和默认不同的动作。空 bindings 表示禁用。
    public static func overrideValues(from rows: [ShortcutEditorRow]) -> [[String: Any]] {
        rows.compactMap { row in
            guard row.customized else { return nil }
            let bindings: [[String: Any]] = row.bindings.map { chord in
                ["key": chord.key, "modifiers": chord.modifiers]
            }
            return ["action": row.action, "bindings": bindings]
        }
    }
}

/// macOS 内置快捷键目录。Colemak 说法按同一物理键换成 QWERTY 字符。
public enum ShortcutCatalog {
    public static func defaults() -> [(action: String, title: String, chords: [ShortcutChord])] {
        var grouped: [String: [ShortcutChord]] = [:]
        var order: [String] = []
        for chord in candidateChords {
            guard let action = KeyBindings.action(for: chord),
                  let name = KeyBindingsConfig.name(for: action)
            else { continue }
            if grouped[name] == nil {
                order.append(name)
            }
            let shortcut = ShortcutChord(keyChord: chord)
            if grouped[name]?.contains(shortcut) != true {
                grouped[name, default: []].append(shortcut)
            }
        }
        if grouped["quick_connect"] == nil {
            order.insert("quick_connect", at: min(8, order.count))
            grouped["quick_connect"] = []
        }
        return order.map { name in
            (name, title(for: name), grouped[name] ?? [])
        }
    }

    public static func title(for action: String) -> String {
        action.split(separator: "_").map { part in
            let word = String(part)
            guard let first = word.first else { return word }
            return first.uppercased() + word.dropFirst()
        }.joined(separator: " ")
    }

    /// 覆盖内置表里会出现的键。顺序决定设置页的排列。
    private static let candidateChords: [KeyChord] = [
        KeyChord(command: true, key: "t"),
        KeyChord(command: true, key: "d"),
        KeyChord(command: true, shift: true, key: "s"),
        KeyChord(command: true, key: "s"),
        KeyChord(command: true, shift: true, key: "d"),
        KeyChord(command: true, key: "w"),
        KeyChord(command: true, shift: true, key: "w"),
        KeyChord(command: true, key: "1"),
        KeyChord(command: true, key: "2"),
        KeyChord(command: true, key: "3"),
        KeyChord(command: true, key: "4"),
        KeyChord(command: true, key: "5"),
        KeyChord(command: true, key: "6"),
        KeyChord(command: true, key: "7"),
        KeyChord(command: true, key: "8"),
        KeyChord(command: true, key: "9"),
        KeyChord(command: true, key: "0"),
        KeyChord(command: true, key: "left"),
        KeyChord(command: true, key: "right"),
        KeyChord(command: true, key: "["),
        KeyChord(command: true, key: "]"),
        KeyChord(command: true, option: true, key: "up"),
        KeyChord(command: true, option: true, key: "down"),
        KeyChord(command: true, key: "p"),
        KeyChord(command: true, shift: true, key: "p"),
        KeyChord(command: true, key: "r"),
        KeyChord(command: true, key: "f"),
        KeyChord(command: true, shift: true, key: "f"),
        KeyChord(command: true, key: "\r"),
        KeyChord(option: true, key: "\r"),
        KeyChord(command: true, shift: true, key: "\r"),
        KeyChord(command: true, shift: true, key: "e"),
        KeyChord(command: true, shift: true, key: "t"),
        KeyChord(command: true, shift: true, key: "g"),
        KeyChord(command: true, shift: true, key: "l"),
        KeyChord(command: true, key: "b"),
        KeyChord(command: true, key: ","),
        KeyChord(command: true, control: true, key: "a"),
        KeyChord(command: true, control: true, key: "s"),
        KeyChord(command: true, key: "="),
        KeyChord(command: true, key: "+"),
        KeyChord(command: true, key: "-"),
        KeyChord(command: true, shift: true, key: "0"),
        KeyChord(control: true, key: "0"),
        KeyChord(command: true, control: true, key: "1"),
        KeyChord(command: true, control: true, key: "2"),
        KeyChord(command: true, control: true, key: "3"),
        KeyChord(command: true, control: true, key: "4"),
        KeyChord(command: true, control: true, key: "5"),
        KeyChord(command: true, control: true, key: "6"),
        KeyChord(command: true, control: true, key: "7"),
        KeyChord(command: true, control: true, key: "8"),
        KeyChord(command: true, control: true, key: "9"),
        KeyChord(command: true, control: true, key: "0"),
        KeyChord(control: true, key: "q"),
        KeyChord(option: true, key: "t"),
        KeyChord(option: true, key: "s"),
        KeyChord(option: true, key: "v"),
        KeyChord(option: true, key: "["),
        KeyChord(option: true, key: "]"),
        KeyChord(option: true, key: "0"),
        KeyChord(option: true, key: "1"),
    ]
}
