import AppKit

/// Tab 栏位置。值来自统一 config.toml 的 `[ui] tab_bar_position`，由
/// MainWindow 在启动和切换时通过 Core 事务读写；这里不再使用 UserDefaults。
enum TabBarPosition: String {
    case top
    case bottom
}
