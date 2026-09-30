import AppKit
import XCTest
@testable import MuxtermAppLib

final class SSHPortPickerE2ETests: XCTestCase {
    private var window: NSWindow!
    private var bar: StatusBarView!

    override func setUp() {
        super.setUp()
        AppE2E.ensureApp()
        window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 960, height: 80),
            styleMask: [.titled, .closable], backing: .buffered, defer: false
        )
        bar = StatusBarView(frame: .zero)
        window.contentView = bar
        window.orderFront(nil)
    }

    override func tearDown() {
        // 与其他 AppKit 测试一致，隐藏窗口以避免 XCTest 的 NSWindow 释放问题。
        bar.testCloseConnectionPopovers()
        window.orderOut(nil)
        bar = nil
        window = nil
        super.tearDown()
    }

    private func port(_ remote: UInt16, local: UInt16? = nil,
                      pending: Bool = false, error: String? = nil,
                      discovered: Bool = false) -> CoreSSHPort {
        CoreSSHPort(remotePort: remote, localPort: local, lanAccessEnabled: false,
                    pending: pending, error: error, discovered: discovered)
    }

    private func find(_ identifier: String, in view: NSView) -> NSView? {
        if view.accessibilityIdentifier() == identifier { return view }
        return view.subviews.lazy.compactMap { self.find(identifier, in: $0) }.first
    }

    private func openPicker(_ ports: [CoreSSHPort], remoteHost: String? = "192.0.2.42",
                            remoteHostError: String? = nil) throws -> NSView {
        bar.updateConnectionStatus(
            (type: "ssh", host: "ssh-alias", status: "connected"),
            trafficRate: 0, totalBytes: 0,
            portListing: CoreSSHPortListing(ports: ports, scanPending: false, scanError: nil,
                                           remoteHost: remoteHost, remoteHostError: remoteHostError)
        )
        bar.testClickStatusDot()
        AppE2E.pump(50)
        let connection = try XCTUnwrap(bar.testPopoverContentView())
        let add = try XCTUnwrap(find("muxterm.statusPopover.ports.add", in: connection) as? NSButton)
        add.performClick(nil)
        AppE2E.pump(100)
        let picker = try XCTUnwrap(bar.testSSHPortPickerContentView())
        picker.layoutSubtreeIfNeeded()
        return picker
    }

    func testForwardAndStopButtonsShareAColumnAcrossAllRowStates() throws {
        let ports = [port(22), port(139), port(3001, discovered: true),
                     port(5432, local: 45001), port(8080, pending: true),
                     port(65535, error: "An unusually long forwarding error that must not move the buttons")]
        let picker = try openPicker(ports)
        let frames = try ports.map { port -> NSRect in
            let button = try XCTUnwrap(find("muxterm.statusPopover.port.\(port.remotePort).action", in: picker))
            return button.convert(button.bounds, to: picker)
        }
        for frame in frames.dropFirst() {
            XCTAssertEqual(frame.minX, frames[0].minX, accuracy: 0.5)
            XCTAssertEqual(frame.width, frames[0].width, accuracy: 0.5)
        }
        XCTAssertNotNil(find("muxterm.statusPopover.port.5432.copy", in: picker))
        XCTAssertNotNil(find("muxterm.statusPopover.port.5432.lan", in: picker))
        XCTAssertNotNil(find("muxterm.statusPopover.port.3001.ignore", in: picker))
    }

    func testEveryRemotePortHasAnOpenButtonIncludingBeforeForwarding() throws {
        let ports = [port(22), port(3001, discovered: true), port(5432, local: 45001),
                     port(8080, pending: true), port(65535, error: "Connection refused")]
        let picker = try openPicker(ports)
        for port in ports {
            XCTAssertNotNil(find("muxterm.statusPopover.port.\(port.remotePort).open", in: picker),
                            "Open 必须用于远端端口，不依赖本地转发状态")
        }
    }

    func testOpenUsesTheSSHTargetIPAndRemotePortWithoutStartingAForward() throws {
        let ports = [port(3001), port(5432, local: 45001), port(8080, pending: true),
                     port(65535, error: "Connection refused")]
        var opened: [String] = []
        var forwards: [UInt16] = []
        bar.openSSHPortURL = { opened.append($0.absoluteString) }
        bar.onSSHPortForward = { port, _ in forwards.append(port) }
        let picker = try openPicker(ports)
        for port in ports {
            let button = try XCTUnwrap(find("muxterm.statusPopover.port.\(port.remotePort).open", in: picker) as? NSButton)
            XCTAssertTrue(button.isEnabled)
            button.performClick(nil)
        }
        XCTAssertEqual(opened, ports.map { "http://192.0.2.42:\($0.remotePort)" })
        XCTAssertTrue(forwards.isEmpty)
        XCTAssertEqual((find("muxterm.statusPopover.port.3001.value", in: picker) as? NSTextField)?.stringValue,
                       "http://192.0.2.42:3001")
    }

    func testOpenFormatsIPv6AndUsesTheNewTargetAfterWorkspaceSwitch() throws {
        var opened: [String] = []
        bar.openSSHPortURL = { opened.append($0.absoluteString) }
        let ports = [port(3001)]
        let picker = try openPicker(ports, remoteHost: "2001:db8::42")
        try XCTUnwrap(find("muxterm.statusPopover.port.3001.open", in: picker) as? NSButton).performClick(nil)
        bar.updateConnectionStatus(
            (type: "ssh", host: "other-alias", status: "connected"), trafficRate: 0, totalBytes: 0,
            portListing: CoreSSHPortListing(ports: ports, scanPending: false, scanError: nil,
                                           remoteHost: "198.51.100.47")
        )
        try XCTUnwrap(find("muxterm.statusPopover.port.3001.open", in: picker) as? NSButton).performClick(nil)
        XCTAssertEqual(opened, ["http://[2001:db8::42]:3001", "http://198.51.100.47:3001"])
    }

    func testOpenWaitsForTheResolvedTargetInsteadOfOpeningTheSSHAlias() throws {
        var opened: [URL] = []
        bar.openSSHPortURL = { opened.append($0) }
        let ports = [port(3001)]
        let picker = try openPicker(ports, remoteHost: nil, remoteHostError: "Could not resolve target")
        let button = try XCTUnwrap(find("muxterm.statusPopover.port.3001.open", in: picker) as? NSButton)
        XCTAssertFalse(button.isEnabled)
        XCTAssertEqual(button.toolTip, "Could not resolve target")
        button.performClick(nil)
        XCTAssertTrue(opened.isEmpty)
        bar.updateConnectionStatus(
            (type: "ssh", host: "ssh-alias", status: "connected"), trafficRate: 0, totalBytes: 0,
            portListing: CoreSSHPortListing(ports: ports, scanPending: true, scanError: nil,
                                           remoteHost: "192.0.2.42")
        )
        let resolved = try XCTUnwrap(find("muxterm.statusPopover.port.3001.open", in: picker) as? NSButton)
        XCTAssertTrue(resolved.isEnabled)
        resolved.performClick(nil)
        XCTAssertEqual(opened.map(\.absoluteString), ["http://192.0.2.42:3001"])
    }

    func testForwardedLocalAddressCopyAndLANControlsKeepTheirSeparateActions() throws {
        let pasteboard = NSPasteboard.general
        let savedItems = (pasteboard.pasteboardItems ?? []).map { source -> NSPasteboardItem in
            let item = NSPasteboardItem()
            for type in source.types {
                if let data = source.data(forType: type) { item.setData(data, forType: type) }
            }
            return item
        }
        defer {
            pasteboard.clearContents()
            pasteboard.writeObjects(savedItems)
        }
        var opened: [String] = []
        var forwards: [(UInt16, Bool)] = []
        var stopped: [UInt16] = []
        var ignored: [UInt16] = []
        bar.openSSHPortURL = { opened.append($0.absoluteString) }
        bar.onSSHPortForward = { forwards.append(($0, $1)) }
        bar.onSSHPortStop = { stopped.append($0) }
        bar.onSSHPortIgnore = { ignored.append($0) }
        let picker = try openPicker([port(3001, discovered: true), port(5432, local: 45001)])
        func click(_ port: UInt16, _ action: String) throws {
            try XCTUnwrap(find("muxterm.statusPopover.port.\(port).\(action)", in: picker) as? NSButton).performClick(nil)
        }
        try click(5432, "address")
        try click(5432, "copy")
        XCTAssertEqual(NSPasteboard.general.string(forType: .string), "http://127.0.0.1:45001")
        try click(5432, "open")
        try click(5432, "lan")
        try click(5432, "action")
        try click(3001, "action")
        try click(3001, "ignore")
        XCTAssertEqual(opened, ["http://127.0.0.1:45001", "http://192.0.2.42:5432"])
        XCTAssertEqual(forwards.map { $0.0 }, [5432, 3001])
        XCTAssertEqual(forwards.map { $0.1 }, [true, false])
        XCTAssertEqual(stopped, [5432])
        XCTAssertEqual(ignored, [3001])
    }

    func testPortColumnsStayInsideThePopoverWithLongIPv6InBothAppearances() throws {
        let ports = [port(22), port(139), port(445), port(3001, discovered: true),
                     port(5432, local: 45001), port(5436), port(8081), port(9000),
                     port(9001), port(65535, error: "A long error message with additional connection details")]
        for (name, appearance) in [("light", NSAppearance.Name.aqua), ("dark", .darkAqua)] {
            let picker = try openPicker(ports, remoteHost: "2001:db8:1234:5678:90ab:cdef:1234:5678")
            picker.appearance = NSAppearance(named: appearance)
            picker.layoutSubtreeIfNeeded()
            let list = try XCTUnwrap(find("muxterm.statusPopover.ports", in: picker))
            let scroll = try XCTUnwrap(list.enclosingScrollView)
            let first = try XCTUnwrap(find("muxterm.statusPopover.port.22.remote", in: picker))
            let firstFrame = try XCTUnwrap(first.superview).convert(first.alignmentRect(forFrame: first.frame), to: list)
            XCTAssertTrue(scroll.documentVisibleRect.contains(firstFrame),
                          "长列表初次打开应从第一个端口开始：visible=\(scroll.documentVisibleRect) first=\(firstFrame)")
            var openX: CGFloat?
            for port in ports {
                let action = try XCTUnwrap(find("muxterm.statusPopover.port.\(port.remotePort).action", in: picker))
                let open = try XCTUnwrap(find("muxterm.statusPopover.port.\(port.remotePort).open", in: picker))
                // AppKit 布局用 alignment rect；按钮 frame 还包含阴影外沿。
                let actionFrame = try XCTUnwrap(action.superview).convert(action.alignmentRect(forFrame: action.frame), to: picker)
                let openFrame = try XCTUnwrap(open.superview).convert(open.alignmentRect(forFrame: open.frame), to: picker)
                XCTAssertGreaterThanOrEqual(openFrame.minX, actionFrame.maxX)
                XCTAssertLessThanOrEqual(openFrame.maxX, picker.bounds.maxX - 8)
                if let openX { XCTAssertEqual(openFrame.minX, openX, accuracy: 0.5) }
                openX = openFrame.minX
            }
            let bitmap = try XCTUnwrap(picker.bitmapImageRepForCachingDisplay(in: picker.bounds))
            picker.cacheDisplay(in: picker.bounds, to: bitmap)
            let background = try XCTUnwrap(bitmap.colorAt(x: 4, y: 4)?.usingColorSpace(.deviceRGB))
            if name == "light" { XCTAssertGreaterThan(background.redComponent, 0.5) }
            else { XCTAssertLessThan(background.redComponent, 0.5) }
            let png = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
            try png.write(to: URL(fileURLWithPath: "/tmp/muxterm-ssh-ports-\(name).png"))
        }
    }
}
