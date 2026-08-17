import Foundation

/// Cmd-P 统一面板的三个 tab（对标 Linux `panel_model::PanelTab`）。
public enum PanelTab: Int, Equatable, Sendable, CaseIterable {
    case workspaces = 0
    case attention = 1
    case search = 2
}

/// 三 tab 面板纯状态：当前 tab + 共享 query（切 tab 必须保留）。
public struct PanelModel: Equatable, Sendable {
    public var tab: PanelTab
    public var query: String

    public init(tab: PanelTab = .workspaces, query: String = "") {
        self.tab = tab
        self.query = query
    }

    public static func open(_ initial: PanelTab) -> PanelModel {
        PanelModel(tab: initial, query: "")
    }

    /// Tab / Shift+Tab 循环：Workspaces → Attention → Search → Workspaces。
    public mutating func cycleTab(back: Bool) {
        let n = PanelTab.allCases.count
        let delta = back ? n - 1 : 1
        let next = (tab.rawValue + delta) % n
        tab = PanelTab(rawValue: next) ?? .workspaces
    }
}
