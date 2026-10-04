//! Markdown and YAML linting for Igneous.
//!
//! The rules are obsidian-linter's: the same IDs, options and behaviour, so a
//! vault keeps its familiar formatting. Configuration comes only from the
//! vault's `.igneous/lint.json` ([`LintSettings`]).
//!
//! [`Linter::lint`] runs the enabled rules over a note and returns the linted
//! text, the [`TextEdit`]s that produce it, and a [`Diagnostic`] for each
//! change. Rules run in a fixed order: a few clean-up rules first, then the
//! rest by category (YAML, headings, footnotes, content, spacing) and ID, then
//! a few tidying rules last. The note is parsed again whenever a rule changes
//! it.
//!
//! No rule may change code, math, Templater tags (`<% … %>`) or regions
//! between `<!-- linter-disable -->` and `<!-- linter-enable -->`; only YAML
//! rules may change the frontmatter. Edits that would are dropped.
//!
//! [`TextEdit`]: igneous_markdown::TextEdit

#![forbid(unsafe_code)]

mod engine;
pub mod moment;
pub mod protect;
mod regexes;
pub mod rule;
pub mod rules;
mod syntax;
pub mod text;
mod yaml;

pub use engine::{Diagnostic, Env, LintResult, Linter, run_rule};
pub use igneous_core::settings::{LintSettings, RuleConfig};
pub use rule::{Category, FileInfo, LintCtx, OptionKind, OptionSpec, Options, Rule};

/// A byte range in a note.
pub type Span = std::ops::Range<usize>;

/// The array styles for the shared `aliasArrayStyle`, `tagArrayStyle` and
/// `defaultArrayStyle` options in `commonStyles`. The last three are only
/// meaningful for tags and aliases.
pub const ARRAY_STYLES: &[&str] = &[
    "single-line",
    "multi-line",
    "single string to single-line",
    "single string to multi-line",
    "single string comma delimited",
    "single string space delimited",
    "single-line space delimited",
];

/// The rules turned on in a new vault: safe ones that only tidy whitespace.
pub const RECOMMENDED: &[&str] = &[
    "trailing-spaces",
    "line-break-at-document-end",
    "consecutive-blank-lines",
];

/// Settings with only the [`RECOMMENDED`] rules on.
pub fn recommended() -> LintSettings {
    let mut settings = LintSettings {
        rules: Default::default(),
        ..LintSettings::default()
    };
    for id in RECOMMENDED {
        settings.rules.insert(
            (*id).to_owned(),
            RuleConfig {
                enabled: true,
                options: Default::default(),
            },
        );
    }
    settings
}
