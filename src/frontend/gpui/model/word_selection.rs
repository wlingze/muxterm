//! 终端双击选词：层层扩大，拖选用空格切词。
//!
//! 移植自 `chrome/ProgressiveWordSelection.swift`。`feature/dogfood-0907*`
//! 点在 `0907` 上：第一次 `0907`，第二次 `dogfood-0907*`，第三次
//! `feature/dogfood-0907*`。中间那一层扩不开就跳过（`cd a/b/c` 的 `b`
//! 第二次直接到 `a/b/c`）。双击后拖选用空白分隔的整词，对齐 Ghostty /
//! iTerm 的 word-drag，不会在 `/` 或 `-` 处切开。
//!
//! 格子里的 CJK 宽字符占位格用 `'\0'` 表示（filler）。Rust 侧和 Swift 一样
//! 只在 span 两端裁掉 filler，中间保留。

/// 选词的三层，值越大越宽。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// 字母、数字、下划线（含 CJK）。
    Identifier = 1,
    /// 标识符 + 连字符 / 点 / 通配等：`-.+%@#?*`。
    Word = 2,
    /// 词 + 路径分隔：`/\~:`。
    Path = 3,
}

impl Level {
    fn next(self) -> Option<Level> {
        match self {
            Level::Identifier => Some(Level::Word),
            Level::Word => Some(Level::Path),
            Level::Path => None,
        }
    }
}

/// 一次选词结果，列区间左闭右开。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub start: usize,
    pub end: usize,
    pub level: Level,
}

impl Selection {
    pub fn contains(&self, column: usize) -> bool {
        column >= self.start && column < self.end
    }
}

/// 第二层：kebab / 文件名 / git 脏标记，不含路径分隔。
pub const WORD_EXTRAS: &[char] = &['-', '.', '*', '?', '+', '%', '#', '@', '='];

/// 第三层：路径与 URI 分隔。
pub const PATH_EXTRAS: &[char] = &['/', '\\', '~', ':'];

/// 判断某个格子是不是宽字符的占位格。
///
/// Swift 还处理了「所有 scalar 都是 0」的组合字符；单个 `Character` 里除
/// NUL 之外不可能是这种形状，因此这里只判 NUL。
pub fn is_filler(ch: char) -> bool {
    ch == '\0'
}

pub fn is_identifier(ch: char) -> bool {
    if is_filler(ch) {
        return false;
    }
    ch.is_alphabetic() || ch.is_numeric() || ch == '_'
}

pub fn is_word_extra(ch: char) -> bool {
    WORD_EXTRAS.contains(&ch)
}

pub fn is_path_extra(ch: char) -> bool {
    PATH_EXTRAS.contains(&ch)
}

pub fn is_word(ch: char) -> bool {
    is_identifier(ch) || is_word_extra(ch) || is_filler(ch)
}

pub fn is_path(ch: char) -> bool {
    is_word(ch) || is_path_extra(ch)
}

pub fn is_bracket_or_quote(ch: char) -> bool {
    "()[]{}<>\"'`".contains(ch)
}

pub fn is_whitespace(ch: char) -> bool {
    if is_filler(ch) {
        return false;
    }
    ch == ' ' || ch.is_whitespace()
}

/// 点选：`previous` 是上一次双击结果，点还在里面就升级，否则从最紧的一层重来。
///
/// 括号 / 引号返回 `None`，让调用方走平衡表达式匹配。
pub fn select(cells: &[char], column: usize, previous: Option<&Selection>) -> Option<Selection> {
    if cells.is_empty() {
        return None;
    }
    let col = column.min(cells.len() - 1);
    let ch = cells[col];
    if is_bracket_or_quote(ch) {
        return None;
    }

    if let Some(previous) = previous {
        if previous.contains(col) {
            return Some(expand(cells, col, *previous));
        }
    }

    if is_whitespace(ch) {
        let (start, end) = span(cells, col, is_whitespace);
        return Some(Selection {
            start,
            end,
            level: Level::Identifier,
        });
    }
    if is_identifier(ch) || (is_filler(ch) && neighboring_identifier(cells, col)) {
        let (start, end) = span(cells, col, is_identifier_or_filler);
        return Some(Selection {
            start,
            end,
            level: Level::Identifier,
        });
    }
    if is_word_extra(ch) {
        let (start, end) = span(cells, col, is_word);
        return Some(Selection {
            start,
            end,
            level: Level::Word,
        });
    }
    if is_path_extra(ch) {
        let (start, end) = span(cells, col, is_path);
        return Some(Selection {
            start,
            end,
            level: Level::Path,
        });
    }
    Some(Selection {
        start: col,
        end: col + 1,
        level: Level::Identifier,
    })
}

/// 空白分隔的整词（点在空白上时就是空白段）。拖选用这个，不按 `/` `-` 切。
pub fn token_span(cells: &[char], column: usize) -> (usize, usize) {
    if cells.is_empty() {
        return (0, 0);
    }
    let col = column.min(cells.len() - 1);
    if is_whitespace(cells[col]) {
        return span(cells, col, is_whitespace);
    }
    span(cells, col, |ch| !is_whitespace(ch))
}

/// 从锚点词拖到目标列：两端都吸附到空白分隔的整词，中间空白一并选上。
pub fn drag_by_tokens(cells: &[char], anchor_column: usize, to_column: usize) -> (usize, usize) {
    let anchor = token_span(cells, anchor_column);
    let target = token_span(cells, to_column);
    (anchor.0.min(target.0), anchor.1.max(target.1))
}

fn is_identifier_or_filler(ch: char) -> bool {
    is_identifier(ch) || is_filler(ch)
}

fn predicate_for(level: Level) -> fn(char) -> bool {
    match level {
        Level::Identifier => is_identifier_or_filler,
        Level::Word => is_word,
        Level::Path => is_path,
    }
}

fn expand(cells: &[char], column: usize, previous: Selection) -> Selection {
    let mut level = previous.level;
    while let Some(next) = level.next() {
        let (start, end) = span(cells, column, predicate_for(next));
        if start < previous.start || end > previous.end {
            return Selection {
                start,
                end,
                level: next,
            };
        }
        level = next;
    }
    previous
}

fn neighboring_identifier(cells: &[char], col: usize) -> bool {
    (col > 0 && is_identifier(cells[col - 1]))
        || (col + 1 < cells.len() && is_identifier(cells[col + 1]))
}

fn span(cells: &[char], column: usize, predicate: fn(char) -> bool) -> (usize, usize) {
    let mut start = column;
    while start > 0 && predicate(cells[start - 1]) {
        start -= 1;
    }
    let mut end = column;
    while end < cells.len() && predicate(cells[end]) {
        end += 1;
    }
    if start < end && is_filler(cells[start]) {
        while start < end && is_filler(cells[start]) {
            start += 1;
        }
    }
    if start < end && is_filler(cells[end - 1]) {
        while end > start && is_filler(cells[end - 1]) {
            end -= 1;
        }
    }
    if start >= end {
        return (column, (column + 1).min(cells.len()));
    }
    (start, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(line: &str) -> Vec<char> {
        line.chars().collect()
    }

    fn selected(cells: &[char], result: Option<&Selection>) -> String {
        match result {
            None => "<none>".to_string(),
            Some(result) => cells[result.start..result.end].iter().collect(),
        }
    }

    fn column(cells: &[char], needle: &str) -> usize {
        let haystack: String = cells.iter().collect();
        haystack
            .find(needle)
            .unwrap_or_else(|| panic!("missing {needle} in {haystack}"))
    }

    /// 对照 `testFirstDoubleClickSelectsPathComponent`。
    #[test]
    fn first_double_click_selects_path_component() {
        let line = cells("cd a/b/c");
        let first = select(&line, 5, None);
        assert_eq!(selected(&line, first.as_ref()), "b");
        assert_eq!(first.unwrap().level, Level::Identifier);
    }

    /// 对照 `testSecondDoubleClickExpandsToWholePathWhenNoHyphenLayer`。
    #[test]
    fn second_double_click_expands_to_whole_path_when_no_hyphen_layer() {
        let line = cells("cd a/b/c");
        let first = select(&line, 5, None);
        let second = select(&line, 5, first.as_ref());
        assert_eq!(selected(&line, second.as_ref()), "a/b/c");
        assert_eq!(second.unwrap().level, Level::Path);
    }

    /// 对照 `testSecondDoubleClickOnAnyComponentOfTheFirstSelectionExpands`。
    ///
    /// 上一层的选区里点任意一列都算「还在里面」，从那一层继续升级，而不是
    /// 重新从最紧的一层开始。
    #[test]
    fn second_double_click_on_any_component_of_the_first_selection_expands() {
        let line = cells("feature/dogfood-0907*");
        // 点 `-` 直接拿第二层：`dogfood-0907*`。
        let first = select(&line, column(&line, "-"), None).unwrap();
        assert_eq!(first.level, Level::Word);
        for col in first.start..first.end {
            let second = select(&line, col, Some(&first)).unwrap();
            assert_eq!(
                selected(&line, Some(&second)),
                "feature/dogfood-0907*",
                "列 {col}"
            );
            assert_eq!(second.level, Level::Path);
        }
        // 选区外的列不沿用上一层。
        let outside = select(&line, column(&line, "feature"), Some(&first)).unwrap();
        assert_eq!(selected(&line, Some(&outside)), "feature");
    }

    /// 对照 `testClickingSlashSelectsThePathImmediately`。
    #[test]
    fn clicking_slash_selects_the_path_immediately() {
        let line = cells("cd a/b/c");
        let result = select(&line, 4, None);
        assert_eq!(selected(&line, result.as_ref()), "a/b/c");
        assert_eq!(result.unwrap().level, Level::Path);
    }

    /// 对照 `testSnakeCaseStaysOneIdentifier`。
    #[test]
    fn snake_case_stays_one_identifier() {
        let line = cells("echo snake_case");
        let result = select(&line, 8, None);
        assert_eq!(selected(&line, result.as_ref()), "snake_case");
        assert_eq!(result.unwrap().level, Level::Identifier);
    }

    /// 对照 `testDottedNameExpandsOnSecondClick`。
    #[test]
    fn dotted_name_expands_on_second_click() {
        let line = cells("cat foo.bar");
        let first = select(&line, 4, None);
        assert_eq!(selected(&line, first.as_ref()), "foo");
        let second = select(&line, 4, first.as_ref());
        assert_eq!(selected(&line, second.as_ref()), "foo.bar");
        assert_eq!(second.unwrap().level, Level::Word);
    }

    /// 对照 `testClickOutsidePreviousSelectionStartsANewWord`。
    #[test]
    fn click_outside_previous_selection_starts_a_new_word() {
        let line = cells("cd a/b/c extra");
        let first = select(&line, 5, None);
        let other = select(&line, 10, first.as_ref());
        assert_eq!(selected(&line, other.as_ref()), "extra");
        assert_eq!(other.unwrap().level, Level::Identifier);
    }

    /// 对照 `testBracketsDeferToSwiftTerm`。
    #[test]
    fn brackets_defer_to_balanced_matching() {
        assert!(select(&cells("foo(bar)"), 3, None).is_none());
        assert!(select(&cells("[hint]"), 0, None).is_none());
    }

    /// 对照 `testHomePathExpandsWithTilde`。
    #[test]
    fn home_path_expands_with_tilde() {
        let line = cells("cd ~/src/muxterm");
        let first = select(&line, 6, None);
        assert_eq!(selected(&line, first.as_ref()), "src");
        let second = select(&line, 6, first.as_ref());
        assert_eq!(selected(&line, second.as_ref()), "~/src/muxterm");
        assert_eq!(second.unwrap().level, Level::Path);
    }

    /// 对照 `testGitBranchFirstClickSelectsDigits`。
    #[test]
    fn git_branch_first_click_selects_digits() {
        let line = cells("~/Developer/self/muxterm feature/dogfood-0907*");
        let col = column(&line, "0907");
        let first = select(&line, col, None);
        assert_eq!(selected(&line, first.as_ref()), "0907");
        assert_eq!(first.unwrap().level, Level::Identifier);
    }

    /// 对照 `testGitBranchSecondClickIncludesHyphenAndStar`。
    #[test]
    fn git_branch_second_click_includes_hyphen_and_star() {
        let line = cells("~/Developer/self/muxterm feature/dogfood-0907*");
        let col = column(&line, "0907");
        let first = select(&line, col, None);
        let second = select(&line, col, first.as_ref());
        assert_eq!(selected(&line, second.as_ref()), "dogfood-0907*");
        assert_eq!(second.unwrap().level, Level::Word);
    }

    /// 对照 `testGitBranchThirdClickIncludesSlash`。
    #[test]
    fn git_branch_third_click_includes_slash() {
        let line = cells("~/Developer/self/muxterm feature/dogfood-0907*");
        let col = column(&line, "0907");
        let first = select(&line, col, None);
        let second = select(&line, col, first.as_ref());
        let third = select(&line, col, second.as_ref());
        assert_eq!(selected(&line, third.as_ref()), "feature/dogfood-0907*");
        assert_eq!(third.unwrap().level, Level::Path);
    }

    /// 对照 `testGitBranchFourthClickStaysOnPathToken`。
    #[test]
    fn git_branch_fourth_click_stays_on_path_token() {
        let line = cells("~/Developer/self/muxterm feature/dogfood-0907*");
        let col = column(&line, "0907");
        let mut result = select(&line, col, None);
        for _ in 0..2 {
            result = select(&line, col, result.as_ref());
        }
        let fourth = select(&line, col, result.as_ref());
        assert_eq!(selected(&line, fourth.as_ref()), "feature/dogfood-0907*");
        assert_eq!(fourth.unwrap().level, Level::Path);
    }

    /// 对照 `testClickingHyphenStartsAtWordLevel`。
    #[test]
    fn clicking_hyphen_starts_at_word_level() {
        let line = cells("feature/dogfood-0907*");
        let col = column(&line, "-");
        let result = select(&line, col, None);
        assert_eq!(selected(&line, result.as_ref()), "dogfood-0907*");
        assert_eq!(result.unwrap().level, Level::Word);
    }

    /// 对照 `testClickingStarStartsAtWordLevel`。
    #[test]
    fn clicking_star_starts_at_word_level() {
        let line = cells("feature/dogfood-0907*");
        let col = column(&line, "*");
        let result = select(&line, col, None);
        assert_eq!(selected(&line, result.as_ref()), "dogfood-0907*");
        assert_eq!(result.unwrap().level, Level::Word);
    }

    /// 对照 `testWhitespaceTokenIsTheSpaceDelimitedRun`。
    #[test]
    fn whitespace_token_is_the_space_delimited_run() {
        let line = cells("~/Developer/self/muxterm feature/dogfood-0907*");
        let branch = token_span(&line, column(&line, "0907"));
        assert_eq!(
            line[branch.0..branch.1].iter().collect::<String>(),
            "feature/dogfood-0907*"
        );
        let path = token_span(&line, column(&line, "Developer"));
        assert_eq!(
            line[path.0..path.1].iter().collect::<String>(),
            "~/Developer/self/muxterm"
        );
    }

    /// 对照 `testDragWithinATokenSnapsToTheWhitespaceWord`。
    #[test]
    fn drag_within_a_token_snaps_to_the_whitespace_word() {
        let line = cells("~/Developer/self/muxterm feature/dogfood-0907*");
        let (start, end) = drag_by_tokens(&line, column(&line, "0907"), column(&line, "feature"));
        assert_eq!(
            line[start..end].iter().collect::<String>(),
            "feature/dogfood-0907*"
        );
    }

    /// 对照 `testDragAcrossSpaceUnitesWholeWords`。
    #[test]
    fn drag_across_space_unites_whole_words() {
        let line = cells("~/Developer/self/muxterm feature/dogfood-0907*");
        let (start, end) = drag_by_tokens(&line, column(&line, "0907"), column(&line, "muxterm"));
        assert_eq!(
            line[start..end].iter().collect::<String>(),
            "~/Developer/self/muxterm feature/dogfood-0907*"
        );
    }

    /// 对照 `testDragOntoWhitespaceKeepsTheAnchorWordAndTheGap`。
    #[test]
    fn drag_onto_whitespace_keeps_the_anchor_word_and_the_gap() {
        let line = cells("alpha  beta");
        let space = line.iter().position(|ch| *ch == ' ').unwrap();
        let (start, end) = drag_by_tokens(&line, 1, space);
        assert_eq!(line[start..end].iter().collect::<String>(), "alpha  ");
    }

    /// 空行没有可选内容；单格也不会越界。
    #[test]
    fn degenerate_grids_stay_in_bounds() {
        assert!(select(&[], 0, None).is_none());
        assert_eq!(token_span(&[], 0), (0, 0));
        assert_eq!(token_span(&cells("ab"), 99), (0, 2));
        let picked = select(&cells("ab"), 99, None).unwrap();
        assert_eq!((picked.start, picked.end), (0, 2));
    }

    /// 宽字符占位格（filler）跟着相邻标识符一起被选中，但不会留在两端。
    #[test]
    fn filler_cells_join_identifiers_without_leading_or_trailing_padding() {
        // "中文" 在格子里是 4 列：中, 占位, 文, 占位。
        let line: Vec<char> = vec!['\0', '中', '\0', '文', '\0', ' '];
        let picked = select(&line, 1, None).unwrap();
        assert_eq!(picked.start, 1);
        assert_eq!(picked.end, 4);
        assert_eq!(picked.level, Level::Identifier);
        // 点在孤立的占位格上：左右都没有标识符，退化成单格。
        let lonely: Vec<char> = vec![' ', '\0', ' '];
        let picked = select(&lonely, 1, None).unwrap();
        assert_eq!((picked.start, picked.end), (1, 2));
    }

    /// 空白双击选中整段空白，与 Swift 的 `span(isWhitespace)` 一致。
    #[test]
    fn whitespace_selection_covers_the_whole_run() {
        let line = cells("a   b");
        let picked = select(&line, 2, None).unwrap();
        assert_eq!(picked.start, 1);
        assert_eq!(picked.end, 4);
        assert_eq!(picked.level, Level::Identifier);
    }
}
