//! Parsing a note into a [`Document`].
//!
//! pulldown-cmark handles CommonMark, GFM tables/strikethrough/footnotes,
//! wikilinks and math. A second pass adds the Obsidian syntax it doesn't know:
//! `==highlights==`, callouts of any type, `%%comments%%`, `^block-ids`,
//! `#tags`, inline footnotes, bare URLs and task statuses other than `x`.

use std::sync::LazyLock;

use pulldown_cmark::{CodeBlockKind, Event, LinkType, Options, Parser, Tag as MdTag};
use regex::Regex;
use smallvec::{SmallVec, smallvec};

use crate::Span;
use crate::frontmatter::{self, Frontmatter};
use crate::link::LinkRef;
use crate::text::{line_end, lines_in, trim_newlines};

pub type Markers = SmallVec<[Span; 2]>;

/// A syntax element. `markers` are the parts of its source that Live Preview
/// hides (e.g. the `**` around bold text).
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub kind: NodeKind,
    pub range: Span,
    pub markers: Markers,
    /// Nesting depth in the syntax tree.
    pub depth: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NodeKind {
    // Blocks
    Heading {
        level: u8,
        setext: bool,
    },
    Paragraph,
    /// A plain block quote. Markers are the `>` prefixes (outermost quote only).
    Quote,
    /// A callout; the index is into [`Document::callouts`].
    Callout(usize),
    CodeBlock {
        fenced: bool,
        lang: Option<String>,
    },
    List {
        ordered: bool,
    },
    /// Markers: the bullet or number, then the task box if any.
    ListItem {
        ordered: bool,
        task: Option<usize>,
    },
    Table,
    TableHead,
    TableRow,
    TableCell,
    Rule,
    HtmlBlock,
    FootnoteDefinition,
    // Inline
    Emphasis,
    Strong,
    Strikethrough,
    Highlight,
    InlineCode,
    Math {
        display: bool,
    },
    /// Index into [`Document::links`].
    Link(usize),
    /// An embed (`![[…]]` or `![](…)`); index into [`Document::links`].
    Embed(usize),
    /// Index into [`Document::tags`].
    Tag(usize),
    FootnoteReference,
    InlineFootnote,
    InlineHtml,
    Comment,
    /// Index into [`Document::block_ids`].
    BlockId(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// `[[target]]`
    Wiki,
    /// `[text](target)`
    Markdown,
    /// `<https://…>`
    Autolink,
    /// A bare `https://…` in text.
    Url,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub range: Span,
    pub kind: LinkKind,
    pub embed: bool,
    pub reference: LinkRef,
    /// The text Live Preview shows for the link.
    pub display_range: Option<Span>,
    pub in_frontmatter: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    /// Without the `#`.
    pub name: String,
    /// Including the `#`.
    pub range: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    pub level: u8,
    pub range: Span,
    pub content: Span,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    /// The whole list item.
    pub item: Span,
    /// The `[x]` box.
    pub marker: Span,
    pub status: char,
}

impl Task {
    pub fn is_done(&self) -> bool {
        self.status != ' '
    }

    /// Byte offset of the status character inside the box.
    pub fn status_offset(&self) -> usize {
        self.marker.start + 1
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fold {
    /// `[!note]+`
    Open,
    /// `[!note]-`
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Callout {
    pub range: Span,
    /// Lower-cased type, e.g. `note`, `warning`, `faq`.
    pub kind: String,
    pub fold: Option<Fold>,
    /// `[!type]` plus any fold sign.
    pub marker: Span,
    pub title: Option<Span>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockId {
    pub id: String,
    /// Including the `^`.
    pub range: Span,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Document {
    pub frontmatter: Option<Frontmatter>,
    /// Sorted by start; an enclosing node comes before the nodes inside it.
    pub nodes: Vec<Node>,
    pub links: Vec<Link>,
    pub tags: Vec<Tag>,
    pub headings: Vec<Heading>,
    pub tasks: Vec<Task>,
    pub callouts: Vec<Callout>,
    pub block_ids: Vec<BlockId>,
    pub comments: Vec<Span>,
}

pub fn parse(text: &str) -> Document {
    let frontmatter = frontmatter::parse(text);
    let base = frontmatter.as_ref().map_or(0, |f| f.range.end);
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_MATH
        | Options::ENABLE_WIKILINKS;

    let mut b = Builder {
        text,
        doc: Document::default(),
        stack: Vec::new(),
        excluded: Vec::new(),
        link_depth: 0,
        raw_depth: 0,
        quote_depth: 0,
    };
    for (event, range) in Parser::new_ext(&text[base..], options).into_offset_iter() {
        b.event(event, range.start + base..range.end + base);
    }
    b.scan_comments(base);
    b.scan_block_ids(base);
    if let Some(fm) = &frontmatter {
        b.scan_frontmatter_links(fm);
    }

    let mut doc = b.doc;
    doc.frontmatter = frontmatter;
    doc.nodes.sort_by(|a, b| {
        a.range
            .start
            .cmp(&b.range.start)
            .then(b.range.end.cmp(&a.range.end))
    });
    doc
}

struct Open {
    node: Option<usize>,
    kind: OpenKind,
    range: Span,
    last_child_end: Option<usize>,
    container: Option<Vec<Run>>,
}

#[derive(Clone, Copy)]
enum OpenKind {
    Other,
    /// Code or HTML block: its text isn't Markdown.
    Raw,
    Quote,
    /// A Markdown link whose markers depend on where its text ends.
    MarkdownLink {
        link: usize,
    },
    /// Any other link or embed.
    Link,
}

/// Contiguous text inside an inline container.
#[derive(Clone)]
struct Run {
    range: Span,
    in_link: bool,
}

struct Builder<'t> {
    text: &'t str,
    doc: Document,
    stack: Vec<Open>,
    /// Code, math and HTML: not scanned for comments, tags or block IDs.
    excluded: Vec<Span>,
    link_depth: usize,
    raw_depth: usize,
    quote_depth: usize,
}

impl Builder<'_> {
    fn event(&mut self, event: Event<'_>, range: Span) {
        match event {
            Event::Start(tag) => self.start(tag, range),
            Event::End(_) => self.end(range),
            Event::Text(_) => self.text_event(range),
            Event::Code(_) => {
                let markers = delimiter_markers(self.text, &range, b'`');
                self.leaf(NodeKind::InlineCode, range.clone(), markers);
                self.excluded.push(range);
            }
            Event::InlineMath(_) | Event::DisplayMath(_) => {
                let display = matches!(event, Event::DisplayMath(_));
                let markers = delimiter_markers(self.text, &range, b'$');
                self.leaf(NodeKind::Math { display }, range.clone(), markers);
                self.excluded.push(range);
            }
            Event::InlineHtml(_) => {
                self.leaf(NodeKind::InlineHtml, range.clone(), smallvec![]);
                self.excluded.push(range);
            }
            Event::FootnoteReference(_) => {
                self.leaf(NodeKind::FootnoteReference, range, smallvec![]);
            }
            Event::Rule => {
                let line = trim_newlines(self.text, &range);
                self.leaf(NodeKind::Rule, range, smallvec![line]);
            }
            Event::Html(_) | Event::SoftBreak | Event::HardBreak | Event::TaskListMarker(_) => {
                self.touch(&range)
            }
        }
    }

    fn push_node(&mut self, kind: NodeKind, range: Span, markers: Markers) -> usize {
        self.doc.nodes.push(Node {
            kind,
            range,
            markers,
            depth: self.stack.len() as u16,
        });
        self.doc.nodes.len() - 1
    }

    fn leaf(&mut self, kind: NodeKind, range: Span, markers: Markers) {
        self.touch(&range);
        if self.raw_depth == 0 {
            self.push_node(kind, range, markers);
        }
    }

    fn touch(&mut self, range: &Span) {
        if let Some(top) = self.stack.last_mut() {
            top.last_child_end = Some(range.end);
        }
    }

    fn start(&mut self, tag: MdTag<'_>, range: Span) {
        let text = self.text;
        let mut open = Open {
            node: None,
            kind: OpenKind::Other,
            range: range.clone(),
            last_child_end: None,
            container: None,
        };
        match tag {
            MdTag::Paragraph => {
                open.node = Some(self.push_node(NodeKind::Paragraph, range, smallvec![]));
                open.container = Some(Vec::new());
            }
            MdTag::Heading { level, .. } => {
                let (setext, markers, content) = heading_parts(text, &range);
                let level = level as u8;
                self.doc.headings.push(Heading {
                    level,
                    range: trim_newlines(text, &range),
                    text: text[content.clone()].trim().to_owned(),
                    content,
                });
                open.node =
                    Some(self.push_node(NodeKind::Heading { level, setext }, range, markers));
                open.container = Some(Vec::new());
            }
            MdTag::BlockQuote(_) => {
                let markers = if self.quote_depth == 0 {
                    quote_prefixes(text, &range)
                } else {
                    smallvec![]
                };
                let kind = match callout_header(text, &range) {
                    Some(mut callout) => {
                        callout.range = trim_newlines(text, &range);
                        self.doc.callouts.push(callout);
                        NodeKind::Callout(self.doc.callouts.len() - 1)
                    }
                    None => NodeKind::Quote,
                };
                open.node = Some(self.push_node(kind, range, markers));
                open.kind = OpenKind::Quote;
                self.quote_depth += 1;
            }
            MdTag::CodeBlock(kind) => {
                let (fenced, lang) = match &kind {
                    CodeBlockKind::Fenced(info) => {
                        (true, info.split_whitespace().next().map(str::to_owned))
                    }
                    CodeBlockKind::Indented => (false, None),
                };
                let markers = if fenced {
                    fence_markers(text, &range)
                } else {
                    smallvec![]
                };
                self.excluded.push(range.clone());
                open.node =
                    Some(self.push_node(NodeKind::CodeBlock { fenced, lang }, range, markers));
                open.kind = OpenKind::Raw;
                self.raw_depth += 1;
            }
            MdTag::HtmlBlock => {
                self.excluded.push(range.clone());
                open.node = Some(self.push_node(NodeKind::HtmlBlock, range, smallvec![]));
                open.kind = OpenKind::Raw;
                self.raw_depth += 1;
            }
            MdTag::List(first) => {
                let kind = NodeKind::List {
                    ordered: first.is_some(),
                };
                open.node = Some(self.push_node(kind, range, smallvec![]));
            }
            MdTag::Item => {
                let (ordered, markers, task) = item_parts(text, &range);
                let task = task.map(|(marker, status)| {
                    self.doc.tasks.push(Task {
                        item: trim_newlines(text, &range),
                        marker,
                        status,
                    });
                    self.doc.tasks.len() - 1
                });
                open.node =
                    Some(self.push_node(NodeKind::ListItem { ordered, task }, range, markers));
                open.container = Some(Vec::new());
            }
            MdTag::FootnoteDefinition(_) => {
                let markers = text[range.clone()]
                    .find("]:")
                    .map(|i| smallvec![range.start..range.start + i + 2])
                    .unwrap_or_default();
                open.node = Some(self.push_node(NodeKind::FootnoteDefinition, range, markers));
            }
            MdTag::Table(_) => {
                open.node = Some(self.push_node(NodeKind::Table, range, smallvec![]))
            }
            MdTag::TableHead => {
                open.node = Some(self.push_node(NodeKind::TableHead, range, smallvec![]))
            }
            MdTag::TableRow => {
                open.node = Some(self.push_node(NodeKind::TableRow, range, smallvec![]))
            }
            MdTag::TableCell => {
                open.node = Some(self.push_node(NodeKind::TableCell, range, smallvec![]));
                open.container = Some(Vec::new());
            }
            MdTag::Emphasis => {
                let markers = symmetric_markers(&range, 1);
                open.node = Some(self.push_node(NodeKind::Emphasis, range, markers));
            }
            MdTag::Strong => {
                let markers = symmetric_markers(&range, 2);
                open.node = Some(self.push_node(NodeKind::Strong, range, markers));
            }
            MdTag::Strikethrough => {
                // Obsidian only treats `~~` as strikethrough.
                if text[range.clone()].starts_with("~~") {
                    let markers = symmetric_markers(&range, 2);
                    open.node = Some(self.push_node(NodeKind::Strikethrough, range, markers));
                }
            }
            MdTag::Link {
                link_type,
                dest_url,
                ..
            } => {
                self.link_depth += 1;
                let (link, markers, markdown) =
                    link_parts(text, &range, link_type, &dest_url, false);
                self.doc.links.push(link);
                let index = self.doc.links.len() - 1;
                open.node = Some(self.push_node(NodeKind::Link(index), range, markers));
                open.kind = if markdown {
                    OpenKind::MarkdownLink { link: index }
                } else {
                    OpenKind::Link
                };
            }
            MdTag::Image {
                link_type,
                dest_url,
                ..
            } => {
                self.link_depth += 1;
                let (link, _, _) = link_parts(text, &range, link_type, &dest_url, true);
                self.doc.links.push(link);
                let index = self.doc.links.len() - 1;
                let markers = smallvec![trim_newlines(text, &range)];
                open.node = Some(self.push_node(NodeKind::Embed(index), range, markers));
                open.kind = OpenKind::Link;
            }
            MdTag::MetadataBlock(_)
            | MdTag::DefinitionList
            | MdTag::DefinitionListTitle
            | MdTag::DefinitionListDefinition
            | MdTag::Superscript
            | MdTag::Subscript => {}
        }
        self.stack.push(open);
    }

    fn end(&mut self, range: Span) {
        let Some(open) = self.stack.pop() else {
            return;
        };
        match open.kind {
            OpenKind::Raw => self.raw_depth -= 1,
            OpenKind::Quote => self.quote_depth -= 1,
            OpenKind::Link => self.link_depth -= 1,
            OpenKind::MarkdownLink { link } => {
                self.link_depth -= 1;
                let start = open.range.start;
                let text_end = open.last_child_end.unwrap_or(start + 1).max(start + 1);
                let display = start + 1..text_end;
                let link = &mut self.doc.links[link];
                link.reference.display = Some(self.text[display.clone()].to_owned());
                link.display_range = Some(display);
                if let Some(node) = open.node {
                    self.doc.nodes[node].markers =
                        smallvec![start..start + 1, text_end..open.range.end];
                }
            }
            OpenKind::Other => {}
        }
        if let Some(runs) = open.container {
            self.scan_container(&runs, &open.range);
        }
        self.touch(&range);
    }

    fn text_event(&mut self, range: Span) {
        self.touch(&range);
        if self.raw_depth > 0 {
            return;
        }
        let in_link = self.link_depth > 0;
        let Some(runs) = self
            .stack
            .iter_mut()
            .rev()
            .find_map(|o| o.container.as_mut())
        else {
            return;
        };
        match runs.last_mut() {
            Some(last) if last.range.end == range.start && last.in_link == in_link => {
                last.range.end = range.end;
            }
            _ => runs.push(Run { range, in_link }),
        }
    }

    // --- Obsidian syntax inside a paragraph, heading, list item or cell -----

    fn scan_container(&mut self, runs: &[Run], container: &Span) {
        self.scan_highlights(runs);
        for run in runs.iter().filter(|r| !r.in_link) {
            self.scan_tags(&run.range);
            self.scan_urls(&run.range);
            self.scan_inline_footnotes(&run.range, container);
        }
    }

    fn scan_highlights(&mut self, runs: &[Run]) {
        let text = self.text;
        let bytes = text.as_bytes();
        let mut delims = Vec::new();
        for run in runs {
            let mut pos = run.range.start;
            while let Some(i) = text[pos..run.range.end].find("==") {
                delims.push(pos + i);
                pos += i + 2;
            }
        }
        let can_open = |d: usize| {
            bytes
                .get(d + 2)
                .is_some_and(|&c| !c.is_ascii_whitespace() && c != b'=')
        };
        let can_close =
            |d: usize| d > 0 && !bytes[d - 1].is_ascii_whitespace() && bytes[d - 1] != b'=';
        let mut opener: Option<usize> = None;
        for d in delims {
            match opener {
                Some(o) if d > o + 2 && can_close(d) => {
                    self.push_node(NodeKind::Highlight, o..d + 2, smallvec![o..o + 2, d..d + 2]);
                    opener = None;
                }
                _ if can_open(d) => opener = Some(d),
                _ => {}
            }
        }
    }

    fn scan_tags(&mut self, run: &Span) {
        let text = self.text;
        let mut pos = run.start;
        while let Some(i) = text[pos..run.end].find('#') {
            let hash = pos + i;
            pos = hash + 1;
            let boundary = text[..hash]
                .chars()
                .next_back()
                .is_none_or(char::is_whitespace);
            if !boundary {
                continue;
            }
            let mut end = hash + 1;
            for (j, c) in text[hash + 1..run.end].char_indices() {
                if !is_tag_char(c) {
                    break;
                }
                end = hash + 1 + j + c.len_utf8();
            }
            while end > hash + 1 && text.as_bytes()[end - 1] == b'/' {
                end -= 1;
            }
            let name = &text[hash + 1..end];
            if name.is_empty() || name.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            self.doc.tags.push(Tag {
                name: name.to_owned(),
                range: hash..end,
            });
            let index = self.doc.tags.len() - 1;
            self.push_node(NodeKind::Tag(index), hash..end, smallvec![]);
            pos = end;
        }
    }

    fn scan_urls(&mut self, run: &Span) {
        static URL: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r#"(?:https?|ftp)://[^\s<>"`]+"#).unwrap());
        let slice = &self.text[run.clone()];
        for m in URL.find_iter(slice) {
            let mut url = m.as_str();
            loop {
                let trimmed = url.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'', '*', '_']);
                let trimmed = if trimmed.ends_with(')')
                    && trimmed.matches('(').count() < trimmed.matches(')').count()
                {
                    &trimmed[..trimmed.len() - 1]
                } else {
                    trimmed
                };
                if trimmed.len() == url.len() {
                    break;
                }
                url = trimmed;
            }
            let start = run.start + m.start();
            let range = start..start + url.len();
            self.doc.links.push(Link {
                range: range.clone(),
                kind: LinkKind::Url,
                embed: false,
                reference: LinkRef {
                    target: url.to_owned(),
                    ..LinkRef::default()
                },
                display_range: Some(range.clone()),
                in_frontmatter: false,
            });
            let index = self.doc.links.len() - 1;
            self.push_node(NodeKind::Link(index), range, smallvec![]);
        }
    }

    fn scan_inline_footnotes(&mut self, run: &Span, container: &Span) {
        let text = self.text;
        let mut pos = run.start;
        while pos < run.end
            && let Some(i) = text[pos..run.end].find("^[")
        {
            let open = pos + i;
            pos = open + 2;
            let mut depth = 1;
            let mut close = None;
            for (j, c) in text[open + 2..container.end].char_indices() {
                match c {
                    '[' => depth += 1,
                    ']' => {
                        depth -= 1;
                        if depth == 0 {
                            close = Some(open + 2 + j);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if let Some(close) = close {
                self.push_node(
                    NodeKind::InlineFootnote,
                    open..close + 1,
                    smallvec![open..open + 2, close..close + 1],
                );
                pos = close + 1;
            }
        }
    }

    // --- whole-note scans ----------------------------------------------------

    fn is_excluded(&self, pos: usize) -> bool {
        self.excluded.iter().any(|r| r.contains(&pos))
    }

    fn scan_comments(&mut self, base: usize) {
        let text = self.text;
        let mut opener: Option<usize> = None;
        let mut pos = base;
        while let Some(i) = text[pos..].find("%%") {
            let at = pos + i;
            pos = at + 2;
            if self.is_excluded(at) {
                continue;
            }
            match opener.take() {
                None => opener = Some(at),
                Some(start) => self.push_comment(start..at + 2, true),
            }
        }
        if let Some(start) = opener {
            // Obsidian hides everything after an unclosed `%%`.
            self.push_comment(start..text.len(), false);
        }
    }

    fn push_comment(&mut self, range: Span, closed: bool) {
        let mut markers: Markers = smallvec![range.start..range.start + 2];
        if closed {
            markers.push(range.end - 2..range.end);
        }
        self.doc.comments.push(range.clone());
        self.push_node(NodeKind::Comment, range, markers);
    }

    fn scan_block_ids(&mut self, base: usize) {
        static BLOCK_ID: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(?:^|[ \t])(\^[A-Za-z0-9-]+)[ \t]*$").unwrap());
        let text = self.text;
        for line in lines_in(text, &(base..text.len())) {
            let Some(m) = BLOCK_ID
                .captures(&text[line.clone()])
                .and_then(|c| c.get(1))
            else {
                continue;
            };
            let range = line.start + m.start()..line.start + m.end();
            if self.is_excluded(range.start) {
                continue;
            }
            self.doc.block_ids.push(BlockId {
                id: text[range.start + 1..range.end].to_owned(),
                range: range.clone(),
            });
            let index = self.doc.block_ids.len() - 1;
            self.push_node(NodeKind::BlockId(index), range, smallvec![]);
        }
    }

    fn scan_frontmatter_links(&mut self, fm: &Frontmatter) {
        static WIKILINK: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"\[\[([^\[\]\n]+?)\]\]").unwrap());
        for caps in WIKILINK.captures_iter(&self.text[fm.body.clone()]) {
            let whole = caps.get(0).unwrap();
            let inner = caps.get(1).unwrap();
            let display = inner
                .as_str()
                .find('|')
                .map(|i| fm.body.start + inner.start() + i + 1..fm.body.start + inner.end());
            self.doc.links.push(Link {
                range: fm.body.start + whole.start()..fm.body.start + whole.end(),
                kind: LinkKind::Wiki,
                embed: false,
                reference: LinkRef::parse_wiki(inner.as_str(), false),
                display_range: display,
                in_frontmatter: true,
            });
        }
    }
}

// --- helpers -------------------------------------------------------------------

fn is_tag_char(c: char) -> bool {
    c.is_alphanumeric()
        || matches!(c, '_' | '-' | '/')
        || (!c.is_ascii() && !c.is_whitespace() && !"，。、；：？！…“”‘’«»–—（）【】".contains(c))
}

fn symmetric_markers(range: &Span, n: usize) -> Markers {
    if range.len() >= 2 * n {
        smallvec![range.start..range.start + n, range.end - n..range.end]
    } else {
        smallvec![]
    }
}

/// Markers for runs of `delim` at both ends (backticks, dollars).
fn delimiter_markers(text: &str, range: &Span, delim: u8) -> Markers {
    let bytes = &text.as_bytes()[range.clone()];
    let open = bytes.iter().take_while(|&&b| b == delim).count();
    let close = bytes.iter().rev().take_while(|&&b| b == delim).count();
    if open == 0 || close == 0 || open + close > bytes.len() {
        return smallvec![];
    }
    smallvec![
        range.start..range.start + open,
        range.end - close..range.end
    ]
}

/// `(setext, markers, content)` for a heading.
fn heading_parts(text: &str, range: &Span) -> (bool, Markers, Span) {
    static ATX_OPEN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^ {0,3}#{1,6}(?:[ \t]+|$)").unwrap());
    static ATX_CLOSE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t]+#+[ \t]*$").unwrap());
    let range = trim_newlines(text, range);
    let first_line = range.start..line_end(text, range.start).min(range.end);
    if let Some(open) = ATX_OPEN.find(&text[first_line.clone()]) {
        let open = range.start..range.start + open.end();
        let mut markers: Markers = smallvec![open.clone()];
        let mut content_end = first_line.end;
        if let Some(close) = ATX_CLOSE.find(&text[open.end..first_line.end]) {
            let close = open.end + close.start()..first_line.end;
            content_end = close.start;
            markers.push(close);
        }
        return (false, markers, open.end..content_end.max(open.end));
    }
    // Setext: the underline is the last line.
    let lines = lines_in(text, &range);
    let underline = lines.last().cloned().unwrap_or(range.clone());
    let content_end = if lines.len() > 1 {
        lines[lines.len() - 2].end
    } else {
        range.start
    };
    (true, smallvec![underline], range.start..content_end)
}

/// The `>` prefixes on each line of a quote.
fn quote_prefixes(text: &str, range: &Span) -> Markers {
    let range = trim_newlines(text, range);
    let mut markers = Markers::new();
    for line in lines_in(text, &range) {
        let bytes = &text.as_bytes()[line.clone()];
        let mut i = bytes
            .iter()
            .take_while(|b| matches!(b, b' ' | b'\t'))
            .count();
        let start = i;
        while bytes.get(i) == Some(&b'>') {
            i += 1;
            if matches!(bytes.get(i), Some(b' ' | b'\t')) {
                i += 1;
            }
        }
        if i > start {
            markers.push(line.start + start..line.start + i);
        }
    }
    markers
}

/// A callout header on the first line of a quote: `> [!type]+ Title`.
fn callout_header(text: &str, range: &Span) -> Option<Callout> {
    let bytes = text.as_bytes();
    let line_end = line_end(text, range.start);
    let mut p = range.start;
    while p < line_end && matches!(bytes[p], b' ' | b'\t') {
        p += 1;
    }
    if bytes.get(p) != Some(&b'>') {
        return None;
    }
    p += 1;
    if matches!(bytes.get(p), Some(b' ' | b'\t')) {
        p += 1;
    }
    if !text[p..line_end].starts_with("[!") {
        return None;
    }
    let close = p + text[p..line_end].find(']')?;
    let kind = &text[p + 2..close];
    if kind.is_empty() || kind.contains(char::is_whitespace) {
        return None;
    }
    let (fold, marker_end) = match bytes.get(close + 1) {
        Some(b'+') => (Some(Fold::Open), close + 2),
        Some(b'-') => (Some(Fold::Closed), close + 2),
        _ => (None, close + 1),
    };
    let title_text = &text[marker_end..line_end];
    let title = (!title_text.trim().is_empty()).then(|| {
        let lead = title_text.len() - title_text.trim_start().len();
        let trail = title_text.len() - title_text.trim_end().len();
        marker_end + lead..line_end - trail
    });
    Some(Callout {
        range: range.clone(),
        kind: kind.to_lowercase(),
        fold,
        marker: p..marker_end,
        title,
    })
}

/// Opening and closing fence lines of a fenced code block.
fn fence_markers(text: &str, range: &Span) -> Markers {
    let range = trim_newlines(text, range);
    let lines = lines_in(text, &range);
    let mut markers: Markers = smallvec![lines[0].clone()];
    if lines.len() > 1 {
        let last = lines.last().unwrap();
        let line = &text[last.clone()];
        if let Some(i) = line.find(['`', '~']) {
            let fence = line[i..].trim_end();
            let ch = fence.as_bytes()[0];
            if fence.len() >= 3 && fence.bytes().all(|b| b == ch) {
                markers.push(last.start + i..last.end);
            }
        }
    }
    markers
}

/// `(ordered, markers, task)` for a list item. Markers are the bullet or
/// number, then the task box; task is `(box, status)`.
fn item_parts(text: &str, range: &Span) -> (bool, Markers, Option<(Span, char)>) {
    let line_end = line_end(text, range.start);
    let line = &text[range.start..line_end];
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    let rest = &line[indent..];
    let bullet_len = if rest.starts_with(['-', '*', '+']) {
        1
    } else {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && matches!(rest.as_bytes().get(digits), Some(b'.' | b')')) {
            digits + 1
        } else {
            0
        }
    };
    if bullet_len == 0 {
        return (false, smallvec![], None);
    }
    let ordered = !rest.starts_with(['-', '*', '+']);
    let bullet = range.start + indent..range.start + indent + bullet_len;
    let mut markers: Markers = smallvec![bullet.clone()];

    let after = &text[bullet.end..line_end];
    let gap = after.len() - after.trim_start_matches([' ', '\t']).len();
    let task = (gap > 0)
        .then(|| {
            let box_start = bullet.end + gap;
            let rest = &text[box_start..line_end];
            let mut chars = rest.chars();
            if chars.next() != Some('[') {
                return None;
            }
            let status = chars.next()?;
            if status == '\n' || chars.next() != Some(']') {
                return None;
            }
            let box_end = box_start + 2 + status.len_utf8();
            match text.as_bytes().get(box_end) {
                None | Some(b' ' | b'\t' | b'\n') => Some((box_start..box_end, status)),
                _ => None,
            }
        })
        .flatten();
    if let Some((marker, _)) = &task {
        markers.push(marker.clone());
    }
    (ordered, markers, task)
}

/// `(link, markers, is_markdown_link)`. Markdown link markers are finished
/// when the link ends.
fn link_parts(
    text: &str,
    range: &Span,
    link_type: LinkType,
    dest: &str,
    embed: bool,
) -> (Link, Markers, bool) {
    let whole = trim_newlines(text, range);
    let base = Link {
        range: whole.clone(),
        kind: LinkKind::Markdown,
        embed,
        reference: LinkRef::default(),
        display_range: None,
        in_frontmatter: false,
    };
    match link_type {
        LinkType::WikiLink { .. } => {
            let open = if embed { 3 } else { 2 };
            let inner = whole.start + open..whole.end.saturating_sub(2).max(whole.start + open);
            let inner_text = &text[inner.clone()];
            let (display, markers): (Span, Markers) = match inner_text.find('|') {
                Some(pipe) => {
                    let pipe = inner.start + pipe;
                    (
                        pipe + 1..inner.end,
                        smallvec![whole.start..pipe + 1, inner.end..whole.end],
                    )
                }
                None => (
                    inner.clone(),
                    smallvec![whole.start..inner.start, inner.end..whole.end],
                ),
            };
            let link = Link {
                kind: LinkKind::Wiki,
                reference: LinkRef::parse_wiki(inner_text, embed),
                display_range: (!embed).then_some(display),
                ..base
            };
            (link, markers, false)
        }
        LinkType::Autolink | LinkType::Email => {
            let display = whole.start + 1..whole.end - 1;
            let link = Link {
                kind: LinkKind::Autolink,
                reference: LinkRef {
                    target: dest.to_owned(),
                    ..LinkRef::default()
                },
                display_range: Some(display),
                ..base
            };
            (
                link,
                smallvec![whole.start..whole.start + 1, whole.end - 1..whole.end],
                false,
            )
        }
        _ => {
            let link = Link {
                reference: LinkRef::parse_markdown(dest),
                ..base
            };
            (link, smallvec![], !embed)
        }
    }
}

#[cfg(test)]
mod tests;
