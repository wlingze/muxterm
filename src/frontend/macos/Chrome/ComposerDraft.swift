import Foundation

/// 独立输入框绑定的目标终端。
///
/// 输入框只负责写文字；真正发送仍走当前 pane 的普通输入通道。`kind` 让
/// 用户在输入前就知道这段话会落在本地还是 SSH 机器上。
public enum ComposerTargetKind: Equatable, Sendable {
    case local
    case ssh
}

public struct ComposerTarget: Equatable, Sendable {
    public let kind: ComposerTargetKind
    /// SSH 别名；本地为空串。
    public let host: String
    public let workspaceName: String
    /// Core workspace id；发送与图片上传都用它定位。
    public let workspaceID: String?
    public let paneID: UInt32?

    public init(
        kind: ComposerTargetKind,
        host: String,
        workspaceName: String,
        workspaceID: String?,
        paneID: UInt32?
    ) {
        self.kind = kind
        self.host = host
        self.workspaceName = workspaceName
        self.workspaceID = workspaceID
        self.paneID = paneID
    }

    /// 「Local」/「SSH」标记；SSH 不带别名，徽章保持短。
    public var badge: String {
        switch kind {
        case .local: return "Local"
        case .ssh: return "SSH"
        }
    }

    /// 徽章旁常驻的完整去向：SSH 显示别名，本地只显示工作区。
    public var detail: String {
        switch kind {
        case .local:
            return workspaceName
        case .ssh:
            return host.isEmpty ? workspaceName : "\(host) · \(workspaceName)"
        }
    }
}

/// 输入框的内容状态。
///
/// 发送时永远只发送用户写的原文：粘贴图片插入的是**纯路径**，不加引号、
/// 不加 Markdown 图片语法、不自动回车。Markdown 只给原文上色，不另开预览、不改要发送的字。
public struct ComposerDraft: Equatable, Sendable {
    public private(set) var text: String

    public init(text: String = "") {
        self.text = text
    }

    public var isBlank: Bool {
        text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    /// 要发给终端的字节内容；空内容返回 nil，让调用方保持面板不动。
    public func payload() -> String? {
        isBlank ? nil : text
    }

    /// 在 UTF-16 选区处插入图片路径，返回新文本与新光标（均是 UTF-16 偏移）。
    ///
    /// 只在需要分隔时才补空格：开头不补前导空格，末尾不补尾随空格，避免
    /// 发送文本里多出不可见的结尾空白。
    public static func insertingImagePath(
        _ path: String,
        into text: String,
        selectedRange: NSRange
    ) -> (text: String, caret: Int) {
        let trimmed = path.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return (text, selectedRange.location) }
        let ns = text as NSString
        let range = ns.clampedRange(selectedRange)
        let before = range.location > 0 ? ns.substring(with: NSRange(location: range.location - 1, length: 1)) : ""
        let afterLocation = range.location + range.length
        let after = afterLocation < ns.length ? ns.substring(with: NSRange(location: afterLocation, length: 1)) : ""
        let needsLeadingSpace = !before.isEmpty && !before.first!.isWhitespace
        let needsTrailingSpace = !after.isEmpty && !after.first!.isWhitespace
        let insertion = (needsLeadingSpace ? " " : "") + trimmed + (needsTrailingSpace ? " " : "")
        let result = ns.replacingCharacters(in: range, with: insertion)
        return (result, range.location + (insertion as NSString).length)
    }
}

private extension NSString {
    /// 输入框回调里的选区偶尔会比文本长一帧；夹到合法范围再替换。
    func clampedRange(_ range: NSRange) -> NSRange {
        let location = min(max(range.location, 0), length)
        let available = length - location
        let count = min(max(range.length, 0), available)
        return NSRange(location: location, length: count)
    }
}

/// 原文上的一段高亮。range 是源文本的 UTF-16 偏移，文本本身不被改写。
public enum ComposerHighlightStyle: Equatable, Sendable {
    /// `#`、`*`、反引号、代码围栏这些语法符号。
    case marker
    case header(Int)
    case bold
    case italic
    case inlineCode
    case codeBlock
    case quote
    case listMarker
    /// 绝对路径 / `~/` 路径，方便在高亮里认出粘进来的图片路径。
    case path
}

public struct ComposerHighlight: Equatable, Sendable {
    public let range: NSRange
    public let style: ComposerHighlightStyle

    public init(range: NSRange, style: ComposerHighlightStyle) {
        self.range = range
        self.style = style
    }
}

/// 单栏编辑器的 Markdown 语法高亮。
///
/// 不生成第二份渲染文本：`#`、`**`、列表符号都留在原文里，复制和
/// Cmd-Enter 发出去的就是用户写的那一串。这里只标出哪些 UTF-16 区间
/// 该上色。未闭合的标记不当高亮，避免打到一半就跳样式。
public enum ComposerMarkdown {
    public static func highlights(_ source: String) -> [ComposerHighlight] {
        guard !source.isEmpty else { return [] }
        let ns = source as NSString
        var spans: [ComposerHighlight] = []
        var inFence = false
        var index = 0
        while index <= ns.length {
            let rest = NSRange(location: index, length: ns.length - index)
            let newline = ns.range(of: "\n", options: [], range: rest)
            let end = newline.location == NSNotFound ? ns.length : newline.location
            let line = ns.substring(with: NSRange(location: index, length: end - index)) as NSString
            highlightLine(line, base: index, inFence: &inFence, spans: &spans)
            if newline.location == NSNotFound { break }
            index = end + 1
        }
        return spans
    }

    private static func highlightLine(
        _ line: NSString,
        base: Int,
        inFence: inout Bool,
        spans: inout [ComposerHighlight]
    ) {
        let trimmed = (line as String).trimmingCharacters(in: .whitespaces)
        if trimmed.hasPrefix("```") {
            if line.length > 0 {
                spans.append(mark(base, line.length, .marker))
            }
            inFence.toggle()
            return
        }
        if inFence {
            if line.length > 0 {
                spans.append(mark(base, line.length, .codeBlock))
            }
            return
        }
        let indent = leadingWhitespace(line)
        let body = line.substring(from: indent) as NSString
        if let heading = atxHeading(body) {
            let markerLength = indent + heading.marker
            spans.append(mark(base, markerLength, .marker))
            let content = line.length - markerLength
            if content > 0 {
                spans.append(mark(base + markerLength, content, .header(heading.level)))
                highlightInline(
                    line.substring(from: markerLength) as NSString,
                    base: base + markerLength,
                    spans: &spans
                )
            }
            return
        }
        if let markerLength = listMarkerLength(body) {
            spans.append(mark(base + indent, markerLength, .listMarker))
            let contentStart = indent + markerLength
            if contentStart < line.length {
                highlightInline(
                    line.substring(from: contentStart) as NSString,
                    base: base + contentStart,
                    spans: &spans
                )
            }
            addPaths(line, base: base, spans: &spans)
            return
        }
        if body.hasPrefix(">") {
            let markerLength = body.hasPrefix("> ") ? 2 : 1
            spans.append(mark(base + indent, markerLength, .marker))
            let contentStart = indent + markerLength
            if contentStart < line.length {
                spans.append(mark(base + contentStart, line.length - contentStart, .quote))
                highlightInline(
                    line.substring(from: contentStart) as NSString,
                    base: base + contentStart,
                    spans: &spans
                )
            }
            return
        }
        if isThematicBreak(trimmed) {
            spans.append(mark(base, line.length, .marker))
            return
        }
        highlightInline(line, base: base, spans: &spans)
        addPaths(line, base: base, spans: &spans)
    }

    private static func atxHeading(_ body: NSString) -> (level: Int, marker: Int)? {
        var hashes = 0
        while hashes < body.length, body.character(at: hashes) == 35, hashes < 6 {
            hashes += 1
        }
        guard (1...6).contains(hashes) else { return nil }
        if hashes == body.length {
            return (hashes, hashes)
        }
        guard body.character(at: hashes) == 32 else { return nil }
        return (hashes, hashes + 1)
    }

    /// `- ` / `* ` / `+ ` / `1. ` 的长度，含标记后的空白。
    private static func listMarkerLength(_ body: NSString) -> Int? {
        guard body.length >= 2 else { return nil }
        let first = body.character(at: 0)
        if first == 45 || first == 42 || first == 43 {
            guard body.character(at: 1) == 32 else { return nil }
            return 2
        }
        var digits = 0
        while digits < body.length, body.character(at: digits) >= 48, body.character(at: digits) <= 57 {
            digits += 1
        }
        guard digits > 0, digits + 1 < body.length,
              body.character(at: digits) == 46,
              body.character(at: digits + 1) == 32
        else { return nil }
        return digits + 2
    }

    private static func isThematicBreak(_ trimmed: String) -> Bool {
        guard trimmed.count >= 3 else { return false }
        return trimmed.allSatisfy { $0 == "-" } || trimmed.allSatisfy { $0 == "*" }
            || trimmed.allSatisfy { $0 == "_" }
    }

    private static func leadingWhitespace(_ line: NSString) -> Int {
        var index = 0
        while index < line.length {
            let ch = line.character(at: index)
            if ch != 32, ch != 9 { break }
            index += 1
        }
        return index
    }

    private static func highlightInline(
        _ line: NSString,
        base: Int,
        spans: inout [ComposerHighlight]
    ) {
        var index = 0
        while index < line.length {
            if line.character(at: index) == 96, let end = closing(line, from: index + 1, token: "`") {
                spans.append(mark(base + index, 1, .marker))
                if end > index + 1 {
                    spans.append(mark(base + index + 1, end - index - 1, .inlineCode))
                }
                spans.append(mark(base + end, 1, .marker))
                index = end + 1
                continue
            }
            if starts(line, index, "**"), let end = closing(line, from: index + 2, token: "**") {
                spans.append(mark(base + index, 2, .marker))
                if end > index + 2 {
                    spans.append(mark(base + index + 2, end - index - 2, .bold))
                }
                spans.append(mark(base + end, 2, .marker))
                index = end + 2
                continue
            }
            if line.character(at: index) == 42, canOpenStar(line, index),
               let end = closing(line, from: index + 1, token: "*") {
                spans.append(mark(base + index, 1, .marker))
                if end > index + 1 {
                    spans.append(mark(base + index + 1, end - index - 1, .italic))
                }
                spans.append(mark(base + end, 1, .marker))
                index = end + 1
                continue
            }
            index += 1
        }
    }

    /// 绝对路径单独上色，但不要盖住代码、标记或已经上过的样式。
    private static func addPaths(
        _ line: NSString,
        base: Int,
        spans: inout [ComposerHighlight]
    ) {
        guard line.length > 0,
              let expression = try? NSRegularExpression(pattern: "(?:^|\\s)((?:~/|/)\\S+)")
        else { return }
        let full = NSRange(location: 0, length: line.length)
        for match in expression.matches(in: line as String, range: full) {
            let group = match.range(at: 1)
            guard group.location != NSNotFound else { continue }
            let range = NSRange(location: base + group.location, length: group.length)
            if spans.contains(where: { NSIntersectionRange($0.range, range).length > 0 }) {
                continue
            }
            spans.append(mark(range.location, range.length, .path))
        }
    }

    private static func canOpenStar(_ line: NSString, _ index: Int) -> Bool {
        guard index + 1 < line.length else { return false }
        let next = line.character(at: index + 1)
        return next != 32 && next != 42 && next != 10
    }

    private static func starts(_ line: NSString, _ index: Int, _ token: String) -> Bool {
        let token = token as NSString
        guard index + token.length <= line.length else { return false }
        return line.substring(with: NSRange(location: index, length: token.length)) == token as String
    }

    /// 返回闭合标记的起点；找不到返回 nil（半截标记保持纯文本）。
    private static func closing(_ line: NSString, from start: Int, token: String) -> Int? {
        guard start < line.length else { return nil }
        let found = line.range(of: token, options: [], range: NSRange(location: start, length: line.length - start))
        guard found.location != NSNotFound, found.location > start else { return nil }
        return found.location
    }

    private static func mark(_ location: Int, _ length: Int, _ style: ComposerHighlightStyle) -> ComposerHighlight {
        ComposerHighlight(range: NSRange(location: location, length: length), style: style)
    }
}
