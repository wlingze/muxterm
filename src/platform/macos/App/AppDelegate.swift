import AppKit

/// NSApplication 入口：解析启动参数、创建 CoreBridge、打开主窗口。
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var mainWindow: MainWindowController?

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)

        do {
            let (backend, socket, session) = Self.resolveBackend(from: CommandLine.arguments)
            let bridge = try CoreBridge(backendType: backend, socket: socket, session: session)
            if socket != nil {
                Thread.sleep(forTimeInterval: 0.3)
                _ = bridge.pollEvents()
            }
            let wc = MainWindowController(bridge: bridge)
            mainWindow = wc
            buildMenu(windowController: wc)
            Self.bringToForeground(windowController: wc)
            // UITest：多抢几次前台（macOS 对 focus-steal 更严，且 XCUITest 依赖 runningForeground）
            if ProcessInfo.processInfo.environment["MUXTERM_UITEST"] == "1" {
                for delay in [0.0, 0.15, 0.4] {
                    DispatchQueue.main.asyncAfter(deadline: .now() + delay) {
                        Self.bringToForeground(windowController: wc)
                    }
                }
            }
        } catch {
            buildMenu(windowController: nil)
            let alert = NSAlert()
            alert.messageText = "无法连接 Muxterm 核心"
            alert.informativeText = error.localizedDescription
            alert.alertStyle = .critical
            alert.runModal()
            NSApp.terminate(nil)
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }

    // MARK: - 菜单

    private func buildMenu(windowController: MainWindowController?) {
        let mainMenu = NSMenu()

        let appMenuItem = NSMenuItem()
        mainMenu.addItem(appMenuItem)
        let appMenu = NSMenu()
        appMenuItem.submenu = appMenu
        appMenu.addItem(
            withTitle: "关于 Muxterm",
            action: #selector(NSApplication.orderFrontStandardAboutPanel(_:)),
            keyEquivalent: ""
        )
        appMenu.addItem(NSMenuItem.separator())
        appMenu.addItem(
            withTitle: "退出 Muxterm",
            action: #selector(NSApplication.terminate(_:)),
            keyEquivalent: "q"
        )

        let fileMenuItem = NSMenuItem()
        mainMenu.addItem(fileMenuItem)
        let fileMenu = NSMenu(title: "文件")
        fileMenuItem.submenu = fileMenu

        let newTab = NSMenuItem(
            title: "新建标签页",
            action: #selector(MainWindowController.newTab),
            keyEquivalent: "t"
        )
        newTab.target = windowController
        fileMenu.addItem(newTab)

        let closePane = NSMenuItem(
            title: "关闭 Pane",
            action: #selector(MainWindowController.closeActivePane),
            keyEquivalent: "d"
        )
        closePane.keyEquivalentModifierMask = .control
        closePane.target = windowController
        fileMenu.addItem(closePane)

        let closeWindow = NSMenuItem(
            title: "关闭窗口",
            action: #selector(MainWindowController.closeActiveWindow),
            keyEquivalent: "w"
        )
        closeWindow.target = windowController
        fileMenu.addItem(closeWindow)

        // Window 菜单：Cmd+1..9 切 tab（避免落到 SwiftTerm noop:）
        let windowMenuItem = NSMenuItem()
        mainMenu.addItem(windowMenuItem)
        let windowMenu = NSMenu(title: "窗口")
        windowMenuItem.submenu = windowMenu
        for i in 1...9 {
            let item = NSMenuItem(
                title: "切换到标签页 \(i)",
                action: #selector(MainWindowController.switchTabByNumber(_:)),
                keyEquivalent: "\(i)"
            )
            item.tag = i
            item.target = windowController
            item.keyEquivalentModifierMask = .command
            windowMenu.addItem(item)
        }

        let editMenuItem = NSMenuItem()
        mainMenu.addItem(editMenuItem)
        let editMenu = NSMenu(title: "编辑")
        editMenuItem.submenu = editMenu
        editMenu.addItem(withTitle: "拷贝", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        editMenu.addItem(withTitle: "粘贴", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        editMenu.addItem(withTitle: "全选", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")

        let viewMenuItem = NSMenuItem()
        mainMenu.addItem(viewMenuItem)
        let viewMenu = NSMenu(title: "视图")
        viewMenuItem.submenu = viewMenu

        let splitH = NSMenuItem(
            title: "水平分割",
            action: #selector(MainWindowController.splitHorizontal),
            keyEquivalent: "d"
        )
        splitH.keyEquivalentModifierMask = .command
        splitH.target = windowController
        viewMenu.addItem(splitH)

        let splitV = NSMenuItem(
            title: "竖直分割",
            action: #selector(MainWindowController.splitVertical),
            keyEquivalent: "d"
        )
        splitV.keyEquivalentModifierMask = [.command, .shift]
        splitV.target = windowController
        viewMenu.addItem(splitV)

        let nextPane = NSMenuItem(
            title: "下一个 Pane",
            action: #selector(MainWindowController.nextPane),
            keyEquivalent: "]"
        )
        nextPane.target = windowController
        viewMenu.addItem(nextPane)

        let prevPane = NSMenuItem(
            title: "上一个 Pane",
            action: #selector(MainWindowController.prevPane),
            keyEquivalent: "["
        )
        prevPane.target = windowController
        viewMenu.addItem(prevPane)

        let commandPalette = NSMenuItem(
            title: "命令面板",
            action: #selector(MainWindowController.openCommandPalette),
            keyEquivalent: "p"
        )
        commandPalette.keyEquivalentModifierMask = [.command, .shift]
        commandPalette.target = windowController
        viewMenu.addItem(commandPalette)

        viewMenu.addItem(NSMenuItem.separator())

        let tabTop = NSMenuItem(
            title: "标签栏在顶部",
            action: #selector(MainWindowController.setTabBarTop(_:)),
            keyEquivalent: ""
        )
        tabTop.target = windowController
        viewMenu.addItem(tabTop)

        let tabBottom = NSMenuItem(
            title: "标签栏在底部",
            action: #selector(MainWindowController.setTabBarBottom(_:)),
            keyEquivalent: ""
        )
        tabBottom.target = windowController
        viewMenu.addItem(tabBottom)

        NSApp.mainMenu = mainMenu
    }

    /// 强制窗口可见并尝试成为前台应用。
    private static func bringToForeground(windowController: MainWindowController) {
        windowController.showWindow(nil)
        if let window = windowController.window {
            window.orderFrontRegardless()
            window.makeKeyAndOrderFront(nil)
        }
        NSApp.activate(ignoringOtherApps: true)
        if #available(macOS 14.0, *) {
            NSRunningApplication.current.activate(options: [.activateAllWindows])
        } else {
            NSRunningApplication.current.activate(options: [.activateAllWindows, .activateIgnoringOtherApps])
        }
    }

    /// 解析 CLI：`-L sock` → tmux；`-s name`（无 -L）→ daemon；都无 → local。
    static func resolveBackend(from args: [String]) -> (String, String?, String?) {
        var socket: String?
        var session: String?
        var i = 1
        while i < args.count {
            let a = args[i]
            if a == "-L" || a == "--socket", i + 1 < args.count {
                socket = args[i + 1]
                i += 2
                continue
            }
            if a == "-s" || a == "--session", i + 1 < args.count {
                session = args[i + 1]
                i += 2
                continue
            }
            i += 1
        }
        if socket != nil {
            return ("tmux", socket, session)
        }
        if session != nil {
            return ("daemon", nil, session)
        }
        return ("local", nil, nil)
    }
}
