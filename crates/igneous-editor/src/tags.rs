//! Live Preview's text tags, coloured from the editor theme.
//!
//! Tags are created in priority order: later tags win, so concealment always
//! beats styling. Colours are set again whenever the theme changes.

use std::cell::RefCell;
use std::collections::HashMap;

use gtk::{gdk, pango, prelude::*};
use igneous_markdown::present::Style;

use crate::theme::{Color, Variant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TagKind {
    Frontmatter,
    CodeBlock,
    Table,
    Html,
    /// Text inside a callout: indented into its panel.
    CalloutBody,
    /// Text inside a quote: indented past its bar.
    QuoteBody,
    /// A callout's first line, indented further to make room for its icon.
    CalloutHeading,
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
    /// A link to a note that doesn't exist.
    Unresolved,
    Tag,
    ListNumber,
    Footnote,
    BlockId,
    Comment,
    TaskDone,
    /// Markdown syntax that is visible (revealed, or in Source mode).
    Syntax,
    /// Hidden but still taking up its width (under checkboxes and bullets).
    Ghost,
    /// Hidden.
    Conceal,
}

impl TagKind {
    pub const ALL: [TagKind; 33] = [
        TagKind::Frontmatter,
        TagKind::CodeBlock,
        TagKind::Table,
        TagKind::Html,
        TagKind::CalloutBody,
        TagKind::QuoteBody,
        TagKind::CalloutHeading,
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
        TagKind::Unresolved,
        TagKind::Tag,
        TagKind::ListNumber,
        TagKind::Footnote,
        TagKind::BlockId,
        TagKind::Comment,
        TagKind::TaskDone,
        TagKind::Syntax,
        TagKind::Ghost,
        TagKind::Conceal,
    ];

    /// The tag that styles text of `style`, if any. Callout titles are
    /// coloured per callout type, by [`Tags::callout_title`].
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
            Style::Quote => TagKind::QuoteBody,
            Style::Callout { .. } => TagKind::CalloutBody,
            Style::QuoteMarker
            | Style::CalloutTitle
            | Style::ListBullet
            | Style::Task { .. }
            | Style::Rule => return None,
        })
    }
}

/// The theme role for a callout type, following Obsidian's aliases.
pub fn callout_role(kind: &str) -> &'static str {
    match kind {
        "abstract" | "summary" | "tldr" | "tip" | "hint" | "important" => "callout-tip",
        "success" | "check" | "done" => "callout-success",
        "question" | "help" | "faq" => "callout-question",
        "warning" | "caution" | "attention" => "callout-warning",
        "failure" | "fail" | "missing" => "callout-failure",
        "danger" | "error" => "callout-danger",
        "bug" => "callout-bug",
        "example" => "callout-example",
        "quote" | "cite" => "callout-quote",
        _ => "callout-note",
    }
}

/// The icon for a callout type.
pub fn callout_icon(kind: &str) -> &'static str {
    match callout_role(kind) {
        "callout-tip" => "starred-symbolic",
        "callout-success" => "object-select-symbolic",
        "callout-question" => "dialog-question-symbolic",
        "callout-warning" => "dialog-warning-symbolic",
        "callout-failure" | "callout-danger" => "dialog-error-symbolic",
        "callout-bug" => "applications-science-symbolic",
        "callout-example" => "view-list-bullet-symbolic",
        "callout-quote" => "chat-message-new-symbolic",
        _ => "dialog-information-symbolic",
    }
}

/// `#rrggbbaa`, for Pango markup and CSS.
pub fn rgba_hex(color: &gdk::RGBA) -> String {
    let channel = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}{:02x}",
        channel(color.red()),
        channel(color.green()),
        channel(color.blue()),
        channel(color.alpha())
    )
}

pub fn rgba(color: Color) -> gdk::RGBA {
    gdk::RGBA::new(
        f32::from(color.r) / 255.0,
        f32::from(color.g) / 255.0,
        f32::from(color.b) / 255.0,
        f32::from(color.a) / 255.0,
    )
}

/// Colours drawn under the text (code panels, callouts, bars, bullets).
#[derive(Clone)]
pub struct Palette {
    pub syntax: gdk::RGBA,
    pub code_background: gdk::RGBA,
    pub quote: gdk::RGBA,
    pub list_marker: gdk::RGBA,
    variant: Variant,
    accent: Color,
}

impl Palette {
    pub fn new(variant: &Variant, accent: Color) -> Self {
        let role = |name| rgba(variant.role(name, accent));
        Self {
            syntax: role("syntax"),
            code_background: role("code-background"),
            quote: role("quote"),
            list_marker: role("list-marker"),
            variant: variant.clone(),
            accent,
        }
    }

    pub fn role(&self, name: &str) -> gdk::RGBA {
        rgba(self.variant.role(name, self.accent))
    }

    pub fn with_alpha(color: &gdk::RGBA, alpha: f32) -> gdk::RGBA {
        gdk::RGBA::new(color.red(), color.green(), color.blue(), alpha)
    }
}

pub struct Tags {
    pub table: gtk::TextTagTable,
    tags: HashMap<TagKind, gtk::TextTag>,
    spacing: RefCell<HashMap<String, gtk::TextTag>>,
    callout_titles: RefCell<HashMap<&'static str, gtk::TextTag>>,
    palette: RefCell<Option<Palette>>,
    /// The wavy underline under problems the linter found.
    pub diagnostic: gtk::TextTag,
}

impl Tags {
    pub fn new() -> Self {
        let table = gtk::TextTagTable::new();
        // Pango treats a foreground alpha of 0 as unset, so use the smallest
        // alpha that isn't.
        let transparent = gdk::RGBA::new(0.0, 0.0, 0.0, 1.0 / 65535.0);
        let mut tags = HashMap::new();
        for kind in TagKind::ALL {
            let b = gtk::TextTag::builder().name(format!("lp-{kind:?}"));
            let tag = match kind {
                TagKind::Frontmatter | TagKind::CodeBlock | TagKind::Table | TagKind::Html => {
                    b.family("monospace")
                }
                TagKind::H1 => b.scale(1.75).weight(700),
                TagKind::H2 => b.scale(1.45).weight(700),
                TagKind::H3 => b.scale(1.25).weight(700),
                TagKind::H4 => b.scale(1.12).weight(700),
                TagKind::H5 => b.scale(1.05).weight(700),
                TagKind::H6 => b.weight(700),
                TagKind::Emphasis => b.style(pango::Style::Italic),
                TagKind::Strong => b.weight(700),
                TagKind::Strikethrough | TagKind::TaskDone => b.strikethrough(true),
                TagKind::InlineCode | TagKind::Math => b.family("monospace"),
                TagKind::Link | TagKind::Url => b.underline(pango::Underline::Single),
                TagKind::Footnote => b.rise(4 * pango::SCALE).scale(0.8),
                TagKind::BlockId => b.scale(0.85),
                TagKind::Ghost => b.foreground_rgba(&transparent),
                TagKind::Conceal => b.scale(0.01).foreground_rgba(&transparent),
                _ => b,
            }
            .build();
            table.add(&tag);
            tags.insert(kind, tag);
        }
        let diagnostic = gtk::TextTag::builder()
            .name("lp-diagnostic")
            .underline(pango::Underline::Error)
            .build();
        table.add(&diagnostic);
        Self {
            table,
            tags,
            diagnostic,
            spacing: RefCell::default(),
            callout_titles: RefCell::default(),
            palette: RefCell::default(),
        }
    }

    pub fn get(&self, kind: TagKind) -> &gtk::TextTag {
        &self.tags[&kind]
    }

    /// Colours every tag from `palette`.
    pub fn set_palette(&self, palette: &Palette) {
        self.diagnostic
            .set_underline_rgba(Some(&palette.role("warning")));
        let fg = |kind: TagKind, role: &str| {
            self.get(kind)
                .set_foreground_rgba(Some(&palette.role(role)));
        };
        fg(TagKind::Frontmatter, "frontmatter");
        fg(TagKind::H1, "heading-1");
        fg(TagKind::H2, "heading-2");
        fg(TagKind::H3, "heading-3");
        fg(TagKind::H4, "heading-4");
        fg(TagKind::H5, "heading-5");
        fg(TagKind::H6, "heading-6");
        fg(TagKind::Strikethrough, "strikethrough");
        fg(TagKind::TaskDone, "strikethrough");
        fg(TagKind::InlineCode, "code");
        fg(TagKind::Math, "math");
        fg(TagKind::Link, "link");
        fg(TagKind::Url, "url");
        fg(TagKind::WikiLink, "wikilink");
        fg(TagKind::Embed, "embed");
        fg(TagKind::Unresolved, "unresolved-link");
        fg(TagKind::Tag, "tag");
        fg(TagKind::ListNumber, "list-marker");
        fg(TagKind::Footnote, "link");
        fg(TagKind::BlockId, "block-id");
        fg(TagKind::Comment, "comment");
        fg(TagKind::Syntax, "syntax");
        self.get(TagKind::Highlight)
            .set_background_rgba(Some(&palette.role("highlight")));
        self.get(TagKind::InlineCode)
            .set_background_rgba(Some(&palette.code_background));
        let tag = palette.role("tag");
        self.get(TagKind::Tag)
            .set_background_rgba(Some(&Palette::with_alpha(&tag, 0.12)));
        for (role, title) in self.callout_titles.borrow().iter() {
            title.set_foreground_rgba(Some(&palette.role(role)));
        }
        self.palette.replace(Some(palette.clone()));
    }

    /// The bold, coloured tag for a callout's title.
    pub fn callout_title(&self, kind: &str) -> gtk::TextTag {
        let role = callout_role(kind);
        if let Some(tag) = self.callout_titles.borrow().get(role) {
            return tag.clone();
        }
        let tag = gtk::TextTag::builder()
            .name(format!("lp-title-{role}"))
            .weight(700)
            .build();
        if let Some(palette) = self.palette.borrow().as_ref() {
            tag.set_foreground_rgba(Some(&palette.role(role)));
        }
        self.table.add(&tag);
        // Below concealment: keep Conceal the highest-priority tag.
        tag.set_priority(self.get(TagKind::Syntax).priority());
        self.callout_titles.borrow_mut().insert(role, tag.clone());
        tag
    }

    /// Puts the text indents for callouts and quotes `margin` pixels from the
    /// view's edge (tags' margins replace the view's rather than add to it).
    pub fn set_left_margin(&self, margin: i32) {
        self.get(TagKind::CalloutBody).set_left_margin(margin + 16);
        self.get(TagKind::QuoteBody).set_left_margin(margin + 14);
        self.get(TagKind::CalloutHeading)
            .set_left_margin(margin + 38);
    }

    /// A tag adding blank space above (`above-N`) or below (`below-N`) a line.
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
