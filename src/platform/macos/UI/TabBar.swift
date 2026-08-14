import AppKit

/// Tab 栏位置（UserDefaults: `muxterm.tabBarPosition`）。
/// 现在控制统一 StatusBar 的位置（顶部/底部）。
enum TabBarPosition: String {
    case top
    case bottom

    static var current: TabBarPosition {
        let raw = UserDefaults.standard.string(forKey: "muxterm.tabBarPosition") ?? "top"
        return TabBarPosition(rawValue: raw) ?? .top
    }

    static func set(_ position: TabBarPosition) {
        UserDefaults.standard.set(position.rawValue, forKey: "muxterm.tabBarPosition")
    }
}
