//! Daily notes: one note per day, named with a Moment-style format in the
//! daily notes folder and started from a template, plus a calendar of the
//! days that have one.

use adw::prelude::*;
use gtk::glib;
use gtk::subclass::prelude::*;
use igneous_core::settings::DailyNoteSettings;
use igneous_core::{TextFile, VaultPath, datefmt};
use jiff::civil::Date;
use jiff::{ToSpan, Zoned, tz::TimeZone};

use crate::window::Window;

/// How far from today previous/next day and date lookups search.
const SEARCH_DAYS: i32 = 3650;

/// The day at the current time of day, for formatting.
fn moment(date: Date) -> Zoned {
    let now = Zoned::now();
    date.to_datetime(now.time())
        .to_zoned(TimeZone::system())
        .unwrap_or(now)
}

/// The note for `date`: the folder, then the formatted date (which may hold
/// folders of its own, e.g. `YYYY/MM/YYYY-MM-DD`), then `.md`.
pub fn path_for(settings: &DailyNoteSettings, date: Date) -> Option<VaultPath> {
    let format = if settings.format.trim().is_empty() {
        "YYYY-MM-DD"
    } else {
        settings.format.trim()
    };
    let name = datefmt::format(&moment(date), format);
    let folder = settings.folder.trim().trim_matches('/');
    let path = if folder.is_empty() {
        format!("{name}.md")
    } else {
        format!("{folder}/{name}.md")
    };
    VaultPath::new(&path).ok()
}

/// The date `path` is the daily note for, searching ten years either side
/// of today.
pub fn date_of(settings: &DailyNoteSettings, path: &VaultPath) -> Option<Date> {
    let today = Zoned::now().date();
    (0..=SEARCH_DAYS).find_map(|offset| {
        [offset, -offset].into_iter().find_map(|days| {
            let date = today.checked_add(days.days()).ok()?;
            (path_for(settings, date).as_ref() == Some(path)).then_some(date)
        })
    })
}

pub fn today() -> Date {
    Zoned::now().date()
}

impl Window {
    fn daily_settings(&self) -> DailyNoteSettings {
        self.ctx().settings.borrow().daily_notes.clone()
    }

    /// Opens the note for `date`, creating it from the template if needed.
    pub fn open_daily_note(&self, date: Date) {
        let settings = self.daily_settings();
        let Some(path) = path_for(&settings, date) else {
            self.toast("The daily note format doesn’t make a valid file name");
            return;
        };
        let abs = self.ctx().abs(&path);
        if !abs.exists() {
            let text = match &settings.template {
                Some(template) => match igneous_core::fs::read_text(&self.ctx().abs(template)) {
                    Ok((file, _)) => crate::templates::render(
                        file.text(),
                        path.stem(),
                        &moment(date),
                        &self.ctx().settings.borrow().templates,
                    ),
                    Err(e) => {
                        self.toast(&format!("Couldn’t read the daily note template: {e}"));
                        String::new()
                    }
                },
                None => String::new(),
            };
            if let Some(parent) = abs.parent()
                && let Err(e) = std::fs::create_dir_all(parent)
            {
                self.toast(&format!("Couldn’t create the daily notes folder: {e}"));
                return;
            }
            if let Err(e) = igneous_core::fs::write_atomic(
                &abs,
                &TextFile::new(text),
                igneous_core::fs::Expect::Absent,
            ) {
                self.toast(&format!("Couldn’t create the daily note: {e}"));
                return;
            }
            self.on_vault_events(vec![igneous_core::watch::VaultEvent::Created(path.clone())]);
        }
        self.open_path(&path, false);
        if let Some(note) = self.selected_note() {
            note.focus_editor();
        }
    }

    /// Opens the existing daily note before (or after) the one open now, or
    /// today's if no daily note is open.
    pub fn step_daily_note(&self, forward: bool) {
        let settings = self.daily_settings();
        let from = self
            .selected_path()
            .and_then(|p| date_of(&settings, &p))
            .unwrap_or_else(today);
        let step: i32 = if forward { 1 } else { -1 };
        let found = (1..=SEARCH_DAYS).find_map(|n| {
            let date = from.checked_add((n * step).days()).ok()?;
            let path = path_for(&settings, date)?;
            self.ctx().abs(&path).is_file().then_some(date)
        });
        match found {
            Some(date) => self.open_daily_note(date),
            None => self.toast(if forward {
                "No later daily notes"
            } else {
                "No earlier daily notes"
            }),
        }
    }

    /// The calendar in the sidebar header: days with a note are marked, and
    /// choosing a day opens (or creates) its note.
    pub(crate) fn set_up_calendar(&self) {
        let calendar = gtk::Calendar::new();
        let today_button = gtk::Button::builder()
            .label("_Today")
            .use_underline(true)
            .css_classes(["pill"])
            .halign(gtk::Align::Center)
            .build();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        content.append(&calendar);
        content.append(&today_button);
        let popover = gtk::Popover::builder().child(&content).build();
        self.imp().calendar_button.set_popover(Some(&popover));

        let mark = glib::clone!(
            #[weak(rename_to = window)]
            self,
            #[weak]
            calendar,
            move || window.mark_daily_notes(&calendar)
        );
        let on_show = mark.clone();
        popover.connect_show(move |_| on_show());
        // Moving between months doesn't select a day, but needs new marks.
        let (prev, next, prev_year) = (mark.clone(), mark.clone(), mark.clone());
        calendar.connect_prev_month(move |_| prev());
        calendar.connect_next_month(move |_| next());
        calendar.connect_prev_year(move |_| prev_year());
        calendar.connect_next_year(move |_| mark());
        calendar.connect_day_selected(glib::clone!(
            #[weak(rename_to = window)]
            self,
            #[weak]
            popover,
            move |calendar| {
                let date = calendar.date();
                let Ok(date) = Date::new(
                    date.year() as i16,
                    date.month() as i8,
                    date.day_of_month() as i8,
                ) else {
                    return;
                };
                popover.popdown();
                window.open_daily_note(date);
            }
        ));
        today_button.connect_clicked(glib::clone!(
            #[weak(rename_to = window)]
            self,
            #[weak]
            popover,
            move |_| {
                popover.popdown();
                window.open_daily_note(today());
            }
        ));
    }

    fn mark_daily_notes(&self, calendar: &gtk::Calendar) {
        calendar.clear_marks();
        let shown = calendar.date();
        let settings = self.daily_settings();
        let (year, month) = (shown.year() as i16, shown.month() as i8);
        let Ok(first) = Date::new(year, month, 1) else {
            return;
        };
        for day in 1..=first.days_in_month() {
            if let Ok(date) = Date::new(year, month, day)
                && let Some(path) = path_for(&settings, date)
                && self.ctx().abs(&path).is_file()
            {
                calendar.mark_day(day as u32);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_and_dates() {
        let settings = DailyNoteSettings {
            folder: "Journal/".into(),
            format: "YYYY/MM/YYYY-MM-DD dddd".into(),
            ..DailyNoteSettings::default()
        };
        let date = Date::new(2026, 10, 5).unwrap();
        let path = path_for(&settings, date).unwrap();
        assert_eq!(path.as_str(), "Journal/2026/10/2026-10-05 Monday.md");
        let near = today().checked_add(12.days()).unwrap();
        assert_eq!(
            date_of(&settings, &path_for(&settings, near).unwrap()),
            Some(near)
        );
        let root = DailyNoteSettings::default();
        assert_eq!(path_for(&root, date).unwrap().as_str(), "2026-10-05.md");
    }
}
