//! A tab showing a `.base` file: its views as a table, cards or a list.
//!
//! Rearranging the table (moving, resizing or sorting columns) is written
//! back into the file as the smallest possible edit, as Obsidian does. Nothing
//! else ever changes the file, except editing its source, which the page
//! offers as a toggle.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

use adw::{prelude::*, subclass::prelude::*};
use gtk::glib;
use igneous_core::fs::{self as corefs, Expect, FileStamp, WriteError};
use igneous_core::watch::VaultEvent;
use igneous_core::{TextFile, VaultPath};
use igneous_editor::Mode;
use igneous_query::bases::{
    BaseFile, Direction, SortKey, ViewData, ViewKind, ViewResult, ViewState, normalise_id,
    value::sort_order,
};

use crate::base_view::{self, Opener, Target};
use crate::index::IndexService;
use crate::note_page::NotePage;
use crate::vault::VaultContext;
use crate::window::Window;

/// How long after the last column change the file is written.
const WRITE_DELAY: Duration = Duration::from_millis(600);

/// A row of the table: a file, or the heading of a group.
#[derive(Debug, Clone)]
enum Item {
    Row(usize),
    Group(String, usize),
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct BasePage {
        pub ctx: OnceCell<Rc<VaultContext>>,
        pub index: OnceCell<Rc<IndexService>>,
        pub path: RefCell<Option<VaultPath>>,
        pub file: RefCell<TextFile>,
        pub stamp: RefCell<Option<FileStamp>>,
        pub base: RefCell<Option<BaseFile>>,
        pub view: Cell<usize>,
        pub data: RefCell<Option<Rc<ViewData>>>,
        /// Bumped by every run, so a slow run can't replace a newer one.
        pub generation: Cell<u64>,
        /// Set while widgets are being built, so their signals aren't taken
        /// as the user rearranging columns.
        pub building: Cell<bool>,
        pub write_timer: RefCell<Option<glib::SourceId>>,
        /// Columns the user has resized since the view was built.
        pub resized: RefCell<HashSet<String>>,
        pub sort_changed: Cell<bool>,
        pub order_changed: Cell<bool>,
        pub column_view: RefCell<Option<gtk::ColumnView>>,
        pub source_note: RefCell<Option<NotePage>>,

        pub views: OnceCell<gtk::DropDown>,
        pub views_model: OnceCell<gtk::StringList>,
        pub count: OnceCell<gtk::Label>,
        pub source_toggle: OnceCell<gtk::ToggleButton>,
        pub banner: OnceCell<adw::Banner>,
        pub stack: OnceCell<gtk::Stack>,
        pub summaries: OnceCell<gtk::Box>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for BasePage {
        const NAME: &'static str = "IgneousBasePage";
        type Type = super::BasePage;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for BasePage {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().build_ui();
        }

        fn dispose(&self) {
            if let Some(id) = self.write_timer.take() {
                id.remove();
            }
        }
    }

    impl WidgetImpl for BasePage {}
    impl BinImpl for BasePage {}
}

glib::wrapper! {
    pub struct BasePage(ObjectSubclass<imp::BasePage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl BasePage {
    pub fn new(ctx: &Rc<VaultContext>, index: &Rc<IndexService>) -> Self {
        let page: Self = glib::Object::new();
        let imp = page.imp();
        imp.ctx.set(ctx.clone()).ok().unwrap();
        imp.index.set(index.clone()).ok().unwrap();
        page
    }

    fn ctx(&self) -> &Rc<VaultContext> {
        self.imp().ctx.get().unwrap()
    }

    fn index(&self) -> &Rc<IndexService> {
        self.imp().index.get().unwrap()
    }

    pub fn path(&self) -> Option<VaultPath> {
        self.imp().path.borrow().clone()
    }

    /// The file was moved or renamed; follow it.
    pub fn set_path(&self, path: VaultPath) {
        self.imp().path.replace(Some(path.clone()));
        if let Some(note) = self.imp().source_note.borrow().as_ref() {
            note.set_path(path);
        }
    }

    /// The name of the view shown.
    pub fn view_name(&self) -> Option<String> {
        let base = self.imp().base.borrow();
        base.as_ref()?
            .views
            .get(self.imp().view.get())
            .map(|v| v.name.clone())
    }

    /// Shows the view called `name`. Returns whether there is one.
    pub fn select_view(&self, name: &str) -> bool {
        let index = self
            .imp()
            .base
            .borrow()
            .as_ref()
            .and_then(|b| b.view_index(name));
        match index {
            Some(index) => {
                self.imp().views.get().unwrap().set_selected(index as u32);
                true
            }
            None => false,
        }
    }

    /// The view's data as last run, for tests.
    #[doc(hidden)]
    pub fn data(&self) -> Option<Rc<ViewData>> {
        self.imp().data.borrow().clone()
    }

    /// Which part of the page is showing: `loading`, `table`, `cards`,
    /// `list`, `message` or `source`.
    pub fn showing(&self) -> String {
        self.imp()
            .stack
            .get()
            .and_then(|s| s.visible_child_name())
            .map(|n| n.to_string())
            .unwrap_or_default()
    }

    /// The table, while one is showing (for tests).
    #[doc(hidden)]
    pub fn column_view(&self) -> Option<gtk::ColumnView> {
        self.imp().column_view.borrow().clone()
    }

    fn window(&self) -> Option<Window> {
        self.root().and_downcast::<Window>()
    }

    fn opener(&self) -> Opener {
        let page = self.downgrade();
        Rc::new(move |target: &Target, new_tab: bool| {
            if let Some(window) = page.upgrade().and_then(|p| p.window()) {
                base_view::opener(window.downgrade())(target, new_tab);
            }
        })
    }

    fn toast(&self, message: &str) {
        if let Some(window) = self.window() {
            window.toast(message);
        }
    }

    // --- widgets -----------------------------------------------------------------

    fn build_ui(&self) {
        let imp = self.imp();
        let views_model = gtk::StringList::new(&[]);
        let views = gtk::DropDown::builder()
            .model(&views_model)
            .tooltip_text("View")
            .build();
        let count = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .css_classes(["dim-label", "caption"])
            .build();
        let source_toggle = gtk::ToggleButton::builder()
            .icon_name("text-x-generic-symbolic")
            .tooltip_text("Edit the Base’s Source")
            .css_classes(["flat"])
            .build();
        let bar = gtk::Box::builder()
            .spacing(12)
            .margin_start(12)
            .margin_end(12)
            .margin_top(6)
            .margin_bottom(6)
            .build();
        bar.append(&views);
        bar.append(&count);
        bar.append(&source_toggle);

        let banner = adw::Banner::new("");
        let stack = gtk::Stack::builder().vexpand(true).build();
        stack.add_named(
            &adw::Spinner::builder()
                .width_request(32)
                .height_request(32)
                .halign(gtk::Align::Center)
                .valign(gtk::Align::Center)
                .build(),
            Some("loading"),
        );
        let summaries = gtk::Box::builder()
            .spacing(18)
            .margin_start(12)
            .margin_end(12)
            .margin_top(6)
            .margin_bottom(6)
            .visible(false)
            .css_classes(["base-summaries"])
            .build();

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&bar);
        content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        content.append(&banner);
        content.append(&stack);
        content.append(&summaries);
        self.set_child(Some(&content));

        views.connect_selected_notify(glib::clone!(
            #[weak(rename_to = page)]
            self,
            move |views| {
                if page.imp().building.get() {
                    return;
                }
                page.imp().view.set(views.selected() as usize);
                page.refresh();
            }
        ));
        source_toggle.connect_toggled(glib::clone!(
            #[weak(rename_to = page)]
            self,
            move |toggle| page.show_source(toggle.is_active())
        ));

        imp.views.set(views).ok().unwrap();
        imp.views_model.set(views_model).ok().unwrap();
        imp.count.set(count).ok().unwrap();
        imp.source_toggle.set(source_toggle).ok().unwrap();
        imp.banner.set(banner).ok().unwrap();
        imp.stack.set(stack).ok().unwrap();
        imp.summaries.set(summaries).ok().unwrap();
    }

    fn set_showing(&self, name: &str) {
        self.imp().stack.get().unwrap().set_visible_child_name(name);
    }

    fn put(&self, name: &str, widget: &impl IsA<gtk::Widget>) {
        let stack = self.imp().stack.get().unwrap();
        if let Some(old) = stack.child_by_name(name) {
            stack.remove(&old);
        }
        stack.add_named(widget, Some(name));
        stack.set_visible_child_name(name);
    }

    fn show_message(&self, title: &str, description: &str) {
        let status = adw::StatusPage::builder()
            .icon_name("view-grid-symbolic")
            .title(title)
            .description(description)
            .build();
        self.put("message", &status);
        self.imp().summaries.get().unwrap().set_visible(false);
    }

    // --- loading -----------------------------------------------------------------

    /// Loads `path` and shows its first view.
    pub fn load(&self, path: &VaultPath) -> Result<(), String> {
        self.imp().path.replace(Some(path.clone()));
        self.read()?;
        self.refresh();
        Ok(())
    }

    /// Reads the file and parses it.
    fn read(&self) -> Result<(), String> {
        let imp = self.imp();
        let path = self.path().ok_or("no file")?;
        let (file, stamp) = corefs::read_text(&self.ctx().abs(&path)).map_err(|e| e.to_string())?;
        imp.file.replace(file);
        imp.stamp.replace(Some(stamp));
        self.parse();
        Ok(())
    }

    fn parse(&self) {
        let imp = self.imp();
        let text = imp.file.borrow().text().to_owned();
        let banner = imp.banner.get().unwrap();
        match BaseFile::parse(&text) {
            Ok(base) => {
                imp.building.set(true);
                let model = imp.views_model.get().unwrap();
                let names: Vec<&str> = base.views.iter().map(|v| v.name.as_str()).collect();
                model.splice(0, model.n_items(), &names);
                let view = imp.view.get().min(base.views.len().saturating_sub(1));
                imp.view.set(view);
                imp.views.get().unwrap().set_selected(view as u32);
                imp.views.get().unwrap().set_visible(base.views.len() > 1);
                imp.building.set(false);
                if base.problems.is_empty() {
                    banner.set_revealed(false);
                } else {
                    banner.set_title(&glib::markup_escape_text(&format!(
                        "Parts of this base can’t be read and are left as written: {}",
                        base.problems.join("; ")
                    )));
                    banner.set_revealed(true);
                }
                imp.base.replace(Some(base));
            }
            Err(e) => {
                imp.base.replace(None);
                banner.set_revealed(false);
                self.show_message("This Base Can’t Be Read", &e.to_string());
            }
        }
    }

    /// Runs the view again, e.g. after the vault changed.
    pub fn refresh(&self) {
        let imp = self.imp();
        if imp.source_toggle.get().unwrap().is_active() {
            return;
        }
        let Some(base) = imp.base.borrow().clone() else {
            return;
        };
        if base.views.is_empty() {
            self.show_message("No Views", "This base doesn’t have any views yet.");
            return;
        }
        let generation = imp.generation.get() + 1;
        imp.generation.set(generation);
        if imp.data.borrow().is_none() {
            self.set_showing("loading");
        }
        let view = imp.view.get();
        let index = self.index().clone();
        let this = self.path();
        let page = self.downgrade();
        glib::spawn_future_local(async move {
            let result = base_view::run_view(&index, base, view, this).await;
            let Some(page) = page.upgrade() else { return };
            if page.imp().generation.get() != generation {
                return;
            }
            page.show_result(result);
        });
    }

    fn show_result(&self, result: Option<ViewResult>) {
        let imp = self.imp();
        // Column changes made while the view ran belong to the old table:
        // save them before it goes.
        self.flush();
        let data = match result {
            Some(ViewResult::Ready(data)) => Rc::new(data),
            Some(ViewResult::Unsupported(kind)) => {
                imp.data.replace(None);
                imp.count.get().unwrap().set_label("");
                self.show_message(
                    &format!("“{kind}” Views Aren’t Supported Yet"),
                    "The view is left as it is in the file.",
                );
                return;
            }
            Some(ViewResult::NoSuchView) => {
                imp.data.replace(None);
                self.show_message("No Such View", "This base doesn’t have that view.");
                return;
            }
            None => {
                imp.data.replace(None);
                self.show_message("Index Unavailable", "The vault’s index couldn’t be opened.");
                return;
            }
        };
        imp.count
            .get()
            .unwrap()
            .set_label(&base_view::count_text(&data));
        let banner = imp.banner.get().unwrap();
        if let Some(error) = data.errors.first() {
            banner.set_title(&glib::markup_escape_text(error));
            banner.set_revealed(true);
        }
        imp.data.replace(Some(data.clone()));
        imp.resized.borrow_mut().clear();
        imp.sort_changed.set(false);
        imp.order_changed.set(false);
        imp.building.set(true);
        match data.kind {
            ViewKind::Table => {
                let table = self.table(&data);
                self.put("table", &table);
            }
            ViewKind::Cards => {
                imp.column_view.replace(None);
                let cards = self.cards(&data);
                self.put("cards", &cards);
            }
            ViewKind::List => {
                imp.column_view.replace(None);
                let list = self.list(&data);
                self.put("list", &list);
            }
        }
        self.show_summaries(&data);
        imp.building.set(false);
    }

    fn show_summaries(&self, data: &ViewData) {
        let summaries = self.imp().summaries.get().unwrap();
        while let Some(child) = summaries.first_child() {
            summaries.remove(&child);
        }
        summaries.set_visible(!data.summaries.is_empty());
        let opener = self.opener();
        for summary in &data.summaries {
            let column = data
                .columns
                .get(summary.column)
                .map_or("", |c| c.name.as_str());
            let item = gtk::Box::builder().spacing(6).build();
            item.append(
                &gtk::Label::builder()
                    .label(format!("{column} · {}", summary.name))
                    .css_classes(["dim-label", "caption"])
                    .build(),
            );
            item.append(&base_view::cell_widget(&summary.value, &opener));
            summaries.append(&item);
        }
    }

    fn items(data: &ViewData) -> gtk::gio::ListStore {
        let store = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        if data.groups.is_empty() {
            for i in 0..data.rows.len() {
                store.append(&glib::BoxedAnyObject::new(Item::Row(i)));
            }
        } else {
            for group in &data.groups {
                store.append(&glib::BoxedAnyObject::new(Item::Group(
                    base_view::group_title(&group.key),
                    group.rows.len(),
                )));
                for i in group.rows.clone() {
                    store.append(&glib::BoxedAnyObject::new(Item::Row(i)));
                }
            }
        }
        store
    }

    fn item(object: &glib::Object) -> Option<Item> {
        object
            .downcast_ref::<glib::BoxedAnyObject>()
            .map(|b| b.borrow::<Item>().clone())
    }

    fn open_item(&self, data: &ViewData, item: Option<Item>, new_tab: bool) {
        if let Some(Item::Row(i)) = item
            && let Some(row) = data.rows.get(i)
        {
            (self.opener())(&Target::File(row.file.clone()), new_tab);
        }
    }

    // --- table -----------------------------------------------------------------------

    fn table(&self, data: &Rc<ViewData>) -> gtk::Widget {
        let imp = self.imp();
        let grouped = !data.groups.is_empty();
        let column_view = gtk::ColumnView::builder()
            .reorderable(true)
            .show_column_separators(true)
            .show_row_separators(true)
            .css_classes(["data-table", "base-table"])
            .build();
        let opener = self.opener();
        let base = imp.base.borrow().clone();
        let view = base
            .as_ref()
            .and_then(|b| b.views.get(imp.view.get()).cloned());
        let mut initial_sort = None;
        for (index, column) in data.columns.iter().enumerate() {
            let factory = gtk::SignalListItemFactory::new();
            factory.connect_setup(|_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().unwrap();
                item.set_child(Some(&gtk::Box::builder().build()));
            });
            let cell_data = data.clone();
            let cell_opener = opener.clone();
            factory.connect_bind(move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().unwrap();
                let holder = item.child().and_downcast::<gtk::Box>().unwrap();
                while let Some(child) = holder.first_child() {
                    holder.remove(&child);
                }
                let Some(entry) = item.item().as_ref().and_then(Self::item) else {
                    return;
                };
                let widget = match entry {
                    Item::Row(row) => {
                        base_view::row_cell_widget(&cell_data, row, index, &cell_opener)
                    }
                    Item::Group(title, count) if index == 0 => gtk::Label::builder()
                        .label(format!("{title} ({count})"))
                        .xalign(0.0)
                        .css_classes(["heading"])
                        .build()
                        .upcast(),
                    Item::Group(..) => gtk::Label::new(None).upcast(),
                };
                widget.set_hexpand(true);
                holder.append(&widget);
            });
            let view_column = gtk::ColumnViewColumn::builder()
                .title(&column.name)
                .factory(&factory)
                .resizable(true)
                .id(&column.id)
                .build();
            match column.width {
                Some(width) => view_column.set_fixed_width(width as i32),
                None => view_column.set_expand(true),
            }
            if !grouped {
                let sort_data = data.clone();
                view_column.set_sorter(Some(&gtk::CustomSorter::new(move |a, b| {
                    let cell = |object: &glib::Object| match Self::item(object) {
                        Some(Item::Row(row)) => sort_data.rows[row].cells[index].clone().ok(),
                        _ => None,
                    };
                    match (cell(a), cell(b)) {
                        (Some(a), Some(b)) => sort_order(&a, &b).into(),
                        (Some(_), None) => gtk::Ordering::Smaller,
                        (None, Some(_)) => gtk::Ordering::Larger,
                        (None, None) => gtk::Ordering::Equal,
                    }
                })));
                if let Some(view) = &view
                    && let Some(key) = view.sort.first()
                    && normalise_id(&key.property) == column.id
                {
                    initial_sort = Some((view_column.clone(), key.direction));
                }
            }
            let page = self.downgrade();
            let id = column.id.clone();
            view_column.connect_fixed_width_notify(move |_| {
                if let Some(page) = page.upgrade()
                    && !page.imp().building.get()
                {
                    page.imp().resized.borrow_mut().insert(id.clone());
                    page.schedule_write();
                }
            });
            column_view.append_column(&view_column);
        }

        // Rows stay in the base's order until a header is clicked.
        let store = Self::items(data);
        let sorted = gtk::SortListModel::new(Some(store), None::<gtk::Sorter>);
        if !grouped {
            sorted.set_sorter(column_view.sorter().as_ref());
        }
        let selection = gtk::SingleSelection::builder()
            .model(&sorted)
            .autoselect(false)
            .can_unselect(true)
            .build();
        column_view.set_model(Some(&selection));
        if let Some((column, direction)) = initial_sort {
            column_view.sort_by_column(
                Some(&column),
                match direction {
                    Direction::Asc => gtk::SortType::Ascending,
                    Direction::Desc => gtk::SortType::Descending,
                },
            );
        }
        if let Some(sorter) = column_view.sorter() {
            let page = self.downgrade();
            sorter.connect_changed(move |_, _| {
                if let Some(page) = page.upgrade()
                    && !page.imp().building.get()
                {
                    page.imp().sort_changed.set(true);
                    page.schedule_write();
                }
            });
        }
        let page = self.downgrade();
        column_view
            .columns()
            .connect_items_changed(move |_, _, _, _| {
                if let Some(page) = page.upgrade()
                    && !page.imp().building.get()
                {
                    page.imp().order_changed.set(true);
                    page.schedule_write();
                }
            });
        let activate_data = data.clone();
        let page = self.downgrade();
        column_view.connect_activate(move |view, position| {
            let Some(page) = page.upgrade() else { return };
            let item = view
                .model()
                .and_then(|m| m.item(position))
                .as_ref()
                .and_then(Self::item);
            page.open_item(&activate_data, item, false);
        });
        imp.column_view.replace(Some(column_view.clone()));
        gtk::ScrolledWindow::builder()
            .child(&column_view)
            .vexpand(true)
            .build()
            .upcast()
    }

    // --- cards and list -------------------------------------------------------------

    fn cards(&self, data: &Rc<ViewData>) -> gtk::Widget {
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            item.set_child(Some(
                &gtk::Box::builder()
                    .orientation(gtk::Orientation::Vertical)
                    .spacing(4)
                    .margin_start(6)
                    .margin_end(6)
                    .margin_top(6)
                    .margin_bottom(6)
                    .css_classes(["card", "base-card"])
                    .build(),
            ));
        });
        let bind_data = data.clone();
        let opener = self.opener();
        factory.connect_bind(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let card = item.child().and_downcast::<gtk::Box>().unwrap();
            while let Some(child) = card.first_child() {
                card.remove(&child);
            }
            match item.item().as_ref().and_then(Self::item) {
                Some(Item::Row(row)) => {
                    let row = &bind_data.rows[row];
                    card.append(
                        &gtk::Label::builder()
                            .label(crate::files::display_name(&row.file, false).0)
                            .xalign(0.0)
                            .ellipsize(gtk::pango::EllipsizeMode::End)
                            .css_classes(["heading"])
                            .build(),
                    );
                    for (column, cell) in bind_data.columns.iter().zip(&row.cells) {
                        if column.id == "file.name" {
                            continue;
                        }
                        card.append(
                            &gtk::Label::builder()
                                .label(&column.name)
                                .xalign(0.0)
                                .css_classes(["caption", "dim-label"])
                                .build(),
                        );
                        card.append(&base_view::cell_widget(cell, &opener));
                    }
                }
                Some(Item::Group(title, count)) => card.append(
                    &gtk::Label::builder()
                        .label(format!("{title} ({count})"))
                        .xalign(0.0)
                        .css_classes(["title-4"])
                        .build(),
                ),
                None => {}
            }
        });
        let selection = gtk::NoSelection::new(Some(Self::items(data)));
        let grid = gtk::GridView::builder()
            .model(&selection)
            .factory(&factory)
            .min_columns(1)
            .max_columns(4)
            .single_click_activate(true)
            .css_classes(["base-cards"])
            .build();
        let activate_data = data.clone();
        let page = self.downgrade();
        grid.connect_activate(move |grid, position| {
            let Some(page) = page.upgrade() else { return };
            let item = grid
                .model()
                .and_then(|m| m.item(position))
                .as_ref()
                .and_then(Self::item);
            page.open_item(&activate_data, item, false);
        });
        gtk::ScrolledWindow::builder()
            .child(&grid)
            .vexpand(true)
            .build()
            .upcast()
    }

    fn list(&self, data: &Rc<ViewData>) -> gtk::Widget {
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            item.set_child(Some(&gtk::Box::builder().spacing(12).build()));
        });
        let bind_data = data.clone();
        let opener = self.opener();
        factory.connect_bind(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let line = item.child().and_downcast::<gtk::Box>().unwrap();
            while let Some(child) = line.first_child() {
                line.remove(&child);
            }
            match item.item().as_ref().and_then(Self::item) {
                Some(Item::Row(row)) => {
                    let row = &bind_data.rows[row];
                    line.append(
                        &gtk::Label::builder()
                            .label(crate::files::display_name(&row.file, false).0)
                            .xalign(0.0)
                            .css_classes(["heading"])
                            .build(),
                    );
                    for (column, cell) in bind_data.columns.iter().zip(&row.cells) {
                        if column.id == "file.name"
                            || matches!(cell, Ok(igneous_query::bases::Value::Null))
                        {
                            continue;
                        }
                        line.append(&base_view::cell_widget(cell, &opener));
                    }
                }
                Some(Item::Group(title, count)) => line.append(
                    &gtk::Label::builder()
                        .label(format!("{title} ({count})"))
                        .xalign(0.0)
                        .css_classes(["title-4"])
                        .build(),
                ),
                None => {}
            }
        });
        let selection = gtk::NoSelection::new(Some(Self::items(data)));
        let list = gtk::ListView::builder()
            .model(&selection)
            .factory(&factory)
            .single_click_activate(true)
            .css_classes(["navigation-sidebar", "base-list"])
            .build();
        let activate_data = data.clone();
        let page = self.downgrade();
        list.connect_activate(move |list, position| {
            let Some(page) = page.upgrade() else { return };
            let item = list
                .model()
                .and_then(|m| m.item(position))
                .as_ref()
                .and_then(Self::item);
            page.open_item(&activate_data, item, false);
        });
        gtk::ScrolledWindow::builder()
            .child(&list)
            .vexpand(true)
            .build()
            .upcast()
    }

    // --- writing view state back ------------------------------------------------------

    fn schedule_write(&self) {
        let imp = self.imp();
        if let Some(id) = imp.write_timer.take() {
            id.remove();
        }
        let page = self.downgrade();
        let id = glib::timeout_add_local_once(WRITE_DELAY, move || {
            if let Some(page) = page.upgrade() {
                page.imp().write_timer.take();
                page.write_view_state();
            }
        });
        imp.write_timer.replace(Some(id));
    }

    /// Writes any pending column changes now (e.g. before closing).
    pub fn flush(&self) {
        if let Some(id) = self.imp().write_timer.take() {
            id.remove();
            self.write_view_state();
        }
    }

    /// The view state the table shows now, keeping each property's spelling
    /// from the file so unchanged entries stay byte-identical.
    fn current_state(&self) -> Option<ViewState> {
        let imp = self.imp();
        let column_view = imp.column_view.borrow().clone()?;
        let base = imp.base.borrow();
        let view = base.as_ref()?.views.get(imp.view.get())?;
        let spelling = |id: &str| -> String {
            view.order
                .iter()
                .chain(view.sort.iter().map(|k| &k.property))
                .chain(view.column_sizes.iter().map(|(k, _)| k))
                .find(|written| normalise_id(written) == id)
                .cloned()
                .unwrap_or_else(|| id.strip_prefix("note.").unwrap_or(id).to_owned())
        };
        let columns: Vec<gtk::ColumnViewColumn> = column_view
            .columns()
            .iter::<gtk::ColumnViewColumn>()
            .filter_map(Result::ok)
            .collect();
        let mut state = ViewState::default();
        if imp.order_changed.get() {
            state.order = Some(
                columns
                    .iter()
                    .filter_map(|c| c.id())
                    .map(|id| spelling(&id))
                    .collect(),
            );
        }
        let resized = imp.resized.borrow();
        if !resized.is_empty() {
            let mut sizes = view.column_sizes.clone();
            for column in &columns {
                let (Some(id), width) = (column.id(), column.fixed_width()) else {
                    continue;
                };
                if width <= 0 || !resized.contains(id.as_str()) {
                    continue;
                }
                match sizes
                    .iter_mut()
                    .find(|(k, _)| normalise_id(k) == id.as_str())
                {
                    Some((_, w)) => *w = width as u32,
                    None => sizes.push((spelling(&id), width as u32)),
                }
            }
            state.column_sizes = Some(sizes);
        }
        if imp.sort_changed.get()
            && let Some(sorter) = column_view.sorter().and_downcast::<gtk::ColumnViewSorter>()
        {
            state.sort = Some(match sorter.primary_sort_column().and_then(|c| c.id()) {
                Some(id) => vec![SortKey {
                    property: spelling(&id),
                    direction: match sorter.primary_sort_order() {
                        gtk::SortType::Descending => Direction::Desc,
                        _ => Direction::Asc,
                    },
                }],
                None => Vec::new(),
            });
        }
        Some(state)
    }

    fn write_view_state(&self) {
        let imp = self.imp();
        let Some(state) = self.current_state() else {
            return;
        };
        let edits = {
            let base = imp.base.borrow();
            let Some(base) = base.as_ref() else { return };
            match base.set_view_state(imp.view.get(), &state) {
                Ok(edits) => edits,
                Err(e) => {
                    self.toast(&format!("Couldn’t save the column changes: {e}"));
                    return;
                }
            }
        };
        imp.order_changed.set(false);
        imp.sort_changed.set(false);
        imp.resized.borrow_mut().clear();
        if edits.is_empty() {
            return;
        }
        let Some(path) = self.path() else { return };
        let mut file = imp.file.borrow().clone();
        file.set_text(igneous_markdown::edit::apply(file.text(), &edits));
        let stamp = imp.stamp.borrow().clone();
        let expect = match &stamp {
            Some(stamp) => Expect::Contents(stamp),
            None => Expect::Anything,
        };
        match corefs::write_atomic(&self.ctx().abs(&path), &file, expect) {
            Ok(stamp) => {
                self.ctx().expect_write(&path, &stamp);
                imp.file.replace(file);
                imp.stamp.replace(Some(stamp));
                self.parse();
                self.index().apply(vec![VaultEvent::Modified(path)]);
            }
            Err(WriteError::ChangedOnDisk { .. }) => {
                self.toast("The base changed on disk, so the column changes weren’t saved");
                let _ = self.read();
                self.refresh();
            }
            Err(WriteError::Io(e)) => self.toast(&format!("Couldn’t save the base: {e}")),
        }
    }

    // --- changes elsewhere ------------------------------------------------------------

    /// The file changed on disk (not because of this page).
    pub fn on_disk_changed(&self) {
        let Some(path) = self.path() else { return };
        let Ok(Some(now)) = corefs::current_stamp(&self.ctx().abs(&path)) else {
            return;
        };
        let same = self
            .imp()
            .stamp
            .borrow()
            .as_ref()
            .is_some_and(|s| s.same_contents(&now));
        if !same && self.read().is_ok() {
            self.refresh();
        }
    }

    /// Shows the source as editable text, or goes back to the views.
    fn show_source(&self, source: bool) {
        let imp = self.imp();
        let Some(path) = self.path() else { return };
        if source {
            self.flush();
            let note = NotePage::new(self.ctx());
            if let Some(window) = self.window() {
                window.style_note(&note);
            }
            note.set_mode(Mode::Source);
            if let Err(e) = note.load(&path) {
                self.toast(&format!("Couldn’t open the source: {e}"));
                imp.source_toggle.get().unwrap().set_active(false);
                return;
            }
            self.put("source", &note);
            imp.summaries.get().unwrap().set_visible(false);
            note.focus_editor();
            imp.source_note.replace(Some(note));
        } else {
            if let Some(note) = imp.source_note.take() {
                note.flush();
            }
            let _ = self.read();
            self.refresh();
        }
    }

    /// The source editor, while it's showing.
    pub fn source_note(&self) -> Option<NotePage> {
        self.imp().source_note.borrow().clone()
    }

    /// Switches between the views and the source (for tests and actions).
    pub fn set_source_shown(&self, shown: bool) {
        self.imp().source_toggle.get().unwrap().set_active(shown);
    }
}

/// `.base` files open in a Bases tab.
pub fn is_base(path: &VaultPath) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("base"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_files() {
        assert!(is_base(&VaultPath::new("Projects/Projects.base").unwrap()));
        assert!(is_base(&VaultPath::new("a.BASE").unwrap()));
        assert!(!is_base(&VaultPath::new("a.md").unwrap()));
    }
}
