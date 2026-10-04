//! The Linter page in Preferences: when to lint, whether to underline
//! problems, each rule with its options, and what to ignore. Every change is
//! saved to `.igneous/lint.json` straight away, keeping keys Igneous doesn't
//! know.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use igneous_core::settings::LintSettings;
use igneous_lint::{Category, OptionKind, OptionSpec, Rule};
use serde_json::Value;

use crate::window::Window;

type Update = Rc<dyn Fn(&dyn Fn(&mut LintSettings))>;

pub fn page(window: &Window, dialog: &adw::PreferencesDialog) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Linter")
        .icon_name("tools-check-spelling-symbolic")
        .build();
    let lint = window.lint().clone();
    if let Some(error) = lint.error() {
        page.add(
            &adw::PreferencesGroup::builder()
                .title("Linter")
                .description(format!(
                    "lint.json can’t be read, so these settings can’t be changed until it’s \
                     fixed or removed: {error}"
                ))
                .build(),
        );
        return page;
    }

    // Saves a change, then rechecks open notes.
    let update: Update = {
        let window = window.downgrade();
        let dialog = dialog.downgrade();
        Rc::new(move |change: &dyn Fn(&mut LintSettings)| {
            let Some(window) = window.upgrade() else {
                return;
            };
            let mut settings = window.lint().settings();
            change(&mut settings);
            if settings == window.lint().settings() {
                return;
            }
            match window.lint().set_settings(settings) {
                Ok(()) => window.refresh_problems(),
                Err(e) => {
                    if let Some(dialog) = dialog.upgrade() {
                        dialog.add_toast(adw::Toast::new(&e));
                    }
                }
            }
        })
    };
    let settings = lint.settings();

    let general = adw::PreferencesGroup::builder()
        .title("Linting")
        .description(
            "Saved with this vault, in .igneous/lint.json. The rules are obsidian-linter’s. \
             Ctrl+Alt+L lints the open note.",
        )
        .build();
    let on_save = adw::SwitchRow::builder()
        .title("Lint When Saving")
        .subtitle(
            "Ctrl+S lints the note before saving it. Automatic saves never lint, so text \
             isn’t rewritten while you type.",
        )
        .active(settings.lint_on_save)
        .build();
    let update_on_save = update.clone();
    on_save.connect_active_notify(move |row| {
        let active = row.is_active();
        update_on_save(&|s| s.lint_on_save = active);
    });
    let underline = adw::SwitchRow::builder()
        .title("Underline Problems")
        .subtitle(
            "Mark what the rules would change. Frontmatter that isn’t valid YAML is always \
             marked.",
        )
        .active(crate::lint::show_problems(&settings))
        .build();
    let update_underline = update.clone();
    underline.connect_active_notify(move |row| {
        let active = row.is_active();
        update_underline(&|s| crate::lint::set_show_problems(s, active));
    });
    general.add(&on_save);
    general.add(&underline);
    page.add(&general);

    let linter = lint.linter();
    for (category, title) in [
        (Category::Yaml, "YAML Rules"),
        (Category::Heading, "Heading Rules"),
        (Category::Footnote, "Footnote Rules"),
        (Category::Content, "Content Rules"),
        (Category::Spacing, "Spacing Rules"),
    ] {
        let rules: Vec<&'static dyn Rule> = igneous_lint::rules::all()
            .iter()
            .copied()
            .filter(|r| r.category() == category)
            .collect();
        if rules.is_empty() {
            continue;
        }
        let group = adw::PreferencesGroup::builder().title(title).build();
        for rule in rules {
            let enabled = settings.rules.get(rule.id()).is_some_and(|c| c.enabled);
            let options = linter.options_for(rule);
            group.add(&rule_row(rule, enabled, &options, &update));
        }
        page.add(&group);
    }

    let ignored = adw::PreferencesGroup::builder()
        .title("Ignored")
        .description("Notes the linter leaves alone. Separate entries with commas.")
        .build();
    for (title, current, folders) in [
        ("Folders", settings.ignore_folders.join(", "), true),
        ("Files", settings.ignore_files.join(", "), false),
    ] {
        let row = adw::EntryRow::builder()
            .title(title)
            .text(current)
            .show_apply_button(true)
            .build();
        let update = update.clone();
        row.connect_apply(move |row| {
            let list = split_list(&row.text());
            update(&|s| {
                if folders {
                    s.ignore_folders = list.clone();
                } else {
                    s.ignore_files = list.clone();
                }
            });
        });
        ignored.add(&row);
    }
    page.add(&ignored);
    page
}

/// A rule's row: a switch, and its options when it has any.
fn rule_row(
    rule: &'static dyn Rule,
    enabled: bool,
    options: &igneous_lint::Options,
    update: &Update,
) -> gtk::Widget {
    let id = rule.id();
    let set_enabled = {
        let update = update.clone();
        move |on: bool| {
            update(&|s| {
                s.rules.entry(id.to_owned()).or_default().enabled = on;
            })
        }
    };
    if rule.options().is_empty() {
        let row = adw::SwitchRow::builder()
            .title(glib::markup_escape_text(rule.name()))
            .subtitle(glib::markup_escape_text(rule.description()))
            .active(enabled)
            .build();
        row.set_widget_name(id);
        row.connect_active_notify(move |row| set_enabled(row.is_active()));
        return row.upcast();
    }
    let row = adw::ExpanderRow::builder()
        .title(glib::markup_escape_text(rule.name()))
        .subtitle(glib::markup_escape_text(rule.description()))
        .show_enable_switch(true)
        .enable_expansion(enabled)
        .build();
    row.set_widget_name(id);
    row.connect_enable_expansion_notify(move |row| set_enabled(row.enables_expansion()));
    for spec in rule.options() {
        let value = options
            .get(spec.key)
            .cloned()
            .unwrap_or_else(|| spec.default_value());
        row.add_row(&option_row(id, spec, &value, update));
    }
    row.upcast()
}

/// An editor for one option, saving into the rule's own options.
fn option_row(
    rule: &'static str,
    spec: &'static OptionSpec,
    value: &Value,
    update: &Update,
) -> gtk::Widget {
    let save = {
        let update = update.clone();
        move |value: Value| {
            update(&|s| {
                s.rules
                    .entry(rule.to_owned())
                    .or_default()
                    .options
                    .insert(spec.key.to_owned(), value.clone());
            })
        }
    };
    let title = glib::markup_escape_text(spec.name);
    let subtitle = glib::markup_escape_text(spec.description);
    match &spec.kind {
        OptionKind::Bool(_) => {
            let row = adw::SwitchRow::builder()
                .title(title)
                .subtitle(subtitle)
                .active(value.as_bool().unwrap_or(false))
                .build();
            row.connect_active_notify(move |row| save(Value::Bool(row.is_active())));
            row.upcast()
        }
        OptionKind::Number(_) => {
            let row = adw::SpinRow::with_range(0.0, 10_000.0, 1.0);
            row.set_title(&title);
            row.set_subtitle(&subtitle);
            row.set_value(value.as_f64().unwrap_or(0.0));
            row.connect_value_notify(move |row| {
                if let Some(n) = serde_json::Number::from_f64(row.value()) {
                    save(Value::Number(n));
                }
            });
            row.upcast()
        }
        OptionKind::Choice { choices, .. } => {
            let row = adw::ComboRow::builder()
                .title(title)
                .subtitle(subtitle)
                .model(&gtk::StringList::new(choices))
                .build();
            let current = value.as_str().unwrap_or_default();
            if let Some(i) = choices.iter().position(|c| *c == current) {
                row.set_selected(i as u32);
            }
            row.connect_selected_notify(move |row| {
                if let Some(choice) = choices.get(row.selected() as usize) {
                    save(Value::String((*choice).to_owned()));
                }
            });
            row.upcast()
        }
        OptionKind::Text(_) | OptionKind::List => {
            let list = matches!(spec.kind, OptionKind::List);
            let text = match value {
                Value::Array(items) => items
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", "),
                Value::String(s) => s.clone(),
                _ => String::new(),
            };
            let row = adw::EntryRow::builder()
                .title(title)
                .text(text)
                .show_apply_button(true)
                .build();
            if !spec.description.is_empty() {
                row.set_tooltip_text(Some(spec.description));
            }
            row.connect_apply(move |row| {
                let text = row.text().to_string();
                save(if list {
                    Value::Array(split_list(&text).into_iter().map(Value::String).collect())
                } else {
                    Value::String(text)
                });
            });
            row.upcast()
        }
    }
}

fn split_list(text: &str) -> Vec<String> {
    text.split([',', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists() {
        assert_eq!(
            split_list(" Templates, Archive/Old ,,\nDaily "),
            ["Templates", "Archive/Old", "Daily"]
        );
        assert!(split_list("  ").is_empty());
    }
}
