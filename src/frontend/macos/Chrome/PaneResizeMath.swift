import Foundation

/// Pane 分隔条拖动的无 GUI 数学，供 AppKit 实现和纯单元测试共同使用。
public enum PaneResizeMath {
    public static let minimumRatio = 0.05
    public static let maximumRatio = 0.95

    /// 将分隔比例限制在可渲染范围内，避免某个 pane 被压成零宽/零高。
    public static func clampedRatio(_ ratio: Double) -> Double {
        min(max(ratio, minimumRatio), maximumRatio)
    }

    /// 根据拖动位移计算新的 first/second 比例。
    public static func ratioAfterDrag(
        startRatio: Double,
        delta: Double,
        totalLength: Double,
        dividerLength: Double
    ) -> Double {
        let usable = max(totalLength - dividerLength, 1)
        let startLength = clampedRatio(startRatio) * usable
        return clampedRatio((startLength + delta) / usable)
    }

    /// 把 backing pixel 长度转换为字符格数量。
    public static func characterCount(pixelLength: Double, cellPixels: Int) -> UInt16? {
        guard cellPixels > 0 else { return nil }
        let count = Int(floor(pixelLength / Double(cellPixels)))
        guard count >= 2, count < 10000 else { return nil }
        return UInt16(count)
    }
}
