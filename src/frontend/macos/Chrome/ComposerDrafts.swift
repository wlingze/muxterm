import Foundation

/// 输入框草稿的稳定身份：一个 pane 一份草稿。
///
/// 草稿不跟着前台焦点走：在 A pane 写了一半切到 B，回来时 A 的内容还在。
/// 没有绑定工作区时（空窗口）仍保留一个占位页，避免输入框打不开。
public struct ComposerDraftKey: Hashable, Sendable {
    public let workspaceID: String
    public let paneID: UInt32

    public init(workspaceID: String, paneID: UInt32) {
        self.workspaceID = workspaceID
        self.paneID = paneID
    }

    public init(target: ComposerTarget?) {
        self.workspaceID = target?.workspaceID ?? ""
        self.paneID = target?.paneID ?? 0
    }

    /// 真正能写回终端的目标。
    public var isBound: Bool { !workspaceID.isEmpty }
}

/// 输入框里的一页：文本、光标和目标都属于这个 pane。
public struct ComposerDraftPage: Equatable, Sendable, Identifiable {
    public let key: ComposerDraftKey
    /// 最近一次打开这一页时的目标快照；写入和上传都用它。
    public var target: ComposerTarget?
    public var text: String
    /// UTF-16 选区；切页后恢复光标。
    public var selection: NSRange

    public var id: ComposerDraftKey { key }

    /// tab 标题：工作区名 + pane 号，保证同一工作区的多个 pane 能区分。
    public var title: String { Self.title(for: target, key: key) }

    public init(
        key: ComposerDraftKey,
        target: ComposerTarget?,
        text: String = "",
        selection: NSRange = NSRange(location: 0, length: 0)
    ) {
        self.key = key
        self.target = target
        self.text = text
        self.selection = selection
    }

    static func title(for target: ComposerTarget?, key: ComposerDraftKey) -> String {
        guard key.isBound else { return "—" }
        let name = target?.workspaceName.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let short = name.isEmpty ? "pane" : String(name.prefix(12))
        return "\(short) #\(key.paneID)"
    }
}

/// 按 pane 保存草稿。窗口关闭只收起窗口，不改这里，所以草稿不会丢。
public struct ComposerDraftStore: Equatable, Sendable {
    public private(set) var pages: [ComposerDraftPage] = []
    public private(set) var activeKey: ComposerDraftKey?

    public init() {}

    public var activePage: ComposerDraftPage? {
        guard let activeKey else { return nil }
        return pages.first { $0.key == activeKey }
    }

    public var activeIndex: Int? {
        guard let activeKey else { return nil }
        return pages.firstIndex { $0.key == activeKey }
    }

    public func page(for key: ComposerDraftKey) -> ComposerDraftPage? {
        pages.first { $0.key == key }
    }

    /// 打开某个目标：已有草稿就切过去并刷新目标快照，没有才新建空页。
    @discardableResult
    public mutating func activate(_ target: ComposerTarget?) -> ComposerDraftPage {
        let key = ComposerDraftKey(target: target)
        if let index = pages.firstIndex(where: { $0.key == key }) {
            if let target {
                pages[index].target = target
            }
            activeKey = key
            return pages[index]
        }
        let page = ComposerDraftPage(key: key, target: target)
        pages.append(page)
        activeKey = key
        return page
    }

    @discardableResult
    public mutating func select(_ key: ComposerDraftKey) -> Bool {
        guard pages.contains(where: { $0.key == key }) else { return false }
        activeKey = key
        return true
    }

    public mutating func select(index: Int) -> Bool {
        guard pages.indices.contains(index) else { return false }
        activeKey = pages[index].key
        return true
    }

    public mutating func update(text: String, selection: NSRange, for key: ComposerDraftKey) {
        guard let index = pages.firstIndex(where: { $0.key == key }) else { return }
        pages[index].text = text
        pages[index].selection = selection
    }

    /// 只清空这一页；其他 pane 的草稿原样保留。
    public mutating func clear(_ key: ComposerDraftKey) {
        guard let index = pages.firstIndex(where: { $0.key == key }) else { return }
        pages[index].text = ""
        pages[index].selection = NSRange(location: 0, length: 0)
    }
}

/// 输入框的竖长窗口尺寸：宽度略小于半屏，高度接近八成屏高。
///
/// 纯函数，方便在测试里验证「竖长、不占满屏、不窄到放不下动作菜单」。
public enum ComposerWindowLayout {
    public static let minimumWidth: CGFloat = 420
    public static let minimumHeight: CGFloat = 460

    public static func size(screenWidth: CGFloat, screenHeight: CGFloat) -> CGSize {
        guard screenWidth > 0, screenHeight > 0 else {
            return CGSize(width: 560, height: 760)
        }
        var width = min(max(screenWidth * 0.38, 460), 720)
        width = min(width, max(320, screenWidth / 2 - 24))
        let height = max(
            minimumHeight,
            min(max(screenHeight * 0.78, width + 160), screenHeight - 48)
        )
        return CGSize(width: width, height: height)
    }
}
