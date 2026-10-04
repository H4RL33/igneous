//! The note view.

use std::cell::Cell;

use gtk::{glib, prelude::*, subclass::prelude::*};
use sourceview::subclass::prelude::*;

/// Widest the text column gets before margins grow, in pixels.
const READABLE_WIDTH: i32 = 720;
const MIN_MARGIN: i32 = 24;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct NoteView {
        pub last_width: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for NoteView {
        const NAME: &'static str = "IgneousNoteView";
        type Type = super::NoteView;
        type ParentType = sourceview::View;
    }

    impl ObjectImpl for NoteView {
        fn constructed(&self) {
            self.parent_constructed();
            let view = self.obj();
            view.set_wrap_mode(gtk::WrapMode::WordChar);
            view.set_top_margin(24);
            view.set_bottom_margin(96);
            view.set_left_margin(MIN_MARGIN);
            view.set_right_margin(MIN_MARGIN);
            view.set_pixels_below_lines(2);
            view.add_css_class("igneous-note");
            // Keep the text column readable: grow the margins on wide windows.
            view.add_tick_callback(|view, _| {
                let view = view.downcast_ref::<super::NoteView>().unwrap();
                let width = view.width();
                if width != view.imp().last_width.get() {
                    view.imp().last_width.set(width);
                    let margin = ((width - READABLE_WIDTH) / 2).max(MIN_MARGIN);
                    if view.left_margin() != margin {
                        view.set_left_margin(margin);
                        view.set_right_margin(margin);
                    }
                }
                glib::ControlFlow::Continue
            });
        }
    }

    impl WidgetImpl for NoteView {}
    impl TextViewImpl for NoteView {}
    impl ViewImpl for NoteView {}
}

glib::wrapper! {
    pub struct NoteView(ObjectSubclass<imp::NoteView>)
        @extends sourceview::View, gtk::TextView, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Scrollable;
}

impl NoteView {
    pub fn new(buffer: &sourceview::Buffer) -> Self {
        glib::Object::builder().property("buffer", buffer).build()
    }
}
