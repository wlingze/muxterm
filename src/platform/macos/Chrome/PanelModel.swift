import Foundation

/// Cmd-P 统一面板的三个 tab（对标 Linux `panel_model::PanelTab`）。
public enum PanelTab: Int, Equatable, Sendable, CaseIterable {
    case workspaces = 0
    case attention = 1
    case search = 2
}

/// 搜索范围：当前 pane / 当前 Workspace / 所有已连接 Workspace。
public enum SearchScope: Equatable, Sendable, CaseIterable {
    case pane
    case workspace
    case all

    /// 对 core 的全局搜索结果做前端范围收窄。
    ///
    /// Workspace ID 是首选边界；`workspacePaneIDs` 仅作为旧 handle 无法提供
    /// Workspace ID 时的兼容回退，避免不同 Workspace 复用 pane 数字时串结果。
    public func filter(
        _ hits: [SearchHit],
        activePane: UInt32?,
        workspaceId: String?,
        workspacePaneIDs: Set<UInt32>
    ) -> [SearchHit] {
        switch self {
        case .pane:
            guard let activePane else { return [] }
            return hits.filter { hit in
                hit.paneId == activePane
                    && (workspaceId == nil || hit.workspaceId == workspaceId)
            }
        case .workspace:
            if let workspaceId {
                return hits.filter { $0.workspaceId == workspaceId }
            }
            return hits.filter { workspacePaneIDs.contains($0.paneId) }
        case .all:
            return hits
        }
    }
}

/// 三 tab 面板纯状态：当前 tab + 共享 query（切 tab 必须保留）。
public struct PanelModel: Equatable, Sendable {
    public var tab: PanelTab
    public var query: String
    public var scope: SearchScope

    public init(
        tab: PanelTab = .workspaces,
        query: String = "",
        scope: SearchScope = .workspace
    ) {
        self.tab = tab
        self.query = query
        self.scope = scope
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
