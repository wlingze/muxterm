//! 独立输入框（composer）的草稿状态与 Markdown 上色。
//!
//! 移植自 macOS `Chrome/ComposerDraft.swift` 与 `Chrome/ComposerDrafts.swift`。
//! 偏移一律用 **UTF-16 code unit**，与 AppKit 的 `NSRange` 完全一致：gpui 的
//! `EntityInputHandler` 也按 UTF-16 报 range，这里不做两套坐标。

use std::fmt;

/// UTF-16 区间，等价于 `NSRange`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextRange {
    pub location: usize,
    pub length: usize,
}

impl TextRange {
    pub const fn new(location: usize, length: usize) -> Self {
        Self { location, length }
    }

    pub fn end(&self) -> usize {
        self.location + self.length
    }

    fn clamped(self, units: usize) -> Self {
        let location = self.location.min(units);
        let available = units - location;
        Self {
            location,
            length: self.length.min(available),
        }
    }

    fn intersects(&self, other: Self) -> bool {
        self.location < other.end() && other.location < self.end()
    }
}

/// 文本的 UTF-16 code unit 列表。所有高亮计算都在它上面做。
pub fn utf16_units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// 文本的 UTF-16 长度。
pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// 按 UTF-16 区间切原文；落在代理对中间时退化成替换字符（与 AppKit 一致）。
pub fn slice_utf16(text: &str, range: TextRange) -> String {
    let units = utf16_units(text);
    let range = range.clamped(units.len());
    String::from_utf16_lossy(&units[range.location..range.end()])
}

/// 把 UTF-16 区间换算成字节区间，供 gpui 的文本切分使用。
pub fn utf16_range_to_bytes(text: &str, range: TextRange) -> std::ops::Range<usize> {
    let mut utf16_seen = 0usize;
    let mut bytes = std::ops::Range { start: 0, end: 0 };
    let mut started = false;
    if range.location == 0 {
        started = true;
        bytes.start = 0;
    }
    for (byte_index, ch) in text.char_indices() {
        if utf16_seen == range.location && !started {
            started = true;
            bytes.start = byte_index;
        }
        if started && utf16_seen == range.end() {
            bytes.end = byte_index;
            return bytes;
        }
        utf16_seen += ch.len_utf16();
    }
    if !started {
        return text.len()..text.len();
    }
    if utf16_seen >= range.location && !started {
        bytes.start = text.len();
    }
    bytes.end = text.len();
    bytes
}

/// 输入框绑定的目标终端类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposerTargetKind {
    Local,
    Ssh,
}

/// 输入框绑定的目标：决定徽章文案，以及发送时落到哪台机器。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerTarget {
    pub kind: ComposerTargetKind,
    /// SSH 别名；本地为空串。
    pub host: String,
    pub workspace_name: String,
    /// Core workspace id；发送与图片上传都用它定位。
    pub workspace_id: Option<String>,
    pub pane_id: Option<u32>,
}

impl ComposerTarget {
    pub fn local(workspace_name: impl Into<String>, workspace_id: impl Into<String>) -> Self {
        Self {
            kind: ComposerTargetKind::Local,
            host: String::new(),
            workspace_name: workspace_name.into(),
            workspace_id: Some(workspace_id.into()),
            pane_id: None,
        }
    }

    /// 「Local」/「SSH」标记；SSH 不带别名，徽章保持短。
    pub fn badge(&self) -> &'static str {
        match self.kind {
            ComposerTargetKind::Local => "Local",
            ComposerTargetKind::Ssh => "SSH",
        }
    }

    /// 徽章旁常驻的完整去向：SSH 显示别名，本地只显示工作区。
    pub fn detail(&self) -> String {
        match self.kind {
            ComposerTargetKind::Local => self.workspace_name.clone(),
            ComposerTargetKind::Ssh => {
                if self.host.is_empty() {
                    self.workspace_name.clone()
                } else {
                    format!("{} · {}", self.host, self.workspace_name)
                }
            }
        }
    }
}

/// 输入框的内容状态。
///
/// 发送时永远只发送用户写的原文：粘贴图片插入的是**纯路径**，不加引号、
/// 不加 Markdown 图片语法、不自动回车。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ComposerDraft {
    text: String,
}

impl ComposerDraft {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
    }

    /// 没有任何用户写下的内容（只有空白字符也算空）。
    pub fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }

    /// 要发给终端的字节内容；空内容返回 `None`，让调用方保持面板不动。
    pub fn payload(&self) -> Option<&str> {
        if self.is_blank() {
            None
        } else {
            Some(&self.text)
        }
    }
}

/// 插入图片路径后的新文本与新光标（都是 UTF-16 偏移）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageInsertion {
    pub text: String,
    pub caret: usize,
}

/// 在 UTF-16 选区处插入图片路径。
///
/// 只在需要分隔时才补空格：开头不补前导空格，末尾不补尾随空格，避免
/// 发送文本里多出不可见的结尾空白。
pub fn inserting_image_path(path: &str, text: &str, selected: TextRange) -> ImageInsertion {
    let trimmed = path.trim();
    let units = utf16_units(text);
    if trimmed.is_empty() {
        return ImageInsertion {
            text: text.to_string(),
            caret: selected.location.min(units.len()),
        };
    }
    let range = selected.clamped(units.len());
    let is_space = |unit: u16| char::from_u32(u32::from(unit)).is_some_and(char::is_whitespace);
    let before = range
        .location
        .checked_sub(1)
        .map(|index| units[index])
        .filter(|unit| !is_space(*unit));
    let after = units
        .get(range.end())
        .copied()
        .filter(|unit| !is_space(*unit));

    let mut insertion = String::new();
    if before.is_some() {
        insertion.push(' ');
    }
    insertion.push_str(trimmed);
    if after.is_some() {
        insertion.push(' ');
    }

    let mut result: Vec<u16> = Vec::with_capacity(units.len() + 8);
    result.extend_from_slice(&units[..range.location]);
    result.extend(insertion.encode_utf16());
    result.extend_from_slice(&units[range.end()..]);
    ImageInsertion {
        text: String::from_utf16_lossy(&result),
        caret: range.location + insertion.encode_utf16().count(),
    }
}

/// 原文上的一段高亮；文本本身不被改写。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposerHighlightStyle {
    /// `#`、`*`、反引号、代码围栏这些语法符号。
    Marker,
    Header(u8),
    Bold,
    Italic,
    InlineCode,
    CodeBlock,
    Quote,
    ListMarker,
    /// 绝对路径 / `~/` 路径，方便认出粘进来的图片路径。
    Path,
}

impl fmt::Display for ComposerHighlightStyle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ComposerHighlightStyle::Marker => write!(f, "marker"),
            ComposerHighlightStyle::Header(level) => write!(f, "header({level})"),
            ComposerHighlightStyle::Bold => write!(f, "bold"),
            ComposerHighlightStyle::Italic => write!(f, "italic"),
            ComposerHighlightStyle::InlineCode => write!(f, "inlineCode"),
            ComposerHighlightStyle::CodeBlock => write!(f, "codeBlock"),
            ComposerHighlightStyle::Quote => write!(f, "quote"),
            ComposerHighlightStyle::ListMarker => write!(f, "listMarker"),
            ComposerHighlightStyle::Path => write!(f, "path"),
        }
    }
}

/// 一段高亮：源文本的 UTF-16 区间 + 样式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComposerHighlight {
    pub range: TextRange,
    pub style: ComposerHighlightStyle,
}

/// 单栏编辑器的 Markdown 语法高亮。
///
/// 不生成第二份渲染文本：`#`、`**`、列表符号都留在原文里，复制和 Cmd-Enter
/// 发出去的就是用户写的那一串。这里只标出哪些 UTF-16 区间该上色。
/// 未闭合的标记不当高亮，避免打到一半就跳样式。
pub fn highlights(source: &str) -> Vec<ComposerHighlight> {
    if source.is_empty() {
        return Vec::new();
    }
    let mut spans = Vec::new();
    let mut in_fence = false;
    let mut base = 0usize;
    for line in source.split('\n') {
        highlight_line(&utf16_units(line), base, &mut in_fence, &mut spans);
        base += utf16_len(line) + 1;
    }
    spans
}

const SPACE: u16 = b' ' as u16;
const TAB: u16 = b'\t' as u16;
const HASH: u16 = b'#' as u16;
const STAR: u16 = b'*' as u16;
const BACKTICK: u16 = b'`' as u16;
const TILDE: u16 = b'~' as u16;
const SLASH: u16 = b'/' as u16;
const DASH: u16 = b'-' as u16;
const PLUS: u16 = b'+' as u16;
const UNDERSCORE: u16 = b'_' as u16;
const DOT: u16 = b'.' as u16;
const GT: u16 = b'>' as u16;

fn is_space(unit: u16) -> bool {
    unit == SPACE || unit == TAB
}

fn is_ascii_unit(unit: u16) -> Option<u8> {
    u8::try_from(unit).ok()
}

fn starts_with(line: &[u16], index: usize, token: &str) -> bool {
    let token = utf16_units(token);
    line.len() >= index + token.len() && line[index..index + token.len()] == token[..]
}

fn mark(location: usize, length: usize, style: ComposerHighlightStyle) -> ComposerHighlight {
    ComposerHighlight {
        range: TextRange::new(location, length),
        style,
    }
}

fn highlight_line(
    line: &[u16],
    base: usize,
    in_fence: &mut bool,
    spans: &mut Vec<ComposerHighlight>,
) {
    let trimmed: Vec<u16> = {
        let start = line.iter().position(|unit| !is_space(*unit)).unwrap_or(0);
        let end = line
            .iter()
            .rposition(|unit| !is_space(*unit))
            .map_or(0, |i| i + 1);
        line[start..end.max(start)].to_vec()
    };
    if starts_with(&trimmed, 0, "```") {
        if !line.is_empty() {
            spans.push(mark(base, line.len(), ComposerHighlightStyle::Marker));
        }
        *in_fence = !*in_fence;
        return;
    }
    if *in_fence {
        if !line.is_empty() {
            spans.push(mark(base, line.len(), ComposerHighlightStyle::CodeBlock));
        }
        return;
    }

    let indent = line.iter().take_while(|unit| is_space(**unit)).count();
    let body = &line[indent..];
    if let Some((level, marker)) = atx_heading(body) {
        let marker_length = indent + marker;
        spans.push(mark(base, marker_length, ComposerHighlightStyle::Marker));
        let content = line.len().saturating_sub(marker_length);
        if content > 0 {
            spans.push(mark(
                base + marker_length,
                content,
                ComposerHighlightStyle::Header(level),
            ));
            highlight_inline(&line[marker_length..], base + marker_length, spans);
        }
        return;
    }
    if let Some(marker_length) = list_marker_length(body) {
        spans.push(mark(
            base + indent,
            marker_length,
            ComposerHighlightStyle::ListMarker,
        ));
        let content_start = indent + marker_length;
        if content_start < line.len() {
            highlight_inline(&line[content_start..], base + content_start, spans);
        }
        add_paths(line, base, spans);
        return;
    }
    if body.first() == Some(&GT) {
        let marker_length = if body.len() > 1 && body[1] == SPACE {
            2
        } else {
            1
        };
        spans.push(mark(
            base + indent,
            marker_length,
            ComposerHighlightStyle::Marker,
        ));
        let content_start = indent + marker_length;
        if content_start < line.len() {
            spans.push(mark(
                base + content_start,
                line.len() - content_start,
                ComposerHighlightStyle::Quote,
            ));
            highlight_inline(&line[content_start..], base + content_start, spans);
        }
        return;
    }
    if is_thematic_break(&trimmed) {
        spans.push(mark(base, line.len(), ComposerHighlightStyle::Marker));
        return;
    }
    highlight_inline(line, base, spans);
    add_paths(line, base, spans);
}

fn atx_heading(body: &[u16]) -> Option<(u8, usize)> {
    let mut hashes = 0usize;
    while hashes < body.len() && body[hashes] == HASH && hashes < 6 {
        hashes += 1;
    }
    if !(1..=6).contains(&hashes) {
        return None;
    }
    if hashes == body.len() {
        return Some((hashes as u8, hashes));
    }
    if body[hashes] != SPACE {
        return None;
    }
    Some((hashes as u8, hashes + 1))
}

/// `- ` / `* ` / `+ ` / `1. ` 的长度，含标记后的空白。
fn list_marker_length(body: &[u16]) -> Option<usize> {
    if body.len() < 2 {
        return None;
    }
    let first = body[0];
    if first == DASH || first == STAR || first == PLUS {
        return (body[1] == SPACE).then_some(2);
    }
    let mut digits = 0usize;
    while digits < body.len() && (u16::from(b'0')..=u16::from(b'9')).contains(&body[digits]) {
        digits += 1;
    }
    if digits == 0 || digits + 1 >= body.len() {
        return None;
    }
    if body[digits] == DOT && body[digits + 1] == SPACE {
        Some(digits + 2)
    } else {
        None
    }
}

fn is_thematic_break(trimmed: &[u16]) -> bool {
    if trimmed.len() < 3 {
        return false;
    }
    [DASH, STAR, UNDERSCORE]
        .iter()
        .any(|symbol| trimmed.iter().all(|unit| unit == symbol))
}

fn highlight_inline(line: &[u16], base: usize, spans: &mut Vec<ComposerHighlight>) {
    let mut index = 0usize;
    while index < line.len() {
        if line[index] == BACKTICK {
            if let Some(end) = closing(line, index + 1, "`") {
                spans.push(mark(base + index, 1, ComposerHighlightStyle::Marker));
                if end > index + 1 {
                    spans.push(mark(
                        base + index + 1,
                        end - index - 1,
                        ComposerHighlightStyle::InlineCode,
                    ));
                }
                spans.push(mark(base + end, 1, ComposerHighlightStyle::Marker));
                index = end + 1;
                continue;
            }
        }
        if starts_with(line, index, "**") {
            if let Some(end) = closing(line, index + 2, "**") {
                spans.push(mark(base + index, 2, ComposerHighlightStyle::Marker));
                if end > index + 2 {
                    spans.push(mark(
                        base + index + 2,
                        end - index - 2,
                        ComposerHighlightStyle::Bold,
                    ));
                }
                spans.push(mark(base + end, 2, ComposerHighlightStyle::Marker));
                index = end + 2;
                continue;
            }
        }
        if line[index] == STAR && can_open_star(line, index) {
            if let Some(end) = closing(line, index + 1, "*") {
                spans.push(mark(base + index, 1, ComposerHighlightStyle::Marker));
                if end > index + 1 {
                    spans.push(mark(
                        base + index + 1,
                        end - index - 1,
                        ComposerHighlightStyle::Italic,
                    ));
                }
                spans.push(mark(base + end, 1, ComposerHighlightStyle::Marker));
                index = end + 1;
                continue;
            }
        }
        index += 1;
    }
}

/// 绝对路径单独上色，但不要盖住代码、标记或已经上过的样式。
fn add_paths(line: &[u16], base: usize, spans: &mut Vec<ComposerHighlight>) {
    let mut index = 0usize;
    while index < line.len() {
        let boundary = index == 0 || is_space(line[index - 1]);
        let head = line[index];
        let is_path_head = head == SLASH || (head == TILDE && line.get(index + 1) == Some(&SLASH));
        if !boundary || !is_path_head {
            index += 1;
            continue;
        }
        let mut end = index;
        while end < line.len() && !is_space(line[end]) {
            end += 1;
        }
        let range = TextRange::new(base + index, end - index);
        if !spans.iter().any(|span| span.range.intersects(range)) {
            spans.push(ComposerHighlight {
                range,
                style: ComposerHighlightStyle::Path,
            });
        }
        index = end.max(index + 1);
    }
}

/// `*` 只有在后面不是空白、不是另一个 `*` 时才算开标记。
fn can_open_star(line: &[u16], index: usize) -> bool {
    if index + 1 >= line.len() {
        return false;
    }
    let next = line[index + 1];
    next != SPACE && next != STAR && next != b'\n' as u16
}

/// 返回闭合标记的起点；找不到返回 `None`（半截标记保持纯文本）。
fn closing(line: &[u16], from: usize, token: &str) -> Option<usize> {
    let token = utf16_units(token);
    if from >= line.len() || token.is_empty() {
        return None;
    }
    let limit = line.len().checked_sub(token.len())?;
    (from..=limit).find(|start| line[*start..*start + token.len()] == token[..])
}

/// 草稿的稳定身份：一个 pane 一份草稿。
///
/// 草稿不跟着前台焦点走：在 A pane 写了一半切到 B，回来时 A 的内容还在。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ComposerDraftKey {
    pub workspace_id: String,
    pub pane_id: u32,
}

impl ComposerDraftKey {
    pub fn new(workspace_id: impl Into<String>, pane_id: u32) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            pane_id,
        }
    }

    /// 从目标推导身份；没有目标时是占位页（空工作区 id + pane 0）。
    pub fn from_target(target: Option<&ComposerTarget>) -> Self {
        Self {
            workspace_id: target
                .and_then(|target| target.workspace_id.clone())
                .unwrap_or_default(),
            pane_id: target.and_then(|target| target.pane_id).unwrap_or(0),
        }
    }

    /// 真正能写回终端的目标。
    pub fn is_bound(&self) -> bool {
        !self.workspace_id.is_empty()
    }
}

/// 输入框里的一页：文本、光标和目标都属于这个 pane。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerDraftPage {
    pub key: ComposerDraftKey,
    /// 最近一次打开这一页时的目标快照；写入和上传都用它。
    pub target: Option<ComposerTarget>,
    pub text: String,
    /// UTF-16 选区；切页后恢复光标。
    pub selection: TextRange,
}

impl ComposerDraftPage {
    pub fn new(key: ComposerDraftKey, target: Option<ComposerTarget>) -> Self {
        Self {
            key,
            target,
            text: String::new(),
            selection: TextRange::default(),
        }
    }

    /// 没有任何用户写下的内容（只有空白字符也算空）。
    pub fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }

    /// tab 标题：工作区名 + pane 号，保证同一工作区的多个 pane 能区分。
    pub fn title(&self) -> String {
        Self::title_for(self.target.as_ref(), &self.key)
    }

    fn title_for(target: Option<&ComposerTarget>, key: &ComposerDraftKey) -> String {
        if !key.is_bound() {
            return "—".to_string();
        }
        let name = target
            .map(|target| target.workspace_name.trim())
            .unwrap_or_default();
        let short = if name.is_empty() {
            "pane".to_string()
        } else {
            name.chars().take(12).collect()
        };
        format!("{short} #{}", key.pane_id)
    }
}

/// 按 pane 保存草稿。窗口关闭只收起窗口，不改这里，所以草稿不会丢。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComposerDraftStore {
    pages: Vec<ComposerDraftPage>,
    active_key: Option<ComposerDraftKey>,
}

impl ComposerDraftStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pages(&self) -> &[ComposerDraftPage] {
        &self.pages
    }

    pub fn active_key(&self) -> Option<&ComposerDraftKey> {
        self.active_key.as_ref()
    }

    pub fn active_page(&self) -> Option<&ComposerDraftPage> {
        let key = self.active_key.as_ref()?;
        self.pages.iter().find(|page| &page.key == key)
    }

    pub fn active_index(&self) -> Option<usize> {
        let key = self.active_key.as_ref()?;
        self.pages.iter().position(|page| &page.key == key)
    }

    pub fn page(&self, key: &ComposerDraftKey) -> Option<&ComposerDraftPage> {
        self.pages.iter().find(|page| &page.key == key)
    }

    /// tab 栏里真正要画的页：当前页，加上已经写了内容的页。
    ///
    /// 空草稿不占位置：打开输入框只该看到自己正在写的那个 pane。
    pub fn visible_pages(&self) -> Vec<&ComposerDraftPage> {
        self.pages
            .iter()
            .filter(|page| Some(&page.key) == self.active_key.as_ref() || !page.is_blank())
            .collect()
    }

    /// 当前页在 `visible_pages` 里的位置；没有页时为 `None`。
    pub fn active_visible_index(&self) -> Option<usize> {
        let key = self.active_key.as_ref()?;
        self.visible_pages()
            .iter()
            .position(|page| &page.key == key)
    }

    /// 打开某个目标：已有草稿就切过去并刷新目标快照，没有才新建空页。
    pub fn activate(&mut self, target: Option<ComposerTarget>) -> &ComposerDraftPage {
        let key = ComposerDraftKey::from_target(target.as_ref());
        if let Some(index) = self.pages.iter().position(|page| page.key == key) {
            if let Some(target) = target {
                self.pages[index].target = Some(target);
            }
            self.active_key = Some(key);
            return &self.pages[index];
        }
        self.pages.push(ComposerDraftPage::new(key.clone(), target));
        self.active_key = Some(key);
        self.pages.last().expect("刚 push 过")
    }

    pub fn select(&mut self, key: &ComposerDraftKey) -> bool {
        if !self.pages.iter().any(|page| &page.key == key) {
            return false;
        }
        self.active_key = Some(key.clone());
        true
    }

    pub fn select_index(&mut self, index: usize) -> bool {
        let Some(page) = self.pages.get(index) else {
            return false;
        };
        self.active_key = Some(page.key.clone());
        true
    }

    /// 按 `visible_pages` 的下标切页；tab 栏只认这一套下标。
    pub fn select_visible(&mut self, index: usize) -> bool {
        let Some(key) = self.visible_pages().get(index).map(|page| page.key.clone()) else {
            return false;
        };
        self.active_key = Some(key);
        true
    }

    pub fn update(
        &mut self,
        text: impl Into<String>,
        selection: TextRange,
        key: &ComposerDraftKey,
    ) {
        if let Some(page) = self.pages.iter_mut().find(|page| &page.key == key) {
            page.text = text.into();
            page.selection = selection;
        }
    }

    /// 只清空这一页；其他 pane 的草稿原样保留。
    pub fn clear(&mut self, key: &ComposerDraftKey) {
        if let Some(page) = self.pages.iter_mut().find(|page| &page.key == key) {
            page.text.clear();
            page.selection = TextRange::default();
        }
    }
}

/// 输入框窗口的竖长尺寸：宽度略小于半屏，高度接近八成屏高。
pub mod window_layout {
    pub const MINIMUM_WIDTH: f32 = 420.0;
    pub const MINIMUM_HEIGHT: f32 = 460.0;

    /// 纯函数，方便验证「竖长、不占满屏、不窄到放不下动作菜单」。
    pub fn size(screen_width: f32, screen_height: f32) -> (f32, f32) {
        if screen_width <= 0.0 || screen_height <= 0.0 {
            return (560.0, 760.0);
        }
        let mut width = (screen_width * 0.38).clamp(460.0, 720.0);
        width = width.min((screen_width / 2.0 - 24.0).max(320.0));
        let height = (screen_height * 0.78)
            .max(width + 160.0)
            .min(screen_height - 48.0)
            .max(MINIMUM_HEIGHT);
        (width, height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssh_target() -> ComposerTarget {
        ComposerTarget {
            kind: ComposerTargetKind::Ssh,
            host: "ryzen".into(),
            workspace_name: "api".into(),
            workspace_id: Some("ssh:ryzen:api".into()),
            pane_id: Some(3),
        }
    }

    /// 对照 `ComposerDraftTests.testSSHBadgeNamesTheMachineAndStaysShort`。
    #[test]
    fn ssh_badge_names_the_machine_and_stays_short() {
        let target = ssh_target();
        assert_eq!(target.badge(), "SSH");
        assert_eq!(target.detail(), "ryzen · api");
    }

    /// 对照 `ComposerDraftTests.testLocalBadgeOnlyShowsTheWorkspace`。
    #[test]
    fn local_badge_only_shows_the_workspace() {
        let target = ComposerTarget {
            kind: ComposerTargetKind::Local,
            host: String::new(),
            workspace_name: "~/Developer/self/muxterm".into(),
            workspace_id: Some("local:shell".into()),
            pane_id: Some(1),
        };
        assert_eq!(target.badge(), "Local");
        assert_eq!(target.detail(), "~/Developer/self/muxterm");
    }

    /// 对照 `ComposerDraftTests.testBlankDraftNeverSendsWhitespace`。
    #[test]
    fn blank_draft_never_sends_whitespace() {
        assert_eq!(ComposerDraft::new("  \n\t").payload(), None);
        assert_eq!(
            ComposerDraft::new("git status").payload(),
            Some("git status")
        );
    }

    /// 对照 `ComposerDraftTests.testImagePathIsInsertedRawWithoutEscaping`。
    #[test]
    fn image_path_is_inserted_raw_without_escaping() {
        let result =
            inserting_image_path("/tmp/muxterm-paste-ab12cd.png", "", TextRange::new(0, 0));
        assert_eq!(result.text, "/tmp/muxterm-paste-ab12cd.png");
        assert_eq!(result.caret, 29);
    }

    /// 对照 `ComposerDraftTests.testImagePathAfterTextAddsExactlyOneSeparator`。
    #[test]
    fn image_path_after_text_adds_exactly_one_separator() {
        let result = inserting_image_path("/tmp/a.png", "see", TextRange::new(3, 0));
        assert_eq!(result.text, "see /tmp/a.png");
        assert_eq!(result.caret, utf16_len("see /tmp/a.png"));
    }

    /// 对照 `ComposerDraftTests.testImagePathReplacesTheSelectionWithoutDoubleSpaces`。
    #[test]
    fn image_path_replaces_the_selection_without_double_spaces() {
        let source = "before OLD after";
        let start = source.find("OLD").expect("OLD");
        let result = inserting_image_path("/tmp/new.png", source, TextRange::new(start, 3));
        assert_eq!(result.text, "before /tmp/new.png after");
        assert!(!result.text.contains("  "), "选区间已经自带空格，不能再补");
    }

    /// 对照 `ComposerDraftTests.testBlankImagePathLeavesTextUntouched`。
    #[test]
    fn blank_image_path_leaves_text_untouched() {
        let result = inserting_image_path("   ", "keep", TextRange::new(4, 0));
        assert_eq!(result.text, "keep");
        assert_eq!(result.caret, 4);
    }

    /// 选区比文本长时夹到合法范围，而不是 panic。
    #[test]
    fn out_of_range_selection_is_clamped() {
        let result = inserting_image_path("/tmp/a.png", "ab", TextRange::new(9, 9));
        assert_eq!(result.text, "ab /tmp/a.png");
        assert_eq!(result.caret, utf16_len("ab /tmp/a.png"));
    }

    /// 对照 `ComposerDraftTests.testMarkdownHighlightsDoNotRewriteTheDraft`。
    #[test]
    fn markdown_highlights_do_not_rewrite_the_draft() {
        let source = "# Hello\n\n/tmp/muxterm-paste-ab12cd.png";
        let spans = highlights(source);
        assert!(
            spans
                .iter()
                .any(|span| span.style == ComposerHighlightStyle::Path
                    && slice_utf16(source, span.range) == "/tmp/muxterm-paste-ab12cd.png"),
            "路径必须作为原文的一部分被标记出来"
        );
        assert!(spans
            .iter()
            .any(|span| span.style == ComposerHighlightStyle::Header(1)
                && slice_utf16(source, span.range) == "Hello"));
        assert!(spans
            .iter()
            .any(|span| span.style == ComposerHighlightStyle::Marker));
    }

    /// 对照 `ComposerDraftTests.testMarkdownHighlightsMarkListSymbolsInsteadOfRenamingThem`。
    #[test]
    fn markdown_highlights_mark_list_symbols() {
        let source = "- one\n1. two";
        let markers: Vec<String> = highlights(source)
            .iter()
            .filter(|span| span.style == ComposerHighlightStyle::ListMarker)
            .map(|span| slice_utf16(source, span.range))
            .collect();
        assert_eq!(markers, ["- ", "1. "]);
    }

    /// 对照 `ComposerDraftTests.testMarkdownHighlightsCoverCodeFenceAndQuote`。
    #[test]
    fn markdown_highlights_cover_code_fence_and_quote() {
        let source = "```\nlet x = 1\n```\n\n> note";
        let spans = highlights(source);
        assert!(spans
            .iter()
            .any(|span| span.style == ComposerHighlightStyle::CodeBlock
                && slice_utf16(source, span.range) == "let x = 1"));
        assert!(spans
            .iter()
            .any(|span| span.style == ComposerHighlightStyle::Quote
                && slice_utf16(source, span.range) == "note"));
    }

    /// 对照 `ComposerDraftTests.testMarkdownHighlightsStayEmptyForAnEmptyDraft`。
    #[test]
    fn markdown_highlights_stay_empty_for_an_empty_draft() {
        assert!(highlights("").is_empty());
    }

    /// 对照 `ComposerDraftTests.testMarkdownHighlightsHitTheExpectedRuns`。
    #[test]
    fn markdown_highlights_hit_the_expected_runs() {
        let source = "# Title\n\n**bold** and `code`";
        let first = |style: ComposerHighlightStyle| {
            highlights(source)
                .into_iter()
                .find(|span| span.style == style)
                .map(|span| slice_utf16(source, span.range))
        };
        assert_eq!(
            first(ComposerHighlightStyle::Header(1)).as_deref(),
            Some("Title")
        );
        assert_eq!(first(ComposerHighlightStyle::Bold).as_deref(), Some("bold"));
        assert_eq!(
            first(ComposerHighlightStyle::InlineCode).as_deref(),
            Some("code")
        );
        assert_eq!(
            first(ComposerHighlightStyle::Path),
            None,
            "纯文本里的 / 不是路径"
        );
    }

    /// 对照 `ComposerDraftTests.testUnclosedMarkersStayPlainUntilTheyAreClosed`。
    #[test]
    fn unclosed_markers_stay_plain() {
        let spans = highlights("**半截");
        assert!(!spans
            .iter()
            .any(|span| span.style == ComposerHighlightStyle::Bold));
    }

    /// 对照 `ComposerDraftTests.testCmdIOpensTheComposerAndShiftIsNotStolen`（键位部分在
    /// `model::shortcuts` 覆盖，这里只确认草稿侧的 `open_composer` 名字）。
    #[test]
    fn composer_action_name_matches_the_config_spelling() {
        assert_eq!(
            crate::frontend::gpui::model::shortcuts::KeyAction::OpenComposer,
            crate::frontend::gpui::model::shortcuts::action_from_name("open_composer")
                .expect("open_composer")
        );
    }

    /// 高亮偏移是 UTF-16；emoji 占两个 code unit。
    #[test]
    fn highlight_offsets_are_utf16() {
        let source = "# 🚀 go";
        let spans = highlights(source);
        let header = spans
            .iter()
            .find(|span| span.style == ComposerHighlightStyle::Header(1))
            .expect("header");
        assert_eq!(header.range, TextRange::new(2, 5), "🚀 占两个 code unit");
        assert_eq!(slice_utf16(source, header.range), "🚀 go");
        assert_eq!(utf16_range_to_bytes(source, header.range), 2..source.len());
    }

    fn target(pane: u32, workspace: &str) -> ComposerTarget {
        ComposerTarget {
            kind: ComposerTargetKind::Local,
            host: String::new(),
            workspace_name: workspace.to_string(),
            workspace_id: Some(format!("local:{workspace}")),
            pane_id: Some(pane),
        }
    }

    /// 对照 `ComposerDraftsTests.testEachPaneKeepsItsOwnDraftWhenSwitching`。
    #[test]
    fn each_pane_keeps_its_own_draft_when_switching() {
        let mut store = ComposerDraftStore::new();
        let key = store.activate(Some(target(1, "api"))).key.clone();
        store.update("first", TextRange::new(5, 0), &key);

        let second = store.activate(Some(target(2, "api"))).key.clone();
        assert_eq!(store.active_page().map(|page| page.text.as_str()), Some(""));
        store.update("second", TextRange::default(), &second);

        store.activate(Some(target(1, "api")));
        assert_eq!(
            store.active_page().map(|page| page.text.as_str()),
            Some("first")
        );
        assert_eq!(
            store.active_page().map(|page| page.selection.location),
            Some(5),
            "切回来光标也要还原"
        );
        let texts: Vec<&str> = store
            .pages()
            .iter()
            .map(|page| page.text.as_str())
            .collect();
        assert_eq!(texts, ["first", "second"]);
    }

    /// 对照 `ComposerDraftsTests.testReactivatingAPaneOnlyRefreshesItsTargetSnapshot`。
    #[test]
    fn reactivating_a_pane_only_refreshes_its_target_snapshot() {
        let mut store = ComposerDraftStore::new();
        let key = store.activate(Some(target(1, "api"))).key.clone();
        store.update("keep", TextRange::default(), &key);

        let refreshed = ComposerTarget {
            kind: ComposerTargetKind::Ssh,
            host: "ryzen".into(),
            workspace_name: "api".into(),
            workspace_id: Some("local:api".into()),
            pane_id: Some(1),
        };
        store.activate(Some(refreshed.clone()));
        assert_eq!(store.pages().len(), 1);
        let page = store.active_page().expect("page");
        assert_eq!(page.text, "keep");
        assert_eq!(page.target.as_ref(), Some(&refreshed));
        assert_eq!(page.title(), "api #1");
    }

    /// 对照 `ComposerDraftsTests.testClearingOnePageLeavesOtherPaneDraftsAlone`。
    #[test]
    fn clearing_one_page_leaves_other_pane_drafts_alone() {
        let mut store = ComposerDraftStore::new();
        let first = store.activate(Some(target(1, "api"))).key.clone();
        store.update("one", TextRange::default(), &first);
        let second = store.activate(Some(target(2, "api"))).key.clone();
        store.update("two", TextRange::default(), &second);

        store.clear(&first);
        assert_eq!(store.page(&first).map(|page| page.text.as_str()), Some(""));
        assert_eq!(
            store.page(&second).map(|page| page.text.as_str()),
            Some("two")
        );
        assert_eq!(store.active_key(), Some(&second), "清空一页不能把焦点挪走");
    }

    /// 对照 `ComposerDraftsTests.testTabTitlesDistinguishPanesInTheSameWorkspace`。
    #[test]
    fn tab_titles_distinguish_panes_in_the_same_workspace() {
        let mut store = ComposerDraftStore::new();
        store.activate(Some(target(1, "api")));
        store.activate(Some(target(2, "another-very-long-workspace-name")));
        let titles: Vec<String> = store.pages().iter().map(|page| page.title()).collect();
        assert_eq!(titles, ["api #1", "another-very #2"]);
    }

    /// 对照 `ComposerDraftsTests.testUnboundPageStaysUsableWithoutAWorkspace`。
    #[test]
    fn unbound_page_stays_usable_without_a_workspace() {
        let mut store = ComposerDraftStore::new();
        let key = store.activate(None).key.clone();
        assert!(!key.is_bound());
        assert_eq!(
            store.active_page().map(|page| page.title()),
            Some("—".to_string())
        );
        store.update("scratch", TextRange::default(), &key);
        assert_eq!(
            store.active_page().map(|page| page.text.as_str()),
            Some("scratch")
        );
    }

    /// 对照 `ComposerDraftsTests.testBlankPagesStayOutOfTheTabStripButKeepTheirContent`。
    #[test]
    fn blank_pages_stay_out_of_the_tab_strip_but_keep_their_content() {
        let mut store = ComposerDraftStore::new();
        let first = store.activate(Some(target(1, "api"))).key.clone();
        assert_eq!(
            store
                .visible_pages()
                .iter()
                .map(|page| page.key.clone())
                .collect::<Vec<_>>(),
            [first.clone()]
        );

        let second = store.activate(Some(target(2, "api"))).key.clone();
        assert_eq!(
            store
                .visible_pages()
                .iter()
                .map(|page| page.key.clone())
                .collect::<Vec<_>>(),
            [second.clone()],
            "两个都还是空的：只剩当前页 visible"
        );

        store.update("typed", TextRange::default(), &second);
        let third = store.activate(Some(target(3, "api"))).key.clone();
        assert_eq!(
            store
                .visible_pages()
                .iter()
                .map(|page| page.key.clone())
                .collect::<Vec<_>>(),
            [second.clone(), third.clone()]
        );
        assert_eq!(store.pages().len(), 3, "隐藏的页面仍然保留内容");

        assert!(store.select(&second));
        assert_eq!(store.active_visible_index(), Some(0));
        store.update("   ", TextRange::default(), &second);
        assert_eq!(
            store
                .visible_pages()
                .iter()
                .map(|page| page.key.clone())
                .collect::<Vec<_>>(),
            [second.clone()],
            "只剩空格等于空草稿"
        );
        assert_eq!(store.page(&third).map(|page| page.text.as_str()), Some(""));
    }

    /// 对照 `ComposerDraftsTests.testWindowSizeIsPortraitAndSlightlyLessThanHalfTheScreen`。
    #[test]
    fn window_size_is_portrait_and_less_than_half_the_screen() {
        for (width, height) in [(1440.0, 900.0), (1920.0, 1080.0), (2560.0, 1440.0)] {
            let (w, h) = window_layout::size(width, height);
            assert!(h > w, "输入框必须是竖长窗口：{w}x{h}");
            assert!(w <= width / 2.0, "{w} 不该超过半屏 {width}");
            assert!(h < height, "{h} 不该占满屏高 {height}");
            assert!(w >= window_layout::MINIMUM_WIDTH);
        }
        assert_eq!(window_layout::size(0.0, -1.0), (560.0, 760.0));
    }
}
