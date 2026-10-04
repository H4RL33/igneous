//! Running the configured rules over a note.

use igneous_core::VaultPath;
use igneous_core::settings::LintSettings;
use igneous_markdown::{Document, TextEdit, frontmatter};
use jiff::Zoned;

use crate::Span;
use crate::protect::{Ignore, RangeSet};
use crate::rule::{FileInfo, LintCtx, Options, Rule, Stage};
use crate::rules;
use crate::syntax;
use crate::text::{apply, edits_between};

/// The rules that run before all others, in this order.
const FIRST: &[&str] = &[
    "format-tags-in-yaml",
    "escape-yaml-special-characters",
    "move-math-block-indicators-to-their-own-line",
];

/// The rules that run after all others, in this order. `add-blank-line-after-yaml`
/// moves after `yaml-timestamp` when there's no frontmatter for it yet.
const LAST: &[&str] = &[
    "blockquote-style",
    "force-yaml-escape",
    "trailing-spaces",
    "consecutive-blank-lines",
    "add-blank-line-after-yaml",
    "yaml-timestamp",
];

/// Passes over a note before giving up on it settling.
const MAX_PASSES: usize = 4;

/// A problem found in a note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// The rule's ID, or `yaml` for frontmatter that isn't valid YAML.
    pub rule: &'static str,
    /// Where, in the text that was linted.
    pub range: Span,
    pub message: String,
}

/// What linting a note produced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LintResult {
    /// The linted text.
    pub text: String,
    /// Edits turning the original text into `text`.
    pub edits: Vec<TextEdit>,
    pub diagnostics: Vec<Diagnostic>,
    /// The note wasn't linted: it's in an ignored folder, is an ignored file,
    /// or its frontmatter disables all rules.
    pub skipped: bool,
}

impl LintResult {
    pub fn changed(&self) -> bool {
        !self.edits.is_empty()
    }
}

/// The configured rules, ready to run.
pub struct Linter {
    plan: Vec<(&'static dyn Rule, Options)>,
    ignore_folders: Vec<String>,
    ignore_files: Vec<String>,
}

impl Linter {
    pub fn new(settings: &LintSettings) -> Self {
        let shared = settings
            .extra
            .get("commonStyles")
            .and_then(|v| v.as_object());
        let mut plan: Vec<(&'static dyn Rule, Options)> = rules::all()
            .iter()
            .filter_map(|rule| {
                let config = settings.rules.get(rule.id())?;
                config.enabled.then(|| {
                    let options = Options::resolve(rule.options(), shared, &config.options);
                    (*rule, options)
                })
            })
            .collect();
        plan.sort_by_key(|(rule, _)| run_order(*rule));
        Self {
            plan,
            ignore_folders: settings
                .ignore_folders
                .iter()
                .map(|f| f.trim_matches('/').to_owned())
                .filter(|f| !f.is_empty())
                .collect(),
            ignore_files: settings.ignore_files.clone(),
        }
    }

    /// The enabled rules, in the order they run.
    pub fn rules(&self) -> impl Iterator<Item = &'static dyn Rule> + '_ {
        self.plan.iter().map(|(rule, _)| *rule)
    }

    /// Whether `path` is in an ignored folder or is an ignored file.
    pub fn is_ignored(&self, path: &VaultPath) -> bool {
        let path = path.as_str();
        self.ignore_files.iter().any(|f| f == path)
            || self.ignore_folders.iter().any(|folder| {
                path.strip_prefix(folder.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
            })
    }

    /// Lints `text` as it stands now.
    pub fn lint(&self, text: &str, path: &VaultPath) -> LintResult {
        let now = Zoned::now();
        let file = FileInfo {
            created: None,
            modified: None,
        };
        self.lint_with(text, path, &now, &file)
    }

    /// Lints `text`, with the time and file dates given.
    pub fn lint_with(
        &self,
        text: &str,
        path: &VaultPath,
        now: &Zoned,
        file: &FileInfo,
    ) -> LintResult {
        let mut result = LintResult {
            text: text.to_owned(),
            ..LintResult::default()
        };
        result.diagnostics.extend(yaml_diagnostic(text));
        if self.is_ignored(path) {
            result.skipped = true;
            return result;
        }
        let disabled = disabled_rules(text);
        if disabled.iter().any(|r| r == "all") {
            result.skipped = true;
            return result;
        }

        let mut run = Run::new(text, now, path, file);
        // One rule's change can give another something to do (as when
        // moving math delimiters makes a new block), so run until nothing
        // changes. That makes linting idempotent.
        for _ in 0..MAX_PASSES {
            let before = run.steps.len();
            let mut deferred: Option<&(&'static dyn Rule, Options)> = None;
            for entry in &self.plan {
                let (rule, options) = entry;
                if disabled.iter().any(|r| r == rule.id()) {
                    continue;
                }
                if rule.id() == "add-blank-line-after-yaml" && syntax::yaml(&run.text).is_none() {
                    deferred = Some(entry);
                    continue;
                }
                run.step(*rule, options);
            }
            if let Some((rule, options)) = deferred {
                run.step(*rule, options);
            }
            if run.steps.len() == before {
                break;
            }
        }

        result.edits = edits_between(text, &run.text);
        result.diagnostics.extend(run.diagnostics);
        result.text = run.text;
        result
    }

    /// The changes one rule alone would make, for a diagnostic's Fix action.
    /// The rule needn't be enabled; it runs with its configured options, or
    /// its defaults.
    pub fn fix_rule(
        &self,
        settings: &LintSettings,
        rule_id: &str,
        text: &str,
        path: &VaultPath,
    ) -> Vec<TextEdit> {
        let Some(rule) = rules::get(rule_id) else {
            return Vec::new();
        };
        let shared = settings
            .extra
            .get("commonStyles")
            .and_then(|v| v.as_object());
        let configured = settings
            .rules
            .get(rule_id)
            .map(|c| c.options.clone())
            .unwrap_or_default();
        let options = Options::resolve(rule.options(), shared, &configured);
        let now = Zoned::now();
        let file = FileInfo {
            created: None,
            modified: None,
        };
        let env = Env {
            now: &now,
            path,
            file: &file,
            already_modified: false,
        };
        run_rule(rule, &options, text, &env)
    }
}

/// Where a rule falls in the run: obsidian-linter's fixed early and late
/// rules, and the rest by category and then ID.
fn run_order(rule: &dyn Rule) -> (Stage, usize, crate::rule::Category, &'static str) {
    match rule.stage() {
        Stage::First => (
            Stage::First,
            FIRST.iter().position(|id| *id == rule.id()).unwrap_or(0),
            rule.category(),
            rule.id(),
        ),
        Stage::Last => (
            Stage::Last,
            LAST.iter().position(|id| *id == rule.id()).unwrap_or(0),
            rule.category(),
            rule.id(),
        ),
        Stage::Regular => (Stage::Regular, 0, rule.category(), rule.id()),
    }
}

/// One lint run: the text so far, and how to map positions back to the
/// original.
struct Run<'a> {
    original_changed: bool,
    text: String,
    doc: Option<Document>,
    steps: Vec<Vec<TextEdit>>,
    diagnostics: Vec<Diagnostic>,
    now: &'a Zoned,
    path: &'a VaultPath,
    file: &'a FileInfo,
}

impl<'a> Run<'a> {
    fn new(text: &str, now: &'a Zoned, path: &'a VaultPath, file: &'a FileInfo) -> Self {
        Self {
            original_changed: false,
            text: text.to_owned(),
            doc: None,
            steps: Vec::new(),
            diagnostics: Vec::new(),
            now,
            path,
            file,
        }
    }

    fn step(&mut self, rule: &'static dyn Rule, options: &Options) {
        let doc = self
            .doc
            .take()
            .unwrap_or_else(|| igneous_markdown::parse(&self.text));
        let env = Env {
            now: self.now,
            path: self.path,
            file: self.file,
            already_modified: self.original_changed,
        };
        let edits = run_rule_on(rule, options, &self.text, Some(&doc), &env);
        if edits.is_empty() {
            self.doc = Some(doc);
            return;
        }
        for edit in &edits {
            let start = self.map_start(edit.range.start);
            let end = self.map_end(edit.range.end).max(start);
            self.diagnostics.push(Diagnostic {
                rule: rule.id(),
                range: start..end,
                message: rule.name().to_owned(),
            });
        }
        self.text = apply(&self.text, &edits);
        self.steps.push(edits);
        self.original_changed = true;
    }

    /// Maps an offset in the current text to the original, taking a position
    /// inside inserted text to the start of what it replaced.
    fn map_start(&self, offset: usize) -> usize {
        self.steps
            .iter()
            .rev()
            .fold(offset, |pos, edits| map_back(edits, pos, false))
    }

    /// Like [`Self::map_start`], but to the end of what inserted text replaced.
    fn map_end(&self, offset: usize) -> usize {
        self.steps
            .iter()
            .rev()
            .fold(offset, |pos, edits| map_back(edits, pos, true))
    }
}

/// Maps an offset back through one step's edits. A start offset inside
/// inserted text goes to the start of what was replaced; an end offset, to
/// its end.
fn map_back(edits: &[TextEdit], pos: usize, to_end: bool) -> usize {
    let mut delta: isize = 0;
    for edit in edits {
        let new_start = (edit.range.start as isize + delta) as usize;
        let new_end = new_start + edit.insert.len();
        if to_end {
            if pos <= new_start {
                return (pos as isize - delta) as usize;
            }
            if pos <= new_end {
                return edit.range.end;
            }
        } else {
            if pos < new_start {
                return (pos as isize - delta) as usize;
            }
            if pos < new_end {
                return edit.range.start;
            }
        }
        delta += edit.insert.len() as isize - edit.range.len() as isize;
    }
    (pos as isize - delta) as usize
}

/// What a rule run needs besides the text.
#[derive(Debug, Clone, Copy)]
pub struct Env<'a> {
    pub now: &'a Zoned,
    pub path: &'a VaultPath,
    pub file: &'a FileInfo,
    /// Whether earlier rules have changed the note in this run.
    pub already_modified: bool,
}

/// Runs one rule over `text`, keeping only edits that are well formed and
/// stay out of protected regions. The edits are sorted and don't overlap.
pub fn run_rule(rule: &dyn Rule, options: &Options, text: &str, env: &Env) -> Vec<TextEdit> {
    run_rule_on(rule, options, text, None, env)
}

fn run_rule_on(
    rule: &dyn Rule,
    options: &Options,
    text: &str,
    doc: Option<&Document>,
    env: &Env,
) -> Vec<TextEdit> {
    let parsed;
    let doc = match doc {
        Some(doc) => doc,
        None => {
            parsed = igneous_markdown::parse(text);
            &parsed
        }
    };
    // Rules that need parsed YAML leave invalid frontmatter alone themselves;
    // line-based ones (like escaping) can still repair it, as in
    // obsidian-linter. Either way it's reported.
    let mut ctx = LintCtx::new(text, doc, env.now, env.path, env.file);
    ctx.already_modified = env.already_modified;
    ctx.protected = guard(&ctx, rule);
    let mut edits = rule.fix(&ctx, options);
    edits.sort_by_key(|e| (e.range.start, e.range.end));
    let mut kept: Vec<TextEdit> = Vec::with_capacity(edits.len());
    for edit in edits {
        let well_formed = edit.range.start <= edit.range.end
            && text.get(edit.range.clone()).is_some()
            && kept
                .last()
                .is_none_or(|last| edit.range.start >= last.range.end);
        let no_op = text.get(edit.range.clone()) == Some(edit.insert.as_str());
        if well_formed && !no_op && !ctx.protected.is_protected(&edit.range) {
            kept.push(edit);
        }
    }
    kept
}

/// The regions a rule may not edit.
fn guard(ctx: &LintCtx, rule: &dyn Rule) -> RangeSet {
    let mut kinds = vec![Ignore::Templater, Ignore::Code, Ignore::InlineCode];
    if !rule.edits_math() {
        kinds.extend([Ignore::Math, Ignore::InlineMath]);
    }
    if !rule.edits_frontmatter() {
        kinds.push(Ignore::Yaml);
    }
    ctx.ignoring(&kinds)
}

/// A diagnostic for frontmatter that isn't valid YAML.
fn yaml_diagnostic(text: &str) -> Option<Diagnostic> {
    let fm = frontmatter::parse(text)?;
    let error = fm.error?;
    Some(Diagnostic {
        rule: "yaml",
        range: fm.body,
        message: format!("The frontmatter isn't valid YAML: {error}"),
    })
}

/// Rules turned off for this note by its `disabled rules` property, as in
/// obsidian-linter. `all` turns off every rule.
fn disabled_rules(text: &str) -> Vec<String> {
    let Some(fm) = frontmatter::parse(text) else {
        return Vec::new();
    };
    fm.entries
        .iter()
        .find(|e| e.key == "disabled rules")
        .map(|e| e.value.string_list())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_positions_back_through_edits() {
        // "abcdef" → replace "cd" with "XYZ" → "abXYZef"
        let edits = vec![TextEdit::replace(2..4, "XYZ")];
        assert_eq!(map_back(&edits, 1, false), 1);
        assert_eq!(map_back(&edits, 3, false), 2);
        assert_eq!(map_back(&edits, 3, true), 4);
        assert_eq!(map_back(&edits, 6, false), 5);
        // An insertion: "abc" → "aXbc"
        let edits = vec![TextEdit::insert(1, "X")];
        assert_eq!(map_back(&edits, 1, false), 1);
        assert_eq!(map_back(&edits, 2, true), 1);
        assert_eq!(map_back(&edits, 3, false), 2);
    }
}
