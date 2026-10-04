//! Text tags for each presentation style, created in priority order: later
//! tags override earlier ones, so concealment always wins.

use std::cell::RefCell;
use std::collections::HashMap;

use gtk::{gdk, pango};
use igneous_markdown::present::Style;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TagKind {
    Frontmatter,
    CodeBlock,
    Table,
    Html,
    H1,
    H2,
    H3,
    H4,
    H5,
    H6,
    Emphasis,
    Strong,
    Strikethrough,
    Highlight,
    InlineCode,
    Math,
    Link,
    WikiLink,
    Url,
    Embed,
    Tag,
    CalloutTitle,
    ListNumber,
    Footnote,
    BlockId,
    Comment,
    TaskDone,
    /// Markdown syntax that is visible (revealed or in Source mode).
    Syntax,
    /// Hidden but still taking up its width (under checkboxes and bullets).
    Ghost,
    /// Hidden.
    Conceal,
}

impl TagKind {
    pub const ALL: [TagKind; 30] = [
        TagKind::Frontmatter,
        TagKind::CodeBlock,
        TagKind::Table,
        TagKind::Html,
        TagKind::H1,
        TagKind::H2,
        TagKind::H3,
        TagKind::H4,
        TagKind::H5,
        TagKind::H6,
        TagKind::Emphasis,
        TagKind::Strong,
        TagKind::Strikethrough,
        TagKind::Highlight,
        TagKind::InlineCode,
        TagKind::Math,
        TagKind::Link,
        TagKind::WikiLink,
        TagKind::Url,
        TagKind::Embed,
        TagKind::Tag,
        TagKind::CalloutTitle,
        TagKind::ListNumber,
        TagKind::Footnote,
        TagKind::BlockId,
        TagKind::Comment,
        TagKind::TaskDone,
        TagKind::Syntax,
        TagKind::Ghost,
        TagKind::Conceal,
    ];

    pub fn for_style(style: &Style) -> Option<TagKind> {
        Some(match style {
            Style::Heading(1) => TagKind::H1,
            Style::Heading(2) => TagKind::H2,
            Style::Heading(3) => TagKind::H3,
            Style::Heading(4) => TagKind::H4,
            Style::Heading(5) => TagKind::H5,
            Style::Heading(_) => TagKind::H6,
            Style::Emphasis => TagKind::Emphasis,
            Style::Strong => TagKind::Strong,
            Style::Strikethrough => TagKind::Strikethrough,
            Style::Highlight => TagKind::Highlight,
            Style::InlineCode => TagKind::InlineCode,
            Style::CodeBlock { .. } => TagKind::CodeBlock,
            Style::CalloutTitle => TagKind::CalloutTitle,
            Style::ListNumber => TagKind::ListNumber,
            Style::TaskDoneText => TagKind::TaskDone,
            Style::Link => TagKind::Link,
            Style::WikiLink => TagKind::WikiLink,
            Style::Url => TagKind::Url,
            Style::Embed => TagKind::Embed,
            Style::Tag => TagKind::Tag,
            Style::Math { .. } => TagKind::Math,
            Style::Comment => TagKind::Comment,
            Style::FootnoteReference | Style::InlineFootnote | Style::FootnoteDefinition => {
                TagKind::Footnote
            }
            Style::BlockId => TagKind::BlockId,
            Style::Table => TagKind::Table,
            Style::Html => TagKind::Html,
            Style::Frontmatter => TagKind::Frontmatter,
            Style::Quote
            | Style::QuoteMarker
            | Style::Callout { .. }
            | Style::ListBullet
            | Style::Task { .. }
            | Style::Rule => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Concealment {
    /// `invisible` tag property.
    Invisible,
    /// Tiny, transparent text (the approach Folio and Scribe use).
    ZeroSize,
}

pub struct Palette {
    pub fg: gdk::RGBA,
    pub accent: gdk::RGBA,
}

impl Palette {
    pub fn with_alpha(c: &gdk::RGBA, alpha: f32) -> gdk::RGBA {
        gdk::RGBA::new(c.red(), c.green(), c.blue(), alpha)
    }
}

pub struct Tags {
    pub table: gtk::TextTagTable,
    tags: HashMap<TagKind, gtk::TextTag>,
    spacing: RefCell<HashMap<String, gtk::TextTag>>,
}

impl Tags {
    pub fn new(palette: &Palette, concealment: Concealment) -> Self {
        let table = gtk::TextTagTable::new();
        let muted = Palette::with_alpha(&palette.fg, 0.45);
        let faint = Palette::with_alpha(&palette.fg, 0.07);
        let accent_bg = Palette::with_alpha(&palette.accent, 0.15);
        // Pango treats a foreground alpha of 0 as unset, so use the smallest
        // non-zero alpha instead.
        let transparent = gdk::RGBA::new(0.0, 0.0, 0.0, 1.0 / 65535.0);
        let mut tags = HashMap::new();
        for kind in TagKind::ALL {
            let b = gtk::TextTag::builder().name(format!("lp-{kind:?}"));
            let tag = match kind {
                TagKind::Frontmatter => b.family("monospace").foreground_rgba(&muted),
                TagKind::CodeBlock | TagKind::Table | TagKind::Html => b.family("monospace"),
                TagKind::H1 => b.scale(1.8).weight(700),
                TagKind::H2 => b.scale(1.5).weight(700),
                TagKind::H3 => b.scale(1.3).weight(700),
                TagKind::H4 => b.scale(1.15).weight(700),
                TagKind::H5 => b.scale(1.05).weight(700),
                TagKind::H6 => b.weight(700).foreground_rgba(&muted),
                TagKind::Emphasis => b.style(pango::Style::Italic),
                TagKind::Strong => b.weight(700),
                TagKind::Strikethrough => b.strikethrough(true),
                TagKind::Highlight => b.background_rgba(&gdk::RGBA::new(0.96, 0.83, 0.18, 0.45)),
                TagKind::InlineCode | TagKind::Math => {
                    b.family("monospace").background_rgba(&faint)
                }
                TagKind::Link | TagKind::WikiLink | TagKind::Url => b
                    .foreground_rgba(&palette.accent)
                    .underline(pango::Underline::Single),
                TagKind::Embed => b.foreground_rgba(&muted),
                TagKind::Tag => b
                    .foreground_rgba(&palette.accent)
                    .background_rgba(&accent_bg),
                TagKind::CalloutTitle => b.weight(700).foreground_rgba(&palette.accent),
                TagKind::ListNumber => b.foreground_rgba(&muted),
                TagKind::Footnote => b
                    .foreground_rgba(&palette.accent)
                    .rise(4 * pango::SCALE)
                    .scale(0.8),
                TagKind::BlockId => b.foreground_rgba(&muted).scale(0.85),
                TagKind::Comment => b.foreground_rgba(&muted),
                TagKind::TaskDone => b.strikethrough(true).foreground_rgba(&muted),
                TagKind::Syntax => b.foreground_rgba(&muted),
                TagKind::Ghost => b.foreground_rgba(&transparent),
                TagKind::Conceal => match concealment {
                    Concealment::Invisible => b.invisible(true),
                    Concealment::ZeroSize => b.scale(0.01).foreground_rgba(&transparent),
                },
            }
            .build();
            table.add(&tag);
            tags.insert(kind, tag);
        }
        Self {
            table,
            tags,
            spacing: RefCell::default(),
        }
    }

    pub fn get(&self, kind: TagKind) -> &gtk::TextTag {
        &self.tags[&kind]
    }

    /// A tag adding `px` of blank space above or below a line.
    pub fn spacing(&self, name: &str) -> gtk::TextTag {
        if let Some(tag) = self.spacing.borrow().get(name) {
            return tag.clone();
        }
        let (side, px) = name.split_once('-').expect("spacing tag name");
        let px: i32 = px.parse().expect("spacing px");
        let b = gtk::TextTag::builder().name(format!("lp-space-{name}"));
        let tag = if side == "above" {
            b.pixels_above_lines(px)
        } else {
            b.pixels_below_lines(px)
        }
        .build();
        self.table.add(&tag);
        self.spacing
            .borrow_mut()
            .insert(name.to_owned(), tag.clone());
        tag
    }
}
