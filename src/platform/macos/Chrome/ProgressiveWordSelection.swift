import Foundation

/// 终端双击选词：第一次选标识符，再次双击扩成路径/更大的 token。
///
/// `cd a/b/c` 里点 `b`：第一次 `b`，第二次 `a/b/c`。`/` 本身不算第一层词，
/// 避免 iTerm 默认把整段路径一次吞掉。括号交给 SwiftTerm 原有平衡匹配。
public enum ProgressiveWordSelection {
    public enum Level: Int, Equatable, Sendable {
        /// 字母、数字、下划线（含 CJK）。
        case identifier = 1
        /// 标识符 + iTerm 风格路径字符 `/\-_.~:@+%#`。
        case path = 2
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

    /// 路径扩展用的附加字符（第一层标识符不含这些）。
    public static let pathExtras: Set<Character> = [
        "/", "\\", "-", ".", "~", ":", "@", "+", "%", "#",
    ]

    /// `cells` 是一行的终端格子（一列一个 Character；CJK 宽字符的占位格用 `\0`）。
    /// `previous` 是上一次双击结果：点还在里面就升级，否则重新从标识符开始。
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

        if let previous, previous.contains(column: col), previous.level == .identifier {
            let expanded = span(cells: cells, column: col, predicate: isPath)
            if expanded.end > expanded.start {
                return Result(start: expanded.start, end: expanded.end, level: .path)
            }
        }

        if isIdentifier(ch) || isFiller(ch) && neighboringIdentifier(cells, col) {
            let range = span(cells: cells, column: col, predicate: isIdentifier)
            return Result(start: range.start, end: range.end, level: .identifier)
        }
        if isPathExtra(ch) {
            let range = span(cells: cells, column: col, predicate: isPath)
            return Result(start: range.start, end: range.end, level: .path)
        }
        if ch == " " || ch.isWhitespace {
            let range = span(cells: cells, column: col) { $0 == " " || $0.isWhitespace }
            return Result(start: range.start, end: range.end, level: .identifier)
        }
        return Result(start: col, end: col + 1, level: .identifier)
    }

    public static func isIdentifier(_ ch: Character) -> Bool {
        if isFiller(ch) { return false }
        return ch.isLetter || ch.isNumber || ch == "_"
    }

    public static func isPathExtra(_ ch: Character) -> Bool {
        pathExtras.contains(ch)
    }

    public static func isPath(_ ch: Character) -> Bool {
        isIdentifier(ch) || isPathExtra(ch) || isFiller(ch)
    }

    public static func isFiller(_ ch: Character) -> Bool {
        ch == "\0" || ch.unicodeScalars.allSatisfy({ $0.value == 0 })
    }

    public static func isBracketOrQuote(_ ch: Character) -> Bool {
        "()[]{}<>\"'`".contains(ch)
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
