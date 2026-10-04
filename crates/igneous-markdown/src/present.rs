//! The Live Preview presentation model.
//!
//! [`spans`] says, for each element of a note: how to style it, which syntax
//! markers to hide, when to show them again, and whether a widget replaces or
//! decorates it. It knows nothing about GTK; the editor maps it onto text tags
//! and overlays.
//!
//! Reveal rule: an element's markers are shown while the cursor or selection
//! touches its reveal range (ends inclusive), as in Obsidian.

use smallvec::{SmallVec, smallvec};

use crate::Span;
use crate::parse::{Document, LinkKind, NodeKind};
use crate::text::{line_end, line_span, trim_newlines};

pub type Markers = SmallVec<[Span; 4]>;

#[derive(Debug, Clone, PartialEq)]
pub struct StyledSpan {
    pub range: Span,
    pub style: Style,
    /// Syntax hidden in Live Preview unless revealed.
    pub markers: Markers,
    pub reveal: Reveal,
    pub replace: Option<Replacement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reveal {
    /// Markers are always hidden in Live Preview.
    Never,
    /// Markers show while the cursor or selection touches this range.
    Within(Span),
}

impl Reveal {
    /// Whether a cursor/selection spanning `selection` reveals the markers.
    pub fn is_revealed_by(&self, selection: &Span) -> bool {
        match self {
            Reveal::Never => false,
            Reveal::Within(range) => selection.start <= range.end && selection.end >= range.start,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Style {
    Heading(u8),
    Emphasis,
    Strong,
    Strikethrough,
    Highlight,
    InlineCode,
    CodeBlock { lang: Option<String> },
    Quote,
    QuoteMarker,
    Callout { kind: String },
    CalloutTitle,
    ListBullet,
    ListNumber,
    Task { done: bool },
    TaskDoneText,
    Link,
    WikiLink,
    Url,
    Embed,
    Tag,
    Math { display: bool },
    Comment,
    FootnoteReference,
    InlineFootnote,
    FootnoteDefinition,
    BlockId,
    Rule,
    Table,
    Html,
    Frontmatter,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Replacement {
    /// Draw a bullet in place of `-`, `*` or `+`.
    Bullet,
    /// Draw a checkbox in place of `- [ ]`. Toggling edits the byte at
    /// `status_offset`.
    Checkbox { checked: bool, status_offset: usize },
    /// Show an image below the line.
    Image {
        target: String,
        size: Option<(u32, Option<u32>)>,
    },
    /// Show another note (or part of one) below the line.
    NoteEmbed { target: String },
    /// Show the properties editor in place of the frontmatter.
    Properties,
    /// Draw a horizontal line.
    Rule,
    /// Draw the callout's icon (and its type as the title when it has none).
    CalloutHeader { kind: String, has_title: bool },
    /// Show the table as a grid while the cursor is outside it.
    Table,
    /// Show display math rendered while the cursor is outside it.
    Math,
    /// A ` ```base ` block: show the base's results while the cursor is
    /// outside it (if the editor can).
    Base,
}

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "svg", "webp", "avif"];

pub fn is_image_target(target: &str) -> bool {
    target
        .rsplit_once('.')
        .is_some_and(|(_, ext)| IMAGE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

pub fn spans(doc: &Document, text: &str) -> Vec<StyledSpan> {
    let mut out = Vec::with_capacity(doc.nodes.len() + 1);
    let mut push = |range: Span, style: Style, markers: Markers, reveal: Reveal, replace| {
        out.push(StyledSpan {
            range,
            style,
            markers,
            reveal,
            replace,
        });
    };

    if let Some(fm) = &doc.frontmatter {
        push(
            fm.range.clone(),
            Style::Frontmatter,
            smallvec![fm.range.clone()],
            Reveal::Never,
            Some(Replacement::Properties),
        );
    }

    for node in &doc.nodes {
        let range = trim_newlines(text, &node.range);
        let markers: Markers = node.markers.iter().cloned().collect();
        let within = Reveal::Within(range.clone());
        match &node.kind {
            NodeKind::Heading { level, .. } => {
                let lines = line_span(text, &range);
                push(
                    range,
                    Style::Heading(*level),
                    markers,
                    Reveal::Within(lines),
                    None,
                );
            }
            NodeKind::Quote => {
                push(range, Style::Quote, smallvec![], Reveal::Never, None);
                for marker in &node.markers {
                    let line = line_span(text, marker);
                    push(
                        marker.clone(),
                        Style::QuoteMarker,
                        smallvec![marker.clone()],
                        Reveal::Within(line),
                        None,
                    );
                }
            }
            NodeKind::Callout(index) => {
                let callout = &doc.callouts[*index];
                let mut markers = markers;
                markers.push(callout.marker.clone());
                push(
                    range,
                    Style::Callout {
                        kind: callout.kind.clone(),
                    },
                    markers,
                    within,
                    Some(Replacement::CalloutHeader {
                        kind: callout.kind.clone(),
                        has_title: callout.title.is_some(),
                    }),
                );
                if let Some(title) = &callout.title {
                    push(
                        title.clone(),
                        Style::CalloutTitle,
                        smallvec![],
                        Reveal::Never,
                        None,
                    );
                }
            }
            NodeKind::CodeBlock { lang, .. } => {
                let base = lang
                    .as_deref()
                    .is_some_and(|l| l.eq_ignore_ascii_case("base"));
                push(
                    range,
                    Style::CodeBlock { lang: lang.clone() },
                    markers,
                    within,
                    base.then_some(Replacement::Base),
                );
            }
            NodeKind::ListItem { ordered, task } => {
                let Some(bullet) = node.markers.first().cloned() else {
                    continue;
                };
                match task {
                    Some(task) => {
                        let task = &doc.tasks[*task];
                        let span = bullet.start..task.marker.end;
                        push(
                            span.clone(),
                            Style::Task {
                                done: task.is_done(),
                            },
                            smallvec![bullet, task.marker.clone()],
                            Reveal::Within(span),
                            Some(Replacement::Checkbox {
                                checked: task.is_done(),
                                status_offset: task.status_offset(),
                            }),
                        );
                        if task.is_done() {
                            let end = line_end(text, task.marker.end);
                            let content = text[task.marker.end..end].trim_start();
                            let start = end - content.len();
                            if start < end {
                                push(
                                    start..end,
                                    Style::TaskDoneText,
                                    smallvec![],
                                    Reveal::Never,
                                    None,
                                );
                            }
                        }
                    }
                    None if !ordered => push(
                        bullet.clone(),
                        Style::ListBullet,
                        smallvec![bullet.clone()],
                        Reveal::Within(bullet),
                        Some(Replacement::Bullet),
                    ),
                    None => push(bullet, Style::ListNumber, smallvec![], Reveal::Never, None),
                }
            }
            NodeKind::Table => {
                let lines = line_span(text, &range);
                push(
                    range.clone(),
                    Style::Table,
                    smallvec![range],
                    Reveal::Within(lines),
                    Some(Replacement::Table),
                );
            }
            NodeKind::Rule => {
                let line = line_span(text, &range);
                push(
                    range,
                    Style::Rule,
                    markers,
                    Reveal::Within(line),
                    Some(Replacement::Rule),
                );
            }
            NodeKind::HtmlBlock | NodeKind::InlineHtml => {
                push(range, Style::Html, smallvec![], Reveal::Never, None)
            }
            NodeKind::FootnoteDefinition => {
                if let Some(label) = node.markers.first() {
                    push(
                        label.clone(),
                        Style::FootnoteDefinition,
                        smallvec![],
                        Reveal::Never,
                        None,
                    );
                }
            }
            NodeKind::Emphasis => push(range, Style::Emphasis, markers, within, None),
            NodeKind::Strong => push(range, Style::Strong, markers, within, None),
            NodeKind::Strikethrough => push(range, Style::Strikethrough, markers, within, None),
            NodeKind::Highlight => push(range, Style::Highlight, markers, within, None),
            NodeKind::InlineCode => push(range, Style::InlineCode, markers, within, None),
            NodeKind::Math { display: true } => {
                // Display math is replaced by the rendered formula.
                let lines = line_span(text, &range);
                push(
                    range.clone(),
                    Style::Math { display: true },
                    smallvec![range],
                    Reveal::Within(lines),
                    Some(Replacement::Math),
                );
            }
            NodeKind::Math { display: false } => {
                push(range, Style::Math { display: false }, markers, within, None)
            }
            NodeKind::Link(index) => {
                let style = match doc.links[*index].kind {
                    LinkKind::Wiki => Style::WikiLink,
                    LinkKind::Url => Style::Url,
                    LinkKind::Markdown | LinkKind::Autolink => Style::Link,
                };
                push(range, style, markers, within, None);
            }
            NodeKind::Embed(index) => {
                let link = &doc.links[*index];
                let target = link.reference.target.clone();
                let replace = if is_image_target(&target) {
                    Replacement::Image {
                        target,
                        size: link.reference.size,
                    }
                } else {
                    Replacement::NoteEmbed { target }
                };
                let line = line_span(text, &range);
                push(
                    range,
                    Style::Embed,
                    markers,
                    Reveal::Within(line),
                    Some(replace),
                );
            }
            NodeKind::Tag(_) => push(range, Style::Tag, smallvec![], Reveal::Never, None),
            NodeKind::FootnoteReference => push(
                range,
                Style::FootnoteReference,
                smallvec![],
                Reveal::Never,
                None,
            ),
            NodeKind::InlineFootnote => push(range, Style::InlineFootnote, markers, within, None),
            NodeKind::Comment => push(range, Style::Comment, smallvec![], Reveal::Never, None),
            NodeKind::BlockId(_) => push(range, Style::BlockId, smallvec![], Reveal::Never, None),
            NodeKind::Paragraph
            | NodeKind::List { .. }
            | NodeKind::TableHead
            | NodeKind::TableRow
            | NodeKind::TableCell => {}
        }
    }
    out.sort_by(|a, b| {
        a.range
            .start
            .cmp(&b.range.start)
            .then(b.range.end.cmp(&a.range.end))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    fn find<'a>(spans: &'a [StyledSpan], style: &Style) -> &'a StyledSpan {
        spans
            .iter()
            .find(|s| &s.style == style)
            .unwrap_or_else(|| panic!("no {style:?} in {spans:#?}"))
    }

    fn markers(text: &str, span: &StyledSpan) -> Vec<String> {
        span.markers
            .iter()
            .map(|m| text[m.clone()].to_owned())
            .collect()
    }

    #[test]
    fn inline_reveal_is_per_element() {
        let text = "aa **bold** cc";
        let spans = spans(&parse(text), text);
        let bold = find(&spans, &Style::Strong);
        assert_eq!(markers(text, bold), ["**", "**"]);
        // Touching either end reveals; elsewhere doesn't.
        assert!(bold.reveal.is_revealed_by(&(3..3)));
        assert!(bold.reveal.is_revealed_by(&(11..11)));
        assert!(!bold.reveal.is_revealed_by(&(1..1)));
        assert!(!bold.reveal.is_revealed_by(&(13..13)));
        // A selection across it reveals.
        assert!(bold.reveal.is_revealed_by(&(0..14)));
    }

    #[test]
    fn heading_reveal_is_per_line() {
        let text = "## Title here\nnext";
        let spans = spans(&parse(text), text);
        let heading = find(&spans, &Style::Heading(2));
        assert_eq!(markers(text, heading), ["## "]);
        assert!(heading.reveal.is_revealed_by(&(10..10)));
        assert!(!heading.reveal.is_revealed_by(&(15..15)));
    }

    #[test]
    fn tasks_and_bullets() {
        let text = "- [x] done thing\n- plain\n1. first\n";
        let spans = spans(&parse(text), text);
        let task = find(&spans, &Style::Task { done: true });
        assert_eq!(markers(text, task), ["-", "[x]"]);
        assert_eq!(
            task.replace,
            Some(Replacement::Checkbox {
                checked: true,
                status_offset: 3
            })
        );
        assert_eq!(
            &text[find(&spans, &Style::TaskDoneText).range.clone()],
            "done thing"
        );
        let bullet = find(&spans, &Style::ListBullet);
        assert_eq!(&text[bullet.range.clone()], "-");
        assert_eq!(bullet.replace, Some(Replacement::Bullet));
        let number = find(&spans, &Style::ListNumber);
        assert!(number.markers.is_empty());
    }

    #[test]
    fn frontmatter_is_replaced() {
        let text = "---\na: 1\n---\nbody";
        let spans = spans(&parse(text), text);
        let fm = &spans[0];
        assert_eq!(fm.style, Style::Frontmatter);
        assert_eq!(fm.markers.first(), Some(&(0..13)));
        assert_eq!(fm.markers.len(), 1);
        assert_eq!(fm.reveal, Reveal::Never);
        assert_eq!(fm.replace, Some(Replacement::Properties));
    }

    #[test]
    fn embeds_choose_widget_by_extension() {
        let text = "![[photo.JPG|200]]\n\n![[Other note#Section]]\n";
        let spans = spans(&parse(text), text);
        let replacements: Vec<_> = spans.iter().filter_map(|s| s.replace.clone()).collect();
        assert_eq!(
            replacements,
            [
                Replacement::Image {
                    target: "photo.JPG".into(),
                    size: Some((200, None))
                },
                Replacement::NoteEmbed {
                    target: "Other note".into()
                },
            ]
        );
    }

    #[test]
    fn base_blocks_can_be_replaced() {
        let text = "```base\nviews:\n  - type: table\n```\n\n```rust\nx\n```\n";
        let spans = spans(&parse(text), text);
        let blocks: Vec<_> = spans
            .iter()
            .filter(|s| matches!(s.style, Style::CodeBlock { .. }))
            .collect();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].replace, Some(Replacement::Base));
        assert_eq!(blocks[1].replace, None);
    }

    #[test]
    fn quotes_reveal_line_by_line() {
        let text = "> one\n> two\n";
        let spans = spans(&parse(text), text);
        let prefixes: Vec<_> = spans
            .iter()
            .filter(|s| s.style == Style::QuoteMarker)
            .collect();
        assert_eq!(prefixes.len(), 2);
        assert!(prefixes[0].reveal.is_revealed_by(&(3..3)));
        assert!(!prefixes[1].reveal.is_revealed_by(&(3..3)));
    }

    #[test]
    fn callouts_reveal_as_a_block() {
        let text = "> [!tip] Title\n> body\n";
        let spans = spans(&parse(text), text);
        let callout = find(&spans, &Style::Callout { kind: "tip".into() });
        assert_eq!(markers(text, callout), ["> ", "> ", "[!tip]"]);
        assert!(callout.reveal.is_revealed_by(&(19..19)));
        assert_eq!(
            &text[find(&spans, &Style::CalloutTitle).range.clone()],
            "Title"
        );
    }
}
