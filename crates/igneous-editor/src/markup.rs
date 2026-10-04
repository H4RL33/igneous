//! Inline Markdown as Pango markup, for text shown in widgets rather than in
//! the buffer: table cells, embeds and property values.

use gtk::glib;
use igneous_markdown::present::{self, Style};
use igneous_markdown::{LinkKind, parse};

/// `text` with its inline syntax hidden and its formatting kept, as markup.
/// `highlight` is the background for `==highlights==` (`#rrggbb[aa]`).
pub fn inline(text: &str, highlight: &str) -> String {
    let highlight_open = format!("<span background=\"{highlight}\">");
    let doc = parse(text);
    let spans = present::spans(&doc, text);
    // What to hide, and where formatting starts and ends.
    let mut hidden = vec![false; text.len()];
    let mut opens: Vec<(usize, &str)> = Vec::new();
    let mut closes: Vec<(usize, &str)> = Vec::new();
    for span in &spans {
        let (open, close) = match span.style {
            Style::Strong => ("<b>", "</b>"),
            Style::Emphasis => ("<i>", "</i>"),
            Style::Strikethrough => ("<s>", "</s>"),
            Style::InlineCode | Style::Math { .. } => ("<tt>", "</tt>"),
            Style::Highlight => (highlight_open.as_str(), "</span>"),
            Style::Link | Style::WikiLink | Style::Url | Style::Tag => ("<u>", "</u>"),
            Style::Comment => {
                hidden[span.range.clone()].fill(true);
                continue;
            }
            _ => continue,
        };
        for marker in &span.markers {
            hidden[marker.clone()].fill(true);
        }
        opens.push((span.range.start, open));
        closes.push((span.range.end, close));
    }
    // A wikilink with an alias shows only the alias.
    for link in &doc.links {
        if link.kind == LinkKind::Wiki
            && let Some(display) = &link.display_range
        {
            for i in link.range.clone() {
                if !display.contains(&i) {
                    hidden[i] = true;
                }
            }
        }
    }
    let mut out = String::with_capacity(text.len() + 16);
    for (i, ch) in text.char_indices() {
        for (_, tag) in closes.iter().rev().filter(|(at, _)| *at == i) {
            out.push_str(tag);
        }
        for (_, tag) in opens.iter().filter(|(at, _)| *at == i) {
            out.push_str(tag);
        }
        if !hidden[i] {
            out.push_str(&glib::markup_escape_text(ch.encode_utf8(&mut [0; 4])));
        }
    }
    for (_, tag) in closes.iter().rev().filter(|(at, _)| *at == text.len()) {
        out.push_str(tag);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting_without_syntax() {
        let inline = |text| inline(text, "#ffff0066");
        assert_eq!(inline("a **b** c"), "a <b>b</b> c");
        assert_eq!(inline("*i* and `x<y`"), "<i>i</i> and <tt>x&lt;y</tt>");
        assert_eq!(inline("[[Note|alias]] here"), "<u>alias</u> here");
        assert_eq!(inline("[[Note]]"), "<u>Note</u>");
        assert_eq!(inline("~~gone~~"), "<s>gone</s>");
        assert_eq!(inline("==hi=="), "<span background=\"#ffff0066\">hi</span>");
        assert_eq!(inline("plain & simple"), "plain &amp; simple");
        assert_eq!(inline("a %%hidden%% b"), "a  b");
    }

    #[test]
    fn markup_is_valid() {
        for text in ["**unclosed", "a *b **c** d*", "[x](y) **z**", "#tag `c`"] {
            let markup = inline(text, "#ffff00");
            assert!(
                gtk::pango::parse_markup(&markup, '\0').is_ok(),
                "{text:?} → {markup:?}"
            );
        }
    }
}
