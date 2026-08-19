import XCTest
@testable import MuxtermAppLib

/// Linux 命令面板已有的纯前端动作，在 macOS 也必须可发现、可执行。
final class CommandPaletteParityE2ETests: XCTestCase {
    func testPaletteExposesExistingFrontendActionsAndTabs() throws {
        let painted = PaintedWorkspace(label: "palette-parity")
        let app = try AppE2E.attachWindow(socket: painted.socket, session: painted.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minTabs: 2))

        app.testOpenCommandPalette()
        AppE2E.pump(80)

        let titles = app.testPaletteTitles()
        let i18n = MuxtermI18n.shared
        for expected in [
            i18n.tr(.quickConnect),
            i18n.tr(.menuSearchPanes),
            i18n.tr(.menuIncreaseFontSize),
            i18n.tr(.menuDecreaseFontSize),
            i18n.tr(.menuResetFontSize),
            i18n.tr(.previousCommand),
            i18n.tr(.nextCommand),
            i18n.tr(.moveTabLeft),
            i18n.tr(.moveTabRight),
            i18n.tr(.menuSwitchTab, arguments: ["number": "1"]),
            i18n.tr(.menuSwitchTab, arguments: ["number": "2"]),
        ] {
            XCTAssertTrue(titles.contains(expected), "命令面板缺少已有前端动作：\(expected). titles=\(titles)")
        }
    }

    func testPaletteSearchActionOpensUnifiedSearchTab() throws {
        let painted = PaintedWorkspace(label: "palette-search")
        let app = try AppE2E.attachWindow(socket: painted.socket, session: painted.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minTabs: 2))

        app.testOpenCommandPalette()
        AppE2E.pump(40)
        app.testSelectPaletteTitle(MuxtermI18n.shared.tr(.menuSearchPanes))
        AppE2E.pump(80)

        XCTAssertFalse(app.testPaletteIsPresented())
        XCTAssertTrue(app.testSearchPanelOpen())
        XCTAssertEqual(app.unifiedPanel.modelTab, .search)
    }

    func testPaletteTabAndFontActionsUseProductionHandlers() throws {
        let fontKey = "muxterm.terminalFontSize"
        let defaults = UserDefaults.standard
        let previousFont = defaults.object(forKey: fontKey)
        defer {
            if let previousFont {
                defaults.set(previousFont, forKey: fontKey)
            } else {
                defaults.removeObject(forKey: fontKey)
            }
        }
        defaults.set(18.0, forKey: fontKey)

        let painted = PaintedWorkspace(label: "palette-actions")
        let app = try AppE2E.attachWindow(socket: painted.socket, session: painted.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minTabs: 2))
        let secondTab = app.testTabIDs()[1]

        app.testOpenCommandPalette()
        AppE2E.pump(40)
        app.testSelectPaletteTitle(
            MuxtermI18n.shared.tr(.menuSwitchTab, arguments: ["number": "2"])
        )
        XCTAssertTrue(AppE2E.wait(timeout: 2) {
            app.testPollOnce()
            return app.testActiveTabID() == secondTab
        })

        app.testOpenCommandPalette()
        AppE2E.pump(40)
        app.testSelectPaletteTitle(MuxtermI18n.shared.tr(.menuIncreaseFontSize))
        XCTAssertEqual(defaults.double(forKey: fontKey), 19.0, accuracy: 0.01)
    }
}
