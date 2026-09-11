import Foundation

/// 终端双击选词：层层扩大，拖选用空格切词。
///
/// `feature/dogfood-0907*` 点在 `0907` 上：第一次 `0907`，第二次
/// `dogfood-0907*`，第三次 `feature/dogfood-0907*`。中间那一层扩不开
/// 就跳过（`cd a/b/c` 的 `b` 第二次直接到 `a/b/c`）。
///
/// 双击后拖选用空白分隔的整词，对齐 Ghostty / iTerm 的 word-drag，
/// 不会在 `/` 或 `-` 处切开。括号交给 SwiftTerm 原有平衡匹配。
public enum ProgressiveWordSelection {
    public enum Level: Int, Equatable, Sendable, Comparable {
        /// 字母、数字、下划线（含 CJK）。
        case identifier = 1
        /// 标识符 + 连字符/点/通配等：`-.+%@#?*`
        case word = 2
        /// 词 + 路径分隔：`/\~:`
        case path = 3

        public static func < (lhs: Level, rhs: Level) -> Bool {
            lhs.rawValue < rhs.rawValue
        }

        var next: Level? {
            Level(rawValue: rawValue + 1)
        }
    }

    public struct Result: Equatable, Sendable {
        /// 列起点（含）。
        public let start: Int
        /// 列终点（不含）。
        public let end: Int
        public let level: Level

        public init(start: Int, end: Int, level: Level) {
            self.start = start
            self.end = end
            self.level = level
        }

        public func contains(column: Int) -> Bool {
            column >= start && column < end
        }
    }

    /// 第二层：kebab / 文件名 / git 脏标记，不含路径分隔。
    public static let wordExtras: Set<Character> = [
        "-", ".", "*", "?", "+", "%", "#", "@", "=",
    ]

    /// 第三层：路径与 URI 分隔。
    public static let pathExtras: Set<Character> = [
        "/", "\\", "~", ":",
    ]

    /// `cells` 是一行的终端格子（一列一个 Character；CJK 宽字符的占位格用 `\0`）。
    /// `previous` 是上一次双击结果：点还在里面就升级，否则重新从最紧的一层开始。
    /// 括号/引号返回 `nil`，让调用方走 SwiftTerm 的平衡表达式。
    public static func select(
        cells: [Character],
        column: Int,
        previous: Result?
    ) -> Result? {
        guard !cells.isEmpty else { return nil }
        let col = min(max(column, 0), cells.count - 1)
        let ch = cells[col]
        if isBracketOrQuote(ch) {
            return nil
        }

        if let previous, previous.contains(column: col) {
            return expand(cells: cells, column: col, from: previous)
        }

        if isWhitespace(ch) {
            let range = span(cells: cells, column: col, predicate: isWhitespace)
            return Result(start: range.start, end: range.end, level: .identifier)
        }
        if isIdentifier(ch) || isFiller(ch) && neighboringIdentifier(cells, col) {
            let range = span(cells: cells, column: col, predicate: isIdentifierOrFiller)
            return Result(start: range.start, end: range.end, level: .identifier)
        }
        if isWordExtra(ch) {
            let range = span(cells: cells, column: col, predicate: isWord)
            return Result(start: range.start, end: range.end, level: .word)
        }
        if isPathExtra(ch) {
            let range = span(cells: cells, column: col, predicate: isPath)
            return Result(start: range.start, end: range.end, level: .path)
        }
        return Result(start: col, end: col + 1, level: .identifier)
    }

    /// 空白分隔的整词（或点在空白上时的空白段）。拖选用这个，不按 `/` `-` 切。
    public static func tokenSpan(cells: [Character], column: Int) -> (start: Int, end: Int) {
        guard !cells.isEmpty else { return (0, 0) }
        let col = min(max(column, 0), cells.count - 1)
        if isWhitespace(cells[col]) {
            return span(cells: cells, column: col, predicate: isWhitespace)
        }
        return span(cells: cells, column: col, predicate: isToken)
    }

    /// 从锚点词拖到目标列：两端都吸附到空白分隔的整词，中间空白一并选上。
    public static func dragByTokens(
        cells: [Character],
        anchorColumn: Int,
        toColumn: Int
    ) -> (start: Int, end: Int) {
        let anchor = tokenSpan(cells: cells, column: anchorColumn)
        let target = tokenSpan(cells: cells, column: toColumn)
        return (
            min(anchor.start, target.start),
            max(anchor.end, target.end)
        )
    }

    public static func isIdentifier(_ ch: Character) -> Bool {
        if isFiller(ch) { return false }
        return ch.isLetter || ch.isNumber || ch == "_"
    }

    public static func isWordExtra(_ ch: Character) -> Bool {
        wordExtras.contains(ch)
    }

    public static func isPathExtra(_ ch: Character) -> Bool {
        pathExtras.contains(ch)
    }

    public static func isWord(_ ch: Character) -> Bool {
        isIdentifier(ch) || isWordExtra(ch) || isFiller(ch)
    }

    public static func isPath(_ ch: Character) -> Bool {
        isWord(ch) || isPathExtra(ch)
    }

    public static func isFiller(_ ch: Character) -> Bool {
        ch == "\0" || ch.unicodeScalars.allSatisfy({ $0.value == 0 })
    }

    public static func isBracketOrQuote(_ ch: Character) -> Bool {
        "()[]{}<>\"'`".contains(ch)
    }

    public static func isWhitespace(_ ch: Character) -> Bool {
        if isFiller(ch) { return false }
        return ch == " " || ch.isWhitespace
    }

    private static func isIdentifierOrFiller(_ ch: Character) -> Bool {
        isIdentifier(ch) || isFiller(ch)
    }

    private static func isToken(_ ch: Character) -> Bool {
        !isWhitespace(ch)
    }

    private static func expand(
        cells: [Character],
        column: Int,
        from previous: Result
    ) -> Result {
        var level = previous.level
        while let next = level.next {
            let range = span(cells: cells, column: column, predicate: predicate(for: next))
            if range.start < previous.start || range.end > previous.end {
                return Result(start: range.start, end: range.end, level: next)
            }
            level = next
        }
        return previous
    }

    private static func predicate(for level: Level) -> (Character) -> Bool {
        switch level {
        case .identifier:
            return isIdentifierOrFiller
        case .word:
            return isWord
        case .path:
            return isPath
        }
    }

    private static func neighboringIdentifier(_ cells: [Character], _ col: Int) -> Bool {
        (col > 0 && isIdentifier(cells[col - 1]))
            || (col + 1 < cells.count && isIdentifier(cells[col + 1]))
    }

    private static func span(
        cells: [Character],
        column: Int,
        predicate: (Character) -> Bool
    ) -> (start: Int, end: Int) {
        var start = column
        while start > 0, predicate(cells[start - 1]) {
            start -= 1
        }
        var end = column
        while end < cells.count, predicate(cells[end]) {
            end += 1
        }
        if start < end, isFiller(cells[start]) {
            while start < end, isFiller(cells[start]) {
                start += 1
            }
        }
        if start < end, isFiller(cells[end - 1]) {
            while end > start, isFiller(cells[end - 1]) {
                end -= 1
            }
        }
        if start >= end {
            return (column, min(column + 1, cells.count))
        }
        return (start, end)
    }
}
