//! Activating the rows of a plain GtkListBox.
//!
//! GtkListBox reports a click on a row (or Enter on a focused one) as
//! `row-activated` on the list. A row's own `activate` signal is handled by
//! activating it through its list again, so re-emitting it from
//! `row-activated` recursed until the stack ran out.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::{glib, prelude::*};

/// The list a row listens to, and its handler there.
type Listening = Rc<RefCell<Option<(glib::WeakRef<gtk::ListBox>, glib::SignalHandlerId)>>>;

/// Runs `action` whenever the list `row` is in activates it: a click, Enter,
/// or `row.emit_activate()`. Follows the row if it moves to another list.
pub fn on_activate(row: &gtk::ListBoxRow, action: impl Fn() + 'static) {
    let action: Rc<dyn Fn()> = Rc::new(action);
    let handler: Listening = Rc::default();
    let connect = move |row: &gtk::ListBoxRow| {
        if let Some((list, id)) = handler.take()
            && let Some(list) = list.upgrade()
        {
            list.disconnect(id);
        }
        let Some(list) = row.parent().and_downcast::<gtk::ListBox>() else {
            return;
        };
        let action = action.clone();
        let this = row.downgrade();
        let id = list.connect_row_activated(move |_, activated| {
            if this.upgrade().as_ref() == Some(activated) {
                action();
            }
        });
        handler.replace(Some((list.downgrade(), id)));
    };
    connect(row);
    row.connect_notify_local(Some("parent"), move |row, _| connect(row));
}
