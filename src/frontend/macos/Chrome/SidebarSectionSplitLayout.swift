import Foundation

#if canImport(CoreGraphics)
import CoreGraphics
#else
public typealias CGFloat = Double
public struct CGSize: Equatable {
    public var width: CGFloat
    public var height: CGFloat
    public init(width: CGFloat, height: CGFloat) {
        self.width = width
        self.height = height
    }
}
public struct CGRect: Equatable {
    public var origin: (x: CGFloat, y: CGFloat)
    public var size: CGSize
    public var height: CGFloat { size.height }
    public var width: CGFloat { size.width }
    public var minY: CGFloat { origin.y }
    public var maxY: CGFloat { origin.y + size.height }
    public init(x: CGFloat, y: CGFloat, width: CGFloat, height: CGFloat) {
        origin = (x, y)
        size = CGSize(width: width, height: height)
    }
}
#endif

/// 侧栏垂直 NSSplitView 的分区高度。0 尺寸时不要排 frame，否则会得到
/// `{0,-25}` 这种乱序，autosave 之后整个侧栏看起来像消失了。
public enum SidebarSectionSplitLayout {
    public static let collapsedHeight: CGFloat = 26
    public static let expandedMinHeight: CGFloat = 80

    /// `nil` = 还不能排（高度不够），调用方必须立刻 return，不能 setPosition。
    public static func heights(
        boundsHeight: CGFloat,
        dividerThickness: CGFloat,
        expanded: [Bool],
        currentHeights: [CGFloat]
    ) -> [CGFloat]? {
        let count = expanded.count
        guard count > 0, currentHeights.count == count else { return nil }
        let total = boundsHeight - dividerThickness * CGFloat(max(count - 1, 0))
        let minimum = expanded.reduce(CGFloat(0)) { sum, isExpanded in
            sum + (isExpanded ? expandedMinHeight : collapsedHeight)
        }
        guard total + 0.5 >= minimum else { return nil }

        let mins = expanded.map { $0 ? expandedMinHeight : collapsedHeight }
        let currentFits = zip(expanded, currentHeights).allSatisfy { isExpanded, current in
            if isExpanded {
                return current + 0.5 >= expandedMinHeight
            }
            return abs(current - collapsedHeight) <= 1
        }
        let currentSum = currentHeights.reduce(0, +)
        if currentFits, abs(currentSum - total) <= 1 {
            return currentHeights
        }

        var heights = mins
        let extra = total - minimum
        if extra > 0 {
            let indexes = expanded.indices.filter { expanded[$0] }
            if indexes.isEmpty {
                heights[count - 1] += extra
            } else {
                let share = extra / CGFloat(indexes.count)
                for index in indexes {
                    heights[index] += share
                }
            }
        }
        return heights
    }

    /// 启动时 split view 常为 `{width, 0}`。委托若直接 return，子视图会留在
    /// `{0,0,0,0}`，NSSplitView 会抱怨 outer edges 对不齐。宽跟上、高为 0。
    public static func degenerateFrames(count: Int, width: CGFloat) -> [CGRect] {
        guard count > 0 else { return [] }
        return Array(
            repeating: CGRect(x: 0, y: 0, width: max(width, 0), height: 0),
            count: count
        )
    }

    public static func frames(
        bounds: CGSize,
        dividerThickness: CGFloat,
        heights: [CGFloat],
        flipped: Bool
    ) -> [CGRect] {
        var result: [CGRect] = []
        result.reserveCapacity(heights.count)
        var cursor: CGFloat = flipped ? 0 : bounds.height
        for height in heights {
            let y: CGFloat
            if flipped {
                y = cursor
                cursor += height + dividerThickness
            } else {
                cursor -= height
                y = cursor
                cursor -= dividerThickness
            }
            result.append(CGRect(x: 0, y: y, width: bounds.width, height: height))
        }
        return result
    }
}
