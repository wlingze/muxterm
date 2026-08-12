import Foundation

/// 目录输入 / 候选选择的纯路径模型。
///
/// 关键约束：
/// - 输入最后一段是补全前缀，列表请求针对**父目录**；
/// - 尾部 `/` 表示已确定进入该目录，不再把完整路径当 basename；
/// - 选择候选只替换当前输入段（= 进入该目录），绝不重复拼接 basename；
/// - `~`、`/`、`.`、`..` 与空输入按目录语义归一化。
public enum DirectoryPathModel {
    /// 列表请求应针对的目录。
    public static func baseDirectory(for raw: String) -> String {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty { return "~" }
        if trimmed == "~" || trimmed == "~/" { return "~" }
        if trimmed == "/" { return "/" }
        if trimmed.hasPrefix("~/") {
            let rest = String(trimmed.dropFirst(2))
            if hasTrailingSlash(rest) {
                return "~/" + trimmingTrailingSlashes(rest)
            }
            let parent = parentPath(rest)
            return parent.isEmpty ? "~" : "~/" + parent
        }
        if trimmed.hasPrefix("/") {
            return hasTrailingSlash(trimmed)
                ? trimmingTrailingSlashes(trimmed)
                : parentPath(trimmed)
        }
        // 相对路径（如 "foo"）：父目录是当前目录。
        return hasTrailingSlash(trimmed)
            ? trimmingTrailingSlashes(trimmed)
            : "."
    }

    /// 当前输入的最后一节（补全过滤前缀）。
    public static func inputPrefix(for raw: String) -> String {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty || trimmed == "~" || trimmed == "/" { return "" }
        if hasTrailingSlash(trimmed) { return "" }
        let normalized = normalizedComponents(trimmed)
        return normalized.last ?? ""
    }

    /// 选择候选 = 进入该目录：候选**替换当前输入段**，绝不拼到完整路径上。
    /// candidate 必须是纯目录名（不含 `/`）。
    public static func applyingSelection(candidate: String, to raw: String) -> String {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty { return candidate + "/" }
        let base = baseDirectory(for: trimmed)
        switch base {
        case "/": return "/" + candidate + "/"
        case "~": return "~/" + candidate + "/"
        case ".": return candidate + "/"
        default: return base + "/" + candidate + "/"
        }
    }

    /// 上级目录。
    public static func applyingGoUp(to raw: String) -> String {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty { return "~" }
        if trimmed == "~" || trimmed == "~/" { return "~" }
        if trimmed == "/" { return "/" }
        let withoutTrailing = trimmingTrailingSlashes(trimmed)
        if withoutTrailing.isEmpty { return "/" }
        if withoutTrailing.hasPrefix("~/") {
            let rest = String(withoutTrailing.dropFirst(2))
            let parent = parentPath(rest)
            return parent.isEmpty ? "~/" : "~/" + parent + "/"
        }
        if withoutTrailing.hasPrefix("/") {
            let parent = parentPath(withoutTrailing)
            return parent.isEmpty || parent == "/" ? "/" : parent + "/"
        }
        let parent = parentPath(withoutTrailing)
        return parent.isEmpty ? "." : parent + "/"
    }

    /// 归一化路径：去尾斜杠、处理 `.` / `..`；空 → `~`。
    public static func resolvedPath(for raw: String) -> String {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty { return "~" }
        if trimmed == "/" { return "/" }
        if trimmed == "~" || trimmed == "~/" { return "~" }

        let isTilde = trimmed.hasPrefix("~/")
        let body = isTilde ? String(trimmed.dropFirst(2)) : trimmed
        var stack: [String] = []
        let isAbsolute = body.hasPrefix("/")
        for component in body.split(separator: "/") {
            switch component {
            case ".":
                continue
            case "..":
                if !stack.isEmpty && stack.last != ".." {
                    stack.removeLast()
                } else if !isAbsolute && !isTilde {
                    stack.append("..")
                }
            default:
                stack.append(String(component))
            }
        }
        let joined = stack.joined(separator: "/")
        if isTilde {
            return joined.isEmpty ? "~" : "~/" + joined
        }
        if isAbsolute {
            return "/" + joined
        }
        return joined.isEmpty ? "." : joined
    }

    // MARK: - Private

    private static func hasTrailingSlash(_ value: String) -> Bool {
        value.hasSuffix("/")
    }

    private static func trimmingTrailingSlashes(_ value: String) -> String {
        var result = value
        while result.hasSuffix("/") {
            result.removeLast()
        }
        return result
    }

    private static func parentPath(_ value: String) -> String {
        let parts = value.split(separator: "/", omittingEmptySubsequences: true)
        guard parts.count > 1 else { return "" }
        let joined = parts.dropLast().joined(separator: "/")
        return value.hasPrefix("/") ? "/" + joined : joined
    }

    private static func normalizedComponents(_ value: String) -> [String] {
        value.split(separator: "/", omittingEmptySubsequences: true).map(String.init)
    }
}

/// 一次目录列表请求的完整标识：generation + 请求 key。
public struct DirectoryListingRequest: Equatable {
    public let generation: UInt64
    public let path: String
    public let isSSH: Bool
    public let alias: String?

    public init(generation: UInt64, path: String, isSSH: Bool, alias: String?) {
        self.generation = generation
        self.path = path
        self.isSSH = isSSH
        self.alias = alias
    }
}

/// 异步目录列表响应。只有请求与当前请求完全一致的响应才允许应用。
public struct DirectoryListingResponse: Equatable {
    public let request: DirectoryListingRequest
    public let directories: [String]

    public init(request: DirectoryListingRequest, directories: [String]) {
        self.request = request
        self.directories = directories
    }
}

/// 目录补全控制器（纯逻辑）：管理当前输入、请求 generation 与候选应用。
///
/// 异步调用方必须在请求发出前调用 `request` / `updateInput` / `select` /
/// `setTransport`，拿到请求快照；响应回来时调用 `apply`，旧的 generation /
/// path / transport / alias 响应全部丢弃。
public struct DirectorySuggestionController {
    public private(set) var text: String
    public private(set) var isSSH: Bool
    public private(set) var alias: String?
    public private(set) var candidates: [String] = []
    private var generation: UInt64 = 0

    public init(path: String = "~") {
        self.text = path
        self.isSSH = false
        self.alias = nil
    }

    /// 当前请求快照（path = 父目录/当前目录，不含输入前缀）。
    public var request: DirectoryListingRequest {
        DirectoryListingRequest(
            generation: generation,
            path: DirectoryPathModel.baseDirectory(for: text),
            isSSH: isSSH,
            alias: isSSH ? alias : nil
        )
    }

    /// 输入变化：更新文本、作废旧候选与旧请求，返回新请求。
    @discardableResult
    public mutating func updateInput(_ newText: String) -> DirectoryListingRequest {
        text = newText.trimmingCharacters(in: .whitespaces)
        invalidate()
        return request
    }

    /// 选择候选：仅接受纯目录名；进入该目录并返回新请求。
    @discardableResult
    public mutating func select(candidate: String) -> DirectoryListingRequest {
        let trimmed = candidate.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty, !trimmed.contains("/") else {
            return request
        }
        // 已进入同名字目录（尾斜杠）：重复选择保持幂等，不再次拼接。
        if lastEnteredComponent(of: text) == trimmed {
            return request
        }
        let next = DirectoryPathModel.applyingSelection(candidate: trimmed, to: text)
        guard next != text else { return request }
        text = next
        invalidate()
        return request
    }

    /// 上级目录。
    @discardableResult
    public mutating func goUp() -> DirectoryListingRequest {
        text = DirectoryPathModel.applyingGoUp(to: text)
        invalidate()
        return request
    }

    /// transport / SSH alias 变化：作废旧候选，返回新请求。
    @discardableResult
    public mutating func setTransport(isSSH: Bool, alias: String? = nil) -> DirectoryListingRequest {
        self.isSSH = isSSH
        self.alias = isSSH ? alias : nil
        invalidate()
        return request
    }

    /// 应用异步响应。只有与当前请求完全一致（generation + path + transport + alias）
    /// 的响应才会更新候选；否则丢弃。
    @discardableResult
    public mutating func apply(_ response: DirectoryListingResponse) -> Bool {
        guard response.request == request else { return false }
        let prefix = DirectoryPathModel.inputPrefix(for: text)
        candidates = response.directories
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty && !$0.contains("/") }
            .filter { prefix.isEmpty || $0.hasPrefix(prefix) }
            .sorted()
        return true
    }

    private mutating func invalidate() {
        generation &+= 1
        candidates = []
    }

    private func lastEnteredComponent(of raw: String) -> String? {
        let trimmed = raw.trimmingCharacters(in: .whitespaces)
        guard trimmed.hasSuffix("/"), trimmed != "/", trimmed != "~/" else { return nil }
        var withoutTrailing = trimmed
        while withoutTrailing.hasSuffix("/") {
            withoutTrailing.removeLast()
        }
        if withoutTrailing == "~" { return nil }
        return withoutTrailing.split(separator: "/").last.map(String.init)
    }
}
