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
/// 不加 Markdown 图片语法、不自动回车。Markdown 只在预览侧解释。
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

/// 预览里的一段文本样式；AppKit 字体/颜色由 UI 层映射。
public enum ComposerPreviewStyle: Equatable, Sendable {
    case body
    case bold
    case italic
    case boldItalic
    case inlineCode
    case codeBlock
    case header(Int)
    case quote
}

public struct ComposerPreviewSpan: Equatable, Sendable {
    /// 在 `ComposerPreview.text` 里的 UTF-16 范围。
    public let range: NSRange
    public let style: ComposerPreviewStyle

    public init(range: NSRange, style: ComposerPreviewStyle) {
        self.range = range
        self.style = style
    }
}

public struct ComposerPreview: Equatable, Sendable {
    public let text: String
    public let spans: [ComposerPreviewSpan]

    public init(text: String, spans: [ComposerPreviewSpan]) {
        self.text = text
        self.spans = spans
    }
}

/// 预览用的 Markdown 渲染；输入框里保存的始终是原始文本。
///
/// 先按行识别块级语法（标题 / 列表 / 引用 / 代码围栏），再用 Foundation 的
/// 行内解析器拿粗体、斜体、行内代码。这样预览保留用户写的行结构：纯路径
/// 不会被合并，列表也不会挤成一行。
public enum ComposerMarkdown {
    public static func preview(_ source: String) -> ComposerPreview {
        guard !source.isEmpty else { return ComposerPreview(text: "", spans: []) }
        var text = ""
        var spans: [ComposerPreviewSpan] = []
        var inFence = false
        for raw in source.split(separator: "\n", omittingEmptySubsequences: false) {
            var line = String(raw)
            if line.hasSuffix("\r") { line.removeLast() }
            let trimmed = line.trimmingCharacters(in: .whitespaces)

            if trimmed.hasPrefix("```") {
                inFence.toggle()
                continue
            }
            if inFence {
                append(line, prefix: "", block: .codeBlock, to: &text, spans: &spans)
                continue
            }
            if trimmed.isEmpty {
                append("", prefix: "", block: nil, to: &text, spans: &spans)
                continue
            }
            if let heading = heading(trimmed) {
                append(
                    heading.text,
                    prefix: "",
                    block: .header(heading.level),
                    to: &text,
                    spans: &spans
                )
                continue
            }
            if let item = listItem(line, trimmed: trimmed) {
                append(
                    item.text,
                    prefix: item.prefix,
                    block: nil,
                    to: &text,
                    spans: &spans
                )
                continue
            }
            if trimmed.hasPrefix(">") {
                var content = trimmed.dropFirst()
                if content.first == " " { content = content.dropFirst() }
                append(String(content), prefix: "", block: .quote, to: &text, spans: &spans)
                continue
            }
            if isThematicBreak(trimmed) {
                append(String(repeating: "─", count: 24), prefix: "", block: nil, to: &text, spans: &spans)
                continue
            }
            append(line, prefix: "", block: nil, to: &text, spans: &spans)
        }
        return ComposerPreview(text: text, spans: spans)
    }

    // MARK: - 块级识别

    private static func heading(_ trimmed: String) -> (level: Int, text: String)? {
        let hashes = trimmed.prefix { $0 == "#" }
        guard (1...6).contains(hashes.count) else { return nil }
        let rest = trimmed.dropFirst(hashes.count)
        guard rest.isEmpty || rest.first == " " else { return nil }
        var content = rest.drop(while: { $0 == " " })
        // 闭合式 ATX 标题（`## Title ##`）：尾部井号不属于正文。
        while content.hasSuffix("#") { content.removeLast() }
        return (hashes.count, String(content).trimmingCharacters(in: .whitespaces))
    }

    private static func listItem(_ line: String, trimmed: String) -> (prefix: String, text: String)? {
        let indent = String(line.prefix { $0 == " " || $0 == "\t" })
        for marker in ["- ", "* ", "+ "] where trimmed.hasPrefix(marker) {
            return (indent + "•  ", String(trimmed.dropFirst(marker.count)))
        }
        let digits = trimmed.prefix { $0.isNumber }
        if !digits.isEmpty {
            let rest = trimmed.dropFirst(digits.count)
            if rest.hasPrefix(". ") {
                return (indent + "\(digits). ", String(rest.dropFirst(2)))
            }
        }
        return nil
    }

    private static func isThematicBreak(_ trimmed: String) -> Bool {
        guard trimmed.count >= 3 else { return false }
        return trimmed.allSatisfy { $0 == "-" } || trimmed.allSatisfy { $0 == "*" }
            || trimmed.allSatisfy { $0 == "_" }
    }

    // MARK: - 行内解析与 span 累积

    private struct InlineRun {
        let text: String
        let style: ComposerPreviewStyle
    }

    /// `block` 非空时覆盖行内样式：标题内部统一按标题字号，代码块不解析行内语法。
    private static func append(
        _ content: String,
        prefix: String,
        block: ComposerPreviewStyle?,
        to text: inout String,
        spans: inout [ComposerPreviewSpan]
    ) {
        if !text.isEmpty { text += "\n" }
        var offset = text.utf16.count
        if !prefix.isEmpty {
            let length = prefix.utf16.count
            spans.append(ComposerPreviewSpan(
                range: NSRange(location: offset, length: length),
                style: block ?? .body
            ))
            text += prefix
            offset += length
        }
        let runs = block == .codeBlock
            ? [InlineRun(text: content, style: .codeBlock)]
            : inlineRuns(content)
        for run in runs {
            let length = run.text.utf16.count
            guard length > 0 else { continue }
            spans.append(ComposerPreviewSpan(
                range: NSRange(location: offset, length: length),
                style: block ?? run.style
            ))
            text += run.text
            offset += length
        }
    }

    private static func inlineRuns(_ content: String) -> [InlineRun] {
        guard !content.isEmpty else { return [] }
        var options = AttributedString.MarkdownParsingOptions()
        options.interpretedSyntax = .inlineOnlyPreservingWhitespace
        options.failurePolicy = .returnPartiallyParsedIfPossible
        guard let parsed = try? AttributedString(markdown: content, options: options) else {
            return [InlineRun(text: content, style: .body)]
        }
        let runs = parsed.runs.map { run in
            InlineRun(
                text: String(parsed[run.range].characters),
                style: inlineStyle(run.inlinePresentationIntent)
            )
        }
        return runs.isEmpty ? [InlineRun(text: content, style: .body)] : runs
    }

    private static func inlineStyle(_ inline: InlinePresentationIntent?) -> ComposerPreviewStyle {
        guard let inline else { return .body }
        if inline.contains(.code) { return .inlineCode }
        let bold = inline.contains(.stronglyEmphasized)
        let italic = inline.contains(.emphasized)
        switch (bold, italic) {
        case (true, true): return .boldItalic
        case (true, false): return .bold
        case (false, true): return .italic
        case (false, false): return .body
        }
    }
}
