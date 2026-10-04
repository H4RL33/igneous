//! Obsidian's search language.
//!
//! [`parse`] turns a search into a [`Query`]; [`Matcher`] compiles it and
//! tests notes against it. The syntax is Obsidian's:
//!
//! | Syntax | Matches |
//! |---|---|
//! | `meeting work` | notes containing both words, in their text or path |
//! | `"star wars"` | the exact phrase; `\"` escapes a quote |
//! | `meeting OR work` | either; `OR` binds more loosely than the implicit AND |
//! | `-work`, `-(work meetup)` | notes without it |
//! | `/\d{4}-\d{2}/` | a regular expression |
//! | `file:` `path:` `content:` | only the file name, the path, or the text |
//! | `tag:#work` | the tag or one nested under it (`#work/project`) |
//! | `line:` `block:` `section:` | all of it within one line, block or section |
//! | `task:` `task-todo:` `task-done:` | within one task, any, open or done |
//! | `match-case:` `ignore-case:` | override the case setting |
//! | `[status]`, `[status:Draft OR Done]` | a property, optionally with a value |
//! | `[duration:<5]`, `[status:null]` | comparisons, and properties with no value |
//!
//! Operators take a word, a phrase, a regex or a parenthesised group:
//! `task:(call OR email)`. Words match anywhere inside words, as in Obsidian:
//! `meet` finds "meeting".
//!
//! Regular expressions use Rust's syntax, which covers the JavaScript syntax
//! Obsidian accepts except lookaround and backreferences.

mod matcher;
mod parse;

pub use matcher::{Hit, Matcher, Snippet, snippets};
pub use parse::{ParseError, parse};

/// A parsed search.
#[derive(Debug, Clone, PartialEq)]
pub enum Query {
    /// An empty search or group. It matches every note; the search pane
    /// shouldn't run an empty search at all.
    Empty,
    Word(String),
    Phrase(String),
    /// The pattern between the slashes.
    Regex(String),
    And(Vec<Query>),
    Or(Vec<Query>),
    Not(Box<Query>),
    Op(Operator, Box<Query>),
    /// `[key]` or `[key:value]`.
    Property {
        key: String,
        value: Option<Box<Query>>,
    },
    /// Inside a property value: `<5`, `>=2024-01-01`.
    Compare(Comparison, String),
    /// Inside a property value: `null`, a property with no value.
    Null,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    File,
    Path,
    Content,
    Tag,
    Line,
    Block,
    Section,
    Task,
    TaskTodo,
    TaskDone,
    MatchCase,
    IgnoreCase,
}

impl Operator {
    pub fn name(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Path => "path",
            Self::Content => "content",
            Self::Tag => "tag",
            Self::Line => "line",
            Self::Block => "block",
            Self::Section => "section",
            Self::Task => "task",
            Self::TaskTodo => "task-todo",
            Self::TaskDone => "task-done",
            Self::MatchCase => "match-case",
            Self::IgnoreCase => "ignore-case",
        }
    }

    pub const ALL: [Operator; 12] = [
        Self::File,
        Self::Path,
        Self::Content,
        Self::Tag,
        Self::Line,
        Self::Block,
        Self::Section,
        Self::Task,
        Self::TaskTodo,
        Self::TaskDone,
        Self::MatchCase,
        Self::IgnoreCase,
    ];

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|op| op.name().eq_ignore_ascii_case(name))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comparison {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchOptions {
    /// The search pane's "Match case" toggle.
    pub match_case: bool,
}

/// What the index can do before running the matcher.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchPlan {
    /// Text every matching note contains, in its path or its text, compared
    /// case-insensitively. Terms shorter than three characters are left out,
    /// so the list suits a trigram FTS5 table as a pre-filter. Notes it lets
    /// through must still be checked with [`Matcher::matches`].
    pub fts_terms: Vec<String>,
    /// Whether the matcher needs each candidate's parsed document to decide
    /// (`block:`, `section:`, `task:`). Without these it parses a note only
    /// to highlight tags and properties in notes that already matched.
    pub needs_document: bool,
    /// Whether files other than notes can match (through `file:`/`path:`).
    pub reaches_files: bool,
}

/// Works out what can be done before matching note by note.
pub fn plan(query: &Query) -> SearchPlan {
    let mut plan = SearchPlan::default();
    required_terms(query, &mut plan.fts_terms);
    plan.fts_terms.retain(|t| t.chars().count() >= 3);
    plan.fts_terms.dedup();
    plan.needs_document = needs_document(query);
    plan.reaches_files = reaches_files(query);
    plan
}

/// Terms that must appear in every match: those reachable from the root
/// through ANDs and text operators, never through OR or negation.
fn required_terms(query: &Query, out: &mut Vec<String>) {
    match query {
        Query::Word(w) | Query::Phrase(w) => out.push(w.clone()),
        Query::And(items) => items.iter().for_each(|q| required_terms(q, out)),
        Query::Op(
            Operator::Content
            | Operator::Line
            | Operator::Block
            | Operator::Section
            | Operator::Task
            | Operator::TaskTodo
            | Operator::TaskDone
            | Operator::MatchCase
            | Operator::IgnoreCase,
            inner,
        ) => required_terms(inner, out),
        _ => {}
    }
}

fn needs_document(query: &Query) -> bool {
    match query {
        Query::And(items) | Query::Or(items) => items.iter().any(needs_document),
        Query::Not(inner) => needs_document(inner),
        Query::Op(
            Operator::Block
            | Operator::Section
            | Operator::Task
            | Operator::TaskTodo
            | Operator::TaskDone,
            _,
        ) => true,
        Query::Op(_, inner) => needs_document(inner),
        _ => false,
    }
}

fn reaches_files(query: &Query) -> bool {
    match query {
        Query::And(items) | Query::Or(items) => items.iter().any(reaches_files),
        Query::Op(Operator::File | Operator::Path, _) => true,
        Query::Op(Operator::MatchCase | Operator::IgnoreCase, inner) => reaches_files(inner),
        _ => false,
    }
}
