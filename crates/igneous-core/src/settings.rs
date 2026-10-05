//! Settings and state stored in `<vault>/.igneous/`.
//!
//! Each file is versioned JSON. Keys this build doesn't know about are kept
//! (`extra`), so an older Igneous doesn't strip settings written by a newer
//! one. Output is deterministic: declaration order, two-space indent and a
//! trailing newline, so saving unchanged settings rewrites nothing.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::fs::{Expect, write_bytes_atomic};
use crate::path::VaultPath;

/// A settings file in `.igneous/`.
pub trait SettingsFile: Serialize + DeserializeOwned + Default {
    const FILE_NAME: &'static str;
    const VERSION: u32;

    /// Upgrades a document written by version `from` to version `from + 1`.
    fn migrate(value: Value, _from: u32) -> Value {
        value
    }

    fn set_version(&mut self, version: u32);
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("{file}: {source}")]
    Parse {
        file: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error(
        "{file} was written by a newer Igneous (format {found}; this build reads up to {supported})"
    )]
    TooNew {
        file: &'static str,
        found: u32,
        supported: u32,
    },
    #[error(transparent)]
    Io(#[from] io::Error),
}

pub fn path_of<T: SettingsFile>(igneous_dir: &Path) -> PathBuf {
    igneous_dir.join(T::FILE_NAME)
}

/// Loads a settings file. A missing file gives the defaults; a malformed one
/// is an error, so callers never overwrite settings they couldn't read.
pub fn load<T: SettingsFile>(igneous_dir: &Path) -> Result<T, SettingsError> {
    let bytes = match std::fs::read(path_of::<T>(igneous_dir)) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(T::default()),
        Err(e) => return Err(e.into()),
    };
    from_bytes(&bytes)
}

pub fn from_bytes<T: SettingsFile>(bytes: &[u8]) -> Result<T, SettingsError> {
    let parse = |source| SettingsError::Parse {
        file: T::FILE_NAME,
        source,
    };
    let mut value: Value = serde_json::from_slice(bytes).map_err(parse)?;
    let found = value
        .get("version")
        .and_then(Value::as_u64)
        .map_or(1, |v| v as u32);
    if found > T::VERSION {
        return Err(SettingsError::TooNew {
            file: T::FILE_NAME,
            found,
            supported: T::VERSION,
        });
    }
    for from in found..T::VERSION {
        value = T::migrate(value, from);
    }
    let mut settings: T = serde_json::from_value(value).map_err(parse)?;
    settings.set_version(T::VERSION);
    Ok(settings)
}

pub fn to_bytes<T: SettingsFile>(settings: &T) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(settings).expect("settings always serialise");
    bytes.push(b'\n');
    bytes
}

/// Saves a settings file, creating `.igneous/` if needed. Returns `false`
/// without touching the disk if the file already holds exactly these bytes.
pub fn save<T: SettingsFile>(igneous_dir: &Path, settings: &T) -> Result<bool, SettingsError> {
    let path = path_of::<T>(igneous_dir);
    let bytes = to_bytes(settings);
    if std::fs::read(&path).is_ok_and(|existing| existing == bytes) {
        return Ok(false);
    }
    std::fs::create_dir_all(igneous_dir)?;
    write_bytes_atomic(&path, &bytes, Expect::Anything).map_err(|e| match e {
        crate::fs::WriteError::Io(e) => SettingsError::Io(e),
        crate::fs::WriteError::ChangedOnDisk { .. } => unreachable!("unconditional write"),
    })?;
    Ok(true)
}

macro_rules! settings_file {
    ($ty:ty, $file:literal, $version:literal) => {
        impl SettingsFile for $ty {
            const FILE_NAME: &'static str = $file;
            const VERSION: u32 = $version;
            fn set_version(&mut self, version: u32) {
                self.version = version;
            }
        }
    };
}

fn one() -> u32 {
    1
}

// --- vault.json ------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct VaultSettings {
    #[serde(default = "one")]
    pub version: u32,
    pub files: FileSettings,
    pub links: LinkSettings,
    pub editor: EditorSettings,
    pub properties: PropertySettings,
    pub daily_notes: DailyNoteSettings,
    pub templates: TemplateSettings,
    /// Note opened when the vault opens, if any.
    pub startup_note: Option<VaultPath>,
    pub recovery: RecoverySettings,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
settings_file!(VaultSettings, "vault.json", 1);

impl Default for VaultSettings {
    fn default() -> Self {
        Self {
            version: 1,
            files: FileSettings::default(),
            links: LinkSettings::default(),
            editor: EditorSettings::default(),
            properties: PropertySettings::default(),
            daily_notes: DailyNoteSettings::default(),
            templates: TemplateSettings::default(),
            startup_note: None,
            recovery: RecoverySettings::default(),
            extra: Map::new(),
        }
    }
}

/// File recovery: snapshots of notes as they're edited, kept outside the
/// vault (in `$XDG_DATA_HOME/igneous/snapshots/`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RecoverySettings {
    /// Minutes between snapshots of a note being edited; 0 takes one at
    /// every save.
    pub interval_minutes: u32,
    /// Snapshots older than this many days are deleted.
    pub keep_days: u32,
    /// The vault's snapshots are kept under this many megabytes, oldest
    /// deleted first.
    pub max_megabytes: u32,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for RecoverySettings {
    fn default() -> Self {
        Self {
            interval_minutes: 5,
            keep_days: 7,
            max_megabytes: 100,
            extra: Map::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FileSettings {
    pub new_note_location: Location,
    pub attachment_location: Location,
    pub trash: TrashMode,
    pub confirm_delete: bool,
    /// Paths or globs hidden from the file tree, search and graph.
    pub excluded: Vec<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for FileSettings {
    fn default() -> Self {
        Self {
            new_note_location: Location::VaultRoot,
            attachment_location: Location::VaultRoot,
            trash: TrashMode::System,
            confirm_delete: true,
            excluded: Vec::new(),
            extra: Map::new(),
        }
    }
}

/// Where new notes or attachments go.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Location {
    VaultRoot,
    /// The folder of the note being edited.
    SameFolder,
    /// A fixed folder in the vault.
    Folder(String),
    /// A subfolder of the folder of the note being edited.
    Subfolder(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrashMode {
    /// The desktop's trash.
    System,
    /// `<vault>/.trash/`
    VaultFolder,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LinkSettings {
    pub style: LinkStyle,
    pub path: LinkPath,
    pub update_on_rename: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for LinkSettings {
    fn default() -> Self {
        Self {
            style: LinkStyle::Wiki,
            path: LinkPath::Shortest,
            update_on_rename: true,
            extra: Map::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkStyle {
    Wiki,
    Markdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkPath {
    Shortest,
    Relative,
    Absolute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EditorMode {
    Live,
    Source,
    Reading,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EditorSettings {
    pub default_mode: EditorMode,
    pub indent_with_tabs: bool,
    pub tab_size: u32,
    pub auto_pair_brackets: bool,
    pub auto_pair_markdown: bool,
    pub smart_lists: bool,
    pub strict_line_breaks: bool,
    pub show_line_numbers: bool,
    pub spellcheck: bool,
    pub vim_mode: bool,
    pub fold_headings: bool,
    pub fold_indent: bool,
    pub autosave_delay_ms: u32,
    /// Show the notes linking here at the end of each note.
    pub backlinks_in_document: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            default_mode: EditorMode::Live,
            indent_with_tabs: true,
            tab_size: 4,
            auto_pair_brackets: true,
            auto_pair_markdown: true,
            smart_lists: true,
            strict_line_breaks: false,
            show_line_numbers: false,
            spellcheck: true,
            vim_mode: false,
            fold_headings: true,
            fold_indent: true,
            autosave_delay_ms: 2000,
            backlinks_in_document: false,
            extra: Map::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PropertySettings {
    /// Property types set explicitly; everything else is inferred.
    pub types: BTreeMap<String, PropertyType>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PropertyType {
    Text,
    List,
    Number,
    Checkbox,
    Date,
    Datetime,
    Tags,
    Aliases,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DailyNoteSettings {
    pub folder: String,
    /// Moment-style date format for file names (see [`crate::datefmt`]).
    pub format: String,
    pub template: Option<VaultPath>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for DailyNoteSettings {
    fn default() -> Self {
        Self {
            folder: String::new(),
            format: "YYYY-MM-DD".into(),
            template: None,
            extra: Map::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TemplateSettings {
    pub folder: Option<String>,
    pub date_format: String,
    pub time_format: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for TemplateSettings {
    fn default() -> Self {
        Self {
            folder: None,
            date_format: "YYYY-MM-DD".into(),
            time_format: "HH:mm".into(),
            extra: Map::new(),
        }
    }
}

// --- appearance.json -------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Appearance {
    #[serde(default = "one")]
    pub version: u32,
    pub color_scheme: ColorScheme,
    /// Overrides the system document font's family.
    pub text_font: Option<String>,
    /// Overrides the system monospace font's family.
    pub monospace_font: Option<String>,
    pub font_scale: f64,
    /// The editor's base size in points, overriding the document font's.
    /// Headings and other sized text scale from it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f64>,
    pub readable_line_length: bool,
    /// The text column's width in pixels with readable line length on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_width: Option<u32>,
    /// The editor theme's id. When unset, the app-wide default is used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub editor_theme: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
settings_file!(Appearance, "appearance.json", 1);

impl Default for Appearance {
    fn default() -> Self {
        Self {
            version: 1,
            color_scheme: ColorScheme::System,
            text_font: None,
            monospace_font: None,
            font_scale: 1.0,
            font_size: None,
            readable_line_length: true,
            line_width: None,
            editor_theme: None,
            extra: Map::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ColorScheme {
    System,
    Light,
    Dark,
}

// --- hotkeys.json ----------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Hotkeys {
    #[serde(default = "one")]
    pub version: u32,
    /// Action name → accelerators (GTK syntax). An empty list unbinds.
    pub bindings: BTreeMap<String, Vec<String>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
settings_file!(Hotkeys, "hotkeys.json", 1);

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            version: 1,
            bindings: BTreeMap::new(),
            extra: Map::new(),
        }
    }
}

// --- workspace.json --------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Workspace {
    #[serde(default = "one")]
    pub version: u32,
    pub tabs: Vec<TabState>,
    pub active_tab: Option<usize>,
    pub sidebar: SidebarState,
    pub inspector: InspectorState,
    pub recently_closed: Vec<TabState>,
    /// Recently opened files, most recent first.
    pub recent_files: Vec<VaultPath>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
settings_file!(Workspace, "workspace.json", 1);

impl Default for Workspace {
    fn default() -> Self {
        Self {
            version: 1,
            tabs: Vec::new(),
            active_tab: None,
            sidebar: SidebarState::default(),
            inspector: InspectorState::default(),
            recently_closed: Vec::new(),
            recent_files: Vec::new(),
            extra: Map::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabState {
    pub kind: TabKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<VaultPath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<EditorMode>,
    /// Cursor position as a byte offset into the note.
    #[serde(default)]
    pub cursor: usize,
    #[serde(default)]
    pub scroll: f64,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub back: Vec<VaultPath>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forward: Vec<VaultPath>,
}

impl TabState {
    pub fn new(kind: TabKind, path: Option<VaultPath>) -> Self {
        Self {
            kind,
            path,
            mode: None,
            cursor: 0,
            scroll: 0.0,
            pinned: false,
            back: Vec::new(),
            forward: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TabKind {
    Note,
    Base,
    Graph,
    Image,
    Diff,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SidebarState {
    pub visible: bool,
    pub pane: SidebarPane,
    pub width: u32,
    pub expanded: Vec<VaultPath>,
}

impl Default for SidebarState {
    fn default() -> Self {
        Self {
            visible: true,
            pane: SidebarPane::Files,
            width: 280,
            expanded: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SidebarPane {
    Files,
    Search,
    Tags,
    Changes,
    Bookmarks,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct InspectorState {
    pub visible: bool,
    pub view: InspectorView,
    pub width: u32,
}

impl Default for InspectorState {
    fn default() -> Self {
        Self {
            visible: false,
            view: InspectorView::Backlinks,
            width: 300,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InspectorView {
    Backlinks,
    Outline,
    LocalGraph,
}

/// Loads and saves `workspace.json`, which unlike the other files must never
/// stop a vault from opening.
#[derive(Debug)]
pub struct WorkspaceStore {
    igneous_dir: PathBuf,
    last_written: Option<Vec<u8>>,
}

impl WorkspaceStore {
    pub fn new(igneous_dir: impl Into<PathBuf>) -> Self {
        Self {
            igneous_dir: igneous_dir.into(),
            last_written: None,
        }
    }

    /// Loads the workspace. Anything unreadable (including leftover merge
    /// conflict markers) gives an empty workspace plus the reason.
    pub fn load(&mut self) -> (Workspace, Option<SettingsError>) {
        match load::<Workspace>(&self.igneous_dir) {
            Ok(workspace) => {
                self.last_written = Some(to_bytes(&workspace));
                (workspace, None)
            }
            Err(e) => (Workspace::default(), Some(e)),
        }
    }

    /// Writes the workspace if it differs from what was last loaded or
    /// written. Debouncing is the caller's job.
    pub fn save_if_changed(&mut self, workspace: &Workspace) -> Result<bool, SettingsError> {
        let bytes = to_bytes(workspace);
        if self.last_written.as_ref() == Some(&bytes) {
            return Ok(false);
        }
        let written = save(&self.igneous_dir, workspace)?;
        self.last_written = Some(bytes);
        Ok(written)
    }
}

// --- git.json --------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GitSettings {
    #[serde(default = "one")]
    pub version: u32,
    /// Sync is off until the user turns it on for the vault.
    pub enabled: bool,
    /// Minutes between commit-and-sync runs; 0 turns it off.
    pub sync_interval: u32,
    /// Minutes between pulls; 0 turns it off.
    pub pull_interval: u32,
    pub pull_on_open: bool,
    pub push: bool,
    pub method: SyncMethod,
    pub commit_message: String,
    /// Moment-style format for `{{date}}`.
    pub date_format: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
settings_file!(GitSettings, "git.json", 1);

impl Default for GitSettings {
    fn default() -> Self {
        Self {
            version: 1,
            enabled: false,
            sync_interval: 5,
            pull_interval: 5,
            pull_on_open: true,
            push: true,
            method: SyncMethod::Merge,
            commit_message: "vault backup: {{date}}".into(),
            date_format: "YYYY-MM-DD HH:mm:ss".into(),
            extra: Map::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncMethod {
    Merge,
    Rebase,
}

// --- lint.json -------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LintSettings {
    #[serde(default = "one")]
    pub version: u32,
    pub lint_on_save: bool,
    /// Rule ID → configuration. Rules not listed are off.
    pub rules: BTreeMap<String, RuleConfig>,
    pub ignore_folders: Vec<String>,
    pub ignore_files: Vec<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
settings_file!(LintSettings, "lint.json", 1);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RuleConfig {
    pub enabled: bool,
    pub options: Map<String, Value>,
}

impl Default for LintSettings {
    fn default() -> Self {
        let rules = [
            "trailing-spaces",
            "line-break-at-document-end",
            "consecutive-blank-lines",
        ]
        .into_iter()
        .map(|id| {
            (
                id.to_owned(),
                RuleConfig {
                    enabled: true,
                    options: Map::new(),
                },
            )
        })
        .collect();
        Self {
            version: 1,
            lint_on_save: false,
            rules,
            ignore_folders: Vec::new(),
            ignore_files: Vec::new(),
            extra: Map::new(),
        }
    }
}

// --- graph.json ------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GraphSettings {
    #[serde(default = "one")]
    pub version: u32,
    /// Search query limiting which notes appear.
    pub filter: String,
    pub show_tags: bool,
    pub show_attachments: bool,
    pub show_orphans: bool,
    pub show_unresolved: bool,
    pub groups: Vec<GraphGroup>,
    pub arrows: bool,
    pub text_fade: f64,
    pub node_size: f64,
    pub link_thickness: f64,
    pub center_force: f64,
    pub repel_force: f64,
    pub link_force: f64,
    pub link_distance: f64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
settings_file!(GraphSettings, "graph.json", 1);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphGroup {
    pub query: String,
    /// CSS colour; `None` uses the next accent-derived colour.
    #[serde(default)]
    pub color: Option<String>,
}

impl Default for GraphSettings {
    fn default() -> Self {
        Self {
            version: 1,
            filter: String::new(),
            show_tags: false,
            show_attachments: false,
            show_orphans: true,
            show_unresolved: false,
            groups: Vec::new(),
            arrows: false,
            text_fade: 0.0,
            node_size: 1.0,
            link_thickness: 1.0,
            center_force: 0.5,
            repel_force: 10.0,
            link_force: 1.0,
            link_distance: 250.0,
            extra: Map::new(),
        }
    }
}

// --- bookmarks.json --------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Bookmarks {
    #[serde(default = "one")]
    pub version: u32,
    pub items: Vec<Bookmark>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
settings_file!(Bookmarks, "bookmarks.json", 1);

impl Default for Bookmarks {
    fn default() -> Self {
        Self {
            version: 1,
            items: Vec::new(),
            extra: Map::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Bookmark {
    File {
        path: VaultPath,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    Folder {
        path: VaultPath,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    Heading {
        path: VaultPath,
        heading: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    Search {
        query: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
}

// --- icons.json ------------------------------------------------------------

/// Custom icons for files and custom colours for files and folders, by
/// path. Igneous moves an entry when it renames or moves the file (or a
/// folder above it) and drops it when it deletes the file; changes made by
/// other programs aren't followed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Icons {
    #[serde(default = "one")]
    pub version: u32,
    /// File path → symbolic icon name, such as `starred-symbolic`.
    pub icons: BTreeMap<VaultPath, String>,
    /// File or folder path → the colour its icon is drawn in, as `#rrggbb`.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub colors: BTreeMap<VaultPath, String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// What [`Icons::remove_under`] took out, so it can be put back.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Removed {
    pub icons: Vec<(VaultPath, String)>,
    pub colors: Vec<(VaultPath, String)>,
}

impl Removed {
    pub fn is_empty(&self) -> bool {
        self.icons.is_empty() && self.colors.is_empty()
    }
}
settings_file!(Icons, "icons.json", 1);

impl Default for Icons {
    fn default() -> Self {
        Self {
            version: 1,
            icons: BTreeMap::new(),
            colors: BTreeMap::new(),
            extra: Map::new(),
        }
    }
}

impl Icons {
    pub fn get(&self, path: &VaultPath) -> Option<&str> {
        self.icons.get(path).map(String::as_str)
    }

    /// Sets the icon for `path`, or removes it with `None` (or an empty
    /// name). Returns whether anything changed.
    pub fn set(&mut self, path: &VaultPath, icon: Option<&str>) -> bool {
        set_entry(&mut self.icons, path, icon)
    }

    pub fn color(&self, path: &VaultPath) -> Option<&str> {
        self.colors.get(path).map(String::as_str)
    }

    /// Sets the colour for `path` (`#rrggbb`), or removes it with `None`.
    /// Returns whether anything changed.
    pub fn set_color(&mut self, path: &VaultPath, color: Option<&str>) -> bool {
        set_entry(&mut self.colors, path, color)
    }

    /// Moves the icons and colours of `from`, a file or a folder, and of
    /// everything under it to `to`. Returns whether anything moved.
    pub fn follow_rename(&mut self, from: &VaultPath, to: &VaultPath) -> bool {
        let icons = move_entries(&mut self.icons, from, to);
        let colors = move_entries(&mut self.colors, from, to);
        icons || colors
    }

    /// Removes the icons and colours of `path` and of everything under it,
    /// returning them.
    pub fn remove_under(&mut self, path: &VaultPath) -> Removed {
        Removed {
            icons: remove_entries(&mut self.icons, path),
            colors: remove_entries(&mut self.colors, path),
        }
    }

    /// Puts back what [`Icons::remove_under`] took out. Returns whether
    /// anything changed.
    pub fn restore(&mut self, removed: &Removed) -> bool {
        let mut changed = false;
        for (path, icon) in &removed.icons {
            changed |= self.set(path, Some(icon));
        }
        for (path, color) in &removed.colors {
            changed |= self.set_color(path, Some(color));
        }
        changed
    }

    /// Whether `path`, or anything under it, has an icon or a colour.
    pub fn has_under(&self, path: &VaultPath) -> bool {
        self.icons
            .keys()
            .chain(self.colors.keys())
            .any(|p| p.starts_with(path))
    }
}

fn set_entry(map: &mut BTreeMap<VaultPath, String>, path: &VaultPath, value: Option<&str>) -> bool {
    match value.filter(|v| !v.is_empty()) {
        Some(value) if map.get(path).map(String::as_str) == Some(value) => false,
        Some(value) => {
            map.insert(path.clone(), value.to_owned());
            true
        }
        None => map.remove(path).is_some(),
    }
}

fn move_entries(map: &mut BTreeMap<VaultPath, String>, from: &VaultPath, to: &VaultPath) -> bool {
    let moved: Vec<(VaultPath, VaultPath)> = map
        .keys()
        .filter_map(|path| Some((path.clone(), rebased(path, from, to)?)))
        .collect();
    // Take every entry out before putting any back, so none overwrites
    // another that hasn't moved yet.
    let entries: Vec<(VaultPath, String)> = moved
        .into_iter()
        .filter_map(|(old, new)| Some((new, map.remove(&old)?)))
        .collect();
    let changed = !entries.is_empty();
    map.extend(entries);
    changed
}

fn remove_entries(
    map: &mut BTreeMap<VaultPath, String>,
    path: &VaultPath,
) -> Vec<(VaultPath, String)> {
    let gone: Vec<VaultPath> = map
        .keys()
        .filter(|p| p.starts_with(path))
        .cloned()
        .collect();
    gone.into_iter()
        .filter_map(|p| {
            let value = map.remove(&p)?;
            Some((p, value))
        })
        .collect()
}

/// `path` after `from` moved to `to`, if it's `from` or under it.
fn rebased(path: &VaultPath, from: &VaultPath, to: &VaultPath) -> Option<VaultPath> {
    if path == from {
        return Some(to.clone());
    }
    let rest = path
        .as_str()
        .strip_prefix(from.as_str())?
        .strip_prefix('/')?;
    to.join(rest).ok()
}

// --- properties.json -------------------------------------------------------

/// Every property name used in the vault, for Add Property's menu, so it
/// needn't wait on the index. Names are only ever added, when a property is
/// made in Igneous (along with any the index found since). Also the icon
/// chosen for a property, shown wherever it appears.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PropertyNames {
    #[serde(default = "one")]
    pub version: u32,
    pub names: Vec<String>,
    /// Property name → symbolic icon name.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub icons: BTreeMap<String, String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
settings_file!(PropertyNames, "properties.json", 1);

impl Default for PropertyNames {
    fn default() -> Self {
        Self {
            version: 1,
            names: Vec::new(),
            icons: BTreeMap::new(),
            extra: Map::new(),
        }
    }
}

impl PropertyNames {
    /// Appends the names not already listed, returning them.
    pub fn add<'a>(&mut self, names: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        let mut added = Vec::new();
        for name in names {
            let name = name.trim();
            if !name.is_empty() && !self.names.iter().any(|n| n == name) {
                self.names.push(name.to_owned());
                added.push(name.to_owned());
            }
        }
        added
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let settings: VaultSettings = load(dir.path()).unwrap();
        assert_eq!(settings, VaultSettings::default());
        assert_eq!(settings.links.style, LinkStyle::Wiki);
    }

    #[test]
    fn save_is_deterministic_and_skips_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let settings = GitSettings::default();
        assert!(save(dir.path(), &settings).unwrap());
        let first = std::fs::read(dir.path().join("git.json")).unwrap();
        assert!(!save(dir.path(), &settings).unwrap());
        let loaded: GitSettings = load(dir.path()).unwrap();
        assert!(!save(dir.path(), &loaded).unwrap());
        assert_eq!(std::fs::read(dir.path().join("git.json")).unwrap(), first);
        assert!(first.ends_with(b"}\n"));
        assert!(
            String::from_utf8(first)
                .unwrap()
                .contains("\n  \"syncInterval\": 5,")
        );
    }

    #[test]
    fn every_file_round_trips() {
        fn check<T: SettingsFile + PartialEq + std::fmt::Debug>() {
            let value = T::default();
            let back: T = from_bytes(&to_bytes(&value)).unwrap();
            assert_eq!(back, value, "{}", T::FILE_NAME);
        }
        check::<VaultSettings>();
        check::<Appearance>();
        check::<Hotkeys>();
        check::<Workspace>();
        check::<GitSettings>();
        check::<LintSettings>();
        check::<GraphSettings>();
        check::<Bookmarks>();
        check::<Icons>();
        check::<PropertyNames>();
    }

    #[test]
    fn property_names_are_only_added() {
        let mut names = PropertyNames::default();
        assert_eq!(
            names.add(["tags", "status", "tags", " ", "status "]),
            ["tags", "status"]
        );
        assert_eq!(names.add(["created", "tags"]), ["created"]);
        assert_eq!(names.names, ["tags", "status", "created"]);
    }

    #[test]
    fn unknown_keys_survive() {
        let json = br#"{"version":1,"links":{"style":"markdown","futureLinkOption":true},"fromTheFuture":[1,2]}"#;
        let settings: VaultSettings = from_bytes(json).unwrap();
        assert_eq!(settings.links.style, LinkStyle::Markdown);
        let out = String::from_utf8(to_bytes(&settings)).unwrap();
        assert!(out.contains("\"futureLinkOption\": true"), "{out}");
        assert!(out.contains("\"fromTheFuture\""), "{out}");
    }

    #[test]
    fn newer_format_is_refused() {
        let err = from_bytes::<GitSettings>(br#"{"version":99}"#).unwrap_err();
        assert!(matches!(err, SettingsError::TooNew { found: 99, .. }));
    }

    #[test]
    fn malformed_settings_are_an_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("lint.json"), "{ nope").unwrap();
        assert!(matches!(
            load::<LintSettings>(dir.path()),
            Err(SettingsError::Parse { .. })
        ));
    }

    #[test]
    fn workspace_store_tolerates_damage() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("workspace.json"),
            "<<<<<<< HEAD\n{\"tabs\":[]}\n=======\n{}\n>>>>>>> origin/main\n",
        )
        .unwrap();
        let mut store = WorkspaceStore::new(dir.path());
        let (workspace, error) = store.load();
        assert_eq!(workspace, Workspace::default());
        assert!(error.is_some());

        // The damaged file is replaced on the next real change.
        let mut changed = workspace.clone();
        changed.tabs.push(TabState {
            kind: TabKind::Note,
            path: Some(VaultPath::new("Home.md").unwrap()),
            mode: Some(EditorMode::Live),
            cursor: 12,
            scroll: 0.0,
            pinned: false,
            back: vec![],
            forward: vec![],
        });
        changed.active_tab = Some(0);
        assert!(store.save_if_changed(&changed).unwrap());
        assert!(!store.save_if_changed(&changed).unwrap());
        let (reloaded, error) = WorkspaceStore::new(dir.path()).load();
        assert!(error.is_none());
        assert_eq!(reloaded, changed);
    }

    #[test]
    fn bookmark_format() {
        let bookmarks = Bookmarks {
            items: vec![Bookmark::Search {
                query: "tag:#rust".into(),
                title: None,
            }],
            ..Bookmarks::default()
        };
        let out = String::from_utf8(to_bytes(&bookmarks)).unwrap();
        assert!(out.contains("\"type\": \"search\""), "{out}");
    }

    fn p(s: &str) -> VaultPath {
        VaultPath::new(s).unwrap()
    }

    #[test]
    fn icons_load_save_and_keep_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let icons: Icons = load(dir.path()).unwrap();
        assert_eq!(icons, Icons::default());

        std::fs::write(
            dir.path().join("icons.json"),
            r#"{"version":1,"icons":{"Home.md":"go-home-symbolic"},"fromTheFuture":{"a":1}}"#,
        )
        .unwrap();
        let mut icons: Icons = load(dir.path()).unwrap();
        assert_eq!(icons.get(&p("Home.md")), Some("go-home-symbolic"));
        assert!(icons.set(&p("Projects/Plan.md"), Some("starred-symbolic")));
        assert!(save(dir.path(), &icons).unwrap());
        let out = std::fs::read_to_string(dir.path().join("icons.json")).unwrap();
        assert_eq!(
            out,
            "{\n  \"version\": 1,\n  \"icons\": {\n    \"Home.md\": \"go-home-symbolic\",\n    \
             \"Projects/Plan.md\": \"starred-symbolic\"\n  },\n  \"fromTheFuture\": {\n    \
             \"a\": 1\n  }\n}\n"
        );
        assert_eq!(load::<Icons>(dir.path()).unwrap(), icons);
    }

    #[test]
    fn icons_set_and_reset() {
        let mut icons = Icons::default();
        assert!(icons.set(&p("a.md"), Some("starred-symbolic")));
        assert!(!icons.set(&p("a.md"), Some("starred-symbolic")));
        assert!(icons.set(&p("a.md"), Some("heart-symbolic")));
        assert_eq!(icons.get(&p("a.md")), Some("heart-symbolic"));
        assert!(icons.set(&p("a.md"), None));
        assert!(!icons.set(&p("a.md"), None));
        assert!(!icons.set(&p("b.md"), Some("")));
        assert!(icons.icons.is_empty());
    }

    #[test]
    fn icons_follow_renames() {
        let mut icons = Icons::default();
        for (path, icon) in [
            ("Home.md", "go-home-symbolic"),
            ("Projects/Plan.md", "starred-symbolic"),
            ("Projects/Deep/Task.md", "check-plain-symbolic"),
            ("Projects2/Other.md", "heart-symbolic"),
        ] {
            icons.set(&p(path), Some(icon));
        }

        assert!(icons.follow_rename(&p("Home.md"), &p("Start.md")));
        assert_eq!(icons.get(&p("Start.md")), Some("go-home-symbolic"));
        assert_eq!(icons.get(&p("Home.md")), None);

        // A folder takes everything under it, but not a folder that only
        // shares its name's start.
        assert!(icons.follow_rename(&p("Projects"), &p("Work/Projects")));
        assert_eq!(
            icons
                .icons
                .keys()
                .map(VaultPath::as_str)
                .collect::<Vec<_>>(),
            [
                "Projects2/Other.md",
                "Start.md",
                "Work/Projects/Deep/Task.md",
                "Work/Projects/Plan.md"
            ]
        );
        assert!(!icons.follow_rename(&p("Elsewhere.md"), &p("Moved.md")));

        // Only the case changes.
        assert!(icons.follow_rename(&p("Start.md"), &p("start.md")));
        assert_eq!(icons.get(&p("start.md")), Some("go-home-symbolic"));
        assert_eq!(icons.get(&p("Start.md")), None);
    }

    #[test]
    fn icons_are_removed_with_their_folder() {
        let mut icons = Icons::default();
        icons.set(&p("Projects/Plan.md"), Some("starred-symbolic"));
        icons.set(&p("Projects/Deep/Task.md"), Some("heart-symbolic"));
        icons.set(&p("Projects2/Other.md"), Some("heart-symbolic"));
        assert_eq!(
            icons.remove_under(&p("Projects/Plan.md")).icons,
            [(p("Projects/Plan.md"), "starred-symbolic".to_owned())]
        );
        icons.set(&p("Projects/Plan.md"), Some("starred-symbolic"));
        let gone = icons.remove_under(&p("Projects"));
        assert_eq!(gone.icons.len(), 2);
        assert_eq!(
            icons
                .icons
                .keys()
                .map(VaultPath::as_str)
                .collect::<Vec<_>>(),
            ["Projects2/Other.md"]
        );
        assert!(icons.remove_under(&p("Nothing")).is_empty());
    }

    #[test]
    fn colours_are_their_own_setting() {
        let dir = tempfile::tempdir().unwrap();
        let mut icons = Icons::default();
        // A folder's colour, with no icon.
        assert!(icons.set_color(&p("Projects"), Some("#e01b24")));
        assert!(!icons.set_color(&p("Projects"), Some("#e01b24")));
        assert!(icons.set_color(&p("Projects/Plan.md"), Some("#3584e4")));
        icons.set(&p("Projects/Plan.md"), Some("starred-symbolic"));
        assert!(save(dir.path(), &icons).unwrap());
        assert_eq!(load::<Icons>(dir.path()).unwrap(), icons);

        // Renames and deletions take colours along with icons.
        assert!(icons.follow_rename(&p("Projects"), &p("Work")));
        assert_eq!(icons.color(&p("Work")), Some("#e01b24"));
        assert_eq!(icons.color(&p("Work/Plan.md")), Some("#3584e4"));
        assert_eq!(icons.color(&p("Projects")), None);
        assert!(icons.has_under(&p("Work")));
        let gone = icons.remove_under(&p("Work"));
        assert_eq!(gone.colors.len(), 2);
        assert_eq!(gone.icons.len(), 1);
        assert!(icons.colors.is_empty() && icons.icons.is_empty());
        assert!(icons.restore(&gone));
        assert_eq!(icons.color(&p("Work")), Some("#e01b24"));

        assert!(icons.set_color(&p("Work"), None));
        assert!(!icons.set_color(&p("Work"), None));
        // No colours, no "colors" key.
        let mut plain = Icons::default();
        plain.set(&p("a.md"), Some("heart-symbolic"));
        save(dir.path(), &plain).unwrap();
        let out = std::fs::read_to_string(dir.path().join("icons.json")).unwrap();
        assert!(!out.contains("colors"));
    }
}
