import Foundation

/// 快速面板里的临时动作列表，交互对齐 Raycast 的 Cmd-K。
///
/// 这不是第四个 `PanelTab`，也不替换工作区 / 提醒 / 搜索列表。Cmd-K 在
/// 面板右下角叠一个小动作菜单，只放当前选中行的动作（打开、关闭、更新、attach、跳转等）。
/// 不带命令面板的全局命令。Esc 先退出嵌套列表，再关掉菜单，原来的查询和 tab 保持不动。
public struct QuickPanelCommand: Equatable, Sendable {
    public let id: String
    public let title: String
    public let detail: String
    public let keywords: String
    public let section: QuickPanelCommandSection

    public init(
        id: String,
        title: String,
        detail: String,
        keywords: String,
        section: QuickPanelCommandSection
    ) {
        self.id = id
        self.title = title
        self.detail = detail
        self.keywords = keywords
        self.section = section
    }

    public var searchText: String {
        "\(title) \(detail) \(keywords)".lowercased()
    }
}

public enum QuickPanelCommandSection: Equatable, Sendable {
    case contextual
    case global
}

/// 进入动作列表时，当前高亮行能提供哪些行内动作。
public enum QuickPanelRowContext: Equatable, Sendable {
    case workspace(name: String, closable: Bool, canUpdate: Bool)
    case target(name: String)
    case existingConnection(name: String)
    case newProject
    case attention(title: String, hidden: Bool)
    case search(line: String)
    case none
}

/// 行内动作的可见文案由调用方提供，Chrome 不依赖 AppKit / i18n。
public struct QuickPanelActionLabels: Equatable, Sendable {
    public var open: String
    public var close: String
    public var update: String
    public var connect: String
    public var edit: String
    public var create: String
    public var attach: String
    public var jump: String
    public var hide: String
    public var show: String
    public var openAttention: String

    public init(
        open: String,
        close: String,
        update: String,
        connect: String,
        edit: String,
        create: String,
        attach: String,
        jump: String,
        hide: String,
        show: String,
        openAttention: String
    ) {
        self.open = open
        self.close = close
        self.update = update
        self.connect = connect
        self.edit = edit
        self.create = create
        self.attach = attach
        self.jump = jump
        self.hide = hide
        self.show = show
        self.openAttention = openAttention
    }
}

public enum QuickPanelActionEscape: Equatable, Sendable {
    /// 还在动作列表里（从语言等嵌套列表退回上一层）。
    case stay
    /// 退出动作列表，调用方应把面板查询恢复成这个值。
    case leave(savedQuery: String)
}

public enum QuickPanelActions {
    public static let open = "row.open"
    public static let close = "row.close"
    public static let update = "row.update"
    public static let connect = "row.connect"
    public static let edit = "row.edit"
    public static let create = "row.create"
    public static let attach = "row.attach"
    public static let jump = "row.jump"
    public static let hide = "row.hide"
    public static let show = "row.show"
    public static let openAttention = "row.openAttention"

    public static func contextual(
        for row: QuickPanelRowContext,
        labels: QuickPanelActionLabels
    ) -> [QuickPanelCommand] {
        switch row {
        case .workspace(let name, let closable, let canUpdate):
            var commands = [
                make(
                    id: open,
                    title: labels.open,
                    detail: name,
                    keywords: "open activate switch workspace 打开 切换 工作区"
                ),
            ]
            if closable {
                commands.append(
                    make(
                        id: close,
                        title: labels.close,
                        detail: name,
                        keywords: "close workspace 关闭 工作区"
                    )
                )
            }
            if canUpdate {
                commands.append(
                    make(
                        id: update,
                        title: labels.update,
                        detail: name,
                        keywords: "update runtime handoff reconnect 更新 运行时"
                    )
                )
            }
            return commands
        case .target(let name):
            return [
                make(
                    id: connect,
                    title: labels.connect,
                    detail: name,
                    keywords: "connect open target 连接 打开"
                ),
                make(
                    id: edit,
                    title: labels.edit,
                    detail: name,
                    keywords: "edit project target 编辑 项目"
                ),
            ]
        case .existingConnection(let name):
            return [
                make(
                    id: attach,
                    title: labels.attach,
                    detail: name,
                    keywords: "attach existing session 接入 已有"
                ),
            ]
        case .newProject:
            return [
                make(
                    id: create,
                    title: labels.create,
                    detail: "",
                    keywords: "new project create 新建 项目"
                ),
            ]
        case .attention(let title, let hidden):
            return [
                make(
                    id: jump,
                    title: labels.jump,
                    detail: title,
                    keywords: "jump attention pane 跳转 提醒"
                ),
                make(
                    id: openAttention,
                    title: labels.openAttention,
                    detail: title,
                    keywords: "open preview attention 打开 预览"
                ),
                make(
                    id: hidden ? show : hide,
                    title: hidden ? labels.show : labels.hide,
                    detail: title,
                    keywords: "hide show attention visibility 隐藏 显示 提醒"
                ),
            ]
        case .search(let line):
            return [
                make(
                    id: jump,
                    title: labels.jump,
                    detail: line,
                    keywords: "jump search hit 跳转 搜索"
                ),
            ]
        case .none:
            return []
        }
    }

    /// 行内动作在前。全局命令沿用调用方给的顺序，重复 id 丢掉。
    public static func merge(
        contextual: [QuickPanelCommand],
        global: [QuickPanelCommand]
    ) -> [QuickPanelCommand] {
        var seen = Set(contextual.map(\.id))
        var merged = contextual
        for command in global where seen.insert(command.id).inserted {
            merged.append(command)
        }
        return merged
    }

    /// 空白查询保留原顺序。多个词都要出现在标题、说明或关键字里。
    public static func filter(
        _ commands: [QuickPanelCommand],
        query: String
    ) -> [QuickPanelCommand] {
        let tokens = query
            .lowercased()
            .split(whereSeparator: \.isWhitespace)
            .map(String.init)
        guard !tokens.isEmpty else { return commands }
        return commands.filter { command in
            let haystack = command.searchText
            return tokens.allSatisfy { haystack.contains($0) }
        }
    }

    private static func make(
        id: String,
        title: String,
        detail: String,
        keywords: String
    ) -> QuickPanelCommand {
        QuickPanelCommand(
            id: id,
            title: title,
            detail: detail,
            keywords: keywords,
            section: .contextual
        )
    }
}

/// Cmd-K 之后的面板状态。`savedQuery` 是进入前的面板查询，动作过滤
/// 写在 `query` 里，两者不能混用。
public struct QuickPanelActionSession: Equatable, Sendable {
    public let savedQuery: String
    public private(set) var query: String
    public private(set) var commands: [QuickPanelCommand]
    private var parentQuery: String?
    private var parentCommands: [QuickPanelCommand]?

    public init(savedQuery: String, commands: [QuickPanelCommand]) {
        self.savedQuery = savedQuery
        self.query = ""
        self.commands = commands
    }

    public var visibleCommands: [QuickPanelCommand] {
        QuickPanelActions.filter(commands, query: query)
    }

    public var isNested: Bool { parentCommands != nil }

    public mutating func setQuery(_ query: String) {
        self.query = query
    }

    /// 语言等二级列表。Esc 回到推入前的动作列表和过滤词。
    public mutating func push(_ commands: [QuickPanelCommand]) {
        parentQuery = query
        parentCommands = self.commands
        query = ""
        self.commands = commands
    }

    public mutating func escape() -> QuickPanelActionEscape {
        if let parentCommands {
            commands = parentCommands
            query = parentQuery ?? ""
            self.parentCommands = nil
            parentQuery = nil
            return .stay
        }
        return .leave(savedQuery: savedQuery)
    }
}

/// Cmd-K 动作菜单贴在快速面板右下角，盖住列表的一角，不占满面板。
///
/// AppKit 原点在左下，所以「右下」是 `maxX` 附近、`minY` 附近。
public enum QuickActionPopoverLayout {
    public static let width: CGFloat = 300
    public static let maxVisibleRows = 7
    public static let rowHeight: CGFloat = 28
    /// 标题、搜索框和内边距，不含动作行。
    public static let chromeHeight: CGFloat = 76
    public static let margin: CGFloat = 8

    public static func size(rowCount: Int) -> CGSize {
        let rows = min(max(rowCount, 1), maxVisibleRows)
        return CGSize(width: width, height: chromeHeight + CGFloat(rows) * rowHeight)
    }

    public static func frame(in bounds: CGRect, rowCount: Int) -> CGRect {
        let fitted = size(rowCount: rowCount)
        let width = min(fitted.width, max(160, bounds.width - margin * 2))
        let height = min(fitted.height, max(chromeHeight, bounds.height - margin * 2))
        return CGRect(
            x: bounds.maxX - margin - width,
            y: bounds.minY + margin,
            width: width,
            height: height
        )
    }
}

/// SwiftTerm `processSizeChange` 用 `ceil(raw * scale) / scale` 当字符宽。
/// 再 `round(cell * scale) / scale` 会把格子变窄，同一块像素会多出几列。
/// Herdr 上这两套列数会来回 resize，TUI 被清成默认背景后停在白屏。
public enum TerminalCellGrid {
    public static func snapped(rawWidth: CGFloat, rawHeight: CGFloat, scale: CGFloat) -> (width: CGFloat, height: CGFloat) {
        let scale = max(scale, 1)
        return (
            width: max(ceil(rawWidth * scale) / scale, 1),
            height: max(ceil(rawHeight * scale) / scale, 1)
        )
    }

    /// 错误的二次取整。测试用它证明会比 `snapped` 多出列。
    public static func pixelRoundedWidth(snappedWidth: CGFloat, scale: CGFloat) -> CGFloat {
        let scale = max(scale, 1)
        return max(CGFloat(Int((snappedWidth * scale).rounded())) / scale, 1)
    }

    public static func columns(width: CGFloat, cellWidth: CGFloat) -> Int {
        guard width > 0, cellWidth > 0 else { return 0 }
        return Int(width / cellWidth)
    }

    /// 列数乘字符宽不得超过可用长度。浮点误差多出的一列会把最右侧字形裁掉。
    public static func fittingCount(length: CGFloat, cell: CGFloat) -> Int {
        guard length > 0, cell > 0 else { return 0 }
        var count = Int(length / cell)
        while count > 0, CGFloat(count) * cell > length + 0.01 {
            count -= 1
        }
        return count
    }
}
