import XCTest
@testable import MuxtermChrome

final class ProgressiveWordSelectionTests: XCTestCase {
    func testFirstDoubleClickSelectsPathComponent() {
        let cells = Array("cd a/b/c")
        let first = ProgressiveWordSelection.select(cells: cells, column: 5, previous: nil)
        XCTAssertEqual(first?.start, 5)
        XCTAssertEqual(first?.end, 6)
        XCTAssertEqual(first?.level, .identifier)
        XCTAssertEqual(String(cells[(first!.start)..<(first!.end)]), "b")
    }

    func testSecondDoubleClickExpandsToWholePath() {
        let cells = Array("cd a/b/c")
        let first = ProgressiveWordSelection.select(cells: cells, column: 5, previous: nil)
        let second = ProgressiveWordSelection.select(cells: cells, column: 5, previous: first)
        XCTAssertEqual(String(cells[(second!.start)..<(second!.end)]), "a/b/c")
        XCTAssertEqual(second?.level, .path)
    }

    func testSecondDoubleClickOnAnyComponentOfTheFirstSelectionExpands() {
        let cells = Array("cd a/b/c")
        let first = ProgressiveWordSelection.select(cells: cells, column: 5, previous: nil)
        let second = ProgressiveWordSelection.select(cells: cells, column: 5, previous: first)
        XCTAssertEqual(String(cells[(second!.start)..<(second!.end)]), "a/b/c")
    }

    func testClickingSlashSelectsThePathImmediately() {
        let cells = Array("cd a/b/c")
        let result = ProgressiveWordSelection.select(cells: cells, column: 4, previous: nil)
        XCTAssertEqual(String(cells[(result!.start)..<(result!.end)]), "a/b/c")
        XCTAssertEqual(result?.level, .path)
    }

    func testSnakeCaseStaysOneIdentifier() {
        let cells = Array("echo snake_case")
        let result = ProgressiveWordSelection.select(cells: cells, column: 8, previous: nil)
        XCTAssertEqual(String(cells[(result!.start)..<(result!.end)]), "snake_case")
        XCTAssertEqual(result?.level, .identifier)
    }

    func testDottedNameExpandsOnSecondClick() {
        let cells = Array("cat foo.bar")
        let first = ProgressiveWordSelection.select(cells: cells, column: 4, previous: nil)
        XCTAssertEqual(String(cells[(first!.start)..<(first!.end)]), "foo")
        let second = ProgressiveWordSelection.select(cells: cells, column: 4, previous: first)
        XCTAssertEqual(String(cells[(second!.start)..<(second!.end)]), "foo.bar")
    }

    func testClickOutsidePreviousSelectionStartsANewWord() {
        let cells = Array("cd a/b/c extra")
        let first = ProgressiveWordSelection.select(cells: cells, column: 5, previous: nil)
        let other = ProgressiveWordSelection.select(cells: cells, column: 10, previous: first)
        XCTAssertEqual(String(cells[(other!.start)..<(other!.end)]), "extra")
        XCTAssertEqual(other?.level, .identifier)
    }

    func testBracketsDeferToSwiftTerm() {
        XCTAssertNil(ProgressiveWordSelection.select(cells: Array("foo(bar)"), column: 3, previous: nil))
        XCTAssertNil(ProgressiveWordSelection.select(cells: Array("[hint]"), column: 0, previous: nil))
    }

    func testHomePathExpandsWithTilde() {
        let cells = Array("cd ~/src/muxterm")
        let first = ProgressiveWordSelection.select(cells: cells, column: 6, previous: nil)
        XCTAssertEqual(String(cells[(first!.start)..<(first!.end)]), "src")
        let second = ProgressiveWordSelection.select(cells: cells, column: 6, previous: first)
        XCTAssertEqual(String(cells[(second!.start)..<(second!.end)]), "~/src/muxterm")
    }
}
