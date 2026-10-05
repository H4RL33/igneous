# Translations

Igneous uses gettext. Translatable text lives in the Blueprint UI files
(`_("…")`), the desktop file, the metainfo and the settings schema; the Rust
code's own messages aren't marked for translation yet.

To start a translation, with a Meson build directory (`meson setup _build`):

1. Refresh the template: `meson compile -C _build igneous-pot`
   (writes `po/igneous.pot`).
2. Copy it to `po/<code>.po`, where `<code>` is the language, such as `de`
   or `pt_BR`, and fill in the header (language, plural forms).
3. Add the code on its own line in `po/LINGUAS`.
4. Translate the strings, for example with GNOME Translation Editor.
5. Build and install as usual; the translation is compiled into
   `share/locale/<code>/LC_MESSAGES/igneous.mo`.

When strings change, `meson compile -C _build igneous-update-po` updates
every `.po` file from the template.

Strings in `C_("context", "…")` carry a context to tell apart words that
are the same in English but not in other languages.
