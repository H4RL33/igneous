//! The container overlay widgets sit in (see `live.rs`).
//!
//! GtkTextView lays overlays out at their minimum size, and counts that
//! minimum in its own, so a table or properties header as wide as the text
//! column would stop the window from ever getting narrower. A slot asks for
//! almost no width but lays its child out at the text column's width,
//! overflowing itself (GTK doesn't clip overflowing children unless asked).
//!
//! It's a plain GtkWidget: a widget with a layout manager (like AdwBin)
//! never has its own `measure` called.

use std::cell::{Cell, RefCell};

use gtk::{glib, prelude::*, subclass::prelude::*};

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Slot {
        /// The width to lay the child out at; 0 uses the child's own.
        pub natural_width: Cell<i32>,
        pub child: RefCell<Option<gtk::Widget>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Slot {
        const NAME: &'static str = "IgneousOverlaySlot";
        type Type = super::Slot;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Slot {
        fn dispose(&self) {
            if let Some(child) = self.child.take() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for Slot {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let Some(child) = self.child.borrow().clone() else {
                return (0, 0, -1, -1);
            };
            let width = self.child_width(&child);
            match orientation {
                gtk::Orientation::Horizontal => (1, width, -1, -1),
                _ => {
                    let (min, natural, _, _) = child.measure(orientation, width);
                    (min, natural, -1, -1)
                }
            }
        }

        fn size_allocate(&self, _width: i32, height: i32, baseline: i32) {
            if let Some(child) = self.child.borrow().as_ref() {
                child.allocate(self.child_width(child), height, baseline, None);
            }
        }
    }

    impl Slot {
        /// The width the child is laid out at: the text column's, or its
        /// own natural width, and never below its minimum.
        pub fn child_width(&self, child: &gtk::Widget) -> i32 {
            let (min, natural, _, _) = child.measure(gtk::Orientation::Horizontal, -1);
            let wanted = self.natural_width.get();
            (if wanted > 0 { wanted } else { natural }).max(min)
        }
    }
}

glib::wrapper! {
    pub struct Slot(ObjectSubclass<imp::Slot>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for Slot {
    fn default() -> Self {
        Self::new()
    }
}

impl Slot {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn set_child(&self, child: Option<&impl IsA<gtk::Widget>>) {
        let imp = self.imp();
        if let Some(old) = imp.child.take() {
            old.unparent();
        }
        if let Some(child) = child {
            let child = child.as_ref().clone();
            // A widget can only have one parent; take it from wherever it is.
            if child.parent().is_some() {
                child.unparent();
            }
            child.set_parent(self);
            imp.child.replace(Some(child));
        }
        self.queue_resize();
    }

    pub fn set_natural_width(&self, width: i32) {
        if self.imp().natural_width.replace(width) != width {
            self.queue_resize();
        }
    }
}
