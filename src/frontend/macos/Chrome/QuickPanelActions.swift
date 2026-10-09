import Foundation

/// 快速面板里的临时动作列表，交互对齐用户要求的 Cmd-K 快速命令。
///
/// 这不是第四个 `PanelTab`。面板仍是工作区 / 提醒 / 搜索；Cmd-K 只在
/// 同一次打开里换成「当前行的动作 + 已有命令」。Esc 先退出嵌套列表，
/// 再把进入前的查询交还给原来的 tab。
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
