import AppKit
import MuxtermChrome

/// iTerm2 风格的字体行：搜索等宽字体、字号、行高和字宽。
///
/// 选中一行即写回字段，不再依赖没有确认按钮的系统字体面板。
/// 行高/字宽在框里按百分比显示，写回配置时仍是 1.0 = 100%。
final class FontSettingsCluster: NSView {
    let familyField = NSTextField(labelWithString: "")
    let sizeField = NSTextField()
    let lineSpacingField = NSTextField()
    let columnSpacingField = NSTextField()

    private let familyButton = NSButton()
    private let favoriteButton = NSButton()
    private let sizeStepper = NSStepper()
    private let lineStepper = NSStepper()
    private let columnStepper = NSStepper()
    private var fontPopover: NSPopover?
    private weak var changeTarget: AnyObject?
    private let changeAction: Selector

    var family: String {
        familyField.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    var extraControls: [(String, NSTextField)] {
        [
            ("/font/size", sizeField),
            ("/font/line_spacing", lineSpacingField),
            ("/font/column_spacing", columnSpacingField),
        ]
    }

    init(values: [String: Any], target: AnyObject?, action: Selector) {
        changeTarget = target
        changeAction = action
        super.init(frame: .zero)
        let font = values["font"] as? [String: Any]
        let family = (font?["family"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
        let familyName = (family?.isEmpty == false ? family! : MuxtermTerminalFont.defaultFamily)
        let size = Self.cgFloat(font?["size"]) ?? MuxtermTerminalFont.defaultSize
        let line = Self.cgFloat(font?["line_spacing"]) ?? MuxtermTerminalFont.defaultSpacing
        let column = Self.cgFloat(font?["column_spacing"]) ?? MuxtermTerminalFont.defaultSpacing

        familyField.stringValue = familyName
        familyField.isHidden = true
        configureNumber(sizeField, identifier: "/font/size", text: Self.whole(size))
        configureNumber(
            lineSpacingField,
            identifier: "/font/line_spacing",
            text: "\(MuxtermTerminalFont.spacingPercent(line))"
        )
        configureNumber(
            columnSpacingField,
            identifier: "/font/column_spacing",
            text: "\(MuxtermTerminalFont.spacingPercent(column))"
        )

        familyButton.bezelStyle = .rounded
        familyButton.setButtonType(.momentaryPushIn)
        familyButton.title = familyName
        familyButton.target = self
        familyButton.action = #selector(chooseFamily(_:))
        familyButton.toolTip = "Search monospace fonts"
        familyButton.setAccessibilityIdentifier("muxterm.settings.font.family")

        favoriteButton.bezelStyle = .rounded
        favoriteButton.setButtonType(.momentaryPushIn)
        favoriteButton.target = self
        favoriteButton.action = #selector(toggleFavorite(_:))
        favoriteButton.setAccessibilityIdentifier("muxterm.settings.font.favorite")
        refreshFavoriteTitle()

        configureStepper(sizeStepper, value: Double(size), minimum: 9, maximum: 72, step: 1, tip: "Font size")
        configureStepper(
            lineStepper,
            value: Double(MuxtermTerminalFont.spacingPercent(line)),
            minimum: 50,
            maximum: 200,
            step: 5,
            tip: "Vertical spacing"
        )
        configureStepper(
            columnStepper,
            value: Double(MuxtermTerminalFont.spacingPercent(column)),
            minimum: 50,
            maximum: 200,
            step: 5,
            tip: "Horizontal spacing"
        )

        refreshFamilyFace()
        familyButton.setContentHuggingPriority(.required, for: .horizontal)
        favoriteButton.setContentHuggingPriority(.required, for: .horizontal)
        familyButton.widthAnchor.constraint(equalToConstant: 220).isActive = true
        for field in [sizeField, lineSpacingField, columnSpacingField] {
            field.widthAnchor.constraint(equalToConstant: 56).isActive = true
        }
        let fontControls = NSStackView(views: [familyButton, favoriteButton])
        fontControls.orientation = .horizontal
        fontControls.alignment = .centerY
        fontControls.spacing = 6
        let rows = NSStackView(views: [
            Self.optionRow(
                Self.localized("settings.font.family", "Font"),
                Self.localized("settings.font.family.description", "Monospace font used by the terminal."),
                fontControls
            ),
            Self.hairline(),
            Self.optionRow(
                Self.localized("settings.font.size", "Size"),
                Self.localized("settings.font.size.description", "Point size."),
                Self.metric(sizeField, sizeStepper)
            ),
            Self.hairline(),
            Self.optionRow(
                Self.localized("settings.font.line_spacing", "Vertical spacing"),
                Self.localized("settings.font.line_spacing.description", "100 keeps the font's natural line height."),
                Self.metric(lineSpacingField, lineStepper)
            ),
            Self.hairline(),
            Self.optionRow(
                Self.localized("settings.font.column_spacing", "Horizontal spacing"),
                Self.localized("settings.font.column_spacing.description", "100 keeps the font's natural character width."),
                Self.metric(columnSpacingField, columnStepper)
            ),
        ])
        rows.orientation = .vertical
        rows.alignment = .width
        rows.spacing = 0
        rows.translatesAutoresizingMaskIntoConstraints = false
        addSubview(rows)
        NSLayoutConstraint.activate([
            rows.leadingAnchor.constraint(equalTo: leadingAnchor),
            rows.trailingAnchor.constraint(equalTo: trailingAnchor),
            rows.topAnchor.constraint(equalTo: topAnchor),
            rows.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not used")
    }

    func applyFamily(_ name: String) {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        familyField.stringValue = trimmed
        refreshFamilyFace()
        refreshFavoriteTitle()
        notifyChange()
        fontPopover?.performClose(nil)
        fontPopover = nil
    }

    @objc private func chooseFamily(_ sender: Any?) {
        let popover = NSPopover()
        popover.behavior = .transient
        popover.contentSize = NSSize(width: 280, height: 360)
        popover.contentViewController = FontFamilyListController(cluster: self)
        fontPopover = popover
        popover.show(relativeTo: familyButton.bounds, of: familyButton, preferredEdge: .minY)
    }

    @objc private func toggleFavorite(_ sender: Any?) {
        MuxtermFontFavorites.toggle(family)
        refreshFavoriteTitle()
    }

    @objc private func stepperChanged(_ sender: NSStepper) {
        let text = Self.whole(CGFloat(sender.doubleValue))
        if sender === sizeStepper {
            sizeField.stringValue = text
        } else if sender === lineStepper {
            lineSpacingField.stringValue = text
        } else if sender === columnStepper {
            columnSpacingField.stringValue = text
        }
        notifyChange()
    }

    private func refreshFavoriteTitle() {
        let starred = MuxtermFontFavorites.contains(family)
        favoriteButton.title = starred ? "★" : "☆"
        favoriteButton.toolTip = starred ? "Remove favorite" : "Favorite this font"
    }

    private func notifyChange() {
        _ = changeTarget?.perform(changeAction, with: self)
    }

    private func configureNumber(_ field: NSTextField, identifier: String, text: String) {
        field.identifier = NSUserInterfaceItemIdentifier(identifier)
        field.alignment = .right
        field.stringValue = text
        field.target = changeTarget
        field.action = changeAction
        field.delegate = changeTarget as? NSTextFieldDelegate
        field.controlSize = .small
    }

    private func configureStepper(
        _ stepper: NSStepper,
        value: Double,
        minimum: Double,
        maximum: Double,
        step: Double,
        tip: String
    ) {
        stepper.minValue = minimum
        stepper.maxValue = maximum
        stepper.increment = step
        stepper.doubleValue = Swift.min(Swift.max(value, minimum), maximum)
        stepper.target = self
        stepper.action = #selector(stepperChanged(_:))
        stepper.toolTip = tip
        stepper.controlSize = .small
    }

    private static func localized(_ key: String, _ fallback: String) -> String {
        MuxtermI18n.shared.trDynamic(key, fallback: fallback)
    }

    private func refreshFamilyFace() {
        familyButton.title = familyField.stringValue
        familyButton.font = NSFont(name: familyField.stringValue, size: 13) ?? .systemFont(ofSize: 13)
    }

    private static func optionRow(_ title: String, _ detail: String, _ control: NSView) -> NSView {
        let titleLabel = NSTextField(labelWithString: title)
        titleLabel.font = .systemFont(ofSize: 13, weight: .semibold)
        let detailLabel = NSTextField(labelWithString: detail)
        detailLabel.font = .systemFont(ofSize: 12)
        detailLabel.textColor = .secondaryLabelColor
        detailLabel.lineBreakMode = .byWordWrapping
        detailLabel.maximumNumberOfLines = 2
        let copy = NSStackView(views: [titleLabel, detailLabel])
        copy.orientation = .vertical
        copy.alignment = .leading
        copy.spacing = 3
        copy.translatesAutoresizingMaskIntoConstraints = false
        control.translatesAutoresizingMaskIntoConstraints = false
        control.setContentHuggingPriority(.required, for: .horizontal)
        let row = NSView()
        row.addSubview(copy)
        row.addSubview(control)
        NSLayoutConstraint.activate([
            copy.leadingAnchor.constraint(equalTo: row.leadingAnchor, constant: 16),
            copy.trailingAnchor.constraint(lessThanOrEqualTo: control.leadingAnchor, constant: -16),
            copy.topAnchor.constraint(equalTo: row.topAnchor, constant: 14),
            copy.bottomAnchor.constraint(equalTo: row.bottomAnchor, constant: -14),
            control.trailingAnchor.constraint(equalTo: row.trailingAnchor, constant: -16),
            control.centerYAnchor.constraint(equalTo: row.centerYAnchor),
        ])
        return row
    }

    private static func hairline() -> NSView {
        let line = NSView()
        line.wantsLayer = true
        line.layer?.backgroundColor = NSColor.separatorColor.cgColor
        line.heightAnchor.constraint(equalToConstant: 1).isActive = true
        return line
    }

    private static func caption(_ text: String) -> NSTextField {
        let label = NSTextField(labelWithString: text)
        label.font = .systemFont(ofSize: 11)
        label.textColor = .secondaryLabelColor
        label.setContentHuggingPriority(.required, for: .horizontal)
        return label
    }

    private static func metric(_ field: NSTextField, _ stepper: NSStepper) -> NSStackView {
        let group = NSStackView(views: [field, stepper])
        group.orientation = .horizontal
        group.alignment = .centerY
        group.spacing = 4
        group.setContentHuggingPriority(.required, for: .horizontal)
        return group
    }

    private static func whole(_ value: CGFloat) -> String {
        String(Int(value.rounded()))
    }

    private static func cgFloat(_ value: Any?) -> CGFloat? {
        guard let number = value as? NSNumber else { return nil }
        return CGFloat(truncating: number)
    }
}

enum MuxtermFontFavorites {
    private static let key = "muxterm.font.favorites"

    static func all() -> [String] {
        UserDefaults.standard.stringArray(forKey: key) ?? []
    }

    static func contains(_ family: String) -> Bool {
        all().contains(family)
    }

    static func toggle(_ family: String) {
        var items = all()
        if let index = items.firstIndex(of: family) {
            items.remove(at: index)
        } else if !family.isEmpty {
            items.insert(family, at: 0)
        }
        UserDefaults.standard.set(items, forKey: key)
    }
}

enum MuxtermMonospaceFonts {
    static let families: [String] = {
        NSFontManager.shared.availableFontFamilies.filter { family in
            guard let font = NSFont(name: family, size: 12) ?? NSFont(name: family, size: 13) else {
                return false
            }
            if font.fontDescriptor.symbolicTraits.contains(.monoSpace) {
                return true
            }
            let wide = font.advancement(forGlyph: font.glyph(withName: "W")).width
            let narrow = font.advancement(forGlyph: font.glyph(withName: "i")).width
            return wide > 0 && abs(wide - narrow) < 0.5
        }.sorted { $0.localizedCaseInsensitiveCompare($1) == .orderedAscending }
    }()
}

private final class FontFamilyListController: NSViewController, NSTableViewDataSource, NSTableViewDelegate, NSSearchFieldDelegate {
    private weak var cluster: FontSettingsCluster?
    private let search = NSSearchField()
    private let table = NSTableView()
    private var visible: [String] = []
    private var ready = false

    init(cluster: FontSettingsCluster) {
        self.cluster = cluster
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not used")
    }

    override func loadView() {
        let root = NSView(frame: NSRect(x: 0, y: 0, width: 280, height: 360))
        search.placeholderString = "Search fonts"
        search.delegate = self
        search.translatesAutoresizingMaskIntoConstraints = false
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("font"))
        column.title = "Font"
        table.addTableColumn(column)
        table.headerView = nil
        table.dataSource = self
        table.delegate = self
        table.target = self
        table.doubleAction = #selector(pickSelected)
        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.documentView = table
        scroll.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(search)
        root.addSubview(scroll)
        NSLayoutConstraint.activate([
            search.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 8),
            search.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -8),
            search.topAnchor.constraint(equalTo: root.topAnchor, constant: 8),
            scroll.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            scroll.topAnchor.constraint(equalTo: search.bottomAnchor, constant: 8),
            scroll.bottomAnchor.constraint(equalTo: root.bottomAnchor),
        ])
        view = root
        reload(filter: "")
    }

    override func viewDidAppear() {
        super.viewDidAppear()
        ready = true
        view.window?.makeFirstResponder(search)
    }

    func controlTextDidChange(_ obj: Notification) {
        reload(filter: search.stringValue)
    }

    func numberOfRows(in tableView: NSTableView) -> Int {
        visible.count
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let name = visible[row]
        let starred = MuxtermFontFavorites.contains(name)
        let label = NSTextField(labelWithString: starred ? "★  \(name)" : name)
        if let font = NSFont(name: name, size: 13) {
            label.font = font
        }
        return label
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        guard ready, table.selectedRow >= 0, table.selectedRow < visible.count else { return }
        pick(visible[table.selectedRow])
    }

    @objc private func pickSelected() {
        guard table.selectedRow >= 0, table.selectedRow < visible.count else { return }
        pick(visible[table.selectedRow])
    }

    private func pick(_ name: String) {
        ready = false
        cluster?.applyFamily(name)
    }

    private func reload(filter: String) {
        let needle = filter.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        let favorites = MuxtermFontFavorites.all()
        let matched = MuxtermMonospaceFonts.families.filter { family in
            needle.isEmpty || family.lowercased().contains(needle)
        }
        let favoriteMatched = favorites.filter { matched.contains($0) }
        let rest = matched.filter { !favoriteMatched.contains($0) }
        visible = favoriteMatched + rest
        table.reloadData()
    }
}
