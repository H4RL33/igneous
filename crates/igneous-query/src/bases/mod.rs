//! Bases: `.base` files and ` ```base ` blocks.
//!
//! A base is a YAML file of filters, formulas, property settings and views
//! ([`BaseFile`]). [`run`] applies one view to every file in the vault and
//! returns its columns and rows, sorted, grouped and summarised.
//!
//! Igneous never rewrites a `.base` file. The only change it makes is the
//! one Obsidian makes when you rearrange a table: [`BaseFile::set_view_state`]
//! returns minimal edits for the view's `order`, `columnSize` and `sort`.
//!
//! Formulas and filters use the expression language in [`expr`], with the
//! functions in [`functions`]. Supported view layouts are table, cards and
//! list; others (such as kanban) give [`ViewResult::Unsupported`].

mod eval;
pub mod expr;
mod file;
pub mod functions;
mod run;
pub mod value;

pub use eval::EvalError;
pub use file::{
    BaseError, BaseFile, Direction, Filter, PropertyConfig, SortKey, View, ViewState, normalise_id,
};
pub use functions::Registry;
pub use run::{
    Cell, Column, Group, Row, RunOptions, Summary, ViewData, ViewKind, ViewResult, evaluate, run,
    run_with,
};
pub use value::{Link, Value};
