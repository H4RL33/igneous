# Using Igneous

Igneous reads and edits folders of Markdown notes ("vaults"), including vaults made with Obsidian. Notes stay ordinary Markdown files; Igneous changes a file only when you edit it, or when you ask for something vault-wide such as renaming a note and updating the links to it.

## Opening a vault

When Igneous starts, it shows your recent vaults and an **Open Folder…** button. Any folder of Markdown files is a vault; nothing needs converting.

- Igneous keeps its own settings in a `.igneous` folder inside the vault: open tabs, appearance, Git and linter settings. Copy or sync the vault and they come along.
- It never reads or writes Obsidian's `.obsidian` folder, so the two apps can share a vault without disturbing each other.
- Each window shows one vault. **Open Vault…** in the main menu (Ctrl+Shift+N) opens another in a new window.
- If another program changes a note, Igneous reloads it. If you had unsaved edits, it shows a banner instead of overwriting either version.

## The window

- **Sidebar (F9):**
  - **Files:** the vault's folders. Right-click for new notes and folders, renaming, moving to the Trash, history and linting; drag to move.
  - **Search** (Ctrl+Shift+F).
  - **Tags.**
  - **Changes**, when the vault is a Git repository.
- **Tabs:** pin a tab from its menu. Ctrl+Shift+T reopens a closed tab. On narrow windows, the tab button shows every tab.
- **Inspector:** the button at the right of the header bar. It shows:
  - what links to the note, and where it's mentioned without a link;
  - what the note links to;
  - its outline;
  - its local graph.
- **Find Note (Ctrl+O):** type part of a name or alias. `note#heading` jumps to a heading; Shift+Enter creates the note you typed.
- **Command palette (Ctrl+P):** every command, with its shortcut.

## Writing

Notes save themselves a moment after you stop typing, and whenever you switch away. Ctrl+S saves straight away.

### Live Preview, Source and Reading

The mode button in the header bar switches to the next mode in turn, and its icon shows which one that is; **Show Note As** in the main menu picks one directly, and Ctrl+E toggles Reading.

- **Live Preview** formats the note as you type and hides Markdown syntax except around the cursor.
- **Source** shows plain Markdown with syntax highlighting.
- **Reading** is Live Preview without editing; a click follows a link.

What Live Preview shows:

- headings, bold, italic, highlights, strikethrough, code and links;
- tasks as checkboxes you can tick;
- bullets, quotes, and callouts with icons and colours;
- tables as grids, `$$…$$` math typeset, images and embedded notes (`![[Note]]`);
- the properties at the top as editable fields.

Move the cursor into any of these to see and edit its source.

**Folding:** the arrows beside headings fold their section. Callouts written `> [!note]-` start folded.

### Links

- Type `[[` to pick a note, `[[Note#` for one of its headings, `[[Note#^` for a block, and `#` for a tag.
- **Ctrl+click** follows a link; **middle-click** opens it in a new tab. Following a link to a note that doesn't exist yet creates it.
- Hold **Ctrl** over a link to preview the note.
- **Alt+←** and **Alt+→** go back and forward through the notes a tab has shown.
- Renaming or moving a note or folder (F2, or drag in the sidebar) rewrites every link to it in the vault. The toast that follows has an **Undo** button.

### Typing helpers

- Enter continues a list or quote; Enter on an empty item ends it.
- Tab and Shift+Tab indent and outdent list items.
- Brackets pair up. Typing `*`, `_`, `` ` ``, `=` or `~` with text selected wraps the selection.
- Pasting a web address over selected text makes a link. Pasting from a web page gives Markdown.
- Pasting an image, or dropping a file, saves it in the vault as an attachment and embeds it. **Preferences → Files & Links** sets where attachments go.

### Properties

The fields at the top of a note are its YAML frontmatter:

- text, lists (tags and aliases too), numbers, checkboxes, and dates with a calendar;
- each field's menu changes its type for the whole vault, or removes it;
- **Add Property** adds one.

Igneous changes only the property you edit, leaving the rest of the frontmatter, comments included, exactly as written.

### Daily notes, templates and bookmarks

- **Daily notes (Ctrl+Alt+D)** opens today's note, creating it if needed. Ctrl+Alt+Page Up and Page Down go to the previous and next ones, and the calendar button in the sidebar opens any day (days with a note are marked).
- **Insert Template** (main menu or command palette) inserts a note from your templates folder at the cursor, filling in `{{title}}`, `{{date}}`, `{{time}}`, `{{date:YYYY-MM-DD}}` and `{{time:HH:mm}}`. The template's properties merge into the note's.
- **Bookmarks** collect notes, folders, headings and searches in the sidebar's Bookmarks page; **Bookmark Note** in the main menu or command palette adds or removes the open note. They follow renames.
- **A startup note** can open whenever the vault does.

**Preferences → Notes** sets the daily notes folder, date format and template, the templates folder, and the startup note.

### File recovery

While you edit, Igneous keeps snapshots of each note: at most one every few minutes, for a week, up to a size limit. **Note Snapshots…** in the main menu lists them. Open one to read it, then use **Restore** to put it back as one step you can undo. Snapshots are kept outside the vault, in `~/.local/share/igneous/snapshots/`.

## Finding things

**Search (Ctrl+Shift+F)** understands Obsidian's search syntax:

| Search | Finds |
|---|---|
| `meeting notes` | notes containing both words |
| `"meeting notes"` | the exact phrase |
| `meeting OR call` | either |
| `-draft` | notes without "draft" |
| `/\d{4}-\d{2}/` | a regular expression |
| `file:roadmap`, `path:Projects` | by name or folder |
| `tag:#project` | by tag, including nested tags |
| `line:(todo urgent)`, `section:(…)`, `block:(…)` | words together on one line, under one heading, or in one paragraph |
| `task-todo:call` | unfinished tasks mentioning "call" |
| `[status:draft]`, `[due]` | by property |

The **Aa** button makes a search case-sensitive.

**Tags** lists every tag as a tree. Click a tag to search for it, or rename it everywhere with its button.

**Bases** (`.base` files) open as tables, cards or lists of the notes they select. To use them:

- Sort, resize or reorder columns; Igneous writes the change back to the `.base` file, as Obsidian does.
- `![[File.base]]` and ` ```base ` blocks show a base inside a note.

**Graph (Ctrl+G)** shows every note and link. To work with it:

- Drag a note to move it, drag the background to pan, and Ctrl+scroll to zoom.
- The settings button filters it with search syntax, colours groups of notes, and tunes the forces.
- The inspector's Graph page shows the open note's neighbourhood.

## Git sync

If the vault is a Git repository, Igneous can keep it in sync, like obsidian-git:

- **The sync button** in the header bar shows the state. Its popover has **Sync Now** (Ctrl+Alt+S), **Pull**, and **Publish Branch** for branches without an upstream.
- **Automatic sync** is in **Preferences → Sync**, off by default. When on, Igneous commits, pulls and pushes every few minutes, and can pull when the vault opens. The commit message template can use `{{date}}`, `{{hostname}}`, `{{numFiles}}` and `{{files}}`.
- **The Changes pane** lists changed files: stage, unstage, discard or view the diff of each, and commit with a message. **History** in a file's menu lists its commits and opens old versions.
- **Conflicts** pause syncing. A banner says how many files are affected, and each one shows a bar with **Keep Mine**, **Keep Theirs** and **Keep Both**. When they're resolved, mark them in the Changes pane and commit; syncing resumes.

Igneous never force-pushes, resets or resolves conflicts on its own. It uses your own `git`, so your SSH keys, credential helpers and hooks apply.

Vaults encrypted with git-crypt aren't supported yet. Igneous refuses to sync them rather than risk pushing plaintext.

## Linting

The linter follows obsidian-linter's rules, with the same names. Turn rules on and set their options in **Preferences → Linter**.

- **Lint Note** (Ctrl+Alt+L) fixes the open note in one undoable step.
- **Lint Folder** (in a folder's menu) and **Lint Vault** (the file tree's empty area, or the command palette) ask first, and suggest committing beforehand if the vault uses Git.
- **Lint When Saving** lints when you press Ctrl+S, never on autosave.
- With **Underline Problems** on, issues are underlined; hover one to see why, and click **Fix** to fix it.

Code, math, frontmatter (except for YAML rules) and anything between `<!-- linter-disable -->` and `<!-- linter-enable -->` are never changed.

## Appearance and editing preferences

- **Appearance:** the editor theme. Themes come with Igneous, from your themes folder, or from the vault. See [themes.md](themes.md) for writing your own.
- **Editor:**
  - the mode notes open in;
  - readable line length;
  - line numbers in Source mode;
  - Vim keybindings (`:w` saves);
  - spell checking;
  - list and pairing helpers, tabs, and the autosave delay;
  - a Linked Mentions section at the end of each note.
- **Files & Links:**
  - where new notes and attachments go;
  - whether deleting uses the Trash or the vault's `.trash` folder;
  - updating links on rename;
  - files to hide.

## Keyboard shortcuts

| Action | Shortcut |
|---|---|
| Find note | Ctrl+O |
| Command palette | Ctrl+P |
| Search vault | Ctrl+Shift+F |
| New note | Ctrl+N |
| Rename note | F2 |
| Save now | Ctrl+S |
| Toggle Reading | Ctrl+E |
| Back / forward | Alt+← / Alt+→ |
| Close tab / reopen closed tab | Ctrl+W / Ctrl+Shift+T |
| Graph | Ctrl+G |
| Today's daily note | Ctrl+Alt+D |
| Previous / next daily note | Ctrl+Alt+Page Up / Ctrl+Alt+Page Down |
| Sync now | Ctrl+Alt+S |
| Lint note | Ctrl+Alt+L |
| Toggle sidebar | F9 |
| Preferences | Ctrl+, |
| Open another vault | Ctrl+Shift+N |
| Keyboard shortcuts | Ctrl+? |
| Quit | Ctrl+Q |

## Where things are kept

| Where | What |
|---|---|
| `<vault>/.igneous/` | the vault's settings, open tabs, appearance, Git, linter, graph, bookmarks and its own themes |
| `~/.cache/igneous/` | the index and generated style schemes; safe to delete |
| `~/.local/share/igneous/themes/` | your own editor themes |
| `~/.local/share/igneous/snapshots/` | file recovery snapshots |
| GSettings `dev.h4rl3y.igneous` | recent vaults, window size, the default editor theme |

## Not yet

- Inline `$…$` math stays as text; display `$$…$$` math is typeset.
- Mermaid diagrams stay as code.
- git-crypt vaults can't be synced yet.
- Editing cells in Bases tables, and Bases' kanban view, are planned for later.
