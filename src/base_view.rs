//! Running bases and drawing their values, shared by the Bases tab and the
//! bases embedded in notes (`![[File.base]]` and ` ```base ` blocks).

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use igneous_core::VaultPath;
use igneous_core::fs::read_text;
use igneous_query::bases::{self, BaseFile, Cell, Value, ViewData, ViewResult};

use crate::index::IndexService;
use crate::query_data::note_data;
use crate::window::Window;

/// Runs view `view` of `base` over every file in the vault, on the index's
/// thread. `this` is the file showing the base.
pub async fn run_view(
    index: &IndexService,
    base: BaseFile,
    view: usize,
    this: Option<VaultPath>,
) -> Option<ViewResult> {
    index
        .query(move |index| {
            let notes: Vec<_> = index.note_rows().ok()?.into_iter().map(note_data).collect();
            let this = this.and_then(|path| notes.iter().find(|n| n.path == path));
            Some(bases::run(&base, view, &notes, this))
        })
        .await
        .flatten()
}

/// What a click on a value leads to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    File(VaultPath),
    Url(String),
    /// A link to a note that doesn't exist yet.
    Missing(String),
}

/// Opens a target; the flag asks for a new tab.
pub type Opener = Rc<dyn Fn(&Target, bool)>;

/// An opener that goes through `window`.
pub fn opener(window: glib::WeakRef<Window>) -> Opener {
    Rc::new(move |target, new_tab| {
        let Some(window) = window.upgrade() else {
            return;
        };
        match target {
            Target::File(path) => window.open_path(path, new_tab),
            Target::Url(url) => {
                gtk::UriLauncher::new(url).launch(
                    Some(&window),
                    gtk::gio::Cancellable::NONE,
                    |_| {},
                );
            }
            Target::Missing(name) => {
                let link = igneous_markdown::LinkRef {
                    target: name.clone(),
                    ..igneous_markdown::LinkRef::default()
                };
                window.follow_link(&link, None, new_tab);
            }
        }
    })
}

fn link_target(link: &bases::Link) -> Target {
    match &link.path {
        Some(path) => Target::File(path.clone()),
        None if link.target.contains("://") || link.target.starts_with("mailto:") => {
            Target::Url(link.target.clone())
        }
        None => Target::Missing(link.target.clone()),
    }
}

fn file_title(path: &VaultPath) -> String {
    crate::files::display_name(path, false).0
}

/// A label showing `text` as a link that opens `target`.
fn link_label(text: &str, target: Target, opener: &Opener) -> gtk::Label {
    let label = gtk::Label::builder()
        .use_markup(true)
        .label(format!(
            "<a href=\"igneous:\">{}</a>",
            glib::markup_escape_text(text)
        ))
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    if matches!(target, Target::Missing(_)) {
        label.add_css_class("dim-label");
    }
    let opener = opener.clone();
    label.connect_activate_link(move |label, _| {
        let new_tab = WidgetExt::display(label)
            .default_seat()
            .and_then(|s| s.keyboard())
            .is_some_and(|k| {
                k.modifier_state()
                    .contains(gtk::gdk::ModifierType::CONTROL_MASK)
            });
        opener(&target, new_tab);
        glib::Propagation::Stop
    });
    label
}

fn text_label(text: &str, xalign: f32) -> gtk::Widget {
    gtk::Label::builder()
        .label(text)
        .xalign(xalign)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build()
        .upcast()
}

/// A widget showing one value.
pub fn value_widget(value: &Value, opener: &Opener) -> gtk::Widget {
    match value {
        Value::Null => text_label("", 0.0),
        Value::Bool(checked) => gtk::CheckButton::builder()
            .active(*checked)
            .can_target(false)
            .focusable(false)
            .halign(gtk::Align::Start)
            .build()
            .upcast(),
        Value::Number(_) => text_label(&value.display(), 1.0),
        Value::Link(link) => link_label(&link.display_text(), link_target(link), opener).upcast(),
        Value::File(path) => {
            link_label(&file_title(path), Target::File(path.clone()), opener).upcast()
        }
        Value::List(items) => {
            let wrap = adw::WrapBox::builder()
                .child_spacing(4)
                .line_spacing(4)
                .build();
            for item in items {
                let pill = match item {
                    Value::Link(_) | Value::File(_) => value_widget(item, opener),
                    _ => text_label(&item.display(), 0.0),
                };
                pill.add_css_class("base-pill");
                wrap.append(&pill);
            }
            wrap.upcast()
        }
        _ => text_label(&value.display(), 0.0),
    }
}

/// A widget showing one cell: its value, or why it has none.
pub fn cell_widget(cell: &Cell, opener: &Opener) -> gtk::Widget {
    match cell {
        Ok(value) => value_widget(value, opener),
        Err(e) => {
            let label = gtk::Label::builder()
                .label("Error")
                .xalign(0.0)
                .tooltip_text(e.to_string())
                .css_classes(["error"])
                .build();
            label.upcast()
        }
    }
}

/// A widget for cell `column` of row `row`. A file's name links to it.
pub fn row_cell_widget(data: &ViewData, row: usize, column: usize, opener: &Opener) -> gtk::Widget {
    let row = &data.rows[row];
    let cell = &row.cells[column];
    let id = data.columns[column].id.as_str();
    if let (Ok(Value::String(name)), "file.name" | "file.basename") = (cell, id) {
        return link_label(name, Target::File(row.file.clone()), opener).upcast();
    }
    cell_widget(cell, opener)
}

/// The text for a group's heading.
pub fn group_title(key: &Value) -> String {
    match key {
        Value::Null => "None".to_owned(),
        Value::File(path) => file_title(path),
        Value::Link(link) => link.display_text(),
        other => other.display(),
    }
}

/// "12 results", or "10 of 12 results" when the view has a limit.
pub fn count_text(data: &ViewData) -> String {
    let shown = data.rows.len();
    let noun = |n: usize| if n == 1 { "result" } else { "results" };
    if shown < data.total {
        format!("{shown} of {} {}", data.total, noun(data.total))
    } else {
        format!("{shown} {}", noun(shown))
    }
}

/// How many rows an embedded base shows before saying how many more there
/// are.
const EMBED_ROWS: usize = 25;

/// A compact, read-only table of a view, for embedding in notes.
pub fn compact_table(data: &ViewData, opener: &Opener) -> gtk::Widget {
    let grid = gtk::Grid::builder()
        .css_classes(["md-table", "base-embed-table"])
        .halign(gtk::Align::Fill)
        .build();
    for (c, column) in data.columns.iter().enumerate() {
        let head = gtk::Box::builder().css_classes(["md-table-cell"]).build();
        head.append(
            &gtk::Label::builder()
                .label(&column.name)
                .xalign(0.0)
                .hexpand(true)
                .css_classes(["md-table-head"])
                .build(),
        );
        grid.attach(&head, c as i32, 0, 1, 1);
    }
    for (r, row) in data.rows.iter().take(EMBED_ROWS).enumerate() {
        for c in 0..row.cells.len() {
            let holder = gtk::Box::builder().css_classes(["md-table-cell"]).build();
            let widget = row_cell_widget(data, r, c, opener);
            widget.set_hexpand(true);
            holder.append(&widget);
            grid.attach(&holder, c as i32, r as i32 + 1, 1, 1);
        }
    }
    let content = gtk::Box::new(gtk::Orientation::Vertical, 4);
    content.append(&grid);
    let mut footer = count_text(data);
    if data.rows.len() > EMBED_ROWS {
        footer = format!("{footer} (showing {EMBED_ROWS})");
    }
    content.append(
        &gtk::Label::builder()
            .label(footer)
            .xalign(0.0)
            .css_classes(["caption", "dim-label"])
            .build(),
    );
    content.upcast()
}

/// Where an embedded base comes from.
pub enum EmbedSource {
    /// `![[File.base]]` or `![[File.base#View]]`.
    File {
        path: VaultPath,
        view: Option<String>,
    },
    /// A ` ```base ` block's body.
    Block(String),
}

fn message(text: &str) -> gtk::Widget {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .css_classes(["dim-label"])
        .margin_top(8)
        .margin_bottom(8)
        .build()
        .upcast()
}

/// A base embedded in a note: it runs once it's first shown (when it can
/// find its window) and then fills itself in. `this` is the embedding note.
pub fn embed(source: EmbedSource, this: Option<VaultPath>) -> gtk::Widget {
    let holder = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["base-embed"])
        .build();
    holder.append(&adw::Spinner::builder().height_request(32).build());
    let source = std::cell::RefCell::new(Some(source));
    holder.connect_map(move |holder| {
        let Some(window) = holder.root().and_downcast::<Window>() else {
            return;
        };
        if let Some(source) = source.take() {
            load_embed(holder, &window, source, this.clone());
        }
    });
    holder.upcast()
}

fn load_embed(holder: &gtk::Box, window: &Window, source: EmbedSource, this: Option<VaultPath>) {
    let index = window.index().clone();
    let opener = opener(window.downgrade());
    let ctx_root = Window::root(window);
    let weak = holder.downgrade();
    glib::spawn_future_local(async move {
        let (text, view_name) = match source {
            EmbedSource::File { path, view } => match read_text(&path.to_fs(&ctx_root)) {
                Ok((file, _)) => (file.text().to_owned(), view),
                Err(e) => {
                    show(&weak, message(&format!("Couldn’t read “{path}”: {e}")));
                    return;
                }
            },
            EmbedSource::Block(text) => (text, None),
        };
        let base = match BaseFile::parse(&text) {
            Ok(base) => base,
            Err(e) => {
                show(&weak, message(&format!("This base can’t be read: {e}")));
                return;
            }
        };
        let view = match &view_name {
            Some(name) => match base.view_index(name) {
                Some(i) => i,
                None => {
                    show(
                        &weak,
                        message(&format!("This base has no view called “{name}”")),
                    );
                    return;
                }
            },
            None => 0,
        };
        let result = run_view(&index, base, view, this).await;
        let widget = match result {
            Some(ViewResult::Ready(data)) => compact_table(&data, &opener),
            Some(ViewResult::Unsupported(kind)) => {
                message(&format!("“{kind}” views aren’t supported yet"))
            }
            Some(ViewResult::NoSuchView) => message("This base has no views"),
            None => message("The vault’s index isn’t available"),
        };
        show(&weak, widget);
    });
}

fn show(holder: &glib::WeakRef<gtk::Box>, widget: gtk::Widget) {
    let Some(holder) = holder.upgrade() else {
        return;
    };
    while let Some(child) = holder.first_child() {
        holder.remove(&child);
    }
    holder.append(&widget);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_titles() {
        assert_eq!(group_title(&Value::Null), "None");
        assert_eq!(group_title(&Value::String("Done".into())), "Done");
        let path = VaultPath::new("Projects/Plan.md").unwrap();
        assert_eq!(group_title(&Value::File(path)), "Plan");
    }
}
