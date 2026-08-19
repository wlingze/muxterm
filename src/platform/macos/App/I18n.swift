import Foundation

/// JSON catalog 的 typed id。业务代码只能引用这个 enum，避免手写 key。
enum MuxtermTextKey: CaseIterable {
    case cancel
    case chooseDirectoryMessage
    case chooseRemoteDirectory
    case chooseSshHost
    case chooseTmuxDirectory
    case chooseTmuxSession
    case closePane
    case closePaneDetail
    case closeTab
    case closeTabDetail
    case closeWindow
    case closeWindowDetail
    case cmdNewPane
    case cmdNewPaneVertical
    case cmdOpenConfig
    case cmdPreferences
    case cmdReloadConfig
    case cmdRenamePane
    case cmdSearchPanes
    case cmdSshConnect
    case cmdSshDisconnect
    case cmdSwitchPaneNext
    case cmdSwitchPanePrevious
    case cmdSwitchTab
    case cmdTmuxAttach
    case cmdTmuxDetach
    case cmdTmuxNew
    case commandPalette
    case commandPalettePlaceholder
    case createAndAttach
    case detach
    case detachDetail
    case errorBridgeConnect
    case errorBridgeCreate
    case errorClosePane
    case errorCloseTab
    case errorCommandFailed
    case errorCoreDiscoveryInvalidJson
    case errorCoreDiscoveryInvalidUtf8
    case errorCoreDiscoveryNoResponse
    case errorCorePoll
    case errorCoreUnavailable
    case errorCreateFailed
    case errorMainWindowUnavailable
    case errorNewTab
    case errorNoSshHosts
    case errorPaletteFailed
    case errorResizeClient
    case errorResizeDivider
    case errorResizePane
    case errorSendControl
    case errorSendInput
    case errorSplitPane
    case errorSshConfig
    case errorSshHostDiscovery
    case errorSwitchPane
    case errorSwitchTab
    case errorTmuxSessionCreation
    case errorTmuxSessionDiscovery
    case freeformUseTypedTarget
    case hintNewTab
    case hintQuit
    case hintSplit
    case hintVerticalSplit
    case language
    case languageCurrent
    case languageDetail
    case languageEnglish
    case languageSimplifiedChinese
    case languageSystem
    case layoutSyncing
    case local
    case localTmuxSessions
    case menuAbout
    case menuClosePane
    case menuCloseWindow
    case menuCommandPalette
    case menuCopy
    case menuDecreaseFontSize
    case menuEdit
    case menuFile
    case menuIncreaseFontSize
    case menuNewTab
    case menuNextPane
    case menuPaste
    case menuPreviousPane
    case menuQuit
    case menuResetFontSize
    case menuSearchPanes
    case menuSelectAll
    case menuSplitHorizontal
    case menuSplitVertical
    case menuSwitchTab
    case menuTabBarBottom
    case menuTabBarTop
    case tabBarTopDetail
    case tabBarBottomDetail
    case menuView
    case menuWindow
    case newSession
    case newSessionDetail
    case newTab
    case newTabDetail
    case newTabTooltip
    case nextPane
    case nextPaneDetail
    case pane
    case paneRenameAction
    case paneRenameCancel
    case paneRenameHint
    case paneRenameTitle
    case paneSearchPlaceholder
    case panes
    case paneAccessibility
    case previousPane
    case previousPaneDetail
    case quitMuxterm
    case quitMuxtermDetail
    case rename
    case renameTab
    case renameTabDetail
    case renameWorkspace
    case renameWorkspaceDetail
    case remoteDirectoryMessage
    case splitPaneHorizontal
    case splitPaneHorizontalDetail
    case splitPaneVertical
    case splitPaneVerticalDetail
    case ssh
    case sshHosts
    case statusConnected
    case statusConnecting
    case statusDisconnected
    case statusError
    case statusExited
    case statusUnknown
    case statusBarModeSwitchTo
    case statusBarModeDetail
    case statusAttention
    case statusDone
    case tabs
    case tabsAccessibility
    case terminalOutputSnippet
    case terminalPane
    case togglePaneFullscreen
    case togglePaneFullscreenDetail
    case themeSwitchTo
    case themeDetail
    case tmuxAttachPlaceholder
    case tmuxAttached
    case tmuxCreateDetail
    case tmuxCreateNew
    case tmuxDaysAgo
    case tmuxHoursAgo
    case tmuxMinutesAgo
    case tmuxSecondsAgo
    case tmuxSessionDetail
    case tmuxUnknown
    case tmuxWindows
    case windowCloseHint

    var id: String {
        switch self {
        case .cancel: return "cancel"
        case .chooseDirectoryMessage: return "choose_directory_message"
        case .chooseRemoteDirectory: return "choose_remote_directory"
        case .chooseSshHost: return "choose_ssh_host"
        case .chooseTmuxDirectory: return "choose_tmux_directory"
        case .chooseTmuxSession: return "choose_tmux_session"
        case .closePane: return "close_pane"
        case .closePaneDetail: return "close_pane_detail"
        case .closeTab: return "close_tab"
        case .closeTabDetail: return "close_tab_detail"
        case .closeWindow: return "close_window"
        case .closeWindowDetail: return "close_window_detail"
        case .cmdNewPane: return "cmd_new_pane"
        case .cmdNewPaneVertical: return "cmd_new_pane_vertical"
        case .cmdOpenConfig: return "cmd_open_config"
        case .cmdPreferences: return "cmd_preferences"
        case .cmdReloadConfig: return "cmd_reload_config"
        case .cmdRenamePane: return "cmd_rename_pane"
        case .cmdSearchPanes: return "cmd_search_panes"
        case .cmdSshConnect: return "cmd_ssh_connect"
        case .cmdSshDisconnect: return "cmd_ssh_disconnect"
        case .cmdSwitchPaneNext: return "cmd_switch_pane_next"
        case .cmdSwitchPanePrevious: return "cmd_switch_pane_previous"
        case .cmdSwitchTab: return "cmd_switch_tab"
        case .cmdTmuxAttach: return "cmd_tmux_attach"
        case .cmdTmuxDetach: return "cmd_tmux_detach"
        case .cmdTmuxNew: return "cmd_tmux_new"
        case .commandPalette: return "command_palette"
        case .commandPalettePlaceholder: return "command_palette_placeholder"
        case .createAndAttach: return "create_and_attach"
        case .detach: return "detach"
        case .detachDetail: return "detach_detail"
        case .errorBridgeConnect: return "error_bridge_connect"
        case .errorBridgeCreate: return "error_bridge_create"
        case .errorClosePane: return "error_close_pane"
        case .errorCloseTab: return "error_close_tab"
        case .errorCommandFailed: return "error_command_failed"
        case .errorCoreDiscoveryInvalidJson: return "error_core_discovery_invalid_json"
        case .errorCoreDiscoveryInvalidUtf8: return "error_core_discovery_invalid_utf8"
        case .errorCoreDiscoveryNoResponse: return "error_core_discovery_no_response"
        case .errorCorePoll: return "error_core_poll"
        case .errorCoreUnavailable: return "error_core_unavailable"
        case .errorCreateFailed: return "error_create_failed"
        case .errorMainWindowUnavailable: return "error_main_window_unavailable"
        case .errorNewTab: return "error_new_tab"
        case .errorNoSshHosts: return "error_no_ssh_hosts"
        case .errorPaletteFailed: return "error_palette_failed"
        case .errorResizeClient: return "error_resize_client"
        case .errorResizeDivider: return "error_resize_divider"
        case .errorResizePane: return "error_resize_pane"
        case .errorSendControl: return "error_send_control"
        case .errorSendInput: return "error_send_input"
        case .errorSplitPane: return "error_split_pane"
        case .errorSshConfig: return "error_ssh_config"
        case .errorSshHostDiscovery: return "error_ssh_host_discovery"
        case .errorSwitchPane: return "error_switch_pane"
        case .errorSwitchTab: return "error_switch_tab"
        case .errorTmuxSessionCreation: return "error_tmux_session_creation"
        case .errorTmuxSessionDiscovery: return "error_tmux_session_discovery"
        case .freeformUseTypedTarget: return "freeform_use_typed_target"
        case .hintNewTab: return "hint_new_tab"
        case .hintQuit: return "hint_quit"
        case .hintSplit: return "hint_split"
        case .hintVerticalSplit: return "hint_vertical_split"
        case .language: return "language"
        case .languageCurrent: return "language_current"
        case .languageDetail: return "language_detail"
        case .languageEnglish: return "language_english"
        case .languageSimplifiedChinese: return "language_simplified_chinese"
        case .languageSystem: return "language_system"
        case .layoutSyncing: return "layout_syncing"
        case .local: return "local"
        case .localTmuxSessions: return "local_tmux_sessions"
        case .menuAbout: return "menu_about"
        case .menuClosePane: return "menu_close_pane"
        case .menuCloseWindow: return "menu_close_window"
        case .menuCommandPalette: return "menu_command_palette"
        case .menuCopy: return "menu_copy"
        case .menuDecreaseFontSize: return "menu_decrease_font_size"
        case .menuEdit: return "menu_edit"
        case .menuFile: return "menu_file"
        case .menuIncreaseFontSize: return "menu_increase_font_size"
        case .menuNewTab: return "menu_new_tab"
        case .menuNextPane: return "menu_next_pane"
        case .menuPaste: return "menu_paste"
        case .menuPreviousPane: return "menu_previous_pane"
        case .menuQuit: return "menu_quit"
        case .menuResetFontSize: return "menu_reset_font_size"
        case .menuSearchPanes: return "menu_search_panes"
        case .menuSelectAll: return "menu_select_all"
        case .menuSplitHorizontal: return "menu_split_horizontal"
        case .menuSplitVertical: return "menu_split_vertical"
        case .menuSwitchTab: return "menu_switch_tab"
        case .menuTabBarBottom: return "menu_tab_bar_bottom"
        case .menuTabBarTop: return "menu_tab_bar_top"
        case .tabBarTopDetail: return "tab_bar_top_detail"
        case .tabBarBottomDetail: return "tab_bar_bottom_detail"
        case .menuView: return "menu_view"
        case .menuWindow: return "menu_window"
        case .newSession: return "new_session"
        case .newSessionDetail: return "new_session_detail"
        case .newTab: return "new_tab"
        case .newTabDetail: return "new_tab_detail"
        case .newTabTooltip: return "new_tab_tooltip"
        case .nextPane: return "next_pane"
        case .nextPaneDetail: return "next_pane_detail"
        case .pane: return "pane"
        case .paneRenameAction: return "pane_rename_action"
        case .paneRenameCancel: return "pane_rename_cancel"
        case .paneRenameHint: return "pane_rename_hint"
        case .paneRenameTitle: return "pane_rename_title"
        case .paneSearchPlaceholder: return "pane_search_placeholder"
        case .panes: return "panes"
        case .paneAccessibility: return "pane_accessibility"
        case .previousPane: return "previous_pane"
        case .previousPaneDetail: return "previous_pane_detail"
        case .quitMuxterm: return "quit_muxterm"
        case .quitMuxtermDetail: return "quit_muxterm_detail"
        case .rename: return "rename"
        case .renameTab: return "rename_tab"
        case .renameTabDetail: return "rename_tab_detail"
        case .renameWorkspace: return "rename_workspace"
        case .renameWorkspaceDetail: return "rename_workspace_detail"
        case .remoteDirectoryMessage: return "remote_directory_message"
        case .splitPaneHorizontal: return "split_pane_horizontal"
        case .splitPaneHorizontalDetail: return "split_pane_horizontal_detail"
        case .splitPaneVertical: return "split_pane_vertical"
        case .splitPaneVerticalDetail: return "split_pane_vertical_detail"
        case .ssh: return "ssh"
        case .sshHosts: return "ssh_hosts"
        case .statusConnected: return "status_connected"
        case .statusConnecting: return "status_connecting"
        case .statusDisconnected: return "status_disconnected"
        case .statusError: return "status_error"
        case .statusExited: return "status_exited"
        case .statusUnknown: return "status_unknown"
        case .statusBarModeSwitchTo: return "statusbar_mode_switch_to"
        case .statusBarModeDetail: return "statusbar_mode_detail"
        case .statusAttention: return "status_attention"
        case .statusDone: return "status_done"
        case .tabs: return "tabs"
        case .tabsAccessibility: return "tabs_accessibility"
        case .terminalOutputSnippet: return "terminal_output_snippet"
        case .terminalPane: return "terminal_pane"
        case .togglePaneFullscreen: return "toggle_pane_fullscreen"
        case .togglePaneFullscreenDetail: return "toggle_pane_fullscreen_detail"
        case .themeSwitchTo: return "theme_switch_to"
        case .themeDetail: return "theme_detail"
        case .tmuxAttachPlaceholder: return "tmux_attach_placeholder"
        case .tmuxAttached: return "tmux_attached"
        case .tmuxCreateDetail: return "tmux_create_detail"
        case .tmuxCreateNew: return "tmux_create_new"
        case .tmuxDaysAgo: return "tmux_days_ago"
        case .tmuxHoursAgo: return "tmux_hours_ago"
        case .tmuxMinutesAgo: return "tmux_minutes_ago"
        case .tmuxSecondsAgo: return "tmux_seconds_ago"
        case .tmuxSessionDetail: return "tmux_session_detail"
        case .tmuxUnknown: return "tmux_unknown"
        case .tmuxWindows: return "tmux_windows"
        case .windowCloseHint: return "window_close_hint"
        }
    }
}

/// macOS 界面语言偏好；`system` 不保存解析后的语言，只保存用户选择。
enum MuxtermLanguage: String, CaseIterable, Equatable {
    case system
    case english = "en"
    case simplifiedChinese = "zh-CN"

    var catalogTag: String {
        switch self {
        case .system:
            return MuxtermLanguage.systemResolved.catalogTag
        case .english:
            return "en"
        case .simplifiedChinese:
            return "zh-CN"
        }
    }

    var displayNameKey: MuxtermTextKey {
        switch self {
        case .system:
            return .languageSystem
        case .english:
            return .languageEnglish
        case .simplifiedChinese:
            return .languageSimplifiedChinese
        }
    }

    static var systemResolved: MuxtermLanguage {
        let preferred = Locale.preferredLanguages.first?.lowercased() ?? "en"
        return preferred.hasPrefix("zh") ? .simplifiedChinese : .english
    }
}

/// 公共 JSON catalog 的 macOS loader。
final class MuxtermI18n {
    static let shared = MuxtermI18n()

    private static let preferenceKey = "muxterm.language"
    private var catalogs: [String: [String: String]] = [:]
    private(set) var language: MuxtermLanguage

    private init() {
        let raw = UserDefaults.standard.string(forKey: Self.preferenceKey) ?? MuxtermLanguage.system.rawValue
        language = MuxtermLanguage(rawValue: raw) ?? .system
    }

    var resolvedLanguage: MuxtermLanguage {
        language == .system ? .systemResolved : language
    }

    @discardableResult
    func setLanguage(_ language: MuxtermLanguage) -> Bool {
        guard self.language != language else { return false }
        self.language = language
        UserDefaults.standard.set(language.rawValue, forKey: Self.preferenceKey)
        NotificationCenter.default.post(name: .muxtermLanguageChanged, object: self)
        return true
    }

    func tr(_ key: MuxtermTextKey, arguments: [String: String] = [:]) -> String {
        var value = catalog(for: resolvedLanguage)[key.id]
            ?? catalog(for: .english)[key.id]
            ?? key.id
        for (name, replacement) in arguments {
            value = value.replacingOccurrences(of: "{{\(name)}}", with: replacement)
        }
        return value
    }

    private func catalog(for language: MuxtermLanguage) -> [String: String] {
        let tag = language == .system ? MuxtermLanguage.systemResolved.catalogTag : language.catalogTag
        if let catalog = catalogs[tag] { return catalog }
        let catalog = MuxtermI18nLocator.loadCatalog(tag: tag, roots: MuxtermI18nLocator.searchRoots())
        catalogs[tag] = catalog
        return catalog
    }
}

/// 查找 JSON catalog，**绝不**走 SPM 生成的 `Bundle.module`。
///
/// `swift build` 把 `Bundle.module` 写成：先查 `.app` 根下的 `<name>.bundle`
/// （该位置无法 codesign），再查编译机上的绝对 `.build/...` 路径；
/// 路径不存在就 `fatalError`。装到 `/Applications` 后一旦本地 `.build`
/// 被清掉，启动会在 `StatusBarView.init` → `tr` 上 SIGTRAP。
/// 打包脚本把 catalog 放到 `Contents/Resources/`，这里按同样路径查找。
enum MuxtermI18nLocator {
    static let spmBundleNames = [
        "MuxtermApp_MuxtermAppLib",
        "MuxtermAppLib_MuxtermAppLib",
        "MuxtermApp_MuxtermApp",
    ]

    static func searchRoots(main: Bundle = .main) -> [URL] {
        var roots: [URL] = []
        var seen = Set<String>()
        func add(_ url: URL?) {
            guard let url else { return }
            let path = url.standardizedFileURL.path
            guard seen.insert(path).inserted else { return }
            roots.append(url)
        }

        add(main.bundleURL)
        add(main.resourceURL)
        add(main.executableURL?.deletingLastPathComponent())
        add(main.bundleURL.appendingPathComponent("Contents/Resources"))
        add(main.bundleURL.appendingPathComponent("Contents/MacOS"))
        // SPM 把 resource bundle 放在 .app / .xctest 旁边，不在 bundle 内部。
        add(main.bundleURL.deletingLastPathComponent())

        let owner = Bundle(for: MuxtermI18n.self)
        add(owner.bundleURL)
        add(owner.resourceURL)
        add(owner.executableURL?.deletingLastPathComponent())
        add(owner.bundleURL.deletingLastPathComponent())

        if let argv0 = CommandLine.arguments.first {
            add(URL(fileURLWithPath: argv0).deletingLastPathComponent())
        }
        return roots
    }

    static func catalogFileURLs(tag: String, roots: [URL]) -> [URL] {
        var urls: [URL] = []
        for root in roots {
            for name in spmBundleNames {
                let bundle = root.appendingPathComponent("\(name).bundle")
                urls.append(bundle.appendingPathComponent("i18n/\(tag).json"))
                urls.append(bundle.appendingPathComponent("\(tag).json"))
            }
            urls.append(root.appendingPathComponent("i18n/\(tag).json"))
            urls.append(root.appendingPathComponent("\(tag).json"))
            urls.append(root.appendingPathComponent("Resources/i18n/\(tag).json"))
        }
        return urls
    }

    static func loadCatalog(tag: String, roots: [URL]) -> [String: String] {
        for url in catalogFileURLs(tag: tag, roots: roots) {
            guard let data = try? Data(contentsOf: url),
                  let catalog = try? JSONDecoder().decode([String: String].self, from: data),
                  !catalog.isEmpty
            else { continue }
            return catalog
        }
        return [:]
    }

    static func localizedString(
        key: MuxtermTextKey,
        language: MuxtermLanguage,
        arguments: [String: String] = [:],
        roots: [URL]
    ) -> String {
        let tag = language.catalogTag
        let primary = loadCatalog(tag: tag, roots: roots)
        let english = tag == "en" ? primary : loadCatalog(tag: "en", roots: roots)
        var value = primary[key.id] ?? english[key.id] ?? key.id
        for (name, replacement) in arguments {
            value = value.replacingOccurrences(of: "{{\(name)}}", with: replacement)
        }
        return value
    }
}

extension Notification.Name {
    static let muxtermLanguageChanged = Notification.Name("muxterm.languageChanged")
}
