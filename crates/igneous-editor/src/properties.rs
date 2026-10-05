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

    /// A name for the kind in actions.
    fn id(self) -> &'static str {
        match self {
            PropertyKind::Text => "text",
            PropertyKind::List => "list",
            PropertyKind::Number => "number",
            PropertyKind::Checkbox => "checkbox",
            PropertyKind::Date => "date",
            PropertyKind::DateTime => "datetime",
            PropertyKind::Tags => "tags",
            PropertyKind::Aliases => "aliases",
        }
    }

    fn from_id(id: &str) -> Option<PropertyKind> {
        PropertyKind::ALL.into_iter().find(|k| k.id() == id)
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
        // Activating Add Property (a click anywhere on it, or Enter) opens
        // its menu.
        list.connect_row_activated(|_, row| {
            if row.has_css_class("add-property")
                && let Some(button) =
                    crate::live::find_descendant(row.upcast_ref(), &|w| w.is::<gtk::MenuButton>())
                        .and_downcast::<gtk::MenuButton>()
            {
                button.popup();
            }
        });
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
        // The names line up, as wide as the widest (up to a point).
        let names = gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal);
        for entry in &fm.entries {
            let kind = self.property_kind(&entry.key, &entry.value);
            list.append(&self.property_row(&entry.key, &entry.value, kind, &names));
        }
        let keys: Vec<&str> = fm.entries.iter().map(|e| e.key.as_str()).collect();
        list.append(&self.add_property_row(&keys));
        list
    }

    /// One property: its name on the left, then a separator, then its
    /// value.
    fn property_row(
        &self,
        key: &str,
        value: &Value,
        kind: PropertyKind,
        names: &gtk::SizeGroup,
    ) -> gtk::Widget {
        let value_widget = match (kind, value) {
            (_, Value::Map(_)) => gtk::Label::builder()
                .label("A nested map, editable in Source mode")
                .xalign(0.0)
                .css_classes(["dim-label"])
                .build()
                .upcast(),
            (PropertyKind::Checkbox, _) => self.checkbox_value(key, value),
            (PropertyKind::Number, _) => self.number_value(key, value),
            (PropertyKind::Date | PropertyKind::DateTime, _) => {
                self.date_value(key, &plain(value), kind == PropertyKind::DateTime)
            }
            (PropertyKind::List | PropertyKind::Tags | PropertyKind::Aliases, _) => {
                self.list_value(key, value.string_list(), kind)
            }
            (PropertyKind::Text, _) => self.text_value(key, value),
        };
        value_widget.set_hexpand(true);
        value_widget.set_valign(gtk::Align::Center);
        value_widget.add_css_class("property-value");

        let name = self.property_name(key, kind);
        names.add_widget(&name);
        let content = gtk::Box::builder()
            .css_classes(["property-content"])
            .build();
        content.append(&name);
        content.append(
            &gtk::Separator::builder()
                .orientation(gtk::Orientation::Vertical)
                .css_classes(["property-separator"])
                .build(),
        );
        content.append(&value_widget);
        gtk::ListBoxRow::builder()
            .child(&content)
            .activatable(false)
            .css_classes(["property"])
            .build()
            .upcast()
    }

    /// A property's icon and name, which open a menu of its type, its icon
    /// and Remove; a remove button shows while the pointer is over it.
    fn property_name(&self, key: &str, kind: PropertyKind) -> gtk::Widget {
        let icon = self
            .host()
            .property_icon(key)
            .unwrap_or_else(|| kind.icon().to_owned());
        let label = gtk::Box::builder().spacing(8).build();
        label.append(&gtk::Image::from_icon_name(&icon));
        label.append(
            &gtk::Label::builder()
                .label(key)
                .xalign(0.0)
                .hexpand(true)
                .max_width_chars(18)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build(),
        );
        let button = gtk::MenuButton::builder()
            .child(&label)
            .tooltip_text(key)
            .hexpand(true)
            .css_classes(["flat", "property-key"])
            .build();
        // The menu is the same for every property; it's set up the first
        // time it opens.
        button.set_create_popup_func(|button| {
            if button.menu_model().is_none() {
                button.set_menu_model(Some(&property_menu()));
            }
        });
        let remove = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text("Remove Property")
            .valign(gtk::Align::Center)
            .action_name("prop.remove")
            .css_classes(["flat", "circular", "property-remove"])
            .build();
        // Not expanding (the button inside would make it), so the names
        // keep one width and the values take the rest.
        let name = gtk::Box::builder()
            .spacing(2)
            .width_request(150)
            .hexpand(false)
            .css_classes(["property-name"])
            .build();
        name.append(&button);
        name.append(&remove);

        let actions = gio::SimpleActionGroup::new();
        let set_kind = gio::SimpleAction::new_stateful(
            "kind",
            Some(glib::VariantTy::STRING),
            &kind.id().to_variant(),
        );
        let owned = key.to_owned();
        set_kind.connect_change_state(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |action, state| {
                let Some(kind) = state.and_then(|s| s.str()).and_then(PropertyKind::from_id) else {
                    return;
                };
                action.set_state(&kind.id().to_variant());
                view.host().set_property_kind(&owned, kind);
                // (The host shows it in every note; without one, just here.)
                view.refresh_properties();
            }
        ));
        actions.add_action(&set_kind);
        let icon = gio::SimpleAction::new("icon", None);
        let owned = key.to_owned();
        icon.connect_activate(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_, _| view.host().choose_property_icon(&owned)
        ));
        actions.add_action(&icon);
        let remove = gio::SimpleAction::new("remove", None);
        let owned = key.to_owned();
        remove.connect_activate(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_, _| view.remove_property(&owned)
        ));
        actions.add_action(&remove);
        name.insert_action_group("prop", Some(&actions));
        name.upcast()
    }

    /// Text, saved on Enter or on leaving the field.
    fn text_value(&self, key: &str, value: &Value) -> gtk::Widget {
        let original = plain(value);
        let entry = gtk::Entry::builder()
            .text(&original)
            .placeholder_text("Empty")
            .css_classes(["flat"])
            .build();
        let key = key.to_owned();
        self.save_on_leave(&entry, move |_, text| {
            (text != original).then(|| (key.clone(), Value::String(text.to_owned())))
        });
        entry.upcast()
    }

    /// A number, saved on Enter or on leaving the field if it is one.
    fn number_value(&self, key: &str, value: &Value) -> gtk::Widget {
        let original = plain(value);
        let entry = gtk::Entry::builder()
            .text(&original)
            .placeholder_text("Empty")
            .input_purpose(gtk::InputPurpose::Number)
            .css_classes(["flat"])
            .build();
        let key = key.to_owned();
        self.save_on_leave(&entry, move |_, text| {
            let text = text.trim();
            if text == original {
                return None;
            }
            let value = if text.is_empty() {
                Value::Null
            } else if let Ok(i) = text.parse::<i64>() {
                Value::Int(i)
            } else {
                Value::Float(text.parse::<f64>().ok()?)
            };
            Some((key.clone(), value))
        });
        entry.connect_changed(|entry| {
            let text = entry.text();
            let text = text.trim();
            if text.is_empty() || text.parse::<f64>().is_ok() {
                entry.remove_css_class("error");
            } else {
                entry.add_css_class("error");
            }
        });
        entry.upcast()
    }

    /// Saves what `value` makes of an entry's text, if anything, on Enter
    /// or when the focus leaves it. Saving rebuilds the properties, so it
    /// waits until GTK is done with the key or focus change.
    fn save_on_leave(
        &self,
        entry: &gtk::Entry,
        value: impl Fn(&NoteView, &str) -> Option<(String, Value)> + 'static,
    ) {
        let value = std::rc::Rc::new(value);
        let save = std::rc::Rc::new(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |text: String| {
                let view = view.downgrade();
                let value = value.clone();
                glib::idle_add_local_once(move || {
                    if let Some(view) = view.upgrade()
                        && let Some((key, value)) = value(&view, &text)
                    {
                        view.set_property(&key, &value);
                    }
                });
            }
        ));
        let on_enter = save.clone();
        entry.connect_activate(move |entry| on_enter(entry.text().to_string()));
        let focus = gtk::EventControllerFocus::new();
        focus.connect_leave(glib::clone!(
            #[weak]
            entry,
            move |_| save(entry.text().to_string())
        ));
        entry.add_controller(focus);
    }

    fn checkbox_value(&self, key: &str, value: &Value) -> gtk::Widget {
        let check = gtk::CheckButton::builder()
            .active(matches!(value, Value::Bool(true)))
            .halign(gtk::Align::Start)
            .build();
        let key = key.to_owned();
        check.connect_toggled(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |check| view.set_property(&key, &Value::Bool(check.is_active()))
        ));
        check.upcast()
    }

    /// The date, which opens a calendar (and a time, with `with_time`).
    fn date_value(&self, key: &str, current: &str, with_time: bool) -> gtk::Widget {
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
        let label = gtk::Label::builder()
            .label(if current.is_empty() {
                "No date"
            } else {
                current
            })
            .xalign(0.0)
            .build();
        if current.is_empty() {
            label.add_css_class("dim-label");
        }
        let pick = gtk::MenuButton::builder()
            .child(&label)
            .tooltip_text("Pick a Date")
            .halign(gtk::Align::Start)
            .css_classes(["flat"])
            .popover(&popover)
            .build();
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
        pick.upcast()
    }

    /// The items as pills, each with a remove button, then a field to add
    /// one.
    fn list_value(&self, key: &str, items: Vec<String>, kind: PropertyKind) -> gtk::Widget {
        let pills = adw::WrapBox::builder()
            .child_spacing(6)
            .line_spacing(4)
            .build();
        for (i, item) in items.iter().enumerate() {
            let label = if kind == PropertyKind::Tags {
                format!("#{}", item.trim_start_matches('#'))
            } else {
                item.clone()
            };
            let pill = gtk::Box::builder()
                .spacing(2)
                .valign(gtk::Align::Center)
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
        pills.upcast()
    }

    /// Add Property: a drop-down of the property names the vault uses (those
    /// the note has are greyed out), then New Property…, which turns the row
    /// into a field for a new name.
    fn add_property_row(&self, existing: &[&str]) -> gtk::Widget {
        let button = gtk::MenuButton::builder()
            .icon_name("pan-down-symbolic")
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Center)
            .tooltip_text("Add Property")
            .css_classes(["flat"])
            .build();
        // The vault's names are one menu that every note shares, so the
        // menu is only set up the first time it opens.
        button.set_create_popup_func(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |button| {
                if button.menu_model().is_none() {
                    button.set_menu_model(Some(&view.add_property_menu()));
                }
            }
        ));
        // The label gives way to a field for New Property….
        let stack = gtk::Stack::builder()
            .hexpand(true)
            .css_classes(["add-property-stack"])
            .build();
        stack.add_named(
            &gtk::Label::builder()
                .label("Add Property")
                .xalign(0.0)
                .build(),
            Some("add"),
        );
        let content = gtk::Box::builder().spacing(8).build();
        content.append(&button);
        content.append(&stack);
        let row = gtk::ListBoxRow::builder()
            .child(&content)
            .activatable(true)
            .css_classes(["property", "add-property"])
            .build();
        let actions = AddPropertyActions::new(self, &row, existing);
        row.insert_action_group("property", Some(&actions));
        row.upcast()
    }

    /// The vault's property names (those the note has are greyed out), then New
    /// Property…, then a way to remake the names from the notes.
    fn add_property_menu(&self) -> gio::Menu {
        let new = gio::Menu::new();
        new.append(Some("_New Property…"), Some("property.new"));
        // Refresh is an icon; its label is the tooltip.
        let refresh = gio::MenuItem::new(Some("Refresh Property Names"), Some("property.refresh"));
        refresh.set_attribute_value("verb-icon", Some(&"view-refresh-symbolic".to_variant()));
        let buttons = gio::Menu::new();
        buttons.append_item(&refresh);
        let buttons = gio::MenuItem::new_section(None, &buttons);
        buttons.set_attribute_value("display-hint", Some(&"horizontal-buttons".to_variant()));
        let menu = gio::Menu::new();
        if let Some(names) = self.host().property_names() {
            menu.append_section(None, &names);
        }
        menu.append_section(None, &new);
        menu.append_item(&buttons);
        menu
    }

    /// Turns Add Property into a field to name a new property: Enter adds
    /// it; Escape, leaving it empty or clicking elsewhere goes back.
    fn name_new_property(&self, row: &gtk::ListBoxRow, existing: Vec<String>) {
        let Some(stack) = crate::live::find_descendant(row.upcast_ref(), &|w| {
            w.has_css_class("add-property-stack")
        })
        .and_downcast::<gtk::Stack>() else {
            return;
        };
        if let Some(old) = stack.child_by_name("new") {
            stack.remove(&old);
        }
        let entry = gtk::Entry::builder()
            .placeholder_text("Property Name")
            .hexpand(true)
            .css_classes(["flat"])
            .build();
        entry.update_property(&[gtk::accessible::Property::Label("Property Name")]);
        stack.add_named(&entry, Some("new"));
        stack.set_visible_child(&entry);

        // Back to Add Property; with `refocus`, the focus goes back to it.
        let back = {
            let (row, stack, field) = (row.downgrade(), stack.downgrade(), entry.downgrade());
            std::rc::Rc::new(move |refocus: bool| {
                let (Some(stack), Some(field)) = (stack.upgrade(), field.upgrade()) else {
                    return;
                };
                if field.parent().is_none() {
                    return;
                }
                stack.set_visible_child_name("add");
                stack.remove(&field);
                if refocus && let Some(row) = row.upgrade() {
                    row.grab_focus();
                }
            })
        };
        entry.connect_activate(glib::clone!(
            #[weak(rename_to = view)]
            self,
            #[strong]
            back,
            move |entry| {
                let key = entry.text().trim().trim_end_matches(':').trim().to_owned();
                if key.is_empty() {
                    back(true);
                } else if existing.contains(&key) {
                    // Adding it again would clear its value.
                    entry.add_css_class("error");
                } else {
                    view.set_property(&key, &Value::Null);
                    view.host().add_property_name(&key);
                }
            }
        ));
        entry.connect_changed(|entry| entry.remove_css_class("error"));
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(glib::clone!(
            #[strong]
            back,
            move |_, key, _, _| {
                if key == gdk::Key::Escape {
                    back(true);
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            }
        ));
        entry.add_controller(keys);
        // Clicking elsewhere in the window goes back; switching to another
        // window doesn't.
        let focus = gtk::EventControllerFocus::new();
        focus.connect_leave(glib::clone!(
            #[weak]
            entry,
            #[strong]
            back,
            move |focus| {
                let focus = focus.downgrade();
                let entry = entry.downgrade();
                let back = back.clone();
                glib::idle_add_local_once(move || {
                    let (Some(focus), Some(entry)) = (focus.upgrade(), entry.upgrade()) else {
                        return;
                    };
                    let active = entry
                        .root()
                        .and_downcast::<gtk::Window>()
                        .is_some_and(|w| w.is_active());
                    if active && !focus.contains_focus() {
                        back(false);
                    }
                });
            }
        ));
        entry.add_controller(focus);
        entry.grab_focus();
    }

    /// Builds the properties again (after a property's type or icon
    /// changes).
    pub fn refresh_properties(&self) {
        self.rebuild_properties();
    }
}

/// The menu each property's name opens: its type, its icon, Remove.
fn property_menu() -> gio::Menu {
    let types = gio::Menu::new();
    for kind in PropertyKind::ALL {
        let item = gio::MenuItem::new(Some(kind.label()), None);
        item.set_action_and_target_value(Some("prop.kind"), Some(&kind.id().to_variant()));
        types.append_item(&item);
    }
    let icon = gio::Menu::new();
    icon.append(Some("Change _Icon…"), Some("prop.icon"));
    let remove = gio::Menu::new();
    remove.append(Some("_Remove Property"), Some("prop.remove"));
    let menu = gio::Menu::new();
    menu.append_section(Some("Type"), &types);
    menu.append_section(None, &icon);
    menu.append_section(None, &remove);
    menu
}

/// An item for the vault's property names menu (see
/// [`Host::property_names`](crate::Host::property_names)): it adds `key` to
/// the note, and is greyed out in notes that already have it.
pub fn property_name_item(key: &str) -> gio::MenuItem {
    gio::MenuItem::new(
        Some(&key.replace('_', "__")),
        Some(&format!("property.{}", add_action(key))),
    )
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
    /// Add Property's actions for one note: `new`, `refresh`, and `add-…`
    /// for each of the vault's names, disabled for those the note has.
    /// They're answered from the name, so nothing is made per name.
    pub struct AddPropertyActions(ObjectSubclass<imp::AddPropertyActions>)
        @implements gio::ActionGroup;
}

impl AddPropertyActions {
    pub(crate) fn new(view: &NoteView, row: &gtk::ListBoxRow, existing: &[&str]) -> Self {
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
        pub row: glib::WeakRef<gtk::ListBoxRow>,
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

        /// Whether `name` is one of the actions, and if so whether it's
        /// enabled.
        fn enabled(&self, name: &str) -> Option<bool> {
            match name {
                "new" | "refresh" => Some(true),
                _ => key_of_action(name).map(|key| !self.existing.borrow().contains(&key)),
            }
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
                    && self.enabled(name).is_some()
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
            self.enabled(name)
                .map(|enabled| (enabled, None, None, None, None))
        }

        fn activate_action(&self, name: &str, _parameter: Option<&glib::Variant>) {
            let Some(view) = self.view.upgrade() else {
                return;
            };
            if name == "new" {
                if let Some(row) = self.row.upgrade() {
                    view.name_new_property(&row, self.existing.borrow().clone());
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
