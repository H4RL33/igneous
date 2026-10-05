//! The properties header shown in place of a note's frontmatter: one row per
//! property with an editor for its type. Every change is a minimal edit to
//! the YAML (igneous_markdown::frontmatter) and one undo step; the rest of
//! the frontmatter, comments included, is left as it is.

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use igneous_markdown::TextEdit;
use igneous_markdown::frontmatter::{self, Frontmatter, Value};

use crate::view::NoteView;

/// How a property is edited, as in Obsidian.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropertyKind {
    Text,
    List,
    Number,
    Checkbox,
    Date,
    DateTime,
    Tags,
    Aliases,
}

impl PropertyKind {
    pub const ALL: [PropertyKind; 8] = [
        PropertyKind::Text,
        PropertyKind::List,
        PropertyKind::Number,
        PropertyKind::Checkbox,
        PropertyKind::Date,
        PropertyKind::DateTime,
        PropertyKind::Tags,
        PropertyKind::Aliases,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PropertyKind::Text => "Text",
            PropertyKind::List => "List",
            PropertyKind::Number => "Number",
            PropertyKind::Checkbox => "Checkbox",
            PropertyKind::Date => "Date",
            PropertyKind::DateTime => "Date & Time",
            PropertyKind::Tags => "Tags",
            PropertyKind::Aliases => "Aliases",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            PropertyKind::Text => "format-justify-left-symbolic",
            PropertyKind::List | PropertyKind::Aliases => "view-list-bullet-symbolic",
            PropertyKind::Number => "accessories-calculator-symbolic",
            PropertyKind::Checkbox => "checkbox-checked-symbolic",
            PropertyKind::Date | PropertyKind::DateTime => "x-office-calendar-symbolic",
            PropertyKind::Tags => "tag-symbolic",
        }
    }

    /// The kind a value looks like, for properties the vault has no type for.
    pub fn infer(key: &str, value: &Value) -> PropertyKind {
        match key {
            "tags" | "tag" => return PropertyKind::Tags,
            "aliases" | "alias" => return PropertyKind::Aliases,
            "cssclasses" => return PropertyKind::List,
            _ => {}
        }
        match value {
            Value::Bool(_) => PropertyKind::Checkbox,
            Value::Int(_) | Value::Float(_) => PropertyKind::Number,
            Value::List(_) => PropertyKind::List,
            Value::String(s) if is_date(s) => PropertyKind::Date,
            Value::String(s) if is_datetime(s) => PropertyKind::DateTime,
            _ => PropertyKind::Text,
        }
    }
}

fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

fn is_datetime(s: &str) -> bool {
    s.len() >= 16 && is_date(&s[..10]) && matches!(s.as_bytes()[10], b'T' | b' ')
}

/// The text of a value as shown in an entry.
fn plain(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::String(s) => s.clone(),
        Value::List(_) => value.string_list().join(", "),
        Value::Map(_) => "{…}".to_owned(),
    }
}

impl NoteView {
    /// Applies edits to the text as one undoable step.
    pub(crate) fn apply_edits(&self, edits: &[TextEdit]) {
        if edits.is_empty() {
            return;
        }
        let text = self.model_text();
        let chars = |byte: usize| text[..byte].chars().count() as i32;
        let mut sorted: Vec<&TextEdit> = edits.iter().collect();
        sorted.sort_by_key(|e| std::cmp::Reverse(e.range.start));
        let buffer = self.buffer();
        buffer.begin_user_action();
        for edit in sorted {
            let mut start = buffer.iter_at_offset(chars(edit.range.start));
            let mut end = buffer.iter_at_offset(chars(edit.range.end));
            buffer.delete(&mut start, &mut end);
            buffer.insert(&mut start, &edit.insert);
        }
        buffer.end_user_action();
    }

    /// Sets a property, adding the frontmatter or the key if needed.
    pub fn set_property(&self, key: &str, value: &Value) {
        if let Ok(edits) = frontmatter::set(&self.model_text(), key, value) {
            self.apply_edits(&edits);
        }
    }

    pub fn remove_property(&self, key: &str) {
        if let Ok(edits) = frontmatter::remove(&self.model_text(), key) {
            self.apply_edits(&edits);
        }
    }

    fn property_kind(&self, key: &str, value: &Value) -> PropertyKind {
        self.host()
            .property_kind(key)
            .unwrap_or_else(|| PropertyKind::infer(key, value))
    }

    /// The properties, or with no frontmatter just Add Property.
    pub(crate) fn properties_widget_for(&self, fm: Option<&Frontmatter>) -> gtk::Widget {
        let list = self.properties_list(fm);
        // Its own box, so the list's rows have had a click before it's kept
        // from the note.
        let block = gtk::Box::new(gtk::Orientation::Vertical, 0);
        block.append(&list);
        crate::live::keep_clicks(&block);
        block.upcast()
    }

    fn properties_list(&self, fm: Option<&Frontmatter>) -> gtk::ListBox {
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list", "properties"])
            .build();
        let Some(fm) = fm else {
            list.append(&self.add_property_row(&[]));
            return list;
        };
        if let Some(error) = &fm.error {
            let row = adw::ActionRow::builder()
                .title("The properties aren’t valid YAML")
                .subtitle(glib::markup_escape_text(error))
                .css_classes(["error"])
                .build();
            row.add_prefix(&gtk::Image::from_icon_name("dialog-warning-symbolic"));
            let source = gtk::Button::builder()
                .label("Edit in Source Mode")
                .valign(gtk::Align::Center)
                .build();
            source.connect_clicked(glib::clone!(
                #[weak(rename_to = view)]
                self,
                move |_| view.set_mode(crate::Mode::Source)
            ));
            row.add_suffix(&source);
            list.append(&row);
            return list;
        }
        for entry in &fm.entries {
            let kind = self.property_kind(&entry.key, &entry.value);
            list.append(&self.property_row(&entry.key, &entry.value, kind));
        }
        let keys: Vec<&str> = fm.entries.iter().map(|e| e.key.as_str()).collect();
        list.append(&self.add_property_row(&keys));
        list
    }

    fn property_row(&self, key: &str, value: &Value, kind: PropertyKind) -> gtk::Widget {
        let row: gtk::Widget = match (kind, value) {
            (_, Value::Map(_)) => adw::ActionRow::builder()
                .title(glib::markup_escape_text(key))
                .subtitle("A nested map, editable in Source mode")
                .build()
                .upcast(),
            (PropertyKind::Checkbox, _) => {
                let row = adw::SwitchRow::builder()
                    .title(glib::markup_escape_text(key))
                    .active(matches!(value, Value::Bool(true)))
                    .build();
                let key = key.to_owned();
                row.connect_active_notify(glib::clone!(
                    #[weak(rename_to = view)]
                    self,
                    move |row| view.set_property(&key, &Value::Bool(row.is_active()))
                ));
                row.upcast()
            }
            (PropertyKind::Number, _) => {
                let current = match value {
                    Value::Int(i) => *i as f64,
                    Value::Float(f) => *f,
                    Value::String(s) => s.parse().unwrap_or(0.0),
                    _ => 0.0,
                };
                let row = adw::SpinRow::with_range(-1e12, 1e12, 1.0);
                row.set_title(&glib::markup_escape_text(key));
                row.set_digits(if matches!(value, Value::Float(_)) {
                    2
                } else {
                    0
                });
                row.set_value(current);
                let key = key.to_owned();
                row.connect_changed(glib::clone!(
                    #[weak(rename_to = view)]
                    self,
                    move |row| {
                        let v = row.value();
                        let value = if v.fract() == 0.0 && row.digits() == 0 {
                            Value::Int(v as i64)
                        } else {
                            Value::Float(v)
                        };
                        view.set_property(&key, &value);
                    }
                ));
                row.upcast()
            }
            (PropertyKind::Date | PropertyKind::DateTime, _) => {
                self.date_row(key, &plain(value), kind == PropertyKind::DateTime)
            }
            (PropertyKind::List | PropertyKind::Tags | PropertyKind::Aliases, _) => {
                self.list_row(key, value.string_list(), kind)
            }
            (PropertyKind::Text, _) => {
                let row = adw::EntryRow::builder()
                    .title(glib::markup_escape_text(key))
                    .text(plain(value))
                    .show_apply_button(true)
                    .build();
                let key = key.to_owned();
                row.connect_apply(glib::clone!(
                    #[weak(rename_to = view)]
                    self,
                    move |row| view.set_property(&key, &Value::String(row.text().to_string()))
                ));
                row.upcast()
            }
        };
        row.set_tooltip_text(Some(kind.label()));
        if let Some(row) = row.downcast_ref::<adw::PreferencesRow>() {
            // Each row can change type or be removed.
            let menu = self.row_menu(key, kind);
            if let Some(action_row) = row.downcast_ref::<adw::ActionRow>() {
                action_row.add_prefix(&gtk::Image::from_icon_name(kind.icon()));
                action_row.add_suffix(&menu);
            } else if let Some(entry_row) = row.downcast_ref::<adw::EntryRow>() {
                entry_row.add_prefix(&gtk::Image::from_icon_name(kind.icon()));
                entry_row.add_suffix(&menu);
            }
        }
        row
    }

    fn row_menu(&self, key: &str, current: PropertyKind) -> gtk::MenuButton {
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .build();
        let popover = gtk::Popover::builder().child(&content).build();
        for kind in PropertyKind::ALL {
            let button = gtk::Button::builder()
                .child(
                    &gtk::Label::builder()
                        .label(if kind == current {
                            format!("✓ {}", kind.label())
                        } else {
                            kind.label().to_owned()
                        })
                        .xalign(0.0)
                        .build(),
                )
                .css_classes(["flat"])
                .build();
            let key = key.to_owned();
            button.connect_clicked(glib::clone!(
                #[weak(rename_to = view)]
                self,
                #[weak]
                popover,
                move |_| {
                    popover.popdown();
                    view.host().set_property_kind(&key, kind);
                    view.rebuild_overlays();
                }
            ));
            content.append(&button);
        }
        content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let remove = gtk::Button::builder()
            .child(&gtk::Label::builder().label("Remove").xalign(0.0).build())
            .css_classes(["flat", "error"])
            .build();
        let key = key.to_owned();
        remove.connect_clicked(glib::clone!(
            #[weak(rename_to = view)]
            self,
            #[weak]
            popover,
            move |_| {
                popover.popdown();
                view.remove_property(&key);
            }
        ));
        content.append(&remove);
        gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text("Property Options")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .popover(&popover)
            .build()
    }

    fn date_row(&self, key: &str, current: &str, with_time: bool) -> gtk::Widget {
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(key))
            .subtitle(if current.is_empty() {
                "No date".to_owned()
            } else {
                glib::markup_escape_text(current).to_string()
            })
            .build();
        let calendar = gtk::Calendar::new();
        if is_date(current.get(..10).unwrap_or("")) {
            let (y, m, d) = (
                current[..4].parse().unwrap_or(2026),
                current[5..7].parse().unwrap_or(1),
                current[8..10].parse().unwrap_or(1),
            );
            if let Ok(date) = glib::DateTime::from_local(y, m, d, 0, 0, 0.0) {
                calendar.set_date(&date);
            }
        }
        let time = gtk::Entry::builder()
            .placeholder_text("HH:MM")
            .text(current.get(11..16).unwrap_or(""))
            .visible(with_time)
            .max_width_chars(6)
            .build();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        content.append(&calendar);
        content.append(&time);
        let popover = gtk::Popover::builder().child(&content).build();
        let pick = gtk::MenuButton::builder()
            .icon_name("x-office-calendar-symbolic")
            .tooltip_text("Pick a Date")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .popover(&popover)
            .build();
        row.add_suffix(&pick);
        let key = key.to_owned();
        let apply = glib::clone!(
            #[weak(rename_to = view)]
            self,
            #[weak]
            calendar,
            #[weak]
            time,
            move || {
                let date = calendar.date();
                let mut text = format!(
                    "{:04}-{:02}-{:02}",
                    date.year(),
                    date.month(),
                    date.day_of_month()
                );
                let t = time.text();
                if with_time {
                    text.push('T');
                    text.push_str(if t.len() == 5 { t.as_str() } else { "00:00" });
                }
                view.set_property(&key, &Value::String(text));
            }
        );
        let on_day = apply.clone();
        calendar.connect_day_selected(move |_| on_day());
        time.connect_activate(move |_| apply());
        row.upcast()
    }

    fn list_row(&self, key: &str, items: Vec<String>, kind: PropertyKind) -> gtk::Widget {
        let title = gtk::Label::builder()
            .label(key)
            .xalign(0.0)
            .css_classes(["caption", "dim-label"])
            .build();
        let pills = adw::WrapBox::builder()
            .child_spacing(6)
            .line_spacing(6)
            .build();
        for (i, item) in items.iter().enumerate() {
            let label = if kind == PropertyKind::Tags {
                format!("#{}", item.trim_start_matches('#'))
            } else {
                item.clone()
            };
            let pill = gtk::Box::builder()
                .spacing(2)
                .css_classes(["property-pill"])
                .build();
            pill.append(&gtk::Label::new(Some(&label)));
            let remove = gtk::Button::builder()
                .icon_name("window-close-symbolic")
                .tooltip_text(format!("Remove {label}"))
                .css_classes(["flat", "circular"])
                .build();
            let key = key.to_owned();
            let rest: Vec<String> = items
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, v)| v.clone())
                .collect();
            remove.connect_clicked(glib::clone!(
                #[weak(rename_to = view)]
                self,
                move |_| {
                    let values = rest.iter().cloned().map(Value::String).collect();
                    view.set_property(&key, &Value::List(values));
                }
            ));
            pill.append(&remove);
            pills.append(&pill);
        }
        let entry = gtk::Entry::builder()
            .placeholder_text("Add…")
            .max_width_chars(12)
            .css_classes(["flat"])
            .build();
        let key = key.to_owned();
        entry.connect_activate(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |entry| {
                let added = entry.text().trim().trim_start_matches('#').to_owned();
                if added.is_empty() {
                    return;
                }
                let mut values: Vec<Value> = items.iter().cloned().map(Value::String).collect();
                values.push(Value::String(added));
                view.set_property(&key, &Value::List(values));
            }
        ));
        pills.append(&entry);
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(12)
            .margin_end(12)
            .build();
        content.append(&title);
        content.append(&pills);
        gtk::ListBoxRow::builder()
            .child(&content)
            .activatable(false)
            .build()
            .upcast()
    }

    /// Add Property: a menu of the property names the vault uses (but this
    /// note doesn't), then New Property…, which turns the row into a field
    /// for a new name.
    fn add_property_row(&self, existing: &[&str]) -> gtk::Widget {
        let row = adw::ActionRow::builder()
            .title("Add Property")
            .activatable(true)
            .build();
        row.add_prefix(&gtk::Image::from_icon_name("list-add-symbolic"));
        let actions = AddPropertyActions::new(self, &row, existing);
        row.insert_action_group("property", Some(&actions));

        // The menu button owns the menu; activating the row opens it. The
        // vault's names are one menu that every note shares, so the menu is
        // only set up the first time it opens.
        let button = gtk::MenuButton::builder()
            .icon_name("pan-down-symbolic")
            .valign(gtk::Align::Center)
            .tooltip_text("Add Property")
            .css_classes(["flat"])
            .build();
        button.set_create_popup_func(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |button| {
                if button.menu_model().is_none() {
                    button.set_menu_model(Some(&view.add_property_menu()));
                }
            }
        ));
        row.add_suffix(&button);
        row.set_activatable_widget(Some(&button));
        row.upcast()
    }

    /// The vault's property names (those the note has are hidden), then New
    /// Property…, then a way to remake the names from the notes.
    fn add_property_menu(&self) -> gio::Menu {
        let new = gio::Menu::new();
        new.append(Some("_New Property…"), Some("property.new"));
        let refresh = gio::Menu::new();
        refresh.append(Some("_Refresh Property Names"), Some("property.refresh"));
        let menu = gio::Menu::new();
        if let Some(names) = self.host().property_names() {
            menu.append_section(None, &names);
        }
        menu.append_section(None, &new);
        menu.append_section(None, &refresh);
        menu
    }

    /// Swaps Add Property for a field to name a new property: Enter adds
    /// it, Escape (or leaving it empty) goes back.
    fn name_new_property(&self, row: &adw::ActionRow) {
        let Some(list) = row.parent().and_downcast::<gtk::ListBox>() else {
            return;
        };
        let entry = adw::EntryRow::builder()
            .title("Property Name")
            .show_apply_button(true)
            .build();
        list.insert(&entry, row.index());
        row.set_visible(false);
        let back = {
            let (row, entry) = (row.downgrade(), entry.downgrade());
            move || {
                if let Some(entry) = entry.upgrade() {
                    if let Some(list) = entry.parent().and_downcast::<gtk::ListBox>() {
                        list.remove(&entry);
                    }
                }
                if let Some(row) = row.upgrade() {
                    row.set_visible(true);
                    row.grab_focus();
                }
            }
        };
        let back = std::rc::Rc::new(back);
        entry.connect_apply(glib::clone!(
            #[weak(rename_to = view)]
            self,
            #[strong]
            back,
            move |entry| {
                let key = entry.text().trim().trim_end_matches(':').to_owned();
                if key.is_empty() {
                    back();
                } else {
                    view.set_property(&key, &Value::Null);
                    view.host().add_property_name(&key);
                }
            }
        ));
        entry.connect_entry_activated(|entry| entry.emit_by_name::<()>("apply", &[]));
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(glib::clone!(
            #[strong]
            back,
            move |_, key, _, _| {
                if key == gdk::Key::Escape {
                    back();
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            }
        ));
        entry.add_controller(keys);
        entry.grab_focus();
    }
}

/// An item for the vault's property names menu (see
/// [`Host::property_names`](crate::Host::property_names)): it adds `key` to
/// the note, and is hidden in notes that already have it.
pub fn property_name_item(key: &str) -> gio::MenuItem {
    let item = gio::MenuItem::new(
        Some(&key.replace('_', "__")),
        Some(&format!("property.{}", add_action(key))),
    );
    item.set_attribute_value("hidden-when", Some(&"action-missing".to_variant()));
    item
}

/// The action that adds `key`: its bytes in hex, as an action's name can't
/// hold every character a key can.
fn add_action(key: &str) -> String {
    use std::fmt::Write;
    key.bytes().fold(String::from("add-"), |mut name, b| {
        let _ = write!(name, "{b:02x}");
        name
    })
}

fn key_of_action(name: &str) -> Option<String> {
    let hex = name.strip_prefix("add-")?;
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    String::from_utf8(bytes).ok()
}

glib::wrapper! {
    /// Add Property's actions for one note: `new`, `refresh`, and `add-…` for each of
    /// the vault's names the note doesn't have. They're answered from the
    /// name, so nothing is made per name.
    pub struct AddPropertyActions(ObjectSubclass<imp::AddPropertyActions>)
        @implements gio::ActionGroup;
}

impl AddPropertyActions {
    pub(crate) fn new(view: &NoteView, row: &adw::ActionRow, existing: &[&str]) -> Self {
        use glib::subclass::prelude::ObjectSubclassIsExt;
        let actions: Self = glib::Object::new();
        let imp = actions.imp();
        imp.view.set(Some(view));
        imp.row.set(Some(row));
        imp.existing
            .replace(existing.iter().map(|k| k.to_string()).collect());
        actions
    }
}

mod imp {
    use std::cell::RefCell;

    use gtk::subclass::prelude::*;

    use super::*;

    #[derive(Default)]
    pub struct AddPropertyActions {
        pub view: glib::WeakRef<NoteView>,
        pub row: glib::WeakRef<adw::ActionRow>,
        /// The note's keys.
        pub existing: RefCell<Vec<String>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AddPropertyActions {
        const NAME: &'static str = "IgneousAddPropertyActions";
        type Type = super::AddPropertyActions;
        type Interfaces = (gio::ActionGroup,);
    }

    impl ObjectImpl for AddPropertyActions {}

    impl AddPropertyActions {
        /// The key `name` adds, if the note doesn't have it yet.
        fn key(&self, name: &str) -> Option<String> {
            key_of_action(name).filter(|key| !self.existing.borrow().contains(key))
        }
    }

    impl ActionGroupImpl for AddPropertyActions {
        fn list_actions(&self) -> Vec<String> {
            let mut names = vec!["new".to_owned(), "refresh".to_owned()];
            let Some(model) = self.view.upgrade().and_then(|v| v.host().property_names()) else {
                return names;
            };
            for i in 0..model.n_items() {
                let action = model
                    .item_attribute_value(i, "action", Some(glib::VariantTy::STRING))
                    .and_then(|a| a.get::<String>());
                if let Some(name) = action.as_deref().and_then(|a| a.strip_prefix("property."))
                    && self.key(name).is_some()
                {
                    names.push(name.to_owned());
                }
            }
            names
        }

        fn query_action(
            &self,
            name: &str,
        ) -> Option<(
            bool,
            Option<glib::VariantType>,
            Option<glib::VariantType>,
            Option<glib::Variant>,
            Option<glib::Variant>,
        )> {
            (name == "new" || name == "refresh" || self.key(name).is_some())
                .then_some((true, None, None, None, None))
        }

        fn activate_action(&self, name: &str, _parameter: Option<&glib::Variant>) {
            let Some(view) = self.view.upgrade() else {
                return;
            };
            if name == "new" {
                if let Some(row) = self.row.upgrade() {
                    view.name_new_property(&row);
                }
            } else if name == "refresh" {
                view.host().refresh_property_names();
            } else if let Some(key) = self.key(name) {
                view.set_property(&key, &Value::Null);
            }
        }

        fn change_action_state(&self, _name: &str, _value: &glib::Variant) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_from_values() {
        let k = PropertyKind::infer;
        assert_eq!(k("tags", &Value::String("a".into())), PropertyKind::Tags);
        assert_eq!(k("done", &Value::Bool(true)), PropertyKind::Checkbox);
        assert_eq!(k("n", &Value::Int(3)), PropertyKind::Number);
        assert_eq!(
            k("d", &Value::String("2026-10-04".into())),
            PropertyKind::Date
        );
        assert_eq!(
            k("d", &Value::String("2026-10-04T09:30".into())),
            PropertyKind::DateTime
        );
        assert_eq!(k("x", &Value::List(vec![])), PropertyKind::List);
        assert_eq!(k("x", &Value::String("words".into())), PropertyKind::Text);
    }

    #[test]
    fn add_actions_name_any_key() {
        for key in ["tags", "due date", "naïve_key", "a.b-c"] {
            let action = add_action(key);
            assert!(gio::Action::name_is_valid(&action), "{action}");
            assert_eq!(key_of_action(&action).as_deref(), Some(key));
        }
        assert_eq!(key_of_action("add-7"), None);
        assert_eq!(key_of_action("new"), None);
    }

    fn view(text: &str) -> NoteView {
        let view = NoteView::new();
        view.source_buffer().set_text(text);
        view
    }

    #[gtk::test]
    fn edits_are_minimal_and_undoable() {
        let text = "---\ntitle: Old # keep this comment\ntags:\n  - a\n---\nbody\n";
        let view = view(text);
        view.set_property("title", &Value::String("New".into()));
        assert_eq!(
            view.model_text(),
            "---\ntitle: New # keep this comment\ntags:\n  - a\n---\nbody\n"
        );
        view.set_property("done", &Value::Bool(true));
        assert!(view.model_text().contains("done: true\n---\nbody"));
        view.remove_property("tags");
        assert!(!view.model_text().contains("tags"));
        let buffer = view.source_buffer();
        buffer.undo();
        buffer.undo();
        buffer.undo();
        assert_eq!(view.model_text(), text);
        view.check_invariants().unwrap();
    }

    #[gtk::test]
    fn adds_frontmatter_when_missing() {
        let view = view("just text\n");
        view.set_property("status", &Value::String("draft".into()));
        assert!(view.model_text().starts_with("---\nstatus: draft\n---\n"));
    }
}
