import AppKit
import UserNotifications

/// 系统通知授权状态的纯策略，避免把权限弹窗带进 XCTest / XCUITest。
enum NativeNotificationAuthorizationPolicy {
    static func shouldRequest(
        _ status: UNAuthorizationStatus,
        hasPendingNotification: Bool
    ) -> Bool {
        hasPendingNotification && status == .notDetermined
    }

    static func canDeliver(_ status: UNAuthorizationStatus) -> Bool {
        switch status {
        case .authorized, .provisional:
            return true
        case .notDetermined, .denied:
            return false
        @unknown default:
            return false
        }
    }
}

enum NativeNotificationLogPolicy {
    static func shouldLogAuthorizationError(
        previouslyLogged: Bool,
        hasSystemError: Bool
    ) -> Bool {
        hasSystemError && !previouslyLogged
    }
}

/// macOS 原生通知入口：第一次确有通知时才请求权限；前台也显示 banner，
/// 点击后回到 Attention。用户拒绝是正常状态，Attention 面板仍然可用。
final class NativeNotificationService: NSObject, UNUserNotificationCenterDelegate {
    static let shared = NativeNotificationService()

    private var onActivate: (() -> Void)?
    private let authorizationLock = NSLock()
    private var authorizationRequestInFlight = false
    private var authorizationErrorLogged = false
    private var pendingAuthorizationCompletions: [(Bool) -> Void] = []

    static var isSuppressedProcess: Bool {
        ProcessInfo.processInfo.environment["XCTestConfigurationFilePath"] != nil
            || ProcessInfo.processInfo.environment["MUXTERM_UITEST"] == "1"
            || NSClassFromString("XCTestCase") != nil
    }

    func configure(onActivate: @escaping () -> Void) {
        self.onActivate = onActivate
        guard !Self.isSuppressedProcess else { return }
        let center = UNUserNotificationCenter.current()
        center.delegate = self
    }

    func post(title: String, body: String) {
        guard !Self.isSuppressedProcess else { return }
        let center = UNUserNotificationCenter.current()
        center.getNotificationSettings { settings in
            if NativeNotificationAuthorizationPolicy.canDeliver(settings.authorizationStatus) {
                Self.deliver(title: title, body: body, on: center)
            } else if NativeNotificationAuthorizationPolicy.shouldRequest(
                settings.authorizationStatus,
                hasPendingNotification: true
            ) {
                self.requestAuthorization(on: center) { granted in
                    guard granted else { return }
                    Self.deliver(title: title, body: body, on: center)
                }
            }
        }
    }

    private func requestAuthorization(
        on center: UNUserNotificationCenter,
        completion: ((Bool) -> Void)? = nil
    ) {
        authorizationLock.lock()
        if authorizationRequestInFlight {
            if let completion {
                pendingAuthorizationCompletions.append(completion)
            }
            authorizationLock.unlock()
            return
        }
        authorizationRequestInFlight = true
        if let completion {
            pendingAuthorizationCompletions.append(completion)
        }
        authorizationLock.unlock()

        center.requestAuthorization(options: [.alert, .sound]) { granted, error in
            self.authorizationLock.lock()
            self.authorizationRequestInFlight = false
            let completions = self.pendingAuthorizationCompletions
            self.pendingAuthorizationCompletions.removeAll()
            let shouldLog = NativeNotificationLogPolicy.shouldLogAuthorizationError(
                previouslyLogged: self.authorizationErrorLogged,
                hasSystemError: error != nil
            )
            if shouldLog {
                self.authorizationErrorLogged = true
            }
            self.authorizationLock.unlock()

            if shouldLog, let error {
                // 真正的 UserNotifications API 错误同一进程只记录一次。
                NSLog("muxterm: notification authorization request failed: %@", error.localizedDescription)
            }
            for completion in completions {
                completion(granted)
            }
        }
    }

    private static func deliver(
        title: String,
        body: String,
        on center: UNUserNotificationCenter
    ) {
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        content.sound = .default
        content.threadIdentifier = "muxterm.attention"
        let request = UNNotificationRequest(
            identifier: "muxterm.attention.\(UUID().uuidString)",
            content: content,
            trigger: nil
        )
        center.add(request) { error in
            if let error {
                NSLog("muxterm: notification delivery failed: %@", error.localizedDescription)
            }
        }
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        completionHandler([.banner, .list, .sound])
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        // 点击后从通知中心移除已展示的同一条，避免用户已经查看但系统列表
        // 仍长期保留旧提醒；Attention 面板的状态由 pane acknowledge 负责。
        let identifier = response.notification.request.identifier
        center.removeDeliveredNotifications(withIdentifiers: [identifier])
        center.removePendingNotificationRequests(withIdentifiers: [identifier])
        DispatchQueue.main.async { [weak self] in
            self?.onActivate?()
        }
        completionHandler()
    }
}

/// NSApplication 入口：解析启动参数、创建 CoreBridge、打开主窗口。
public final class AppDelegate: NSObject, NSApplicationDelegate {
    private var mainWindow: MainWindowController?
    private var languageObserver: NSObjectProtocol?

    public override init() {
        super.init()
    }

    public func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)

        do {
            let options = Self.resolveBackend(from: CommandLine.arguments)
            // 调试日志：CLI `muxterm gui --debug --log-file` 转发参数，这里在
            // app 进程内初始化 core 的 tracing（写文件或 stderr）。
            if options.debug || options.logFile != nil {
                let rc = CoreBridge.initLogging(debug: options.debug, logFile: options.logFile)
                if rc != 0 {
                    NSLog("muxterm: 初始化日志失败")
                }
            }
            let (backend, socket, session) = (
                options.backend,
                options.socket,
                options.session,
            )
            let bridge = try CoreBridge(backendType: backend, socket: socket, session: session)
            if socket != nil {
                Thread.sleep(forTimeInterval: 0.3)
                _ = bridge.pollEvents()
            }
            let wc = MainWindowController(bridge: bridge, debug: options.debug)
            mainWindow = wc
            NativeNotificationService.shared.configure { [weak wc] in
                wc?.revealAttentionFromSystemNotification()
            }
            buildMenu(windowController: wc)
            languageObserver = NotificationCenter.default.addObserver(
                forName: .muxtermLanguageChanged,
                object: nil,
                queue: .main
            ) { [weak self] _ in
                guard let self else { return }
                self.buildMenu(windowController: self.mainWindow)
            }
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
            alert.messageText = MuxtermI18n.shared.tr(.errorCoreUnavailable)
            alert.informativeText = error.localizedDescription
            alert.alertStyle = .critical
            alert.runModal()
            NSApp.terminate(nil)
        }
    }

    public func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
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
            withTitle: MuxtermI18n.shared.tr(.menuAbout),
            action: #selector(NSApplication.orderFrontStandardAboutPanel(_:)),
            keyEquivalent: ""
        )
        let preferences = NSMenuItem(
            title: MuxtermI18n.shared.tr(.cmdPreferences),
            action: #selector(MainWindowController.openPreferences),
            keyEquivalent: ","
        )
        preferences.keyEquivalentModifierMask = .command
        preferences.target = windowController
        appMenu.addItem(preferences)
        appMenu.addItem(NSMenuItem.separator())
        appMenu.addItem(
            withTitle: MuxtermI18n.shared.tr(.menuQuit),
            action: #selector(NSApplication.terminate(_:)),
            keyEquivalent: "q"
        )

        let fileMenuItem = NSMenuItem()
        mainMenu.addItem(fileMenuItem)
        let fileMenu = NSMenu(title: MuxtermI18n.shared.tr(.menuFile))
        fileMenuItem.submenu = fileMenu

        let newTab = NSMenuItem(
            title: MuxtermI18n.shared.tr(.menuNewTab),
            action: #selector(MainWindowController.newTab),
            keyEquivalent: "t"
        )
        newTab.target = windowController
        fileMenu.addItem(newTab)

        let renameWorkspace = NSMenuItem(
            title: MuxtermI18n.shared.tr(.renameWorkspace),
            action: #selector(MainWindowController.renameCurrentWorkspace),
            keyEquivalent: ""
        )
        renameWorkspace.target = windowController
        fileMenu.addItem(renameWorkspace)

        let closePane = NSMenuItem(
            title: MuxtermI18n.shared.tr(.menuClosePane),
            action: #selector(MainWindowController.closeActivePane),
            keyEquivalent: ""
        )
        closePane.target = windowController
        fileMenu.addItem(closePane)

        let closeWindow = NSMenuItem(
            title: MuxtermI18n.shared.tr(.menuCloseWindow),
            action: #selector(MainWindowController.closeActiveWindow),
            keyEquivalent: "w"
        )
        closeWindow.target = windowController
        fileMenu.addItem(closeWindow)

        // Window 菜单：Cmd+1..9 切 tab（避免落到 SwiftTerm noop:）
        let windowMenuItem = NSMenuItem()
        mainMenu.addItem(windowMenuItem)
        let windowMenu = NSMenu(title: MuxtermI18n.shared.tr(.menuWindow))
        windowMenuItem.submenu = windowMenu
        let renameTab = NSMenuItem(
            title: MuxtermI18n.shared.tr(.renameTab),
            action: #selector(MainWindowController.renameActiveTab),
            keyEquivalent: ""
        )
        renameTab.target = windowController
        windowMenu.addItem(renameTab)
        let moveTabLeft = NSMenuItem(
            title: MuxtermI18n.shared.tr(.moveTabLeft),
            action: #selector(MainWindowController.moveActiveTabLeft),
            keyEquivalent: ""
        )
        moveTabLeft.target = windowController
        windowMenu.addItem(moveTabLeft)
        let moveTabRight = NSMenuItem(
            title: MuxtermI18n.shared.tr(.moveTabRight),
            action: #selector(MainWindowController.moveActiveTabRight),
            keyEquivalent: ""
        )
        moveTabRight.target = windowController
        windowMenu.addItem(moveTabRight)
        windowMenu.addItem(NSMenuItem.separator())
        for i in 1...9 {
            let item = NSMenuItem(
                title: MuxtermI18n.shared.tr(.menuSwitchTab, arguments: ["number": "\(i)"]),
                action: #selector(MainWindowController.switchTabByNumber(_:)),
                keyEquivalent: "\(i)"
            )
            item.tag = i
            item.target = windowController
            item.keyEquivalentModifierMask = .command
            windowMenu.addItem(item)
        }
        let lastTab = NSMenuItem(
            title: MuxtermI18n.shared.tr(.switchLastTab),
            action: #selector(MainWindowController.switchToLastTab),
            keyEquivalent: "0"
        )
        lastTab.target = windowController
        lastTab.keyEquivalentModifierMask = .option
        windowMenu.addItem(lastTab)

        let editMenuItem = NSMenuItem()
        mainMenu.addItem(editMenuItem)
        let editMenu = NSMenu(title: MuxtermI18n.shared.tr(.menuEdit))
        editMenuItem.submenu = editMenu
        editMenu.addItem(withTitle: MuxtermI18n.shared.tr(.menuCopy), action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        editMenu.addItem(withTitle: MuxtermI18n.shared.tr(.menuPaste), action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        editMenu.addItem(withTitle: MuxtermI18n.shared.tr(.menuSelectAll), action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")

        let viewMenuItem = NSMenuItem()
        mainMenu.addItem(viewMenuItem)
        let viewMenu = NSMenu(title: MuxtermI18n.shared.tr(.menuView))
        viewMenuItem.submenu = viewMenu

        // Cmd+D = 上下（竖直），Cmd+Shift+D = 水平。
        let splitV = NSMenuItem(
            title: MuxtermI18n.shared.tr(.menuSplitVertical),
            action: #selector(MainWindowController.splitVertical),
            keyEquivalent: "d"
        )
        splitV.keyEquivalentModifierMask = .command
        splitV.target = windowController
        viewMenu.addItem(splitV)

        let splitH = NSMenuItem(
            title: MuxtermI18n.shared.tr(.menuSplitHorizontal),
            action: #selector(MainWindowController.splitHorizontal),
            keyEquivalent: "d"
        )
        splitH.keyEquivalentModifierMask = [.command, .shift]
        splitH.target = windowController
        viewMenu.addItem(splitH)

        let movePaneToNewTab = NSMenuItem(
            title: MuxtermI18n.shared.tr(.movePaneToNewTab),
            action: #selector(MainWindowController.moveActivePaneToNewTab),
            keyEquivalent: ""
        )
        movePaneToNewTab.target = windowController
        viewMenu.addItem(movePaneToNewTab)

        let fullscreen = NSMenuItem(
            title: MuxtermI18n.shared.tr(.togglePaneFullscreen),
            action: #selector(MainWindowController.toggleActivePaneFullscreen),
            keyEquivalent: "\r"
        )
        fullscreen.keyEquivalentModifierMask = .command
        fullscreen.target = windowController
        viewMenu.addItem(fullscreen)

        let nextPane = NSMenuItem(
            title: MuxtermI18n.shared.tr(.menuNextPane),
            action: #selector(MainWindowController.nextPane),
            keyEquivalent: "]"
        )
        nextPane.target = windowController
        viewMenu.addItem(nextPane)

        let prevPane = NSMenuItem(
            title: MuxtermI18n.shared.tr(.menuPreviousPane),
            action: #selector(MainWindowController.prevPane),
            keyEquivalent: "["
        )
        prevPane.target = windowController
        viewMenu.addItem(prevPane)

        let previousCommand = NSMenuItem(
            title: MuxtermI18n.shared.tr(.previousCommand),
            action: #selector(MainWindowController.jumpToPreviousCommand),
            keyEquivalent: "\u{F700}"
        )
        previousCommand.keyEquivalentModifierMask = [.command, .option]
        previousCommand.target = windowController
        viewMenu.addItem(previousCommand)

        let nextCommand = NSMenuItem(
            title: MuxtermI18n.shared.tr(.nextCommand),
            action: #selector(MainWindowController.jumpToNextCommand),
            keyEquivalent: "\u{F701}"
        )
        nextCommand.keyEquivalentModifierMask = [.command, .option]
        nextCommand.target = windowController
        viewMenu.addItem(nextCommand)

        let increaseFont = NSMenuItem(
            title: MuxtermI18n.shared.tr(.menuIncreaseFontSize),
            action: #selector(MainWindowController.increaseTerminalFontSize(_:)),
            keyEquivalent: "="
        )
        increaseFont.keyEquivalentModifierMask = [.command, .shift]
        increaseFont.target = windowController
        viewMenu.addItem(increaseFont)

        let decreaseFont = NSMenuItem(
            title: MuxtermI18n.shared.tr(.menuDecreaseFontSize),
            action: #selector(MainWindowController.decreaseTerminalFontSize(_:)),
            keyEquivalent: "-"
        )
        decreaseFont.keyEquivalentModifierMask = .command
        decreaseFont.target = windowController
        viewMenu.addItem(decreaseFont)

        let resetFont = NSMenuItem(
            title: MuxtermI18n.shared.tr(.menuResetFontSize),
            action: #selector(MainWindowController.resetTerminalFontSize(_:)),
            keyEquivalent: "0"
        )
        resetFont.keyEquivalentModifierMask = .command
        resetFont.target = windowController
        viewMenu.addItem(resetFont)

        viewMenu.addItem(NSMenuItem.separator())

        let quickConnectItem = NSMenuItem(
            title: MuxtermI18n.shared.tr(.quickConnect),
            action: #selector(MainWindowController.openQuickConnect),
            keyEquivalent: "p"
        )
        quickConnectItem.keyEquivalentModifierMask = .command
        quickConnectItem.target = windowController
        viewMenu.addItem(quickConnectItem)

        let commandPalette = NSMenuItem(
            title: MuxtermI18n.shared.tr(.menuCommandPalette),
            action: #selector(MainWindowController.openCommandPalette),
            keyEquivalent: "p"
        )
        commandPalette.keyEquivalentModifierMask = [.command, .shift]
        commandPalette.target = windowController
        viewMenu.addItem(commandPalette)

        let searchPanes = NSMenuItem(
            title: MuxtermI18n.shared.tr(.menuSearchPanes),
            action: #selector(MainWindowController.openWorkspaceSearchPanel),
            keyEquivalent: "f"
        )
        searchPanes.keyEquivalentModifierMask = .command
        searchPanes.target = windowController
        viewMenu.addItem(searchPanes)

        let searchAll = NSMenuItem(
            title: MuxtermI18n.shared.tr(.menuSearchPanes),
            action: #selector(MainWindowController.openGlobalSearchPanel),
            keyEquivalent: "f"
        )
        searchAll.keyEquivalentModifierMask = [.command, .shift]
        searchAll.target = windowController
        viewMenu.addItem(searchAll)

        let attention = NSMenuItem(
            title: MuxtermI18n.shared.tr(.panelAttention),
            action: #selector(MainWindowController.openAttentionPanel),
            keyEquivalent: "r"
        )
        attention.keyEquivalentModifierMask = .command
        attention.target = windowController
        viewMenu.addItem(attention)

        viewMenu.addItem(NSMenuItem.separator())

        let tabTop = NSMenuItem(
            title: MuxtermI18n.shared.tr(.menuTabBarTop),
            action: #selector(MainWindowController.setTabBarTop(_:)),
            keyEquivalent: ""
        )
        tabTop.target = windowController
        viewMenu.addItem(tabTop)

        let tabBottom = NSMenuItem(
            title: MuxtermI18n.shared.tr(.menuTabBarBottom),
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

    /// 启动参数：后端类型 + 调试日志选项。
    struct LaunchOptions {
        let backend: String
        let socket: String?
        let session: String?
        let debug: Bool
        let logFile: String?
    }

    /// 解析 CLI：`-L sock` → tmux；`-s name`（无 -L）→ daemon；都无 → local。
    /// 同时识别 `--debug` / `--log-file`（调试日志选项）。
    static func resolveBackend(from args: [String]) -> LaunchOptions {
        var socket: String?
        var session: String?
        var debug = false
        var logFile: String?
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
            if a == "--debug" {
                debug = true
                i += 1
                continue
            }
            if a == "--log-file", i + 1 < args.count {
                logFile = args[i + 1]
                i += 2
                continue
            }
            i += 1
        }
        if socket != nil {
            return LaunchOptions(
                backend: "tmux",
                socket: socket,
                session: session,
                debug: debug,
                logFile: logFile
            )
        } else if session != nil {
            return LaunchOptions(
                backend: "daemon",
                socket: nil,
                session: session,
                debug: debug,
                logFile: logFile
            )
        }
        return LaunchOptions(
            backend: "local",
            socket: nil,
            session: nil,
            debug: debug,
            logFile: logFile
        )
    }
}
