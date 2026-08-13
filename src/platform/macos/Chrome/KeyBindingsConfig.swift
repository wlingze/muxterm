import Foundation

/// 从 `~/.config/muxterm/config.toml` 的 `[[keybindings]]` 解析 macOS 快捷键。
///
/// 格式与 core 的 config.example.toml 一致：
/// ```toml
/// [[keybindings]]
/// key = "d"
/// mods = ["command"]        # command/super、shift、option/alt、control
/// action = "split_pane_vertical"
/// ```
/// 解析结果优先于内置默认键位；未覆盖的键仍走默认。
public enum KeyBindingsConfig {
    /// 支持的 action 名 → KeyAction 映射。
    public static func action(from name: String) -> KeyAction? {
        switch name {
        case "new_window", "new_tab": return .newTab
        case "new_pane": return .splitHorizontal
        case "new_pane_vertical": return .splitVertical
        case "switch_pane_prev": return .prevPane
        case "switch_pane_next": return .nextPane
        case "close_pane": return .closePane
        case "close_window": return .closeWindow
        case "command_palette": return .commandPalette
        case "quick_connect": return .quickConnect
        case "quit": return .quit
        case "increase_font_size": return .increaseFontSize
        case "decrease_font_size": return .decreaseFontSize
        case "reset_font_size": return .resetFontSize
        case "toggle_pane_fullscreen": return .togglePaneFullscreen
        default:
            // switch_tab_N
            if name.hasPrefix("switch_tab_"),
               let n = Int(name.dropFirst("switch_tab_".count)),
               (1...9).contains(n)
            {
                return .switchTab(n)
            }
            return nil
        }
    }

    /// 解析整个 config.toml 文本，返回自定义键位映射。
    public static func parse(toml: String) -> [KeyChord: KeyAction] {
        var result: [KeyChord: KeyAction] = [:]
        // 逐行扫描 [[keybindings]] 块
        var currentKey: String?
        var currentMods: [String] = []
        var currentAction: String?
        var inSection = false

        func flush() {
            guard inSection, let key = currentKey, let actionName = currentAction else {
                return
            }
            guard let action = action(from: actionName) else { return }
            let chord = KeyChord(
                command: currentMods.contains { $0 == "command" || $0 == "super" },
                shift: currentMods.contains("shift"),
                option: currentMods.contains { $0 == "option" || $0 == "alt" },
                control: currentMods.contains("control"),
                key: key
            )
            result[chord] = action
        }

        for rawLine in toml.split(separator: "\n") {
            let line = rawLine.trimmingCharacters(in: .whitespaces)
            if line.hasPrefix("[[keybindings]]") {
                flush()
                inSection = true
                currentKey = nil
                currentMods = []
                currentAction = nil
                continue
            }
            guard inSection else { continue }
            if line.hasPrefix("[") || line.isEmpty {
                continue
            }
            if let eq = line.firstIndex(of: "=") {
                let name = line[..<eq].trimmingCharacters(in: .whitespaces)
                let value = line[line.index(after: eq)...]
                    .trimmingCharacters(in: .whitespaces)
                    .trimmingCharacters(in: CharacterSet(charactersIn: "\""))
                switch name {
                case "key":
                    currentKey = Self.tomlStringValue(value)
                case "mods":
                    // 形如 ["command", "shift"]
                    currentMods = value
                        .trimmingCharacters(in: CharacterSet(charactersIn: "[]"))
                        .split(separator: ",")
                        .map { $0.trimmingCharacters(in: .whitespaces)
                            .trimmingCharacters(in: CharacterSet(charactersIn: "\"")) }
                case "action":
                    currentAction = value
                default:
                    break
                }
            }
        }
        flush()
        return result
    }

    /// 解码 TOML 字符串字面量里的转义（`\r` / `\n` / `\t` / `\\` / `\"`）。
    private static func tomlStringValue(_ raw: String) -> String {
        var s = raw
        if s.hasPrefix("\""), s.hasSuffix("\""), s.count >= 2 {
            s.removeFirst()
            s.removeLast()
        }
        var out = ""
        var i = s.startIndex
        while i < s.endIndex {
            let c = s[i]
            let next = s.index(after: i)
            if c == "\\", next < s.endIndex {
                switch s[next] {
                case "r": out.append("\r")
                case "n": out.append("\n")
                case "t": out.append("\t")
                case "\\": out.append("\\")
                case "\"": out.append("\"")
                default: out.append(c)
                }
                i = next
            } else {
                out.append(c)
            }
            i = s.index(after: i)
        }
        return out
    }

    /// 默认配置文件路径：~/.config/muxterm/config.toml
    public static var defaultConfigURL: URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".config/muxterm/config.toml", isDirectory: false)
    }
}
