import AppKit
import Foundation
import MuxtermChrome

struct MuxtermConfigTransactionRequest {
    let operations: [[String: Any]]
    let completion: (Result<Void, Error>) -> Void
}

typealias MuxtermConfigTransactionHandler =
    (MuxtermConfigTransactionRequest) -> Bool

/// AppKit settings renderer backed by Core's Schema/Manifest transaction API.
///
/// The window renders every field published by the Core manifest; a new Core
/// field only needs a manifest entry to appear here. Controls are keyed by JSON
/// Pointer and are always written back through `SettingsService` transactions,
/// never by parsing or rewriting `config.toml` in Swift.
func settingsCategoryTitle(id: String, titleKey: String) -> String {
    let keyTail = titleKey.split(separator: ".").last.map(String.init) ?? ""
    let source = keyTail.isEmpty ? id : keyTail
    let fallback = source
        .replacingOccurrences(of: "_", with: " ")
        .replacingOccurrences(of: "-", with: " ")
        .split(separator: " ")
        .map { word in
            guard let first = word.first else { return "" }
            return first.uppercased() + word.dropFirst()
        }
        .joined(separator: " ")
    return settingsText(titleKey.isEmpty ? "settings.\(id)" : titleKey, fallback: fallback)
}

private func settingsText(
    _ key: String,
    fallback: String,
    arguments: [String: String] = [:]
) -> String {
    MuxtermI18n.shared.trDynamic(key, fallback: fallback, arguments: arguments)
}

private func settingsHumanize(_ raw: String) -> String {
    raw.replacingOccurrences(of: ".", with: " ")
        .replacingOccurrences(of: "/", with: " ")
        .replacingOccurrences(of: "_", with: " ")
        .replacingOccurrences(of: "-", with: " ")
        .split(separator: " ")
        .map { word in
            guard let first = word.first else { return "" }
            return first.uppercased() + word.dropFirst()
        }
        .joined(separator: " ")
}

private func settingsFieldTitle(path: String, titleKey: String) -> String {
    let fallback: String = switch path {
    case "/font/family": "Font family"
    case "/font/size": "Font size"
    case "/font/fallback": "Fallback fonts"
    case "/theme/name": "Theme"
    case "/theme/light": "Light theme"
    case "/theme/dark": "Dark theme"
    case "/statusbar/mode": "Status bar appearance"
    case "/tmux/auto_mouse": "Enable tmux mouse mode"
    case "/tmux/default_session": "Default workspace"
    case "/tmux/socket": "tmux socket"
    case "/pool/max_slots": "Workspace reminder limit"
    case "/scrollback/lines": "Scrollback lines"
    case "/pane/default_command": "Default shell command"
    case "/pane/workdir": "Initial working directory"
    case "/attention/enabled": "Workspace attention"
    case "/attention/blocked_regex": "Blocked output patterns"
    case "/attention/debounce_ms": "Notification delay"
    case "/ui/tab_bar_position": "Tab bar position"
    case "/ui/tab_bar_style": "Tab width"
    case "/ui/tab_bar_height": "Tab bar height"
    case "/ui/show_title_bar": "Show title bar"
    case "/ui/borderless": "Borderless window"
    case "/ssh/host": "Default SSH host"
    case "/ssh/port": "SSH port"
    case "/ssh/user": "SSH user"
    case "/ssh/key_path": "SSH private key"
    case "/behavior/on_last_pane_exit": "When the last pane exits"
    case "/behavior/on_program_exit_abnormal": "When a command fails"
    case "/platform/linux/client_side_decorations": "Client-side decorations"
    case "/platform/macos/option_as_alt": "Treat Option as Alt"
    case "macos.developer_tools": "Developer Tools"
    case "/shortcuts/preset": "Keyboard layout"
    case "/shortcuts/primary_key": "Primary modifier"
    case "/projects": "Saved projects"
    case "/shortcuts/overrides": "Custom shortcuts"
    default: settingsHumanize(
        titleKey.hasPrefix("settings.")
            ? String(titleKey.dropFirst("settings.".count))
            : (path.split(separator: "/").last.map(String.init) ?? path)
    )
    }
    return settingsText(titleKey, fallback: fallback)
}

private func settingsFieldDescription(path: String, titleKey: String) -> String {
    let fallback: String = switch path {
    case "/font/family": "The typeface used to draw terminal text."
    case "/font/size": "Adjust the terminal scale without changing your display settings."
    case "/font/fallback": "Comma-separated fonts used when the primary family is missing a glyph."
    case "/theme/name": "Choose a fixed theme or follow your system appearance."
    case "/theme/light": "Theme used when the system is in light mode."
    case "/theme/dark": "Theme used when the system is in dark mode."
    case "/statusbar/mode": "Match tmux status text, colors, and title widths, or use equal-width Muxterm tabs."
    case "/tmux/auto_mouse": "Forward mouse interactions to attached tmux workspaces."
    case "/tmux/default_session": "Workspace to attach on launch; leave empty to start locally."
    case "/tmux/socket": "Optional named tmux socket. Empty uses the default server."
    case "/pool/max_slots": "Show a reminder when this many open workspaces are retained."
    case "/scrollback/lines": "History kept for each newly created pane."
    case "/pane/default_command": "Command started for a new local pane."
    case "/pane/workdir": "Directory used when a new local pane starts."
    case "/attention/enabled": "Show attention badges when a workspace is waiting for you."
    case "/attention/blocked_regex": "One regular expression per line that marks output as blocked."
    case "/attention/debounce_ms": "Wait this long before raising a new attention signal."
    case "/ui/tab_bar_position": "Place the workspace tab bar above or below the terminal."
    case "/ui/tab_bar_style": "For workspaces without tmux status data, fill the bar equally or fit each tab to its title."
    case "/ui/tab_bar_height": "Height of the compact tab bar in pixels."
    case "/ui/show_title_bar": "Keep the native window title visible."
    case "/ui/borderless": "Remove the outer window border when supported by the desktop."
    case "/ssh/host": "Fallback SSH host used by remote connections."
    case "/ssh/port": "TCP port used for the default SSH connection."
    case "/ssh/user": "Remote user name; empty uses the current local user."
    case "/ssh/key_path": "Private key path; empty allows ssh-agent to provide credentials."
    case "/behavior/on_last_pane_exit": "Choose what remains after the final pane closes."
    case "/behavior/on_program_exit_abnormal": "Choose how Muxterm handles a non-zero command exit."
    case "/platform/linux/client_side_decorations": "Let Muxterm draw its own window controls."
    case "/platform/macos/option_as_alt": "Use the Option key as an Alt modifier on macOS."
    case "macos.developer_tools":
        "Lets Muxterm run locally built, unsigned tools. macOS will list this app under Privacy & Security → Developer Tools; you still have to turn the switch on."
    case "/shortcuts/preset": "Start from a QWERTY or Colemak action layout."
    case "/shortcuts/primary_key": "Modifier used for the primary shortcut set."
    case "/projects": "Reusable workspace launch profiles shared by Quick Connect."
    case "/shortcuts/overrides": "Override or disable individual action bindings."
    default: "Configure this setting for new Muxterm sessions."
    }
    return settingsText("\(titleKey).description", fallback: fallback)
}

private func settingsApplyLabel(_ mode: String) -> String {
    let fallback: String
    switch mode {
    case "immediate": fallback = "LIVE"
    case "next_workspace": fallback = "NEXT WORKSPACE"
    case "macos": fallback = "macOS"
    default: fallback = "ON SAVE"
    }
    return settingsText("settings.apply.\(mode)", fallback: fallback)
}

private func settingsOptionLabel(path: String, value: String) -> String {
    let fallback: String = switch (path, value) {
    case ("/theme/name", "system"): "Follow system"
    case ("/theme/name", "black"), ("/theme/dark", "black"), ("/theme/light", "black"): "Black"
    case ("/theme/name", "white"), ("/theme/dark", "white"), ("/theme/light", "white"): "White"
    case ("/statusbar/mode", "tmux"): "Match tmux"
    case ("/statusbar/mode", "theme"): "Use Muxterm theme"
    case ("/ui/tab_bar_position", "top"): "Top"
    case ("/ui/tab_bar_position", "bottom"): "Bottom"
    case ("/ui/tab_bar_style", "equal_width"): "Fill available width"
    case ("/ui/tab_bar_style", "compact"): "Fit title"
    case ("/behavior/on_last_pane_exit", "close_window"): "Close the window"
    case ("/behavior/on_last_pane_exit", "keep_empty"): "Keep an empty window"
    case ("/behavior/on_last_pane_exit", "new_shell"): "Open a new shell"
    case ("/behavior/on_program_exit_abnormal", "notify"): "Keep and notify"
    case ("/behavior/on_program_exit_abnormal", "close"): "Close the pane"
    case ("/behavior/on_program_exit_abnormal", "keep"): "Keep the pane"
    case ("/shortcuts/primary_key", "auto"): "Automatic"
    case ("/shortcuts/primary_key", "alt"): "Alt"
    case ("/shortcuts/primary_key", "command"): "Command"
    case ("/shortcuts/primary_key", "control"): "Control"
    case ("/shortcuts/primary_key", "super"): "Super"
    default: settingsHumanize(value)
    }
    return settingsText("settings.option.\(value)", fallback: fallback)
}

private func settingsCategoryHint(_ id: String) -> String {
    let fallback: String = switch id {
    case "appearance": "Fonts & colors"
    case "runtime": "Workspaces"
    case "attention": "Agent signals"
    case "ui": "Window chrome"
    case "ssh": "Remote access"
    case "behavior": "Exit rules"
    case "platform": "Desktop specific"
    case "projects": "Launch profiles"
    case "shortcuts": "Keyboard"
    default: "General"
    }
    return settingsText("settings.category.\(id).hint", fallback: fallback)
}

private func settingsCategoryDescription(_ id: String) -> String {
    let fallback: String = switch id {
    case "appearance": "Tune the terminal you look at all day: type, scale, and color."
    case "runtime": "Set defaults for new workspaces, panes, and terminal history."
    case "attention": "Decide when Muxterm should surface work that needs your attention."
    case "ui": "Shape the surrounding window chrome and tab bar."
    case "ssh": "Defaults used when opening remote workspaces over SSH."
    case "behavior": "Choose what Muxterm does when panes or commands exit."
    case "platform": "Options specific to the desktop platform you are running on."
    case "projects": "Save the workspaces you return to most often."
    case "shortcuts": "Choose a keyboard preset and customize individual actions."
    default: "Configure this part of Muxterm."
    }
    return settingsText("settings.category.\(id).description", fallback: fallback)
}

private func settingsCategoryIcon(_ id: String) -> String {
    switch id {
    case "appearance": return "Aa"
    case "runtime": return "▣"
    case "attention": return "◉"
    case "ui": return "▤"
    case "ssh": return "↗"
    case "behavior": return "↯"
    case "platform": return "⌘"
    case "projects": return "▦"
    case "shortcuts": return "⌨"
    default: return "•"
    }
}

private func settingsSectionTitle(_ id: String) -> String {
    let fallback: String = switch id {
    case "appearance": "Terminal"
    case "runtime": "Workspace defaults"
    case "attention": "Attention"
    case "ui": "Interface"
    case "ssh": "SSH defaults"
    case "behavior": "Exit behavior"
    case "platform": "Platform"
    case "projects": "Workspace profiles"
    case "shortcuts": "Keyboard shortcuts"
    default: "Settings"
    }
    return settingsText("settings.section.\(id)", fallback: fallback)
}

private func settingsValue(at path: String, in values: [String: Any]) -> Any? {
    var current: Any = values
    for segment in path.split(separator: "/") {
        guard let dictionary = current as? [String: Any],
              let next = dictionary[String(segment)]
        else { return nil }
        current = next
    }
    return current
}

private func settingsStyleCard(_ view: NSView, fill: NSColor = .controlBackgroundColor) {
    view.wantsLayer = true
    view.layer?.backgroundColor = fill.cgColor
    view.layer?.borderColor = NSColor.separatorColor.withAlphaComponent(0.7).cgColor
    view.layer?.borderWidth = 1
    view.layer?.cornerRadius = 7
}

final class SettingsWindowController: NSWindowController, NSWindowDelegate,
    NSTextFieldDelegate, NSSearchFieldDelegate, NSTableViewDataSource, NSTableViewDelegate
{
    var onApplied: (([[String: Any]]) -> Void)?
    private struct Category {
        let id: String
        let title: String
        let hint: String
        let description: String
        let searchText: String
        let fields: [[String: Any]]
    }

    private let bridge: CoreBridge
    private let quickConnectStore: QuickConnectStore
    private let configTransaction: MuxtermConfigTransactionHandler
    private var controls: [String: NSView] = [:]
    private var baselines: [String: Any] = [:]
    private var pendingFontPath: String?
    private var dirty = false {
        didSet { updateFooterPresentation() }
    }
    private var summaryLabel = NSTextField(labelWithString: "")
    private weak var applyButton: NSButton?
    private var languageObserver: NSObjectProtocol?
    private let searchField = NSSearchField()
    private let categoryTable = NSTableView()
    private let categoryScroll = NSScrollView()
    private let sidebarView = NSView()
    private let pagesContainer = NSView()
    private var categories: [Category] = []
    private var visibleCategoryIDs: [String] = []
    private var selectedCategoryID: String?
    private var pages: [String: NSScrollView] = [:]
    private var projectEditorView: SettingsProjectEditorView?
    private var activeProjectEditor: TargetConfigWindow?
    private var developerToolsStatusLabel: NSTextField?
    private var developerToolsButton: NSButton?

    init(
        bridge: CoreBridge,
        quickConnectStore: QuickConnectStore? = nil,
        configTransaction: @escaping MuxtermConfigTransactionHandler
    ) {
        self.bridge = bridge
        self.configTransaction = configTransaction
        self.quickConnectStore = quickConnectStore ?? Self.makeCoreBackedStore(
            bridge: bridge,
            configTransaction: configTransaction
        )
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 980, height: 720),
            styleMask: [.titled, .closable, .resizable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        window.title = settingsText("settings.window.title", fallback: "Settings")
        window.titleVisibility = .hidden
        window.titlebarAppearsTransparent = true
        window.titlebarSeparatorStyle = .none
        window.isMovableByWindowBackground = true
        window.minSize = NSSize(width: 760, height: 520)
        super.init(window: window)
        window.delegate = self
        window.setAccessibilityIdentifier("muxterm.settingsWindow")
        loadSnapshotAndBuild()
        languageObserver = NotificationCenter.default.addObserver(
            forName: .muxtermLanguageChanged,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            guard let self else { return }
            self.window?.title = settingsText("settings.window.title", fallback: "Settings")
            if !self.dirty {
                self.loadSnapshotAndBuild()
            }
        }
    }

    convenience init(bridge: CoreBridge, quickConnectStore: QuickConnectStore? = nil) {
        self.init(
            bridge: bridge,
            quickConnectStore: quickConnectStore,
            configTransaction: bridge.directConfigTransactionHandler()
        )
    }

    private static func makeCoreBackedStore(
        bridge: CoreBridge,
        configTransaction: @escaping MuxtermConfigTransactionHandler
    ) -> QuickConnectStore {
        let projects = projects(from: bridge)
        return QuickConnectStore(projects: projects) { [weak bridge] updated in
            guard bridge != nil else { return }
            let operations: [[String: Any]] = [[
                "op": "replace",
                "path": "/projects",
                "value": QuickConnectStore.projectJSON(from: updated),
            ]]
            let accepted = configTransaction(MuxtermConfigTransactionRequest(
                operations: operations,
                completion: { result in
                    if case .failure(let error) = result {
                        CoreBridge.log(
                            "failed to persist projects from settings: \(error.localizedDescription)",
                            level: "error"
                        )
                    }
                }
            ))
            if !accepted {
                CoreBridge.log("config transaction was not accepted", level: "error")
            }
        }
    }

    private static func projects(from bridge: CoreBridge) -> [TargetConfig] {
        guard let text = bridge.configDescribeJSON(),
              let data = text.data(using: .utf8),
              let envelope = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let payload = envelope["data"] as? [String: Any],
              let values = payload["values"] as? [String: Any],
              let projects = values["projects"] as? [[String: Any]]
        else { return [] }
        return QuickConnectStore.targetConfigs(from: projects)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    deinit {
        if let languageObserver {
            NotificationCenter.default.removeObserver(languageObserver)
        }
    }

    override func showWindow(_ sender: Any?) {
        loadSnapshotAndBuild()
        super.showWindow(sender)
        window?.makeKeyAndOrderFront(sender)
        refreshDeveloperToolsStatus()
    }

    func windowDidBecomeKey(_ notification: Notification) {
        refreshDeveloperToolsStatus()
    }

    // MARK: - Manifest rendering

    private func loadSnapshotAndBuild() {
        guard let text = bridge.configDescribeJSON(),
              let data = text.data(using: .utf8),
              let envelope = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let payload = envelope["data"] as? [String: Any],
              let values = payload["values"] as? [String: Any],
              let manifest = payload["manifest"] as? [String: Any]
        else { return }
        buildView(in: window!, values: values, manifest: manifest)
        loadValues(values)
    }

    private func buildView(in window: NSWindow, values: [String: Any], manifest: [String: Any]) {
        NSLayoutConstraint.deactivate(sidebarView.constraints)
        NSLayoutConstraint.deactivate(pagesContainer.constraints)
        sidebarView.subviews.forEach { $0.removeFromSuperview() }
        pagesContainer.subviews.forEach { $0.removeFromSuperview() }
        categoryTable.tableColumns.forEach { categoryTable.removeTableColumn($0) }
        controls = [:]
        baselines = [:]
        dirty = false
        categories = []
        visibleCategoryIDs = []
        selectedCategoryID = nil
        pages = [:]
        projectEditorView = nil

        guard let groups = manifest["groups"] as? [[String: Any]] else {
            let label = NSTextField(labelWithString: settingsText(
                "settings.manifest.missing",
                fallback: "No settings manifest"
            ))
            label.alignment = .center
            window.contentView = label
            return
        }

        for group in groups {
            guard let id = group["id"] as? String,
                  !id.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            else { continue }
            let titleKey = group["title_key"] as? String ?? ""
            let fields = group["fields"] as? [[String: Any]] ?? []
            let title = settingsCategoryTitle(id: id, titleKey: titleKey)
            let hint = settingsCategoryHint(id)
            let description = settingsCategoryDescription(id)
            var searchParts = [id, titleKey, title, hint, description]
            for field in fields {
                guard let path = field["path"] as? String else { continue }
                let fieldTitle = settingsFieldTitle(
                    path: path,
                    titleKey: field["title_key"] as? String ?? ""
                )
                searchParts += [
                    path,
                    field["title_key"] as? String ?? "",
                    field["description_key"] as? String ?? "",
                    fieldTitle,
                    settingsFieldDescription(
                        path: path,
                        titleKey: field["title_key"] as? String ?? ""
                    ),
                ]
                if let options = field["options"] as? [String] {
                    searchParts += options.flatMap { [$0, settingsOptionLabel(path: path, value: $0)] }
                }
            }
            if id == "platform" {
                searchParts += [
                    "developer tools",
                    "开发者工具",
                    settingsFieldTitle(path: "macos.developer_tools", titleKey: "settings.platform.developer_tools"),
                    settingsFieldDescription(
                        path: "macos.developer_tools",
                        titleKey: "settings.platform.developer_tools"
                    ),
                ]
            }
            categories.append(Category(
                id: id,
                title: title,
                hint: hint,
                description: description,
                searchText: searchParts.joined(separator: " ").lowercased(),
                fields: fields
            ))
        }
        visibleCategoryIDs = categories.map(\.id)

        let root = NSView()
        root.wantsLayer = true
        root.layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor
        root.setAccessibilityIdentifier("muxterm.settings.root")

        let header = NSView()
        header.translatesAutoresizingMaskIntoConstraints = false
        header.wantsLayer = true
        header.layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor
        root.addSubview(header)

        let headerTitle = NSTextField(labelWithString: settingsText(
            "settings.window.title",
            fallback: "Settings"
        ))
        headerTitle.translatesAutoresizingMaskIntoConstraints = false
        headerTitle.font = .systemFont(ofSize: 17, weight: .semibold)
        header.addSubview(headerTitle)

        searchField.translatesAutoresizingMaskIntoConstraints = false
        searchField.placeholderString = settingsText(
            "settings.search.placeholder",
            fallback: "Search settings"
        )
        searchField.toolTip = settingsText(
            "settings.search.tooltip",
            fallback: "Search by setting name or keyword"
        )
        searchField.delegate = self
        searchField.stringValue = ""
        searchField.setAccessibilityIdentifier("muxterm.settings.search")
        header.addSubview(searchField)

        let headerSeparator = NSView()
        headerSeparator.translatesAutoresizingMaskIntoConstraints = false
        headerSeparator.wantsLayer = true
        headerSeparator.layer?.backgroundColor = NSColor.separatorColor.cgColor
        root.addSubview(headerSeparator)

        sidebarView.translatesAutoresizingMaskIntoConstraints = false
        sidebarView.wantsLayer = true
        sidebarView.layer?.backgroundColor = NSColor.controlBackgroundColor.withAlphaComponent(0.48).cgColor
        sidebarView.setAccessibilityIdentifier("muxterm.settings.categories")
        pagesContainer.translatesAutoresizingMaskIntoConstraints = false
        pagesContainer.setAccessibilityIdentifier("muxterm.settings.pages")

        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("category"))
        categoryTable.addTableColumn(column)
        categoryTable.headerView = nil
        categoryTable.rowHeight = 44
        categoryTable.intercellSpacing = NSSize(width: 0, height: 1)
        categoryTable.style = .sourceList
        categoryTable.backgroundColor = .clear
        categoryTable.usesAlternatingRowBackgroundColors = false
        categoryTable.dataSource = self
        categoryTable.delegate = self
        categoryTable.setAccessibilityIdentifier("muxterm.settings.categoryList")
        categoryScroll.translatesAutoresizingMaskIntoConstraints = false
        categoryScroll.drawsBackground = false
        categoryScroll.borderType = .noBorder
        categoryScroll.hasVerticalScroller = true
        categoryScroll.autohidesScrollers = true
        categoryScroll.documentView = categoryTable

        let sidebarTitle = NSTextField(labelWithString: settingsText(
            "settings.sidebar.title",
            fallback: "CONFIGURATION"
        ))
        sidebarTitle.translatesAutoresizingMaskIntoConstraints = false
        sidebarTitle.font = .systemFont(ofSize: 10, weight: .bold)
        sidebarTitle.textColor = .tertiaryLabelColor
        sidebarView.addSubview(sidebarTitle)
        sidebarView.addSubview(categoryScroll)
        root.addSubview(sidebarView)

        let right = NSView()
        right.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(right)
        right.addSubview(pagesContainer)

        for category in categories {
            let page = makePage(for: category, values: values)
            page.translatesAutoresizingMaskIntoConstraints = false
            page.isHidden = true
            pages[category.id] = page
            pagesContainer.addSubview(page)
            NSLayoutConstraint.activate([
                page.leadingAnchor.constraint(equalTo: pagesContainer.leadingAnchor),
                page.trailingAnchor.constraint(equalTo: pagesContainer.trailingAnchor),
                page.topAnchor.constraint(equalTo: pagesContainer.topAnchor),
                page.bottomAnchor.constraint(equalTo: pagesContainer.bottomAnchor),
            ])
        }

        let separator = NSView()
        separator.wantsLayer = true
        separator.layer?.backgroundColor = NSColor.separatorColor.cgColor
        separator.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(separator)

        let footer = NSView()
        footer.translatesAutoresizingMaskIntoConstraints = false
        footer.wantsLayer = true
        footer.layer?.backgroundColor = NSColor.controlBackgroundColor.withAlphaComponent(0.48).cgColor
        right.addSubview(footer)
        summaryLabel.textColor = .secondaryLabelColor
        summaryLabel.font = .systemFont(ofSize: 11)
        summaryLabel.lineBreakMode = .byTruncatingTail
        summaryLabel.maximumNumberOfLines = 1
        summaryLabel.translatesAutoresizingMaskIntoConstraints = false
        footer.addSubview(summaryLabel)
        let cancel = NSButton(
            title: settingsText("settings.cancel", fallback: "Cancel"),
            target: self,
            action: #selector(cancelSettings)
        )
        cancel.translatesAutoresizingMaskIntoConstraints = false
        cancel.bezelStyle = .rounded
        cancel.setAccessibilityIdentifier("muxterm.settings.cancel")
        let apply = NSButton(
            title: settingsText("settings.apply", fallback: "Apply"),
            target: self,
            action: #selector(applySettings)
        )
        apply.translatesAutoresizingMaskIntoConstraints = false
        apply.bezelStyle = .rounded
        apply.keyEquivalent = "\r"
        apply.toolTip = settingsText(
            "settings.apply.tooltip",
            fallback: "Write changes to config.toml"
        )
        apply.setAccessibilityIdentifier("muxterm.settings.apply")
        applyButton = apply
        footer.addSubview(cancel)
        footer.addSubview(apply)

        NSLayoutConstraint.activate([
            header.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            header.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            header.topAnchor.constraint(equalTo: root.topAnchor),
            header.heightAnchor.constraint(equalToConstant: 58),
            headerTitle.leadingAnchor.constraint(equalTo: header.leadingAnchor, constant: 78),
            headerTitle.centerYAnchor.constraint(equalTo: header.centerYAnchor),
            headerTitle.trailingAnchor.constraint(lessThanOrEqualTo: searchField.leadingAnchor, constant: -20),
            searchField.trailingAnchor.constraint(equalTo: header.trailingAnchor, constant: -20),
            searchField.centerYAnchor.constraint(equalTo: header.centerYAnchor),
            searchField.widthAnchor.constraint(equalToConstant: 230),
            headerSeparator.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            headerSeparator.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            headerSeparator.topAnchor.constraint(equalTo: header.bottomAnchor),
            headerSeparator.heightAnchor.constraint(equalToConstant: 1),
            sidebarView.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            sidebarView.topAnchor.constraint(equalTo: headerSeparator.bottomAnchor),
            sidebarView.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            sidebarView.widthAnchor.constraint(equalToConstant: 208),
            sidebarTitle.leadingAnchor.constraint(equalTo: sidebarView.leadingAnchor, constant: 18),
            sidebarTitle.trailingAnchor.constraint(equalTo: sidebarView.trailingAnchor, constant: -12),
            sidebarTitle.topAnchor.constraint(equalTo: sidebarView.topAnchor, constant: 14),
            categoryScroll.leadingAnchor.constraint(equalTo: sidebarView.leadingAnchor),
            categoryScroll.trailingAnchor.constraint(equalTo: sidebarView.trailingAnchor),
            categoryScroll.topAnchor.constraint(equalTo: sidebarTitle.bottomAnchor, constant: 8),
            categoryScroll.bottomAnchor.constraint(equalTo: sidebarView.bottomAnchor, constant: -8),
            separator.leadingAnchor.constraint(equalTo: sidebarView.trailingAnchor),
            separator.topAnchor.constraint(equalTo: headerSeparator.bottomAnchor),
            separator.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            separator.widthAnchor.constraint(equalToConstant: 1),
            right.leadingAnchor.constraint(equalTo: separator.trailingAnchor),
            right.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            right.topAnchor.constraint(equalTo: headerSeparator.bottomAnchor),
            right.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            pagesContainer.leadingAnchor.constraint(equalTo: right.leadingAnchor),
            pagesContainer.trailingAnchor.constraint(equalTo: right.trailingAnchor),
            pagesContainer.topAnchor.constraint(equalTo: right.topAnchor),
            pagesContainer.bottomAnchor.constraint(equalTo: footer.topAnchor),
            footer.leadingAnchor.constraint(equalTo: right.leadingAnchor),
            footer.trailingAnchor.constraint(equalTo: right.trailingAnchor),
            footer.bottomAnchor.constraint(equalTo: right.bottomAnchor),
            footer.heightAnchor.constraint(equalToConstant: 52),
            summaryLabel.leadingAnchor.constraint(equalTo: footer.leadingAnchor, constant: 24),
            summaryLabel.centerYAnchor.constraint(equalTo: footer.centerYAnchor),
            summaryLabel.trailingAnchor.constraint(lessThanOrEqualTo: cancel.leadingAnchor, constant: -12),
            apply.trailingAnchor.constraint(equalTo: footer.trailingAnchor, constant: -24),
            apply.centerYAnchor.constraint(equalTo: footer.centerYAnchor),
            cancel.trailingAnchor.constraint(equalTo: apply.leadingAnchor, constant: -8),
            cancel.centerYAnchor.constraint(equalTo: footer.centerYAnchor),
        ])

        window.contentView = root
        categoryTable.reloadData()
        selectCategory(visibleCategoryIDs.first)
    }

    private func makePage(for category: Category, values: [String: Any]) -> NSScrollView {
        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.borderType = .noBorder
        scroll.autohidesScrollers = true
        scroll.setAccessibilityIdentifier("muxterm.settings.page.\(category.id)")

        let stack = NSStackView()
        stack.translatesAutoresizingMaskIntoConstraints = false
        stack.orientation = .vertical
        stack.alignment = .width
        stack.spacing = 14
        stack.edgeInsets = NSEdgeInsets(top: 22, left: 26, bottom: 28, right: 30)
        stack.setContentHuggingPriority(.defaultLow, for: .horizontal)
        stack.setContentCompressionResistancePriority(.required, for: .horizontal)
        let contentWidth: CGFloat = -56

        let title = NSTextField(labelWithString: category.title)
        title.font = .systemFont(ofSize: 20, weight: .semibold)
        title.textColor = .labelColor
        let description = NSTextField(labelWithString: category.description)
        description.font = .systemFont(ofSize: 12)
        description.textColor = .secondaryLabelColor
        description.lineBreakMode = .byWordWrapping
        description.maximumNumberOfLines = 2
        let headingCopy = NSStackView(views: [title, description])
        headingCopy.orientation = .vertical
        headingCopy.alignment = .leading
        headingCopy.spacing = 3
        let icon = NSTextField(labelWithString: settingsCategoryIcon(category.id))
        icon.setAccessibilityIdentifier("muxterm.settings.pageIcon.\(category.id)")
        icon.alignment = .center
        icon.font = .systemFont(ofSize: 17, weight: .bold)
        icon.textColor = .controlAccentColor
        icon.wantsLayer = true
        icon.layer?.backgroundColor = NSColor.controlAccentColor.withAlphaComponent(0.12).cgColor
        icon.layer?.cornerRadius = 9
        icon.widthAnchor.constraint(equalToConstant: 42).isActive = true
        icon.heightAnchor.constraint(equalToConstant: 42).isActive = true
        let pageHeader = NSStackView(views: [icon, headingCopy])
        pageHeader.orientation = .horizontal
        pageHeader.alignment = .centerY
        pageHeader.spacing = 13
        pageHeader.setContentHuggingPriority(.defaultLow, for: .horizontal)
        stack.addArrangedSubview(pageHeader)
        pageHeader.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: contentWidth).isActive = true

        let sectionTitle = NSTextField(labelWithString: settingsSectionTitle(category.id))
        sectionTitle.font = .systemFont(ofSize: 11, weight: .bold)
        sectionTitle.textColor = .secondaryLabelColor
        stack.addArrangedSubview(sectionTitle)
        sectionTitle.widthAnchor.constraint(
            equalTo: stack.widthAnchor,
            constant: contentWidth
        ).isActive = true

        for field in category.fields {
            guard let path = field["path"] as? String else { continue }
            let control = makeControl(field: field, values: values)
            controls[path] = control
            baselines[path] = value(at: path, in: values)
            let titleKey = field["title_key"] as? String ?? ""
            let row = settingRow(
                title: settingsFieldTitle(
                    path: path,
                    titleKey: titleKey
                ),
                description: settingsFieldDescription(path: path, titleKey: titleKey),
                apply: settingsApplyLabel(field["apply"] as? String ?? "commit"),
                path: path,
                control: control
            )
            settingsStyleCard(row, fill: NSColor.controlBackgroundColor.withAlphaComponent(0.44))
            stack.addArrangedSubview(row)
            row.widthAnchor.constraint(
                equalTo: stack.widthAnchor,
                constant: contentWidth
            ).isActive = true
        }
        if category.id == "appearance" {
            let preview = settingsAppearancePreview(values: values)
            stack.addArrangedSubview(preview)
            preview.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: contentWidth).isActive = true
        }
        if category.id == "platform" {
            let row = makeDeveloperToolsRow()
            stack.addArrangedSubview(row)
            row.widthAnchor.constraint(
                equalTo: stack.widthAnchor,
                constant: contentWidth
            ).isActive = true
        }
        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow, for: .vertical)
        stack.addArrangedSubview(spacer)
        spacer.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: contentWidth).isActive = true

        scroll.documentView = stack
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: scroll.contentView.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: scroll.contentView.trailingAnchor),
            stack.topAnchor.constraint(equalTo: scroll.contentView.topAnchor),
            stack.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor),
        ])
        return scroll
    }

    private func settingRow(
        title: String,
        description: String,
        apply: String,
        path: String,
        control: NSView
    ) -> NSView {
        let titleLabel = NSTextField(labelWithString: title)
        titleLabel.font = .systemFont(ofSize: 12, weight: .semibold)
        titleLabel.textColor = .labelColor
        titleLabel.lineBreakMode = .byTruncatingTail

        let descriptionLabel = NSTextField(labelWithString: description)
        descriptionLabel.font = .systemFont(ofSize: 11)
        descriptionLabel.textColor = .secondaryLabelColor
        descriptionLabel.lineBreakMode = .byWordWrapping
        descriptionLabel.maximumNumberOfLines = 2

        let badge = NSTextField(labelWithString: apply)
        badge.font = .systemFont(ofSize: 8.5, weight: .bold)
        badge.textColor = .controlAccentColor
        badge.alignment = .center
        badge.wantsLayer = true
        badge.layer?.backgroundColor = NSColor.controlAccentColor.withAlphaComponent(0.11).cgColor
        badge.layer?.cornerRadius = 4
        badge.setContentHuggingPriority(.required, for: .horizontal)
        badge.setContentCompressionResistancePriority(.required, for: .horizontal)
        badge.widthAnchor.constraint(equalToConstant: 88).isActive = true
        badge.heightAnchor.constraint(equalToConstant: 17).isActive = true

        let titleRow = NSStackView(views: [titleLabel, badge, NSView()])
        titleRow.orientation = .horizontal
        titleRow.alignment = .centerY
        titleRow.spacing = 8
        titleRow.setContentHuggingPriority(.defaultLow, for: .horizontal)
        let copy = NSStackView(views: [titleRow, descriptionLabel])
        copy.orientation = .vertical
        copy.alignment = .width
        copy.spacing = 4
        copy.setContentHuggingPriority(.defaultLow, for: .horizontal)
        copy.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        let row = NSView()
        row.translatesAutoresizingMaskIntoConstraints = false
        row.identifier = NSUserInterfaceItemIdentifier("muxterm.settings.row.\(path)")
        row.setAccessibilityIdentifier("muxterm.settings.row.\(path)")
        copy.translatesAutoresizingMaskIntoConstraints = false
        control.translatesAutoresizingMaskIntoConstraints = false
        row.addSubview(copy)
        row.addSubview(control)

        let isFullWidth = control is NSScrollView || control is SettingsProjectEditorView
        if isFullWidth {
            NSLayoutConstraint.activate([
                copy.leadingAnchor.constraint(equalTo: row.leadingAnchor, constant: 16),
                copy.trailingAnchor.constraint(equalTo: row.trailingAnchor, constant: -16),
                copy.topAnchor.constraint(equalTo: row.topAnchor, constant: 14),
                control.leadingAnchor.constraint(equalTo: row.leadingAnchor, constant: 16),
                control.trailingAnchor.constraint(equalTo: row.trailingAnchor, constant: -16),
                control.topAnchor.constraint(equalTo: copy.bottomAnchor, constant: 12),
                control.bottomAnchor.constraint(equalTo: row.bottomAnchor, constant: -14),
            ])
            return row
        }

        let controlWidth: CGFloat = 250
        if control is NSPopUpButton || control is NSTextField {
            control.widthAnchor.constraint(equalToConstant: controlWidth).isActive = true
        } else if let composite = control as? NSStackView {
            composite.widthAnchor.constraint(equalToConstant: controlWidth).isActive = true
        } else if !(control is NSButton) {
            control.widthAnchor.constraint(lessThanOrEqualToConstant: controlWidth).isActive = true
        }
        control.setContentHuggingPriority(.required, for: .horizontal)
        control.setContentCompressionResistancePriority(.required, for: .horizontal)
        NSLayoutConstraint.activate([
            copy.leadingAnchor.constraint(equalTo: row.leadingAnchor, constant: 16),
            copy.trailingAnchor.constraint(lessThanOrEqualTo: control.leadingAnchor, constant: -20),
            copy.topAnchor.constraint(equalTo: row.topAnchor, constant: 14),
            copy.bottomAnchor.constraint(equalTo: row.bottomAnchor, constant: -14),
            control.trailingAnchor.constraint(equalTo: row.trailingAnchor, constant: -16),
            control.centerYAnchor.constraint(equalTo: row.centerYAnchor),
        ])
        return row
    }

    private func makeControl(field: [String: Any], values: [String: Any]) -> NSView {
        let path = field["path"] as? String ?? ""
        let kind = field["control"] as? String ?? "text"
        let baseline = value(at: path, in: values)

        switch kind {
        case "switch":
            let checkbox = NSButton(checkboxWithTitle: "", target: self, action: #selector(controlChanged(_:)))
            checkbox.identifier = NSUserInterfaceItemIdentifier(path)
            checkbox.state = (baseline as? Bool == true) ? .on : .off
            return checkbox
        case "number":
            let field = NSTextField()
            field.identifier = NSUserInterfaceItemIdentifier(path)
            field.alignment = .right
            field.target = self
            field.action = #selector(controlChanged(_:))
            field.delegate = self
            field.placeholderString = "0"
            if let number = baseline as? NSNumber {
                field.stringValue = number.stringValue
            }
            field.controlSize = .regular
            field.widthAnchor.constraint(greaterThanOrEqualToConstant: 110).isActive = true
            field.setContentHuggingPriority(.defaultHigh, for: .horizontal)
            return field
        case "multiline":
            let textView = NSTextView()
            textView.isVerticallyResizable = true
            textView.isHorizontallyResizable = false
            textView.autoresizingMask = [.width]
            textView.textContainer?.widthTracksTextView = true
            textView.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
            textView.identifier = NSUserInterfaceItemIdentifier(path)
            let scroll = NSScrollView()
            scroll.hasVerticalScroller = true
            scroll.autohidesScrollers = true
            scroll.borderType = .bezelBorder
            scroll.documentView = textView
            scroll.heightAnchor.constraint(equalToConstant: 94).isActive = true
            if let items = baseline as? [String] {
                textView.string = items.joined(separator: "\n")
            }
            return scroll
        case "select", "theme_picker":
            let popup = NSPopUpButton(frame: .zero, pullsDown: false)
            popup.identifier = NSUserInterfaceItemIdentifier(path)
            popup.target = self
            popup.action = #selector(controlChanged(_:))
            if let options = field["options"] as? [String] {
                for option in options {
                    popup.addItem(withTitle: settingsOptionLabel(path: path, value: option))
                    popup.lastItem?.representedObject = option
                }
            }
            let current = baseline as? String ?? ""
            if popup.itemArray.first(where: { $0.representedObject as? String == current }) == nil {
                popup.addItem(withTitle: settingsOptionLabel(path: path, value: current))
                popup.lastItem?.representedObject = current
            }
            if let item = popup.itemArray.first(where: { $0.representedObject as? String == current }) {
                popup.select(item)
            }
            return popup
        case "font_fallback", "string_list":
            let entry = NSTextField()
            entry.identifier = NSUserInterfaceItemIdentifier(path)
            entry.target = self
            entry.action = #selector(controlChanged(_:))
            entry.delegate = self
            entry.placeholderString = "Noto Sans Mono, monospace"
            if let items = baseline as? [String] {
                entry.stringValue = items.joined(separator: ", ")
            }
            entry.controlSize = .regular
            entry.setContentHuggingPriority(.defaultLow, for: .horizontal)
            return entry
        case "font_picker":
            let row = NSStackView()
            row.orientation = .horizontal
            row.spacing = 8
            let entry = NSTextField()
            entry.identifier = NSUserInterfaceItemIdentifier(path)
            entry.target = self
            entry.action = #selector(controlChanged(_:))
            entry.delegate = self
            entry.placeholderString = "JetBrains Mono"
            if let family = baseline as? String {
                entry.stringValue = family
            }
            entry.controlSize = .regular
            entry.setContentHuggingPriority(.defaultLow, for: .horizontal)
            let choose = NSButton(
                title: settingsText("settings.choose", fallback: "Choose…"),
                target: self,
                action: #selector(chooseFont(_:))
            )
            choose.identifier = NSUserInterfaceItemIdentifier(path)
            choose.bezelStyle = .rounded
            row.addArrangedSubview(entry)
            row.addArrangedSubview(choose)
            row.setContentHuggingPriority(.defaultLow, for: .horizontal)
            return row
        case "project_editor":
            let editor = SettingsProjectEditorView(projects: quickConnectStore.projects)
            editor.onNew = { [weak self] in
                self?.openProjectEditor(nil)
            }
            editor.onEdit = { [weak self] index in
                self?.openProjectEditor(at: index)
            }
            editor.onDelete = { [weak self] index in
                self?.deleteProject(at: index)
            }
            projectEditorView = editor
            return editor
        case "shortcut_editor":
            let label = NSTextField(labelWithString: settingsText(
                "settings.shortcuts.manager_hint",
                fallback: "Open the shortcut manager from the main command palette."
            ))
            label.identifier = NSUserInterfaceItemIdentifier(path)
            label.textColor = .secondaryLabelColor
            label.font = .systemFont(ofSize: 11)
            return label
        default:
            let entry = NSTextField()
            entry.identifier = NSUserInterfaceItemIdentifier(path)
            entry.target = self
            entry.action = #selector(controlChanged(_:))
            entry.delegate = self
            entry.placeholderString = settingsInputPlaceholder(path)
            if let text = baseline as? String {
                entry.stringValue = text
            }
            entry.controlSize = .regular
            entry.setContentHuggingPriority(.defaultLow, for: .horizontal)
            return entry
        }
    }

    private func selectCategory(_ id: String?) {
        guard let id, visibleCategoryIDs.contains(id), pages[id] != nil else {
            selectedCategoryID = nil
            pages.values.forEach { $0.isHidden = true }
            categoryTable.deselectAll(nil)
            return
        }
        selectedCategoryID = id
        for (pageID, page) in pages {
            page.isHidden = pageID != id
        }
        if let row = visibleCategoryIDs.firstIndex(of: id), categoryTable.selectedRow != row {
            categoryTable.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        }
    }

    private func applySearch() {
        let query = searchField.stringValue
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .lowercased()
        visibleCategoryIDs = categories
            .filter { query.isEmpty || $0.searchText.contains(query) }
            .map(\.id)
        categoryTable.reloadData()
        if let selectedCategoryID, visibleCategoryIDs.contains(selectedCategoryID) {
            selectCategory(selectedCategoryID)
        } else {
            selectCategory(visibleCategoryIDs.first)
        }
    }

    private func loadValues(_ values: [String: Any]) {
        updateFooterPresentation()
    }

    private func updateFooterPresentation() {
        guard summaryLabel.superview != nil else { return }
        if dirty {
            summaryLabel.stringValue = settingsText(
                "settings.unsaved",
                fallback: "Unsaved changes"
            )
            summaryLabel.textColor = .systemOrange
        } else {
            summaryLabel.stringValue = settingsText(
                "settings.summary",
                fallback: "Synced with Core · {{count}} project(s)",
                arguments: ["count": "\(quickConnectStore.projects.count)"]
            )
            summaryLabel.textColor = .secondaryLabelColor
        }
        applyButton?.contentTintColor = dirty ? .controlAccentColor : .secondaryLabelColor
    }

    private func refreshProjectEditor() {
        projectEditorView?.setProjects(quickConnectStore.projects)
        loadValues([:])
    }

    private func openProjectEditor(at index: Int) {
        guard quickConnectStore.projects.indices.contains(index) else { return }
        openProjectEditor(quickConnectStore.projects[index])
    }

    private func openProjectEditor(_ config: TargetConfig?) {
        guard let owner = window else { return }
        let hosts: [SSHHostInfo]
        switch ConnectionDiscovery().sshHosts() {
        case .success(let value):
            hosts = value
        case .failure:
            hosts = []
        }
        let availableRuntimes = (try? CoreBridge.runtimeCatalog())?
            .compactMap { TargetRuntime(rawValue: $0.id) }
            ?? TargetRuntime.allCases
        let editor = TargetConfigWindow(
            editing: config,
            owner: owner,
            store: quickConnectStore,
            sshHosts: hosts,
            availableRuntimes: availableRuntimes
        )
        activeProjectEditor = editor
        editor.onSave = { [weak self] saved in
            guard let self else { return }
            if let config {
                self.quickConnectStore.updateProject(saved, replacing: config)
            } else {
                self.quickConnectStore.upsertProject(saved)
            }
            self.refreshProjectEditor()
            self.activeProjectEditor = nil
        }
        editor.onCancel = { [weak self] in
            self?.activeProjectEditor = nil
        }
    }

    private func deleteProject(at index: Int) {
        guard quickConnectStore.projects.indices.contains(index), let owner = window else { return }
        let project = quickConnectStore.projects[index]
        let alert = NSAlert()
        alert.messageText = settingsText(
            "settings.projects.delete_title",
            fallback: "Delete Project?"
        )
        alert.informativeText = settingsText(
            "settings.projects.delete_message",
            fallback: "Remove \"{{name}}\" from the saved projects?",
            arguments: ["name": project.name]
        )
        alert.alertStyle = .warning
        alert.addButton(withTitle: settingsText("settings.cancel", fallback: "Cancel"))
        alert.addButton(withTitle: settingsText("settings.delete", fallback: "Delete"))
        alert.beginSheetModal(for: owner) { [weak self] response in
            guard response == .alertSecondButtonReturn, let self else { return }
            self.quickConnectStore.removeProject(config: project)
            self.refreshProjectEditor()
        }
    }

    // MARK: - Value collection and Apply/Cancel

    private func collectOperations() -> [[String: Any]] {
        var operations: [[String: Any]] = []
        for (path, view) in controls {
            guard let value = controlValue(view, path: path, baseline: baselines[path]) else {
                continue
            }
            operations.append(["op": "replace", "path": path, "value": value])
        }
        return operations
    }

    private func controlValue(_ view: NSView, path: String, baseline: Any?) -> Any? {
        if let editor = view as? SettingsProjectEditorView {
            return QuickConnectStore.projectJSON(from: editor.projects)
        }
        if path == "/shortcuts/overrides" {
            return nil
        }
        if let popup = view as? NSPopUpButton {
            return popup.selectedItem?.representedObject as? String ?? popup.titleOfSelectedItem
        }
        if let checkbox = view as? NSButton {
            return checkbox.state == .on
        }
        if let field = view as? NSTextField {
            if path == "/font/fallback" || path.hasSuffix("/fallback") {
                return field.stringValue
                    .split(separator: ",")
                    .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
                    .filter { !$0.isEmpty }
            }
            if let number = baseline as? NSNumber, let parsed = Double(field.stringValue) {
                let type = number.objCType.pointee
                if type == Int8(Character("q").asciiValue!)
                    || type == Int8(Character("Q").asciiValue!)
                    || type == Int8(Character("i").asciiValue!)
                    || type == Int8(Character("I").asciiValue!) {
                    return Int(parsed.rounded())
                }
                return parsed
            }
            return field.stringValue
        }
        if let scroll = view as? NSScrollView, let textView = scroll.documentView as? NSTextView {
            return textView.string.split(separator: "\n").map(String.init).map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
        }
        if let row = view as? NSStackView {
            // font_picker row: 第一个 NSTextField 就是 family。
            for subview in row.arrangedSubviews {
                if let field = subview as? NSTextField {
                    return field.stringValue
                }
            }
        }
        return nil
    }

    @objc private func applySettings() {
        let operations = collectOperations()
        let request = MuxtermConfigTransactionRequest(
            operations: operations,
            completion: { [weak self] result in
                guard let self else { return }
                switch result {
                case .success:
                    self.onApplied?(operations)
                    self.dirty = false
                    self.window?.close()
                case .failure(let error):
                    self.showSettingsApplyError(error)
                }
            }
        )
        if !configTransaction(request) {
            showSettingsApplyError(
                CoreBridgeDiscoveryError.message("config transaction was not accepted")
            )
        }
    }

    private func showSettingsApplyError(_ error: Error) {
        guard let owner = window else { return }
        let alert = NSAlert()
        alert.messageText = settingsText(
            "settings.save_error",
            fallback: "Unable to save settings"
        )
        alert.informativeText = error.localizedDescription
        alert.alertStyle = .warning
        alert.beginSheetModal(for: owner, completionHandler: nil)
    }

    @objc private func cancelSettings() {
        if dirty {
            confirmDiscard { [weak self] in
                self?.dirty = false
                self?.window?.close()
            }
        } else {
            window?.close()
        }
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        if dirty {
            confirmDiscard { [weak self] in
                self?.dirty = false
                self?.window?.close()
            }
            return false
        }
        return true
    }

    private func confirmDiscard(onDiscard: @escaping () -> Void) {
        guard let window else { return }
        let alert = NSAlert()
        alert.messageText = settingsText(
            "settings.discard.title",
            fallback: "Discard unsaved changes?"
        )
        alert.informativeText = settingsText(
            "settings.discard.message",
            fallback: "Your edits have not been applied to config.toml."
        )
        alert.alertStyle = .warning
        alert.addButton(withTitle: settingsText("settings.cancel", fallback: "Cancel"))
        alert.addButton(withTitle: settingsText("settings.discard.action", fallback: "Discard"))
        alert.beginSheetModal(for: window) { response in
            if response == .alertSecondButtonReturn {
                onDiscard()
            }
        }
    }

    @objc private func controlChanged(_ sender: Any?) {
        dirty = true
    }

    func controlTextDidChange(_ obj: Notification) {
        if let field = obj.object as? NSSearchField, field === searchField {
            applySearch()
            return
        }
        dirty = true
    }

    // MARK: - Category table

    func numberOfRows(in tableView: NSTableView) -> Int {
        visibleCategoryIDs.count
    }

    func tableView(
        _ tableView: NSTableView,
        viewFor tableColumn: NSTableColumn?,
        row: Int
    ) -> NSView? {
        guard visibleCategoryIDs.indices.contains(row),
              let category = categories.first(where: { $0.id == visibleCategoryIDs[row] })
        else { return nil }
        let identifier = NSUserInterfaceItemIdentifier("muxterm.settings.categoryCell")
        let cell = (tableView.makeView(withIdentifier: identifier, owner: self) as? SettingsCategoryCellView)
            ?? SettingsCategoryCellView(frame: .zero)
        cell.identifier = identifier
        cell.configure(title: category.title, hint: category.hint, icon: settingsCategoryIcon(category.id))
        cell.setAccessibilityIdentifier("muxterm.settings.category.\(category.id)")
        return cell
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        let row = categoryTable.selectedRow
        guard visibleCategoryIDs.indices.contains(row) else { return }
        selectCategory(visibleCategoryIDs[row])
    }

    @objc private func chooseFont(_ sender: NSButton) {
        pendingFontPath = sender.identifier?.rawValue
        let fontManager = NSFontManager.shared
        fontManager.target = self
        fontManager.action = #selector(changeFont(_:))
        fontManager.orderFrontFontPanel(sender)
    }

    @objc private func changeFont(_ sender: NSFontManager) {
        guard let path = pendingFontPath, let field = textField(at: path) else { return }
        let current = NSFont(name: field.stringValue, size: 13) ?? NSFont.systemFont(ofSize: 13)
        field.stringValue = sender.convert(current).fontName
        dirty = true
    }

    private func textField(at path: String) -> NSTextField? {
        guard let view = controls[path] else { return nil }
        if let field = view as? NSTextField {
            return field
        }
        if let row = view as? NSStackView {
            for subview in row.arrangedSubviews {
                if let field = subview as? NSTextField {
                    return field
                }
            }
        }
        return nil
    }

    // MARK: - JSON Pointer lookup

    private func value(at path: String, in values: [String: Any]) -> Any? {
        let segments = path.split(separator: "/").map(String.init)
        var current: Any = values
        for segment in segments {
            guard let dict = current as? [String: Any], let next = dict[segment] else {
                return nil
            }
            current = next
        }
        return current
    }

    // MARK: - In-process E2E hooks

    func testCategoryIDs() -> [String] {
        categories.map(\.id)
    }

    func testVisibleCategoryIDs() -> [String] {
        visibleCategoryIDs
    }

    func testSelectedCategoryID() -> String? {
        selectedCategoryID
    }

    func testVisiblePageID() -> String? {
        pages.first(where: { !$0.value.isHidden })?.key
    }

    func testSelectCategory(_ id: String) {
        selectCategory(id)
    }

    private func makeDeveloperToolsRow() -> NSView {
        let status = NSTextField(labelWithString: "")
        status.font = .systemFont(ofSize: 11)
        status.textColor = .secondaryLabelColor
        status.lineBreakMode = .byTruncatingTail
        status.setAccessibilityIdentifier("muxterm.settings.developerTools.status")
        developerToolsStatusLabel = status

        let button = NSButton(
            title: DeveloperToolAccessPolicy.actionTitle(authorized: false),
            target: self,
            action: #selector(requestDeveloperTools(_:))
        )
        button.bezelStyle = .rounded
        button.setAccessibilityIdentifier("muxterm.settings.developerTools.action")
        developerToolsButton = button

        let control = NSStackView(views: [status, button])
        control.orientation = .vertical
        control.alignment = .trailing
        control.spacing = 6

        let row = settingRow(
            title: settingsFieldTitle(
                path: "macos.developer_tools",
                titleKey: "settings.platform.developer_tools"
            ),
            description: settingsFieldDescription(
                path: "macos.developer_tools",
                titleKey: "settings.platform.developer_tools"
            ),
            apply: settingsApplyLabel("macos"),
            path: "macos.developer_tools",
            control: control
        )
        settingsStyleCard(row, fill: NSColor.controlBackgroundColor.withAlphaComponent(0.44))
        refreshDeveloperToolsStatus()
        return row
    }

    private func refreshDeveloperToolsStatus() {
        let status = DeveloperToolAccess.status()
        let authorized = DeveloperToolAccessPolicy.isAuthorized(status)
        developerToolsButton?.title = settingsText(
            authorized
                ? "settings.platform.developer_tools.open"
                : "settings.platform.developer_tools.request",
            fallback: DeveloperToolAccessPolicy.actionTitle(authorized: authorized)
        )
        let statusKey = "settings.platform.developer_tools.status.\(status.rawValue)"
        let fallback: String = switch status {
        case .authorized: "Allowed"
        case .denied: "Denied — enable it in System Settings"
        case .restricted: "Restricted by a profile or parental controls"
        case .notDetermined: "Not listed yet — request access to add Muxterm"
        }
        developerToolsStatusLabel?.stringValue = settingsText(statusKey, fallback: fallback)
    }

    @objc private func requestDeveloperTools(_ sender: Any?) {
        DeveloperToolAccess.request { granted in
            DispatchQueue.main.async { [weak self] in
                self?.refreshDeveloperToolsStatus()
                if DeveloperToolAccessPolicy.shouldOpenSettings(afterRequestGranted: granted) {
                    DeveloperToolAccess.openSystemSettings()
                }
            }
        }
    }

    func testTextField(path: String) -> NSTextField? {
        textField(at: path)
    }

    func testSetSearchQuery(_ query: String) {
        searchField.stringValue = query
        applySearch()
    }

    func testSidebarWidth() -> CGFloat {
        window?.contentView?.layoutSubtreeIfNeeded()
        return sidebarView.frame.width
    }

    func testSearchPlaceholder() -> String? {
        searchField.placeholderString
    }

    func testVisiblePageIsScrollable() -> Bool {
        guard let selectedCategoryID, let page = pages[selectedCategoryID] else { return false }
        return page.hasVerticalScroller
    }

    func testControl(path: String) -> NSView? {
        controls[path]
    }

    func testProjectEditorVisible() -> Bool {
        projectEditorView != nil
    }

    func testProjectNames() -> [String] {
        quickConnectStore.projects.map(\.name)
    }

    func testHasNewProjectButton() -> Bool {
        projectEditorView?.hasNewProjectButton == true
    }

    func testProjectEditorContainsPlaceholder() -> Bool {
        projectEditorView?.containsPlaceholder == true
    }

    func testOpenProjectEditor(at index: Int) {
        openProjectEditor(at: index)
    }

    func testOpenNewProjectEditor() {
        openProjectEditor(nil)
    }

    func testActiveTargetConfigWindow() -> TargetConfigWindow? {
        activeProjectEditor
    }

    func testApplySettings() {
        applySettings()
    }

    func testDeveloperToolsRowVisible() -> Bool {
        findSettingsView(
            pages["platform"]?.documentView,
            id: "muxterm.settings.row.macos.developer_tools"
        ) != nil
    }

    func testDeveloperToolsActionTitle() -> String? {
        developerToolsButton?.title
    }

    func testDeveloperToolsStatusText() -> String? {
        developerToolsStatusLabel?.stringValue
    }

    @objc func testRequestDeveloperTools() {
        requestDeveloperTools(nil)
    }
}

private func findSettingsView(_ root: NSView?, id: String) -> NSView? {
    guard let root else { return nil }
    if root.accessibilityIdentifier() == id { return root }
    for child in root.subviews {
        if let hit = findSettingsView(child, id: id) { return hit }
    }
    return nil
}

private func settingsInputPlaceholder(_ path: String) -> String? {
    switch path {
    case "/tmux/default_session":
        return settingsText("settings.placeholder.workspace", fallback: "workspace name")
    case "/tmux/socket":
        return settingsText("settings.placeholder.socket", fallback: "default socket")
    case "/pane/default_command": return "$SHELL"
    case "/pane/workdir": return "$HOME"
    case "/ssh/host": return "example.com"
    case "/ssh/user": return settingsText("settings.placeholder.optional", fallback: "optional")
    case "/ssh/key_path": return "~/.ssh/id_ed25519"
    default: return nil
    }
}

private func settingsAppearancePreview(values: [String: Any]) -> NSView {
    let preview = NSStackView()
    preview.orientation = .vertical
    preview.alignment = .width
    preview.spacing = 12
    preview.setContentHuggingPriority(.defaultLow, for: .horizontal)
    preview.setContentCompressionResistancePriority(.required, for: .horizontal)
    settingsStyleCard(preview, fill: NSColor.controlBackgroundColor.withAlphaComponent(0.56))

    let title = NSTextField(labelWithString: settingsText(
        "settings.preview.title",
        fallback: "Terminal preview"
    ))
    title.font = .systemFont(ofSize: 13, weight: .bold)
    let badge = NSTextField(labelWithString: settingsText(
        "settings.preview.badge",
        fallback: "PREVIEW"
    ))
    badge.font = .systemFont(ofSize: 9, weight: .bold)
    badge.alignment = .center
    badge.textColor = .controlAccentColor
    badge.wantsLayer = true
    badge.layer?.backgroundColor = NSColor.controlAccentColor.withAlphaComponent(0.12).cgColor
    badge.layer?.cornerRadius = 5
    badge.widthAnchor.constraint(greaterThanOrEqualToConstant: 70).isActive = true
    let titleRow = NSStackView(views: [title, NSView(), badge])
    titleRow.orientation = .horizontal
    titleRow.alignment = .centerY
    titleRow.spacing = 8
    titleRow.edgeInsets = NSEdgeInsets(top: 16, left: 16, bottom: 0, right: 16)
    titleRow.setContentHuggingPriority(.defaultLow, for: .horizontal)
    preview.addArrangedSubview(titleRow)

    let terminal = NSStackView()
    terminal.orientation = .vertical
    terminal.alignment = .width
    terminal.spacing = 8
    terminal.edgeInsets = NSEdgeInsets(top: 12, left: 12, bottom: 12, right: 12)
    terminal.setContentHuggingPriority(.defaultLow, for: .horizontal)
    terminal.setContentCompressionResistancePriority(.required, for: .horizontal)
    terminal.wantsLayer = true
    terminal.layer?.backgroundColor = NSColor(calibratedRed: 0.06, green: 0.08, blue: 0.11, alpha: 1).cgColor
    terminal.layer?.cornerRadius = 8

    let dots = NSStackView()
    dots.orientation = .horizontal
    dots.spacing = 5
    for color in [NSColor.systemRed, NSColor.systemYellow, NSColor.systemGreen] {
        let dot = NSTextField(labelWithString: "●")
        dot.font = .systemFont(ofSize: 9)
        dot.textColor = color
        dots.addArrangedSubview(dot)
    }
    terminal.addArrangedSubview(dots)
    let prompt = NSTextField(labelWithString: settingsText(
        "settings.preview.prompt",
        fallback: "$ muxterm  --workspace ready"
    ))
    prompt.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
    prompt.textColor = NSColor(calibratedRed: 0.55, green: 0.91, blue: 1, alpha: 1)
    terminal.addArrangedSubview(prompt)
    let output = NSTextField(labelWithString: settingsText(
        "settings.preview.output",
        fallback: "Connected  ·  2 panes  ·  waiting for input"
    ))
    output.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
    output.textColor = NSColor(calibratedRed: 0.66, green: 0.71, blue: 0.78, alpha: 1)
    terminal.addArrangedSubview(output)
    preview.addArrangedSubview(terminal)

    let family = (settingsValue(at: "/font/family", in: values) as? String)
        .flatMap { $0.isEmpty ? nil : $0 } ?? "JetBrains Mono"
    let size = (settingsValue(at: "/font/size", in: values) as? NSNumber)?.doubleValue ?? 13
    let themeValue = settingsValue(at: "/theme/name", in: values) as? String ?? "system"
    let sizeText = String(format: "%.1f", size)
    let themeText = settingsOptionLabel(path: "/theme/name", value: themeValue)
    let summary = NSTextField(labelWithString: family + "  ·  " + sizeText + " pt  ·  " + themeText)
    summary.font = .systemFont(ofSize: 10)
    summary.textColor = .tertiaryLabelColor
    let summaryRow = NSStackView(views: [summary])
    summaryRow.alignment = .leading
    summaryRow.edgeInsets = NSEdgeInsets(top: 0, left: 16, bottom: 16, right: 16)
    preview.addArrangedSubview(summaryRow)
    return preview
}

private final class SettingsCategoryCellView: NSTableCellView {
    private let iconLabel = NSTextField(labelWithString: "")
    private let titleLabel = NSTextField(labelWithString: "")
    private let hintLabel = NSTextField(labelWithString: "")

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        build()
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        build()
    }

    func configure(title: String, hint: String, icon: String) {
        titleLabel.stringValue = title
        hintLabel.stringValue = hint
        iconLabel.stringValue = icon
    }

    private func build() {
        iconLabel.translatesAutoresizingMaskIntoConstraints = false
        iconLabel.alignment = .center
        iconLabel.font = .systemFont(ofSize: 15, weight: .bold)
        iconLabel.textColor = .controlAccentColor
        iconLabel.wantsLayer = true
        iconLabel.layer?.backgroundColor = NSColor.controlAccentColor.withAlphaComponent(0.10).cgColor
        iconLabel.layer?.cornerRadius = 6
        iconLabel.widthAnchor.constraint(equalToConstant: 28).isActive = true

        titleLabel.translatesAutoresizingMaskIntoConstraints = false
        titleLabel.font = .systemFont(ofSize: 12, weight: .semibold)
        titleLabel.lineBreakMode = .byTruncatingTail

        hintLabel.translatesAutoresizingMaskIntoConstraints = false
        hintLabel.font = .systemFont(ofSize: 10)
        hintLabel.textColor = .secondaryLabelColor
        hintLabel.lineBreakMode = .byTruncatingTail

        let copy = NSStackView(views: [titleLabel, hintLabel])
        copy.translatesAutoresizingMaskIntoConstraints = false
        copy.orientation = .vertical
        copy.alignment = .leading
        copy.spacing = 1

        addSubview(iconLabel)
        addSubview(copy)
        textField = titleLabel
        NSLayoutConstraint.activate([
            iconLabel.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 14),
            iconLabel.centerYAnchor.constraint(equalTo: centerYAnchor),
            copy.leadingAnchor.constraint(equalTo: iconLabel.trailingAnchor, constant: 10),
            copy.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -10),
            copy.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }
}

/// Settings 中的 Project 列表；编辑细节复用 TargetConfigWindow。
private final class SettingsProjectEditorView: NSView {
    var onNew: (() -> Void)?
    var onEdit: ((Int) -> Void)?
    var onDelete: ((Int) -> Void)?

    private(set) var projects: [TargetConfig]
    private let rows = NSStackView()
    private let newButton = NSButton(
        title: settingsText("settings.projects.new", fallback: "New project…"),
        target: nil,
        action: nil
    )

    init(projects: [TargetConfig]) {
        self.projects = projects
        super.init(frame: .zero)
        build()
        setProjects(projects)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    var hasNewProjectButton: Bool {
        newButton.accessibilityIdentifier() == "muxterm.settings.projects.new"
    }

    var containsPlaceholder: Bool {
        rows.arrangedSubviews.contains { view in
            (view as? NSTextField)?.stringValue == settingsText(
                "settings.projects.empty",
                fallback: "No projects yet"
            )
        }
    }

    func setProjects(_ projects: [TargetConfig]) {
        self.projects = projects
        rows.arrangedSubviews.forEach {
            rows.removeArrangedSubview($0)
            $0.removeFromSuperview()
        }
        if projects.isEmpty {
            let empty = NSTextField(labelWithString: settingsText(
                "settings.projects.empty",
                fallback: "No projects yet"
            ))
            empty.textColor = .secondaryLabelColor
            empty.font = .systemFont(ofSize: 12)
            empty.translatesAutoresizingMaskIntoConstraints = false
            rows.addArrangedSubview(empty)
            empty.widthAnchor.constraint(equalTo: rows.widthAnchor).isActive = true
            return
        }
        for (index, project) in projects.enumerated() {
            let row = makeRow(project: project, index: index)
            rows.addArrangedSubview(row)
            row.widthAnchor.constraint(equalTo: rows.widthAnchor).isActive = true
        }
    }

    private func build() {
        translatesAutoresizingMaskIntoConstraints = false

        let stack = NSStackView()
        stack.translatesAutoresizingMaskIntoConstraints = false
        stack.orientation = .vertical
        stack.alignment = .width
        stack.spacing = 10
        stack.setContentHuggingPriority(.defaultLow, for: .horizontal)
        stack.setContentCompressionResistancePriority(.required, for: .horizontal)

        newButton.target = self
        newButton.action = #selector(newProject)
        newButton.bezelStyle = .rounded
        newButton.setAccessibilityIdentifier("muxterm.settings.projects.new")
        let toolbar = NSView()
        toolbar.translatesAutoresizingMaskIntoConstraints = false
        newButton.translatesAutoresizingMaskIntoConstraints = false
        toolbar.addSubview(newButton)
        NSLayoutConstraint.activate([
            newButton.trailingAnchor.constraint(equalTo: toolbar.trailingAnchor),
            newButton.topAnchor.constraint(equalTo: toolbar.topAnchor),
            newButton.bottomAnchor.constraint(equalTo: toolbar.bottomAnchor),
        ])
        stack.addArrangedSubview(toolbar)
        toolbar.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true

        rows.orientation = .vertical
        rows.alignment = .width
        rows.spacing = 8
        rows.translatesAutoresizingMaskIntoConstraints = false
        stack.addArrangedSubview(rows)
        rows.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true

        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor),
            stack.topAnchor.constraint(equalTo: topAnchor),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
    }

    private func makeRow(project: TargetConfig, index: Int) -> NSView {
        let name = NSTextField(labelWithString: project.name)
        name.translatesAutoresizingMaskIntoConstraints = false
        name.font = .systemFont(ofSize: 13, weight: .semibold)
        name.lineBreakMode = .byTruncatingTail

        let details = NSTextField(labelWithString: projectDetail(project))
        details.translatesAutoresizingMaskIntoConstraints = false
        details.textColor = .secondaryLabelColor
        details.font = .systemFont(ofSize: 11)
        details.lineBreakMode = .byTruncatingMiddle

        let info = NSStackView(views: [name, details])
        info.orientation = .vertical
        info.alignment = .leading
        info.spacing = 2
        info.setContentHuggingPriority(.defaultLow, for: .horizontal)
        info.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        info.translatesAutoresizingMaskIntoConstraints = false

        let edit = NSButton(
            title: settingsText("settings.edit", fallback: "Edit"),
            target: self,
            action: #selector(editProject(_:))
        )
        edit.translatesAutoresizingMaskIntoConstraints = false
        edit.tag = index
        edit.bezelStyle = .rounded
        edit.setContentHuggingPriority(.required, for: .horizontal)
        edit.setContentCompressionResistancePriority(.required, for: .horizontal)
        edit.setAccessibilityIdentifier("muxterm.settings.projects.\(index).edit")
        let delete = NSButton(
            title: settingsText("settings.delete", fallback: "Delete"),
            target: self,
            action: #selector(deleteProject(_:))
        )
        delete.translatesAutoresizingMaskIntoConstraints = false
        delete.tag = index
        delete.bezelStyle = .rounded
        delete.contentTintColor = .systemRed
        delete.setContentHuggingPriority(.required, for: .horizontal)
        delete.setContentCompressionResistancePriority(.required, for: .horizontal)
        delete.setAccessibilityIdentifier("muxterm.settings.projects.\(index).delete")

        let row = NSView()
        row.translatesAutoresizingMaskIntoConstraints = false
        row.addSubview(info)
        row.addSubview(edit)
        row.addSubview(delete)
        NSLayoutConstraint.activate([
            info.leadingAnchor.constraint(equalTo: row.leadingAnchor, constant: 12),
            info.trailingAnchor.constraint(equalTo: edit.leadingAnchor, constant: -8),
            info.topAnchor.constraint(equalTo: row.topAnchor, constant: 10),
            info.bottomAnchor.constraint(equalTo: row.bottomAnchor, constant: -10),
            edit.trailingAnchor.constraint(equalTo: delete.leadingAnchor, constant: -8),
            edit.centerYAnchor.constraint(equalTo: row.centerYAnchor),
            delete.trailingAnchor.constraint(equalTo: row.trailingAnchor, constant: -12),
            delete.centerYAnchor.constraint(equalTo: row.centerYAnchor),
        ])
        settingsStyleCard(row, fill: NSColor.controlBackgroundColor.withAlphaComponent(0.42))
        row.setAccessibilityIdentifier("muxterm.settings.projects.\(index)")
        return row
    }

    private func projectDetail(_ project: TargetConfig) -> String {
        let transport: String
        switch project.transport {
        case .local:
            transport = "local"
        case .ssh(let name):
            transport = "ssh:\(name)"
        }
        let path = project.path.isEmpty
            ? settingsText("settings.projects.no_path", fallback: "(no path)")
            : project.path
        return "\(project.runtime.rawValue) · \(transport) · \(path)"
    }

    @objc private func newProject() {
        onNew?()
    }

    @objc private func editProject(_ sender: NSButton) {
        onEdit?(sender.tag)
    }

    @objc private func deleteProject(_ sender: NSButton) {
        onDelete?(sender.tag)
    }
}
