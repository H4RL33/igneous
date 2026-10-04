//! What a rule is, what it's given, and its options.

use std::cell::RefCell;
use std::collections::HashMap;

use igneous_core::VaultPath;
use igneous_markdown::{Document, TextEdit};
use jiff::Zoned;
use serde_json::{Map, Value};

use crate::Span;
use crate::protect::{Ignore, Projection, RangeSet, project};
use crate::regexes;
use crate::syntax::{self, Element};

/// Rule categories, in the order rules run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Yaml,
    Heading,
    Footnote,
    Content,
    Spacing,
    Paste,
}

/// When a rule runs relative to the others. obsidian-linter runs most rules
/// by category, but runs a few before or after the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    /// Cleans up YAML or math before other rules look at them.
    First,
    Regular,
    /// Tidies up after the others.
    Last,
}

/// A lint rule. Rules are stateless; everything they need is in the context
/// and options.
pub trait Rule: Sync + Send {
    /// obsidian-linter's ID for the rule, e.g. `trailing-spaces`.
    fn id(&self) -> &'static str;
    fn category(&self) -> Category;
    /// A short name, like "Trailing spaces".
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn options(&self) -> &'static [OptionSpec] {
        &[]
    }
    fn stage(&self) -> Stage {
        Stage::Regular
    }
    /// Whether the rule edits the frontmatter, which is protected from every
    /// other rule.
    fn edits_frontmatter(&self) -> bool {
        self.category() == Category::Yaml
    }
    /// Whether the rule edits the layout of math blocks, which is otherwise
    /// protected. Only `move-math-block-indicators-to-their-own-line` does.
    fn edits_math(&self) -> bool {
        false
    }
    /// The changes the rule makes to `ctx.text`.
    fn fix(&self, ctx: &LintCtx, options: &Options) -> Vec<TextEdit>;
}

/// A rule's option, for the preferences UI and for defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct OptionSpec {
    /// The key in `lint.json`, as in obsidian-linter's settings.
    pub key: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub kind: OptionKind,
    /// Set for options that come from the linter's shared style settings
    /// (such as the YAML quote character) rather than the rule's own.
    pub shared: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OptionKind {
    Bool(bool),
    Text(&'static str),
    Number(f64),
    Choice {
        choices: &'static [&'static str],
        default: &'static str,
    },
    /// A list of strings, empty by default.
    List,
    /// A text option with no value by default.
    OptionalText,
}

impl OptionSpec {
    pub const fn new(key: &'static str, name: &'static str, kind: OptionKind) -> Self {
        Self {
            key,
            name,
            description: "",
            kind,
            shared: false,
        }
    }

    pub const fn describe(mut self, description: &'static str) -> Self {
        self.description = description;
        self
    }

    pub const fn shared(mut self) -> Self {
        self.shared = true;
        self
    }

    pub fn default_value(&self) -> Value {
        match &self.kind {
            OptionKind::Bool(b) => Value::Bool(*b),
            OptionKind::Text(s) => Value::String((*s).to_owned()),
            OptionKind::Number(n) => {
                serde_json::Number::from_f64(*n).map_or(Value::Null, Value::Number)
            }
            OptionKind::Choice { default, .. } => Value::String((*default).to_owned()),
            OptionKind::List => Value::Array(Vec::new()),
            OptionKind::OptionalText => Value::Null,
        }
    }
}

/// Settings several rules share, as (key in `commonStyles` in `lint.json`,
/// the rule option it sets). Both are obsidian-linter's; the defaults are the
/// rules' own.
pub const SHARED_OPTIONS: &[(&str, &str)] = &[
    ("aliasArrayStyle", "aliasArrayStyle"),
    ("tagArrayStyle", "tagArrayStyle"),
    ("defaultArrayStyle", "defaultArrayStyle"),
    ("escapeCharacter", "defaultEscapeCharacter"),
    (
        "removeUnnecessaryEscapeCharsForMultiLineArrays",
        "removeUnnecessaryEscapeCharsForMultiLineArrays",
    ),
    (
        "minimumNumberOfDollarSignsToBeAMathBlock",
        "minimumNumberOfDollarSignsToBeAMathBlock",
    ),
];

/// A rule's resolved options: its defaults, then the shared styles, then
/// what's configured for the rule.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Options {
    values: Map<String, Value>,
}

impl Options {
    pub fn resolve(
        schema: &[OptionSpec],
        shared: Option<&Map<String, Value>>,
        configured: &Map<String, Value>,
    ) -> Self {
        let mut values = Map::new();
        for spec in schema {
            values.insert(spec.key.to_owned(), spec.default_value());
        }
        for (common_key, option_key) in SHARED_OPTIONS {
            if !schema.iter().any(|s| s.key == *option_key) {
                continue;
            }
            if let Some(value) = shared.and_then(|s| s.get(*common_key)) {
                values.insert((*option_key).to_owned(), value.clone());
            }
        }
        for (key, value) in configured {
            values.insert(key.clone(), value.clone());
        }
        Self { values }
    }

    /// Options from a JSON object, with no defaults filled in.
    pub fn from_map(values: Map<String, Value>) -> Self {
        Self { values }
    }

    pub fn as_map(&self) -> &Map<String, Value> {
        &self.values
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(key).filter(|v| !v.is_null())
    }

    pub fn bool(&self, key: &str) -> bool {
        self.get(key).and_then(Value::as_bool).unwrap_or(false)
    }

    pub fn str(&self, key: &str) -> &str {
        self.get(key).and_then(Value::as_str).unwrap_or("")
    }

    pub fn number(&self, key: &str) -> f64 {
        self.get(key).and_then(Value::as_f64).unwrap_or(0.0)
    }

    pub fn list(&self, key: &str) -> Vec<String> {
        match self.get(key) {
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
            Some(Value::String(s)) => s
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
            _ => Vec::new(),
        }
    }

    pub fn set(&mut self, key: &str, value: impl Into<Value>) {
        self.values.insert(key.to_owned(), value.into());
    }
}

/// Facts about the file being linted that some rules need.
#[derive(Debug, Clone)]
pub struct FileInfo {
    pub created: Option<Zoned>,
    pub modified: Option<Zoned>,
}

/// What a rule is given.
pub struct LintCtx<'a> {
    pub text: &'a str,
    pub doc: &'a Document,
    /// Regions no edit may touch: code, math, Templater tags and
    /// `linter-disable` sections, plus the frontmatter for rules that don't
    /// edit it.
    pub protected: RangeSet,
    pub now: &'a Zoned,
    pub path: &'a VaultPath,
    pub file: &'a FileInfo,
    /// Whether earlier rules have changed the note in this run.
    pub already_modified: bool,
    ranges: RefCell<HashMap<Ignore, Vec<Span>>>,
}

impl<'a> LintCtx<'a> {
    pub fn new(
        text: &'a str,
        doc: &'a Document,
        now: &'a Zoned,
        path: &'a VaultPath,
        file: &'a FileInfo,
    ) -> Self {
        Self {
            text,
            doc,
            protected: RangeSet::default(),
            now,
            path,
            file,
            already_modified: false,
            ranges: RefCell::new(HashMap::new()),
        }
    }

    /// The regions of one type.
    pub fn regions(&self, kind: Ignore) -> Vec<Span> {
        if let Some(ranges) = self.ranges.borrow().get(&kind) {
            return ranges.clone();
        }
        let ranges = self.find(kind);
        self.ranges.borrow_mut().insert(kind, ranges.clone());
        ranges
    }

    fn find(&self, kind: Ignore) -> Vec<Span> {
        let (text, doc) = (self.text, self.doc);
        match kind {
            Ignore::CustomIgnore => syntax::custom_ignore(text),
            Ignore::Templater => regexes::TEMPLATER_COMMAND
                .find_iter(text)
                .map(|m| m.range())
                .collect(),
            Ignore::Yaml => syntax::yaml(text).map(|(r, _)| r).into_iter().collect(),
            Ignore::Table => syntax::tables(text),
            Ignore::Code => syntax::positions(text, doc, Element::Code),
            Ignore::InlineCode => syntax::positions(text, doc, Element::InlineCode),
            Ignore::Math => syntax::positions(text, doc, Element::Math),
            Ignore::InlineMath => syntax::positions(text, doc, Element::InlineMath),
            Ignore::Html => syntax::positions(text, doc, Element::Html),
            Ignore::List => syntax::positions(text, doc, Element::List),
            Ignore::Image => syntax::positions(text, doc, Element::Image),
            Ignore::Link => syntax::positions(text, doc, Element::Link),
            Ignore::WikiLink => regexes::WIKI_LINK
                .find_iter(text)
                .map(|m| m.range())
                .collect(),
            Ignore::Tag => syntax::tags(text),
        }
    }

    /// The regions of the given types, plus `linter-disable` sections, which
    /// every rule leaves alone.
    pub fn ignoring(&self, kinds: &[Ignore]) -> RangeSet {
        let mut ranges = self.regions(Ignore::CustomIgnore);
        for kind in kinds {
            ranges.extend(self.regions(*kind));
        }
        RangeSet::new(ranges)
    }

    /// The note with the given types (and `linter-disable` sections) replaced
    /// by placeholders.
    pub fn projection(&self, kinds: &[Ignore]) -> Projection<'a> {
        let mut all: Vec<Ignore> = kinds.to_vec();
        all.push(Ignore::CustomIgnore);
        all.sort();
        all.dedup();
        let regions: Vec<(Ignore, Vec<Span>)> =
            all.into_iter().map(|k| (k, self.regions(k))).collect();
        project(self.text, &regions)
    }

    pub fn positions(&self, element: Element) -> Vec<Span> {
        syntax::positions(self.text, self.doc, element)
    }
}
