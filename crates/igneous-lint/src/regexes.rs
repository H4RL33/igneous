//! Regular expressions shared by the rules, ported from obsidian-linter's
//! `utils/regex.ts`. JavaScript's lookarounds aren't available, so the few
//! expressions that need them are checked in code where they're used.

use std::sync::LazyLock;

use regex::Regex;

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid regex")
}

/// A heading: indentation, hashes, the space after them, the text, and any
/// closing hashes. Multi-line; a `\r` ends a line too, as in JavaScript.
pub static ALL_HEADERS: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?mR)^([ \t]*)(#+)([ \t]+)([^\n\r]*?)([ \t]+#+)?$"));

/// `![[target|alias|size]]` and `[[target]]`.
pub static WIKI_LINK: LazyLock<Regex> =
    LazyLock::new(|| re(r"(!?)\[{2}([^\]\[\n|]+)(\|([^\]\[\n|]+))?(\|([^\]\[\n|]+))?\]{2}"));

/// `[text](target)` and `![alt](src)`.
pub static GENERIC_LINK: LazyLock<Regex> = LazyLock::new(|| re(r"(!?)\[([^\[]*)\](\(.*\))"));

/// A `#tag` with the whitespace (or text start) before it. The tag must also
/// contain a character that isn't a digit, which the caller checks, since it
/// needs a lookahead.
pub static TAG_WITH_LEADING_WHITESPACE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(\s|\A)(#[\p{L}\-_0-9/\p{Emoji_Presentation}]+)"));

/// The `[ ]` of a task at the start of the text.
pub static CHECKLIST_BOX_STARTS_TEXT: LazyLock<Regex> = LazyLock::new(|| re(r"\A\[.\]"));

/// A line starting with a list marker. Multi-line.
pub static STARTS_WITH_LIST_MARKER: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?m)^\s*(- |\* |\+ |\d+[.)] |- (\[.\]) )"));

/// A callout's `> [!type]`. Multi-line.
pub static CALLOUT: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^(>\s*)+\[![^\s]*\]"));

/// A code fence inside a blockquote. Multi-line.
pub static CODE_BLOCK_BLOCKQUOTE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?m)^\n?(>\s*)+((```)|(~~~))"));

/// An empty blockquote line inside math. Multi-line.
pub static EMPTY_LINE_MATH_BLOCKQUOTE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?m)^ {0,3}(>( |\t)*)+\$*?$"));

/// Starts with a blockquote marker. Multi-line.
pub static STARTS_WITH_BLOCKQUOTE: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^\s*(>\s*)+"));

/// A table's separator row. Multi-line.
pub static TABLE_SEPARATOR: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?m)(\|? *:?-{1,}:? *\|?)(\| *:?-{1,}:? *\|?)*( |\t)*$"));

/// The pipe (and any blockquote markers or indentation) a table row starts
/// with. Multi-line.
pub static TABLE_STARTING_PIPE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?m)^(((?:>[ \t]*)+)|([ ]{0,3}))\|"));

/// A line with a pipe in it.
pub static TABLE_ROW: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)[^\n]*?\|[^\n]*?(\n|$)"));

/// Two or more blank lines, possibly holding whitespace.
pub static MULTIPLE_BLANK_LINES: LazyLock<Regex> = LazyLock::new(|| {
    re("(\n([\t\u{0b}\u{0c}\r \u{a0}\u{2000}-\u{200b}\u{2028}-\u{2029}\u{3000}]+)?){2,}\n")
});

/// An HTML entity ending a line, like `&amp;`.
pub static HTML_ENTITY_AT_END: LazyLock<Regex> = LazyLock::new(|| re(r"(?im)&[^\s]+;$"));

/// `<!-- linter-disable -->` (or `%% linter-disable %%`).
pub static CUSTOM_IGNORE_START: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?:<!-{2,}|%%) *linter-disable *(?:-{2,}>|%%)"));

/// `<!-- linter-enable -->` (or `%% linter-enable %%`).
pub static CUSTOM_IGNORE_END: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?:<!-{2,}|%%) *linter-enable *(?:-{2,}>|%%)"));

/// Templater's `<% … %>`.
pub static TEMPLATER_COMMAND: LazyLock<Regex> = LazyLock::new(|| re(r"(?s)<%.*?%>"));
