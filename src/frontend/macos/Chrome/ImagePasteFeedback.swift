import Foundation

/// 图片粘贴的用户可见阶段。
///
/// 粘贴一张图要经过「读剪贴板并转 PNG → 上传到目标机器 → 把路径写进 pane」。
/// 上传阶段对 SSH 目标可能是几百毫秒到数十秒，这段时间终端里什么都不会出现，
/// 所以每个阶段都要有用户马上能看见的提示，结果也要在屏幕上留一小会儿。
public enum ImagePasteStage: Equatable, Sendable {
    /// 没有进行中的粘贴，也没有需要展示的结果。
    case idle
    /// 正在读剪贴板并转成 PNG。
    case encoding
    /// 正在把 PNG 传到目标机器。
    case uploading
    /// 已把路径写进原 pane。`path` 是目标机器上的绝对路径。
    case done(path: String)
    /// 失败；消息直接给用户看，不再只藏在状态栏红点的 tooltip 里。
    case failed(message: String)

    /// 进行中：不会自己消失，也不可关闭。
    var isProgress: Bool {
        switch self {
        case .encoding, .uploading: return true
        case .idle, .done, .failed: return false
        }
    }

    /// 结果阶段：停留 `ImagePasteFeedback.hold(for:)` 后自动消失，可手动关闭。
    var isResult: Bool {
        switch self {
        case .done, .failed: return true
        case .idle, .encoding, .uploading: return false
        }
    }
}

/// 阶段到横幅内容的映射和停留时间（纯逻辑，便于单测）。
public struct ImagePasteFeedback: Equatable, Sendable {
    public enum Tone: Equatable, Sendable {
        case progress
        case success
        case failure
    }

    public let stage: ImagePasteStage
    public let tone: Tone
    /// 进行中的阶段为 true：横幅显示转圈。
    public let showsSpinner: Bool
    /// 成功时是目标机器上的完整路径，失败时是错误消息；进行中为 nil。
    public let detail: String?

    /// 成功提示停留时间：足够看清「已粘贴」又不挡终端。
    public static let successHold: TimeInterval = 2.5
    /// 失败提示停留更久，用户也可以手动关掉。
    public static let failureHold: TimeInterval = 10
    /// 极短的进度也要让横幅画出来，避免一帧内闪过。
    public static let minimumVisible: TimeInterval = 0.4

    /// 阶段对应的展示；`.idle` 返回 nil，表示横幅应隐藏。
    public static func make(_ stage: ImagePasteStage) -> ImagePasteFeedback? {
        switch stage {
        case .idle:
            return nil
        case .encoding, .uploading:
            return ImagePasteFeedback(stage: stage, tone: .progress, showsSpinner: true, detail: nil)
        case .done(let path):
            return ImagePasteFeedback(stage: stage, tone: .success, showsSpinner: false, detail: path)
        case .failed(let message):
            return ImagePasteFeedback(stage: stage, tone: .failure, showsSpinner: false, detail: message)
        }
    }

    /// 路径最后一段；空路径或只有斜杠时退回原字符串，避免横幅显示成空白。
    public static func fileName(of path: String) -> String {
        let last = path.split(separator: "/", omittingEmptySubsequences: true).last
        return last.map(String.init) ?? path
    }

    /// 结果阶段之后多久自动消失；进行中的阶段不自动消失。
    public static func hold(for stage: ImagePasteStage) -> TimeInterval? {
        switch stage {
        case .done: return successHold
        case .failed: return failureHold
        case .idle, .encoding, .uploading: return nil
        }
    }
}

/// 横幅时间线：进度至少停留 `minimumVisible`，结果停留 `hold(for:)` 后回到
/// `.idle`。不依赖 UI，由事件泵每个 tick 驱动，所以可以直接单测。
public struct ImagePasteTimeline: Equatable, Sendable {
    /// 当前展示的阶段。
    public private(set) var stage: ImagePasteStage = .idle
    private var shownSince: Date?
    /// 进度还没到最短显示时间就到了的结果：先挂起，到点再落下来。
    private var pending: ImagePasteStage?

    public init() {}

    /// 进行中，或还有挂起的结果没有落下来。此时不应开始第二次粘贴。
    public var isBusy: Bool { stage.isProgress || pending != nil }

    /// 当前应显示的阶段：结果超过停留时间后即使还没 tick 也算已消失。
    public func visibleStage(now: Date) -> ImagePasteStage {
        guard let shownSince, let hold = ImagePasteFeedback.hold(for: stage) else { return stage }
        return now.timeIntervalSince(shownSince) >= hold ? .idle : stage
    }

    /// 推进到新阶段。
    public mutating func advance(to next: ImagePasteStage, now: Date) {
        // 进度刚开始就收到结果：先让进度画出来，结果挂起。
        if stage.isProgress, next.isResult, let shownSince,
           now.timeIntervalSince(shownSince) < ImagePasteFeedback.minimumVisible {
            pending = next
            return
        }
        pending = nil
        // 同一阶段重复报告不重置计时，否则最短显示时间会被一直往后推。
        guard next != stage else { return }
        stage = next
        shownSince = now
    }

    /// 每个 tick 调用：到点落下挂起的结果，并清掉过期的结果。
    public mutating func tick(now: Date) {
        if let pending, let shownSince,
           now.timeIntervalSince(shownSince) >= ImagePasteFeedback.minimumVisible {
            self.pending = nil
            stage = pending
            self.shownSince = now
            return
        }
        if pending == nil, visibleStage(now: now) == .idle, stage != .idle {
            stage = .idle
            shownSince = nil
        }
    }

    /// 用户手动关掉当前结果。进行中的阶段不可关闭，否则用户会重复粘贴。
    public mutating func dismiss() {
        guard stage.isResult else { return }
        stage = .idle
        shownSince = nil
        pending = nil
    }
}
