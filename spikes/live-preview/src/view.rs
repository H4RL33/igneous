//! A GtkSourceView subclass that lets the controller draw block decorations
//! (code backgrounds, callout panels, quote bars, bullets, rules) beneath the
//! text.

use std::cell::RefCell;

use gtk::{glib, subclass::prelude::*};
use sourceview::subclass::prelude::*;

pub type DecorFn = Box<dyn Fn(&LpView, &gtk::Snapshot)>;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct LpView {
        pub decor: RefCell<Option<DecorFn>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LpView {
        const NAME: &'static str = "IgneousSpikeView";
        type Type = super::LpView;
        type ParentType = sourceview::View;
    }

    impl ObjectImpl for LpView {}
    impl WidgetImpl for LpView {}

    impl TextViewImpl for LpView {
        fn snapshot_layer(&self, layer: gtk::TextViewLayer, snapshot: gtk::Snapshot) {
            self.parent_snapshot_layer(layer, snapshot.clone());
            if layer == gtk::TextViewLayer::BelowText
                && let Some(decor) = self.decor.borrow().as_ref()
            {
                decor(&self.obj(), &snapshot);
            }
        }
    }

    impl ViewImpl for LpView {}
}

glib::wrapper! {
    pub struct LpView(ObjectSubclass<imp::LpView>)
        @extends sourceview::View, gtk::TextView, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Scrollable;
}

impl LpView {
    pub fn new(buffer: &sourceview::Buffer) -> Self {
        glib::Object::builder().property("buffer", buffer).build()
    }

    pub fn set_decor(&self, decor: DecorFn) {
        self.imp().decor.replace(Some(decor));
    }
}
