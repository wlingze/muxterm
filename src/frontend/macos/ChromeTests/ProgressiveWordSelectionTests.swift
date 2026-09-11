import XCTest
@testable import MuxtermChrome

final class ProgressiveWordSelectionTests: XCTestCase {
    private func selected(_ cells: [Character], _ result: ProgressiveWordSelection.Result?) -> String? {
        guard let result else { return nil }
        return String(cells[result.start..<result.end])
    }

    private func column(_ cells: [Character], of needle: String, occurrence: Int = 0) -> Int {
        let haystack = String(cells)
        var start = haystack.startIndex
        var seen = 0
        while let range = haystack[start...].range(of: needle) {
            if seen == occurrence {
                return haystack.distance(from: haystack.startIndex, to: range.lowerBound)
            }
            seen += 1
            start = range.upperBound
        }
        XCTFail("missing \(needle) in \(haystack)")
        return 0
    }

    func testFirstDoubleClickSelectsPathComponent() {
        let cells = Array("cd a/b/c")
        let first = ProgressiveWordSelection.select(cells: cells, column: 5, previous: nil)
        XCTAssertEqual(selected(cells, first), "b")
        XCTAssertEqual(first?.level, .identifier)
    }

    func testSecondDoubleClickExpandsToWholePathWhenNoHyphenLayer() {
        let cells = Array("cd a/b/c")
        let first = ProgressiveWordSelection.select(cells: cells, column: 5, previous: nil)
        let second = ProgressiveWordSelection.select(cells: cells, column: 5, previous: first)
        XCTAssertEqual(selected(cells, second), "a/b/c")
        XCTAssertEqual(second?.level, .path)
    }

    func testSecondDoubleClickOnAnyComponentOfTheFirstSelectionExpands() {
        let cells = Array("cd a/b/c")
        let first = ProgressiveWordSelection.select(cells: cells, column: 5, previous: nil)
        let second = ProgressiveWordSelection.select(cells: cells, column: 5, previous: first)
        XCTAssertEqual(selected(cells, second), "a/b/c")
    }

    func testClickingSlashSelectsThePathImmediately() {
        let cells = Array("cd a/b/c")
        let result = ProgressiveWordSelection.select(cells: cells, column: 4, previous: nil)
        XCTAssertEqual(selected(cells, result), "a/b/c")
        XCTAssertEqual(result?.level, .path)
    }

    func testSnakeCaseStaysOneIdentifier() {
        let cells = Array("echo snake_case")
        let result = ProgressiveWordSelection.select(cells: cells, column: 8, previous: nil)
        XCTAssertEqual(selected(cells, result), "snake_case")
        XCTAssertEqual(result?.level, .identifier)
    }

    func testDottedNameExpandsOnSecondClick() {
        let cells = Array("cat foo.bar")
        let first = ProgressiveWordSelection.select(cells: cells, column: 4, previous: nil)
        XCTAssertEqual(selected(cells, first), "foo")
        let second = ProgressiveWordSelection.select(cells: cells, column: 4, previous: first)
        XCTAssertEqual(selected(cells, second), "foo.bar")
        XCTAssertEqual(second?.level, .word)
    }

    func testClickOutsidePreviousSelectionStartsANewWord() {
        let cells = Array("cd a/b/c extra")
        let first = ProgressiveWordSelection.select(cells: cells, column: 5, previous: nil)
        let other = ProgressiveWordSelection.select(cells: cells, column: 10, previous: first)
        XCTAssertEqual(selected(cells, other), "extra")
        XCTAssertEqual(other?.level, .identifier)
    }

    func testBracketsDeferToSwiftTerm() {
        XCTAssertNil(ProgressiveWordSelection.select(cells: Array("foo(bar)"), column: 3, previous: nil))
        XCTAssertNil(ProgressiveWordSelection.select(cells: Array("[hint]"), column: 0, previous: nil))
    }

    func testHomePathExpandsWithTilde() {
        let cells = Array("cd ~/src/muxterm")
        let first = ProgressiveWordSelection.select(cells: cells, column: 6, previous: nil)
        XCTAssertEqual(selected(cells, first), "src")
        let second = ProgressiveWordSelection.select(cells: cells, column: 6, previous: first)
        XCTAssertEqual(selected(cells, second), "~/src/muxterm")
        XCTAssertEqual(second?.level, .path)
    }

    func testGitBranchFirstClickSelectsDigits() {
        let cells = Array("~/Developer/self/muxterm feature/dogfood-0907*")
        let col = column(cells, of: "0907")
        let first = ProgressiveWordSelection.select(cells: cells, column: col, previous: nil)
        XCTAssertEqual(selected(cells, first), "0907")
        XCTAssertEqual(first?.level, .identifier)
    }

    func testGitBranchSecondClickIncludesHyphenAndStar() {
        let cells = Array("~/Developer/self/muxterm feature/dogfood-0907*")
        let col = column(cells, of: "0907")
        let first = ProgressiveWordSelection.select(cells: cells, column: col, previous: nil)
        let second = ProgressiveWordSelection.select(cells: cells, column: col, previous: first)
        XCTAssertEqual(selected(cells, second), "dogfood-0907*")
        XCTAssertEqual(second?.level, .word)
    }

    func testGitBranchThirdClickIncludesSlash() {
        let cells = Array("~/Developer/self/muxterm feature/dogfood-0907*")
        let col = column(cells, of: "0907")
        let first = ProgressiveWordSelection.select(cells: cells, column: col, previous: nil)
        let second = ProgressiveWordSelection.select(cells: cells, column: col, previous: first)
        let third = ProgressiveWordSelection.select(cells: cells, column: col, previous: second)
        XCTAssertEqual(selected(cells, third), "feature/dogfood-0907*")
        XCTAssertEqual(third?.level, .path)
    }

    func testGitBranchFourthClickStaysOnPathToken() {
        let cells = Array("~/Developer/self/muxterm feature/dogfood-0907*")
        let col = column(cells, of: "0907")
        var result = ProgressiveWordSelection.select(cells: cells, column: col, previous: nil)
        result = ProgressiveWordSelection.select(cells: cells, column: col, previous: result)
        result = ProgressiveWordSelection.select(cells: cells, column: col, previous: result)
        let fourth = ProgressiveWordSelection.select(cells: cells, column: col, previous: result)
        XCTAssertEqual(selected(cells, fourth), "feature/dogfood-0907*")
        XCTAssertEqual(fourth?.level, .path)
    }

    func testClickingHyphenStartsAtWordLevel() {
        let cells = Array("feature/dogfood-0907*")
        let result = ProgressiveWordSelection.select(
            cells: cells,
            column: column(cells, of: "-"),
            previous: nil
        )
        XCTAssertEqual(selected(cells, result), "dogfood-0907*")
        XCTAssertEqual(result?.level, .word)
    }

    func testClickingStarStartsAtWordLevel() {
        let cells = Array("feature/dogfood-0907*")
        let result = ProgressiveWordSelection.select(
            cells: cells,
            column: column(cells, of: "*"),
            previous: nil
        )
        XCTAssertEqual(selected(cells, result), "dogfood-0907*")
        XCTAssertEqual(result?.level, .word)
    }

    func testWhitespaceTokenIsTheSpaceDelimitedRun() {
        let cells = Array("~/Developer/self/muxterm feature/dogfood-0907*")
        let branch = ProgressiveWordSelection.tokenSpan(cells: cells, column: column(cells, of: "0907"))
        XCTAssertEqual(String(cells[branch.start..<branch.end]), "feature/dogfood-0907*")
        let path = ProgressiveWordSelection.tokenSpan(cells: cells, column: column(cells, of: "Developer"))
        XCTAssertEqual(String(cells[path.start..<path.end]), "~/Developer/self/muxterm")
    }

    func testDragWithinATokenSnapsToTheWhitespaceWord() {
        let cells = Array("~/Developer/self/muxterm feature/dogfood-0907*")
        let range = ProgressiveWordSelection.dragByTokens(
            cells: cells,
            anchorColumn: column(cells, of: "0907"),
            toColumn: column(cells, of: "feature")
        )
        XCTAssertEqual(String(cells[range.start..<range.end]), "feature/dogfood-0907*")
    }

    func testDragAcrossSpaceUnitesWholeWords() {
        let cells = Array("~/Developer/self/muxterm feature/dogfood-0907*")
        let range = ProgressiveWordSelection.dragByTokens(
            cells: cells,
            anchorColumn: column(cells, of: "0907"),
            toColumn: column(cells, of: "muxterm")
        )
        XCTAssertEqual(
            String(cells[range.start..<range.end]),
            "~/Developer/self/muxterm feature/dogfood-0907*"
        )
    }

    func testDragOntoWhitespaceKeepsTheAnchorWordAndTheGap() {
        let cells = Array("alpha  beta")
        let space = cells.firstIndex(of: " ") ?? 5
        let range = ProgressiveWordSelection.dragByTokens(
            cells: cells,
            anchorColumn: 1,
            toColumn: space
        )
        XCTAssertEqual(String(cells[range.start..<range.end]), "alpha  ")
    }
}
