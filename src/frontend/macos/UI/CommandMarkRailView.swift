import AppKit
import MuxtermChrome

/// 终端右侧覆盖层命令轨：绿=成功，红=失败。不占 tmux 字符格。
final class CommandMarkRailView: NSView {
    var onSelectMark: ((CommandMarkTick) -> Void)?
    var onSelectOffset: ((UInt32) -> Void)?
    var onExpandedChange: ((Bool) -> Void)?

    private(set) var marks: [CommandMarkTick] = []
    private(set) var maxOffset: UInt32 = 0
    private(set) var ticks: [PlacedCommandMark] = []
    private(set) var expanded = false
    private(set) var hasOK = false
    private(set) var hasFail = false

    let okProxy = NSView()
    let failProxy = NSView()

    override var isFlipped: Bool { false }

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        layer?.backgroundColor = NSColor.clear.cgColor
        setAccessibilityIdentifier("muxterm.cmdMark.rail")
        setAccessibilityElement(true)
        setAccessibilityRole(.scrollBar)
        setAccessibilityLabel("命令刻度")

        for proxy in [okProxy, failProxy] {
            proxy.translatesAutoresizingMaskIntoConstraints = false
            proxy.setAccessibilityElement(true)
            addSubview(proxy)
        }
        okProxy.setAccessibilityIdentifier("muxterm.cmdMark.ok")
        failProxy.setAccessibilityIdentifier("muxterm.cmdMark.fail")
        okProxy.setAccessibilityRole(.button)
        failProxy.setAccessibilityRole(.button)
        NSLayoutConstraint.activate([
            okProxy.leadingAnchor.constraint(equalTo: leadingAnchor),
            okProxy.trailingAnchor.constraint(equalTo: trailingAnchor),
            okProxy.topAnchor.constraint(equalTo: topAnchor),
            okProxy.heightAnchor.constraint(equalToConstant: 1),
            failProxy.leadingAnchor.constraint(equalTo: leadingAnchor),
            failProxy.trailingAnchor.constraint(equalTo: trailingAnchor),
            failProxy.bottomAnchor.constraint(equalTo: bottomAnchor),
            failProxy.heightAnchor.constraint(equalToConstant: 1),
        ])
        okProxy.isHidden = true
        failProxy.isHidden = true
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    func apply(marks: [CommandMarkTick], maxOffset: UInt32) {
        self.marks = marks
        self.maxOffset = maxOffset
        hasOK = marks.contains { $0.kind == .ok }
        hasFail = marks.contains { $0.kind == .fail }
        okProxy.isHidden = !hasOK
        failProxy.isHidden = !hasFail
        let lastOK = marks.reversed().first { $0.kind == .ok }
        let lastFail = marks.reversed().first { $0.kind == .fail }
        okProxy.toolTip = lastOK.map { "成功：\($0.command)（退出码 \($0.exitCode ?? 0)）" }
        failProxy.toolTip = lastFail.map { "失败：\($0.command)（退出码 \($0.exitCode ?? 0)）" }
        okProxy.setAccessibilityValue(lastOK?.command)
        failProxy.setAccessibilityValue(lastFail?.command)
        isHidden = !CommandMarkRailLayout.isVisible(markCount: marks.count)
        relayoutTicks()
        needsDisplay = true
    }

    override func layout() {
        super.layout()
        relayoutTicks()
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(
            NSTrackingArea(
                rect: bounds,
                options: [.mouseEnteredAndExited, .mouseMoved, .activeInKeyWindow, .inVisibleRect],
                owner: self,
                userInfo: nil
            )
        )
    }

    override func mouseEntered(with event: NSEvent) {
        setExpanded(true)
        updateHover(with: event)
    }

    override func mouseExited(with event: NSEvent) {
        setExpanded(false)
        toolTip = nil
    }

    override func mouseMoved(with event: NSEvent) {
        updateHover(with: event)
    }

    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        if let hit = CommandMarkRailLayout.hitTest(ticks: ticks, y: point.y) {
            onSelectMark?(hit.mark)
            return
        }
        onSelectOffset?(
            CommandMarkRailLayout.trackOffset(
                y: point.y,
                height: bounds.height,
                maxOffset: maxOffset
            )
        )
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        guard !isHidden, !ticks.isEmpty, bounds.height > 8 else { return }

        let trackWidth: CGFloat = expanded ? 3 : 2
        let track = NSRect(
            x: (bounds.width - trackWidth) / 2,
            y: CommandMarkRailLayout.inset,
            width: trackWidth,
            height: max(bounds.height - CommandMarkRailLayout.inset * 2, 1)
        )
        NSColor.separatorColor.withAlphaComponent(expanded ? 0.55 : 0.28).setFill()
        NSBezierPath(roundedRect: track, xRadius: 1, yRadius: 1).fill()

        for tick in ticks {
            let color: NSColor
            switch tick.mark.kind {
            case .ok:
                color = NSColor.systemGreen
            case .fail:
                color = NSColor.systemRed
            case .pending:
                color = NSColor.tertiaryLabelColor
            }
            color.withAlphaComponent(expanded ? 1 : 0.92).setFill()
            let width: CGFloat = expanded ? 10 : 6
            let rect = NSRect(
                x: (bounds.width - width) / 2,
                y: tick.y - CommandMarkRailLayout.tickHeight / 2,
                width: width,
                height: CommandMarkRailLayout.tickHeight
            )
            NSBezierPath(roundedRect: rect, xRadius: 1, yRadius: 1).fill()
        }
    }

    private func relayoutTicks() {
        ticks = CommandMarkRailLayout.place(
            marks: marks,
            maxOffset: maxOffset,
            height: bounds.height
        )
    }

    private func setExpanded(_ expanded: Bool) {
        guard self.expanded != expanded else { return }
        self.expanded = expanded
        needsDisplay = true
        onExpandedChange?(expanded)
    }

    private func updateHover(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        if let hit = CommandMarkRailLayout.hitTest(ticks: ticks, y: point.y) {
            let code = hit.mark.exitCode.map(String.init) ?? "…"
            let status = hit.mark.kind == .fail ? "失败" : (hit.mark.kind == .ok ? "成功" : "进行中")
            toolTip = "\(status) · \(hit.mark.command)（退出码 \(code)）"
        } else {
            toolTip = "命令时间线"
        }
    }
}
