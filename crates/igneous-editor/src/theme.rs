//! Editor themes.
//!
//! A theme is a small TOML file with a light and/or a dark variant. Each
//! variant has a palette (a background, a foreground and eight accents) and
//! may say what its colours are used for: its *roles*, such as headings,
//! links or tags. Every role has a sensible default, so a palette alone is a
//! complete theme. Igneous turns each variant into a GtkSourceView style
//! scheme (see [`scheme_xml`]); Live Preview uses the same roles.
//!
//! The format is documented in `docs/themes.md`.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

// --- colours ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Parses `#rgb`, `#rrggbb` or `#rrggbbaa`.
    pub fn parse(s: &str) -> Option<Self> {
        let hex = s.strip_prefix('#')?;
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        match hex.len() {
            3 => {
                let digit = |i: usize| u8::from_str_radix(&hex[i..=i], 16).ok().map(|d| d * 17);
                Some(Self::rgb(digit(0)?, digit(1)?, digit(2)?))
            }
            6 => Some(Self::rgb(byte(0)?, byte(2)?, byte(4)?)),
            8 => Some(Self {
                r: byte(0)?,
                g: byte(2)?,
                b: byte(4)?,
                a: byte(6)?,
            }),
            _ => None,
        }
    }

    /// `self` moved `t` of the way towards `other` (0.0 is `self`).
    pub fn mix(self, other: Color, t: f32) -> Color {
        let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Color {
            r: lerp(self.r, other.r),
            g: lerp(self.g, other.g),
            b: lerp(self.b, other.b),
            a: lerp(self.a, other.a),
        }
    }

    pub fn with_alpha(self, alpha: f32) -> Color {
        Color {
            a: (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
            ..self
        }
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)?;
        if self.a != 255 {
            write!(f, "{:02x}", self.a)?;
        }
        Ok(())
    }
}

// --- palette and roles -------------------------------------------------------------

/// The colours a variant can define. Only `background` and `foreground` are
/// required.
pub const PALETTE: &[&str] = &[
    "background",
    "foreground",
    "surface",
    "muted",
    "cursor",
    "selection",
    "red",
    "orange",
    "yellow",
    "green",
    "cyan",
    "blue",
    "purple",
    "pink",
];

const ACCENTS: &[&str] = &[
    "red", "orange", "yellow", "green", "cyan", "blue", "purple", "pink",
];

#[derive(Debug, Clone, Copy)]
enum Fallback {
    /// A palette colour.
    Palette(&'static str),
    /// Another role.
    Role(&'static str),
    /// `t` of the way from one palette colour to another.
    Mix(&'static str, &'static str, f32),
}

use Fallback::{Mix, Palette as P, Role as R};

/// Every role, with what it uses when a theme doesn't say. Roles that name a
/// background are marked as such in `docs/themes.md`.
const ROLES: &[(&str, Fallback)] = &[
    // The editor itself.
    ("text", P("foreground")),
    ("background", P("background")),
    ("current-line", Mix("background", "foreground", 0.05)),
    ("line-number", P("muted")),
    ("cursor", P("cursor")),
    ("selection", P("selection")),
    ("search-match", Mix("background", "yellow", 0.5)),
    // Markdown.
    ("syntax", P("muted")),
    ("heading", P("blue")),
    ("heading-1", R("heading")),
    ("heading-2", R("heading")),
    ("heading-3", R("heading")),
    ("heading-4", R("heading")),
    ("heading-5", R("heading")),
    ("heading-6", R("heading")),
    ("link", P("blue")),
    ("url", R("link")),
    ("wikilink", R("link")),
    ("unresolved-link", P("muted")),
    ("embed", R("link")),
    ("tag", P("cyan")),
    ("highlight", Mix("background", "yellow", 0.4)),
    ("code", P("green")),
    ("code-background", P("surface")),
    ("quote", P("muted")),
    ("list-marker", P("orange")),
    ("comment", P("muted")),
    ("frontmatter", P("purple")),
    ("math", P("orange")),
    ("block-id", P("muted")),
    ("strikethrough", P("muted")),
    ("callout", R("callout-note")),
    ("callout-note", P("blue")),
    ("callout-tip", P("cyan")),
    ("callout-success", P("green")),
    ("callout-question", P("yellow")),
    ("callout-warning", P("orange")),
    ("callout-failure", P("red")),
    ("callout-danger", P("red")),
    ("callout-bug", P("red")),
    ("callout-example", P("purple")),
    ("callout-quote", P("muted")),
    // Code in other languages.
    ("keyword", P("purple")),
    ("string", P("green")),
    ("number", P("orange")),
    ("constant", P("orange")),
    ("type", P("yellow")),
    ("function", P("blue")),
    ("preprocessor", P("pink")),
    ("special", P("cyan")),
    ("error", P("red")),
    ("warning", P("orange")),
    // Git.
    ("diff-added", P("green")),
    ("diff-removed", P("red")),
    ("conflict", P("red")),
    ("conflict-background", Mix("background", "red", 0.12)),
];

/// Names of every role a theme can set.
pub fn role_names() -> impl Iterator<Item = &'static str> {
    ROLES.iter().map(|(name, _)| *name)
}

/// A colour, or the desktop's accent colour (known only when drawing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paint {
    Color(Color),
    Accent,
}

impl Paint {
    pub fn resolve(self, accent: Color) -> Color {
        match self {
            Paint::Color(c) => c,
            Paint::Accent => accent,
        }
    }
}

// --- themes ------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Shipped with Igneous.
    BuiltIn,
    /// In the user's themes folder.
    User,
    /// In the vault's `.igneous/themes`.
    Vault,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    /// E.g. "Mocha" for Catppuccin's dark variant.
    pub name: Option<String>,
    roles: BTreeMap<&'static str, Paint>,
    /// Every [`PALETTE`] colour, with defaults filled in.
    palette: BTreeMap<&'static str, Color>,
}

impl Variant {
    /// A [`PALETTE`] colour, such as "red" or "background".
    pub fn palette(&self, key: &str) -> Option<Color> {
        self.palette.get(key).copied()
    }

    /// The colour for `role`. Unknown roles fall back to the text colour.
    pub fn role(&self, role: &str, accent: Color) -> Color {
        self.roles
            .get(role)
            .or_else(|| self.roles.get("text"))
            .copied()
            .unwrap_or(Paint::Color(Color::rgb(0, 0, 0)))
            .resolve(accent)
    }

    fn uses_accent(&self) -> bool {
        self.roles.values().any(|p| *p == Paint::Accent)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    /// The file name without `.toml`.
    pub id: String,
    pub name: String,
    pub url: Option<String>,
    pub light: Option<Variant>,
    pub dark: Option<Variant>,
    pub origin: Origin,
    /// Things that were ignored, such as unknown keys.
    pub warnings: Vec<String>,
}

impl Theme {
    /// The variant for a light or dark desktop, falling back to the other one
    /// for themes that only have one.
    pub fn variant(&self, dark: bool) -> &Variant {
        let (preferred, other) = if dark {
            (&self.dark, &self.light)
        } else {
            (&self.light, &self.dark)
        };
        preferred
            .as_ref()
            .or(other.as_ref())
            .expect("parsed themes have a variant")
    }

    /// The variant actually used is dark.
    pub fn variant_is_dark(&self, dark: bool) -> bool {
        if dark {
            self.dark.is_some()
        } else {
            self.light.is_none()
        }
    }

    /// "Catppuccin Mocha", or just "Dracula".
    pub fn display_name(&self, dark: bool) -> String {
        match &self.variant(dark).name {
            Some(variant) => format!("{} {variant}", self.name),
            None => self.name.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ThemeError {
    #[error("not valid TOML: {0}")]
    Toml(String),
    #[error("the theme has no `name`")]
    MissingName,
    #[error("the theme has neither a [light] nor a [dark] variant")]
    NoVariant,
    #[error("[{variant}] is missing `{key}`")]
    MissingColor {
        variant: &'static str,
        key: &'static str,
    },
    #[error(
        "[{variant}] `{key}` is {value:?}, which isn't a colour (#rrggbb), a palette colour or `accent`"
    )]
    BadColor {
        variant: &'static str,
        key: String,
        value: String,
    },
}

/// Parses a theme file. `id` is its file name without `.toml`.
pub fn parse(id: &str, source: &str, origin: Origin) -> Result<Theme, ThemeError> {
    let table: toml::Table = source
        .parse()
        .map_err(|e: toml::de::Error| ThemeError::Toml(e.message().to_owned()))?;
    let mut warnings = Vec::new();
    let name = table
        .get("name")
        .and_then(toml::Value::as_str)
        .ok_or(ThemeError::MissingName)?
        .to_owned();
    let url = table
        .get("url")
        .and_then(toml::Value::as_str)
        .map(str::to_owned);
    for key in table.keys() {
        if !matches!(key.as_str(), "name" | "url" | "light" | "dark") {
            warnings.push(format!("unknown key `{key}`"));
        }
    }
    let mut variant = |key: &'static str| -> Result<Option<Variant>, ThemeError> {
        match table.get(key) {
            None => Ok(None),
            Some(toml::Value::Table(t)) => parse_variant(key, t, &mut warnings).map(Some),
            Some(_) => Err(ThemeError::Toml(format!("`{key}` must be a table"))),
        }
    };
    let light = variant("light")?;
    let dark = variant("dark")?;
    if light.is_none() && dark.is_none() {
        return Err(ThemeError::NoVariant);
    }
    Ok(Theme {
        id: id.to_owned(),
        name,
        url,
        light,
        dark,
        origin,
        warnings,
    })
}

fn parse_variant(
    variant: &'static str,
    table: &toml::Table,
    warnings: &mut Vec<String>,
) -> Result<Variant, ThemeError> {
    let color_at = |key: &str, value: &toml::Value| -> Result<Color, ThemeError> {
        value
            .as_str()
            .and_then(Color::parse)
            .ok_or_else(|| ThemeError::BadColor {
                variant,
                key: key.to_owned(),
                value: value.to_string(),
            })
    };

    // The palette, with defaults for what's missing.
    let mut palette: BTreeMap<&'static str, Color> = BTreeMap::new();
    for key in PALETTE {
        if let Some(value) = table.get(*key) {
            palette.insert(key, color_at(key, value)?);
        }
    }
    for key in ["background", "foreground"] {
        if !palette.contains_key(key) {
            return Err(ThemeError::MissingColor {
                variant,
                key: if key == "background" {
                    "background"
                } else {
                    "foreground"
                },
            });
        }
    }
    let (bg, fg) = (palette["background"], palette["foreground"]);
    for accent in ACCENTS {
        palette.entry(accent).or_insert(fg);
    }
    palette.entry("surface").or_insert(bg.mix(fg, 0.08));
    palette.entry("muted").or_insert(fg.mix(bg, 0.45));
    palette.entry("cursor").or_insert(fg);
    let blue = palette["blue"];
    palette.entry("selection").or_insert(blue.with_alpha(0.3));

    for key in table.keys() {
        if !PALETTE.contains(&key.as_str()) && !matches!(key.as_str(), "name" | "roles") {
            warnings.push(format!("[{variant}] unknown key `{key}`"));
        }
    }

    // Roles the theme sets.
    let mut set: BTreeMap<&'static str, Paint> = BTreeMap::new();
    if let Some(roles) = table.get("roles") {
        let Some(roles) = roles.as_table() else {
            return Err(ThemeError::Toml(format!(
                "[{variant}.roles] must be a table"
            )));
        };
        for (key, value) in roles {
            let Some((role, _)) = ROLES.iter().find(|(r, _)| r == key) else {
                warnings.push(format!("[{variant}.roles] unknown role `{key}`"));
                continue;
            };
            let paint = match value.as_str() {
                Some("accent") => Paint::Accent,
                Some(name) if palette.contains_key(name) => Paint::Color(palette[name]),
                _ => Paint::Color(color_at(key, value)?),
            };
            set.insert(role, paint);
        }
    }

    // Everything else from its default, following role chains.
    fn resolve(
        role: &'static str,
        set: &BTreeMap<&'static str, Paint>,
        palette: &BTreeMap<&'static str, Color>,
        depth: u8,
    ) -> Paint {
        if let Some(paint) = set.get(role) {
            return *paint;
        }
        let fallback = ROLES
            .iter()
            .find(|(r, _)| *r == role)
            .map(|(_, f)| *f)
            .unwrap_or(P("foreground"));
        match fallback {
            P(name) => Paint::Color(palette[name]),
            Mix(a, b, t) => Paint::Color(palette[a].mix(palette[b], t)),
            R(other) if depth < 8 => resolve(other, set, palette, depth + 1),
            R(_) => Paint::Color(palette["foreground"]),
        }
    }
    let roles = ROLES
        .iter()
        .map(|(role, _)| (*role, resolve(role, &set, &palette, 0)))
        .collect();

    Ok(Variant {
        name: table
            .get("name")
            .and_then(toml::Value::as_str)
            .map(str::to_owned),
        roles,
        palette,
    })
}

// --- the built-in themes and loading -------------------------------------------------

const BUILT_IN: &[(&str, &str)] = &[
    ("adwaita", include_str!("../data/themes/adwaita.toml")),
    ("catppuccin", include_str!("../data/themes/catppuccin.toml")),
    (
        "catppuccin-frappe",
        include_str!("../data/themes/catppuccin-frappe.toml"),
    ),
    (
        "catppuccin-macchiato",
        include_str!("../data/themes/catppuccin-macchiato.toml"),
    ),
    ("dracula", include_str!("../data/themes/dracula.toml")),
    ("gruvbox", include_str!("../data/themes/gruvbox.toml")),
    ("nord", include_str!("../data/themes/nord.toml")),
    ("rose-pine", include_str!("../data/themes/rose-pine.toml")),
    ("solarized", include_str!("../data/themes/solarized.toml")),
    (
        "tokyo-night",
        include_str!("../data/themes/tokyo-night.toml"),
    ),
];

/// The theme used when none is chosen or the chosen one is missing.
pub const DEFAULT_THEME: &str = "adwaita";

pub fn builtin() -> Vec<Theme> {
    BUILT_IN
        .iter()
        .map(|(id, source)| parse(id, source, Origin::BuiltIn).expect("built-in themes are valid"))
        .collect()
}

/// Every available theme: built in, then the user's, then the vault's. A later
/// theme with the same id replaces an earlier one.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    themes: Vec<Theme>,
    /// Files that couldn't be read as themes.
    pub errors: Vec<(PathBuf, String)>,
}

impl Catalog {
    pub fn load(user_dir: Option<&Path>, vault_dir: Option<&Path>) -> Self {
        let mut catalog = Catalog {
            themes: builtin(),
            errors: Vec::new(),
        };
        for (dir, origin) in [(user_dir, Origin::User), (vault_dir, Origin::Vault)] {
            if let Some(dir) = dir {
                catalog.load_dir(dir, origin);
            }
        }
        catalog.themes.sort_by(|a, b| {
            (a.id != DEFAULT_THEME)
                .cmp(&(b.id != DEFAULT_THEME))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        catalog
    }

    fn load_dir(&mut self, dir: &Path, origin: Origin) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "toml"))
            .collect();
        paths.sort();
        for path in paths {
            let id = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let parsed = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|source| parse(&id, &source, origin).map_err(|e| e.to_string()));
            match parsed {
                Ok(theme) => {
                    self.themes.retain(|t| t.id != theme.id);
                    self.themes.push(theme);
                }
                Err(e) => self.errors.push((path, e)),
            }
        }
    }

    pub fn themes(&self) -> &[Theme] {
        &self.themes
    }

    pub fn get(&self, id: &str) -> Option<&Theme> {
        self.themes.iter().find(|t| t.id == id)
    }

    /// The theme `id`, or the default theme if there's no such theme.
    pub fn get_or_default(&self, id: &str) -> &Theme {
        self.get(id)
            .or_else(|| self.get(DEFAULT_THEME))
            .expect("the default theme is built in")
    }
}

// --- GtkSourceView style schemes ---------------------------------------------------

/// A short stable hash for cache file names (FNV-1a).
fn fnv(data: &str) -> u64 {
    data.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The GtkSourceView style scheme for one variant of `theme`, as
/// `(scheme id, XML)`. `accent` stands in for the `accent` role value.
pub fn scheme_xml(theme: &Theme, dark: bool, accent: Color) -> (String, String) {
    scheme_xml_for(theme, dark, accent, false)
}

/// Like [`scheme_xml`], but with only the editor's own colours (text,
/// background, cursor, selection…) and no syntax styles. Live Preview uses
/// it: GtkSourceView's syntax engine still runs, so spellcheck knows what
/// to skip, but Live Preview's own tags do all the styling.
pub fn chrome_scheme_xml(theme: &Theme, dark: bool, accent: Color) -> (String, String) {
    scheme_xml_for(theme, dark, accent, true)
}

fn scheme_xml_for(theme: &Theme, dark: bool, accent: Color, chrome_only: bool) -> (String, String) {
    let variant = theme.variant(dark);
    let is_dark = theme.variant_is_dark(dark);
    let c = |role: &str| variant.role(role, accent).to_string();

    // GtkSourceView style name → attributes.
    let styles: Vec<(&str, String)> = vec![
        (
            "text",
            format!(
                r#"foreground="{}" background="{}""#,
                c("text"),
                c("background")
            ),
        ),
        (
            "current-line",
            format!(r#"background="{}""#, c("current-line")),
        ),
        (
            "current-line-number",
            format!(
                r#"foreground="{}" background="{}""#,
                c("text"),
                c("current-line")
            ),
        ),
        (
            "line-numbers",
            format!(
                r#"foreground="{}" background="{}""#,
                c("line-number"),
                c("background")
            ),
        ),
        ("cursor", format!(r#"foreground="{}""#, c("cursor"))),
        ("selection", format!(r#"background="{}""#, c("selection"))),
        (
            "selection-unfocused",
            format!(r#"background="{}""#, c("selection")),
        ),
        (
            "search-match",
            format!(
                r#"foreground="{}" background="{}""#,
                c("text"),
                c("search-match")
            ),
        ),
        ("bracket-match", r#"bold="true""#.to_owned()),
        ("right-margin", format!(r#"foreground="{}""#, c("syntax"))),
        ("draw-spaces", format!(r#"foreground="{}""#, c("syntax"))),
        // Markdown, through the generic styles markdown.lang maps to.
        (
            "def:heading",
            format!(r#"foreground="{}" bold="true""#, c("heading")),
        ),
        ("def:link-text", format!(r#"foreground="{}""#, c("link"))),
        (
            "def:link-destination",
            format!(r#"foreground="{}" underline="low""#, c("url")),
        ),
        (
            "def:link-symbol",
            format!(r#"foreground="{}""#, c("syntax")),
        ),
        (
            "def:inline-code",
            format!(
                r#"foreground="{}" background="{}""#,
                c("code"),
                c("code-background")
            ),
        ),
        (
            "def:preformatted-section",
            format!(r#"foreground="{}""#, c("code")),
        ),
        (
            "def:list-marker",
            format!(r#"foreground="{}" bold="true""#, c("list-marker")),
        ),
        ("def:emphasis", r#"italic="true""#.to_owned()),
        ("def:strong-emphasis", r#"bold="true""#.to_owned()),
        (
            "def:thematic-break",
            format!(r#"foreground="{}""#, c("syntax")),
        ),
        (
            "def:comment",
            format!(r#"foreground="{}" italic="true""#, c("comment")),
        ),
        ("def:note", format!(r#"background="{}""#, c("highlight"))),
        (
            "def:deletion",
            format!(
                r#"foreground="{}" strikethrough="true""#,
                c("strikethrough")
            ),
        ),
        ("def:insertion", format!(r#"foreground="{}""#, c("string"))),
        ("def:underlined", r#"underline="single""#.to_owned()),
        (
            "markdown:blockquote-marker",
            format!(r#"foreground="{}" bold="true""#, c("quote")),
        ),
        ("markdown:label", format!(r#"foreground="{}""#, c("syntax"))),
        (
            "markdown:image-marker",
            format!(r#"foreground="{}""#, c("embed")),
        ),
        (
            "markdown:line-break",
            format!(r#"background="{}""#, c("code-background")),
        ),
        // Obsidian syntax (igneous-markdown.lang).
        (
            "igneous-markdown:frontmatter",
            format!(r#"foreground="{}""#, c("frontmatter")),
        ),
        (
            "igneous-markdown:comment",
            format!(r#"foreground="{}" italic="true""#, c("comment")),
        ),
        (
            "igneous-markdown:callout",
            format!(r#"foreground="{}" bold="true""#, c("callout")),
        ),
        (
            "igneous-markdown:embed",
            format!(r#"foreground="{}""#, c("embed")),
        ),
        (
            "igneous-markdown:wikilink",
            format!(r#"foreground="{}""#, c("wikilink")),
        ),
        (
            "igneous-markdown:highlight",
            format!(r#"background="{}""#, c("highlight")),
        ),
        (
            "igneous-markdown:strikethrough",
            format!(
                r#"foreground="{}" strikethrough="true""#,
                c("strikethrough")
            ),
        ),
        (
            "igneous-markdown:math",
            format!(r#"foreground="{}""#, c("math")),
        ),
        (
            "igneous-markdown:tag",
            format!(r#"foreground="{}""#, c("tag")),
        ),
        (
            "igneous-markdown:block-id",
            format!(r#"foreground="{}""#, c("block-id")),
        ),
        (
            "igneous-markdown:conflict-marker",
            format!(
                r#"foreground="{}" background="{}" bold="true""#,
                c("conflict"),
                c("conflict-background")
            ),
        ),
        // Diffs.
        (
            "diff:added-line",
            format!(r#"foreground="{}""#, c("diff-added")),
        ),
        (
            "diff:removed-line",
            format!(r#"foreground="{}""#, c("diff-removed")),
        ),
        ("diff:location", format!(r#"foreground="{}""#, c("syntax"))),
        (
            "diff:diff-file",
            format!(r#"foreground="{}" bold="true""#, c("heading")),
        ),
        (
            "diff:special-case",
            format!(r#"foreground="{}""#, c("syntax")),
        ),
        // Code in other languages.
        (
            "def:keyword",
            format!(r#"foreground="{}" bold="true""#, c("keyword")),
        ),
        (
            "def:statement",
            format!(r#"foreground="{}" bold="true""#, c("keyword")),
        ),
        ("def:string", format!(r#"foreground="{}""#, c("string"))),
        ("def:number", format!(r#"foreground="{}""#, c("number"))),
        ("def:constant", format!(r#"foreground="{}""#, c("constant"))),
        (
            "def:type",
            format!(r#"foreground="{}" bold="true""#, c("type")),
        ),
        ("def:function", format!(r#"foreground="{}""#, c("function"))),
        (
            "def:preprocessor",
            format!(r#"foreground="{}""#, c("preprocessor")),
        ),
        (
            "def:special-char",
            format!(r#"foreground="{}""#, c("special")),
        ),
        (
            "def:shebang",
            format!(r#"foreground="{}" bold="true""#, c("comment")),
        ),
        (
            "def:error",
            format!(r#"underline="error" underline-color="{}""#, c("error")),
        ),
        (
            "def:warning",
            format!(r#"underline="error" underline-color="{}""#, c("warning")),
        ),
    ];

    let variant_word = if is_dark { "dark" } else { "light" };
    let mut body = String::new();
    // Syntax styles are namespaced (`def:…`, `markdown:…`); chrome isn't.
    for (name, attrs) in styles
        .iter()
        .filter(|(name, _)| !chrome_only || !name.contains(':'))
    {
        body.push_str(&format!("  <style name=\"{name}\" {attrs}/>\n"));
    }
    let accent_part = if variant.uses_accent() {
        accent.to_string()
    } else {
        String::new()
    };
    let hash = fnv(&format!("{}{body}{accent_part}", theme.name));
    let kind = if chrome_only { "-live" } else { "" };
    let id = format!("igneous-{}-{variant_word}{kind}-{hash:08x}", theme.id);
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!-- Generated by Igneous from the \"{}\" editor theme. -->\n\
         <style-scheme id=\"{id}\" name=\"{}\" version=\"1.0\">\n\
         \x20 <metadata>\n\
         \x20   <property name=\"variant\">{variant_word}</property>\n\
         \x20 </metadata>\n\
         {body}</style-scheme>\n",
        xml_escape(&theme.id),
        xml_escape(&theme.display_name(dark)),
    );
    (id, xml)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCENT: Color = Color::rgb(0x35, 0x84, 0xe4);

    #[test]
    fn colours() {
        assert_eq!(Color::parse("#abc"), Some(Color::rgb(0xaa, 0xbb, 0xcc)));
        assert_eq!(Color::parse("#1e1e2e"), Some(Color::rgb(0x1e, 0x1e, 0x2e)));
        assert_eq!(Color::parse("#9399b24d").map(|c| c.a), Some(0x4d));
        for bad in ["1e1e2e", "#1e1e2", "#gggggg", "", "#"] {
            assert_eq!(Color::parse(bad), None, "{bad}");
        }
        assert_eq!(
            Color::rgb(0, 0, 0)
                .mix(Color::rgb(255, 255, 255), 0.5)
                .to_string(),
            "#808080"
        );
        assert_eq!(Color::rgb(1, 2, 3).with_alpha(0.3).to_string(), "#0102034d");
    }

    #[test]
    fn every_built_in_theme_is_clean() {
        let themes = builtin();
        assert!(themes.iter().any(|t| t.id == DEFAULT_THEME));
        for theme in &themes {
            assert!(
                theme.warnings.is_empty(),
                "{}: {:?}",
                theme.id,
                theme.warnings
            );
            for dark in [false, true] {
                let (id, xml) = scheme_xml(theme, dark, ACCENT);
                assert!(id.starts_with(&format!("igneous-{}-", theme.id)));
                assert!(xml.contains("<style name=\"text\""));
            }
        }
    }

    #[test]
    fn a_palette_alone_is_a_theme() {
        let theme = parse(
            "mono",
            "name = \"Mono\"\n[dark]\nbackground = \"#000000\"\nforeground = \"#ffffff\"\n",
            Origin::User,
        )
        .unwrap();
        let dark = theme.variant(true);
        // Accents fall back to the foreground; derived colours mix.
        assert_eq!(dark.role("link", ACCENT).to_string(), "#ffffff");
        assert_eq!(dark.role("current-line", ACCENT).to_string(), "#0d0d0d");
        // A dark-only theme is used on light desktops too.
        assert_eq!(theme.variant(false), dark);
        assert!(theme.variant_is_dark(false));
        assert_eq!(theme.display_name(false), "Mono");
    }

    #[test]
    fn roles_follow_overrides_and_chains() {
        let theme = parse(
            "t",
            r##"
            name = "T"
            [light]
            name = "Day"
            background = "#ffffff"
            foreground = "#000000"
            blue = "#0000ff"
            red = "#ff0000"
            [light.roles]
            heading = "red"
            heading-2 = "#00ff00"
            link = "accent"
            "##,
            Origin::User,
        )
        .unwrap();
        let v = theme.variant(false);
        assert_eq!(v.role("heading-1", ACCENT).to_string(), "#ff0000");
        assert_eq!(v.role("heading-2", ACCENT).to_string(), "#00ff00");
        assert_eq!(v.role("link", ACCENT), ACCENT);
        assert_eq!(v.role("wikilink", ACCENT), ACCENT, "wikilinks follow links");
        assert_eq!(v.role("callout", ACCENT).to_string(), "#0000ff");
        assert_eq!(theme.display_name(false), "T Day");
        // Using the accent makes the scheme depend on it.
        let (a, _) = scheme_xml(&theme, false, ACCENT);
        let (b, _) = scheme_xml(&theme, false, Color::rgb(1, 2, 3));
        assert_ne!(a, b);
    }

    #[test]
    fn errors_and_warnings() {
        let bg_only = "name = \"X\"\n[dark]\nbackground = \"#000000\"\n";
        assert_eq!(
            parse("x", bg_only, Origin::User),
            Err(ThemeError::MissingColor {
                variant: "dark",
                key: "foreground"
            })
        );
        assert_eq!(
            parse("x", "name = \"X\"\n", Origin::User),
            Err(ThemeError::NoVariant)
        );
        assert_eq!(
            parse("x", "[dark]\n", Origin::User),
            Err(ThemeError::MissingName)
        );
        assert!(matches!(
            parse("x", "name = ", Origin::User),
            Err(ThemeError::Toml(_))
        ));
        let bad = "name = \"X\"\n[dark]\nbackground = \"black\"\nforeground = \"#fff\"\n";
        assert!(matches!(
            parse("x", bad, Origin::User),
            Err(ThemeError::BadColor { .. })
        ));
        let odd = "name = \"X\"\nauthor = \"me\"\n[dark]\nbackground = \"#000\"\nforeground = \"#fff\"\nteal = \"#0ff\"\n[dark.roles]\nheadline = \"red\"\n";
        let theme = parse("x", odd, Origin::User).unwrap();
        assert_eq!(theme.warnings.len(), 3, "{:?}", theme.warnings);
    }

    #[test]
    fn catalog_layers_override_by_id() {
        let user = tempfile::tempdir().unwrap();
        let vault = tempfile::tempdir().unwrap();
        let theme = |name: &str| {
            format!("name = \"{name}\"\n[dark]\nbackground = \"#000\"\nforeground = \"#fff\"\n")
        };
        std::fs::write(user.path().join("dracula.toml"), theme("My Dracula")).unwrap();
        std::fs::write(user.path().join("mine.toml"), theme("Mine")).unwrap();
        std::fs::write(vault.path().join("mine.toml"), theme("Vault Mine")).unwrap();
        std::fs::write(vault.path().join("broken.toml"), "name = ").unwrap();
        std::fs::write(vault.path().join("notes.txt"), "ignored").unwrap();
        let catalog = Catalog::load(Some(user.path()), Some(vault.path()));
        assert_eq!(catalog.get("dracula").unwrap().name, "My Dracula");
        assert_eq!(catalog.get("mine").unwrap().name, "Vault Mine");
        assert_eq!(catalog.get("mine").unwrap().origin, Origin::Vault);
        assert_eq!(catalog.errors.len(), 1);
        assert_eq!(catalog.themes()[0].id, DEFAULT_THEME);
        assert_eq!(catalog.get_or_default("missing").id, DEFAULT_THEME);
    }

    #[test]
    fn scheme_xml_is_well_formed() {
        let theme = builtin()
            .into_iter()
            .find(|t| t.id == "catppuccin")
            .unwrap();
        let (id, xml) = scheme_xml(&theme, true, ACCENT);
        assert!(xml.contains(&format!("id=\"{id}\"")));
        assert!(xml.contains("name=\"Catppuccin Mocha\""));
        assert!(xml.contains("<property name=\"variant\">dark</property>"));
        assert!(
            xml.contains(r##"<style name="text" foreground="#cdd6f4" background="#1e1e2e"/>"##)
        );
        assert!(xml.matches("<style name=").count() > 40);
    }
}
