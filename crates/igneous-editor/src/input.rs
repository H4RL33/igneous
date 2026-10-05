//! Typing behaviours: lists continue on Enter and indent with Tab, brackets
//! pair up, Markdown markers wrap a selection, and pasting is smart (a URL
//! over a selection becomes a link, HTML becomes Markdown, files become
//! attachments).

use gtk::{gdk, gio, glib, prelude::*, subclass::prelude::*};

use crate::view::{Mode, NoteView};

/// Editing options, from the vault's editor settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputOptions {
    pub smart_lists: bool,
    pub indent_with_tabs: bool,
    pub tab_size: u32,
    pub auto_pair_brackets: bool,
    pub auto_pair_markdown: bool,
}

impl Default for InputOptions {
    fn default() -> Self {
        Self {
            smart_lists: true,
            indent_with_tabs: true,
            tab_size: 4,
            auto_pair_brackets: true,
            auto_pair_markdown: true,
        }
    }
}

/// What Enter does at the end of `line` (the text before the cursor):
/// the text to insert, or `None` to let the newline through. Returns
/// `Some(None)` to end the list (remove the empty item's marker).
pub fn continue_list(line: &str) -> Option<Option<String>> {
    let indent_len = line.len() - line.trim_start_matches([' ', '\t']).len();
    let (indent, rest) = line.split_at(indent_len);
    // Quotes and callouts continue with their `>` prefix.
    if let Some(after) = rest.strip_prefix('>') {
        let prefix = &rest[..rest.len() - after.trim_start_matches(['>', ' ']).len()];
        let content = &rest[prefix.len()..];
        return Some(if content.trim().is_empty() {
            None
        } else {
            Some(format!("\n{indent}{prefix}"))
        });
    }
    let (marker, content) = if let Some(after) = rest
        .strip_prefix("- ")
        .or_else(|| rest.strip_prefix("* "))
        .or_else(|| rest.strip_prefix("+ "))
    {
        let bullet = &rest[..2];
        match after.strip_prefix("[") {
            Some(task) if task.len() >= 2 && &task[1..2] == "]" => (
                format!("{bullet}[ ] "),
                task.get(2..).unwrap_or("").trim_start(),
            ),
            _ => (bullet.to_owned(), after),
        }
    } else {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        let delimiter = rest.as_bytes().get(digits).copied();
        if digits == 0
            || !matches!(delimiter, Some(b'.' | b')'))
            || rest.as_bytes().get(digits + 1) != Some(&b' ')
        {
            return None;
        }
        let n: u64 = rest[..digits].parse().ok()?;
        let after = &rest[digits + 2..];
        let (next, content) = match after.strip_prefix("[") {
            Some(task) if task.len() >= 2 && &task[1..2] == "]" => (
                format!("{}{} [ ] ", n + 1, delimiter? as char),
                task.get(2..).unwrap_or("").trim_start(),
            ),
            _ => (format!("{}{} ", n + 1, delimiter? as char), after),
        };
        (next, content)
    };
    Some(if content.trim().is_empty() {
        None
    } else {
        Some(format!("\n{indent}{marker}"))
    })
}

/// Whether `line` is a list item (so Tab indents it).
fn is_list_item(line: &str) -> bool {
    let rest = line.trim_start_matches([' ', '\t']);
    rest.starts_with("- ") || rest.starts_with("* ") || rest.starts_with("+ ") || rest == "-" || {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        digits > 0 && matches!(rest.as_bytes().get(digits), Some(b'.' | b')'))
    }
}

fn closing(c: char) -> Option<char> {
    Some(match c {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        _ => return None,
    })
}

/// Whether `text` is a web address worth turning into a link.
pub fn is_url(text: &str) -> bool {
    let t = text.trim();
    !t.contains(char::is_whitespace)
        && ["http://", "https://", "mailto:", "obsidian://"]
            .iter()
            .any(|p| t.starts_with(p))
}

impl NoteView {
    pub fn set_input_options(&self, options: InputOptions) {
        self.imp().input.replace(options);
    }

    fn input_options(&self) -> InputOptions {
        self.imp().input.borrow().clone()
    }

    pub(crate) fn connect_input(&self) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed(glib::clone!(
            #[weak(rename_to = view)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, key, _, state| view.on_key(key, state)
        ));
        self.add_controller(keys);

        self.connect_paste_clipboard(|view| {
            let Some(view) = view.downcast_ref::<NoteView>() else {
                return;
            };
            if view.is_editable() {
                view.stop_signal_emission_by_name("paste-clipboard");
                view.smart_paste();
            }
        });

        let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        drop.connect_drop(glib::clone!(
            #[weak(rename_to = view)]
            self,
            #[upgrade_or]
            false,
            move |_, value, x, y| {
                let Ok(files) = value.get::<gdk::FileList>() else {
                    return false;
                };
                let (bx, by) =
                    view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
                if let Some(iter) = view.iter_at_location(bx, by) {
                    view.buffer().place_cursor(&iter);
                }
                view.attach_files(files.files())
            }
        ));
        self.add_controller(drop);
    }

    fn on_key(&self, key: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
        if !self.is_editable() || self.mode() == Mode::Reading {
            return glib::Propagation::Proceed;
        }
        let modifiers = state
            & (gdk::ModifierType::CONTROL_MASK
                | gdk::ModifierType::ALT_MASK
                | gdk::ModifierType::SUPER_MASK
                | gdk::ModifierType::SHIFT_MASK);
        let options = self.input_options();
        match key {
            gdk::Key::Return | gdk::Key::KP_Enter
                if modifiers.is_empty() && self.start_properties() =>
            {
                glib::Propagation::Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter
                if modifiers.is_empty() && options.smart_lists =>
            {
                self.enter()
            }
            gdk::Key::Tab if modifiers.is_empty() => self.indent(true),
            gdk::Key::ISO_Left_Tab | gdk::Key::Tab
                if modifiers == gdk::ModifierType::SHIFT_MASK =>
            {
                self.indent(false)
            }
            _ => match key.to_unicode() {
                Some(c) if (modifiers - gdk::ModifierType::SHIFT_MASK).is_empty() => {
                    self.typed(c, &options)
                }
                _ => glib::Propagation::Proceed,
            },
        }
    }

    /// `---` and Enter on a note's first line starts its properties, as in
    /// Obsidian: the closing `---` is added, the cursor moves below it and
    /// Add Property takes the focus. Undo puts the `---` back. Returns
    /// whether it did.
    pub(crate) fn start_properties(&self) -> bool {
        if self.mode() != Mode::Live {
            return false;
        }
        let buffer = self.buffer();
        if buffer.has_selection() {
            return false;
        }
        let cursor = buffer.iter_at_mark(&buffer.get_insert());
        if cursor.line() != 0 || !cursor.ends_line() {
            return false;
        }
        let (_, line) = self.line_before_cursor();
        let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true);
        if line.trim_end() != "---" || igneous_markdown::frontmatter::detect(&text).is_some() {
            return false;
        }
        buffer.begin_user_action();
        let mut at = cursor;
        let closing = if at.is_end() { "\n---\n" } else { "\n---" };
        buffer.insert(&mut at, closing);
        let body = buffer.iter_at_line(2).unwrap_or_else(|| buffer.end_iter());
        buffer.place_cursor(&body);
        buffer.end_user_action();
        self.imp().focus_new_property.set(true);
        true
    }

    /// [`NoteView::start_properties`], for tests.
    #[doc(hidden)]
    pub fn start_properties_for_test(&self) -> bool {
        self.start_properties()
    }

    fn line_before_cursor(&self) -> (gtk::TextIter, String) {
        let buffer = self.buffer();
        let cursor = buffer.iter_at_mark(&buffer.get_insert());
        let mut start = cursor;
        start.set_line_offset(0);
        (cursor, start.slice(&cursor).to_string())
    }

    fn enter(&self) -> glib::Propagation {
        let buffer = self.buffer();
        if buffer.has_selection() {
            return glib::Propagation::Proceed;
        }
        let (mut cursor, line) = self.line_before_cursor();
        // Only at the end of the line.
        if !cursor.ends_line() {
            return glib::Propagation::Proceed;
        }
        match continue_list(&line) {
            Some(Some(next)) => {
                buffer.begin_user_action();
                buffer.insert(&mut cursor, &next);
                buffer.end_user_action();
                self.scroll_mark_onscreen(&buffer.get_insert());
                glib::Propagation::Stop
            }
            Some(None) => {
                // An empty item: Enter ends the list.
                let mut start = cursor;
                start.set_line_offset(0);
                buffer.begin_user_action();
                buffer.delete(&mut start, &mut cursor);
                buffer.end_user_action();
                glib::Propagation::Stop
            }
            None => glib::Propagation::Proceed,
        }
    }

    /// Tab and Shift+Tab indent and outdent list items (every line of a
    /// selection).
    fn indent(&self, deeper: bool) -> glib::Propagation {
        let buffer = self.buffer();
        let (a, b) = buffer.selection_bounds().unwrap_or_else(|| {
            let c = buffer.iter_at_mark(&buffer.get_insert());
            (c, c)
        });
        let (first, last) = (a.line(), b.line());
        let line_text = |line: i32| {
            let start = buffer.iter_at_line(line).unwrap_or(buffer.end_iter());
            let mut end = start;
            end.forward_to_line_end();
            start.slice(&end).to_string()
        };
        if !(first..=last).any(|l| is_list_item(&line_text(l))) {
            return glib::Propagation::Proceed;
        }
        let options = self.input_options();
        let unit = if options.indent_with_tabs {
            "\t".to_owned()
        } else {
            " ".repeat(options.tab_size as usize)
        };
        buffer.begin_user_action();
        for line in first..=last {
            let text = line_text(line);
            if !is_list_item(&text) {
                continue;
            }
            let Some(mut start) = buffer.iter_at_line(line) else {
                continue;
            };
            if deeper {
                buffer.insert(&mut start, &unit);
            } else {
                let remove = if text.starts_with('\t') {
                    1
                } else {
                    text.chars()
                        .take_while(|c| *c == ' ')
                        .count()
                        .min(options.tab_size as usize)
                };
                let mut end = start;
                end.forward_chars(remove as i32);
                buffer.delete(&mut start, &mut end);
            }
        }
        buffer.end_user_action();
        glib::Propagation::Stop
    }

    /// Pairs brackets, wraps a selection in Markdown markers, and types over
    /// a closing character that's already there.
    fn typed(&self, c: char, options: &InputOptions) -> glib::Propagation {
        let buffer = self.buffer();
        let selection = buffer.selection_bounds();
        let wraps = (options.auto_pair_markdown && matches!(c, '*' | '_' | '`' | '=' | '~'))
            || (options.auto_pair_brackets && closing(c).is_some());
        if let Some((mut a, mut b)) = selection
            && wraps
        {
            let close = closing(c).unwrap_or(c).to_string();
            let open = c.to_string();
            let (start_offset, end_offset) = (a.offset(), b.offset());
            buffer.begin_user_action();
            buffer.insert(&mut b, &close);
            a = buffer.iter_at_offset(start_offset);
            buffer.insert(&mut a, &open);
            buffer.end_user_action();
            let a = buffer.iter_at_offset(start_offset + 1);
            let b = buffer.iter_at_offset(end_offset + 1);
            buffer.select_range(&a, &b);
            return glib::Propagation::Stop;
        }
        if selection.is_some() || !options.auto_pair_brackets {
            return glib::Propagation::Proceed;
        }
        let mut cursor = buffer.iter_at_mark(&buffer.get_insert());
        // Typing a closing bracket that's already next: step over it.
        if matches!(c, ')' | ']' | '}') && cursor.char() == c {
            cursor.forward_char();
            buffer.place_cursor(&cursor);
            return glib::Propagation::Stop;
        }
        if let Some(close) = closing(c) {
            // Only before whitespace, punctuation or the end of the line.
            let next = cursor.char();
            if next == '\0' || next.is_whitespace() || matches!(next, ')' | ']' | '}' | ',' | '.') {
                buffer.begin_user_action();
                buffer.insert(&mut cursor, &format!("{c}{close}"));
                buffer.end_user_action();
                let mut back = buffer.iter_at_mark(&buffer.get_insert());
                back.backward_char();
                buffer.place_cursor(&back);
                return glib::Propagation::Stop;
            }
        }
        glib::Propagation::Proceed
    }

    // --- pasting ---------------------------------------------------------------

    fn smart_paste(&self) {
        let clipboard = self.clipboard();
        let formats = clipboard.formats();
        let view = self.clone();
        glib::spawn_future_local(async move {
            // Files first (copied in Files), then images, then HTML, then text.
            if formats.contains_type(gdk::FileList::static_type())
                && let Ok(value) = clipboard
                    .read_value_future(gdk::FileList::static_type(), glib::Priority::DEFAULT)
                    .await
                && let Ok(files) = value.get::<gdk::FileList>()
                && view.attach_files(files.files())
            {
                return;
            }
            if formats.contains_type(gdk::Texture::static_type())
                && !formats.contain_mime_type("text/plain")
                && let Ok(Some(texture)) = clipboard.read_texture_future().await
            {
                let bytes = texture.save_to_png_bytes();
                if let Some(link) = view.host().save_attachment("Pasted image.png", &bytes) {
                    view.insert_at_cursor(&link);
                }
                return;
            }
            let text = clipboard.read_text_future().await.ok().flatten();
            if formats.contain_mime_type("text/html")
                && let Some(markdown) = read_html(&clipboard).await
                && !markdown.trim().is_empty()
            {
                view.insert_at_cursor(&markdown);
                return;
            }
            let Some(text) = text else { return };
            let buffer = view.buffer();
            if is_url(&text)
                && let Some((a, b)) = buffer.selection_bounds()
            {
                let label = a.slice(&b).to_string();
                if !label.contains('\n') {
                    view.insert_at_cursor(&format!("[{label}]({})", text.trim()));
                    return;
                }
            }
            view.insert_at_cursor(&text);
        });
    }

    /// Replaces the selection (if any) with `text`, as one undo step.
    pub(crate) fn insert_at_cursor(&self, text: &str) {
        let buffer = self.buffer();
        buffer.begin_user_action();
        buffer.delete_selection(true, self.is_editable());
        buffer.insert_at_cursor(text);
        buffer.end_user_action();
        self.scroll_mark_onscreen(&buffer.get_insert());
    }

    /// Copies files into the vault as attachments and embeds them at the
    /// cursor. Returns whether anything was attached.
    fn attach_files(&self, files: Vec<gio::File>) -> bool {
        let host = self.host();
        let links: Vec<String> = files
            .iter()
            .filter_map(|f| f.path())
            .filter_map(|p| host.attach_file(&p))
            .collect();
        if links.is_empty() {
            return false;
        }
        self.insert_at_cursor(&links.join("\n"));
        true
    }
}

async fn read_html(clipboard: &gdk::Clipboard) -> Option<String> {
    let (stream, _) = clipboard
        .read_future(&["text/html"], glib::Priority::DEFAULT)
        .await
        .ok()?;
    let bytes = stream
        .read_bytes_future(4 * 1024 * 1024, glib::Priority::DEFAULT)
        .await
        .ok()?;
    let html = String::from_utf8_lossy(&bytes);
    htmd::convert(&html).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_continue() {
        assert_eq!(continue_list("- item"), Some(Some("\n- ".into())));
        assert_eq!(continue_list("\t* nested"), Some(Some("\n\t* ".into())));
        assert_eq!(continue_list("- [x] done"), Some(Some("\n- [ ] ".into())));
        assert_eq!(continue_list("3. third"), Some(Some("\n4. ".into())));
        assert_eq!(continue_list("9) nine"), Some(Some("\n10) ".into())));
        assert_eq!(continue_list("> quoted"), Some(Some("\n> ".into())));
        assert_eq!(continue_list("> [!note] Title"), Some(Some("\n> ".into())));
        // Empty items end the list.
        assert_eq!(continue_list("- "), Some(None));
        assert_eq!(continue_list("- [ ] "), Some(None));
        assert_eq!(continue_list("2. "), Some(None));
        // Not lists.
        assert_eq!(continue_list("plain text"), None);
        assert_eq!(continue_list("-not a list"), None);
        assert_eq!(continue_list("2026. a year"), Some(Some("\n2027. ".into())));
    }

    #[test]
    fn urls() {
        assert!(is_url("https://example.com/a?b=c"));
        assert!(is_url(" mailto:me@example.com\n"));
        assert!(!is_url("not a url"));
        assert!(!is_url("https://a b"));
    }

    fn view(text: &str) -> NoteView {
        let view = NoteView::new();
        let buffer = view.source_buffer();
        buffer.set_text(text);
        buffer.place_cursor(&buffer.end_iter());
        view
    }

    #[gtk::test]
    fn enter_continues_and_ends_lists() {
        let view = view("- one");
        assert_eq!(view.enter(), glib::Propagation::Stop);
        assert_eq!(view.model_text(), "- one\n- ");
        assert_eq!(view.enter(), glib::Propagation::Stop);
        assert_eq!(view.model_text(), "- one\n");
        view.check_invariants().unwrap();
    }

    #[gtk::test]
    fn tab_indents_list_items() {
        let view = view("- one\n- two");
        assert_eq!(view.indent(true), glib::Propagation::Stop);
        assert_eq!(view.model_text(), "- one\n\t- two");
        assert_eq!(view.indent(false), glib::Propagation::Stop);
        assert_eq!(view.model_text(), "- one\n- two");
        let plain = NoteView::new();
        plain.source_buffer().set_text("text");
        assert_eq!(plain.indent(true), glib::Propagation::Proceed);
    }

    #[gtk::test]
    fn pairs_and_wraps() {
        let options = InputOptions::default();
        let link = view("see ");
        assert_eq!(link.typed('[', &options), glib::Propagation::Stop);
        assert_eq!(link.model_text(), "see []");
        assert_eq!(link.typed(']', &options), glib::Propagation::Stop);
        assert_eq!(link.model_text(), "see []");
        let bold = view("bold me");
        let buffer = bold.buffer();
        buffer.select_range(&buffer.iter_at_offset(0), &buffer.iter_at_offset(4));
        bold.typed('*', &options);
        bold.typed('*', &options);
        assert_eq!(bold.model_text(), "**bold** me");
    }
}
