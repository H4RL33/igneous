# Editor themes

Igneous colours its editor with themes. A theme is a small TOML file with a light variant, a dark variant, or both. The editor uses the variant that matches your desktop's style, and switches when the desktop does.

Themes colour the editor only. The rest of the window follows your desktop, including tools such as Rewaita that recolour libadwaita apps.

## Choosing a theme

Open **Preferences** (Ctrl+,) and pick a theme under **Editor Theme**.

- The choice is saved in the vault, in `.igneous/appearance.json` as `editorTheme`.
- Vaults that haven't chosen a theme use whichever one you picked most recently in any vault.
- Preferences reloads theme files each time it opens, so a theme you've just added appears straight away.

## Where themes come from

| Location | Used by |
|---|---|
| Built in | Every vault. Adwaita (the default), Catppuccin, Catppuccin Frappé, Catppuccin Macchiato, Dracula, Gruvbox, Nord, Rosé Pine, Solarized, Tokyo Night. |
| `~/.local/share/igneous/themes/` | Every vault. The folder button in Preferences opens it. |
| `<vault>/.igneous/themes/` | That vault only. These themes travel with it. |

- **The file name is the theme's id:** `paper.toml` is the theme `paper`.
- **Later rows win:** a theme replaces any theme with the same id in a row above it. To change a built-in theme, copy it into one of these folders under the same name.

## A minimal theme

A background and a foreground are enough. Every other colour has a sensible default.

```toml
name = "Paper"

[light]
background = "#fafaf7"
foreground = "#222222"
```

## A full theme

This is Igneous's Catppuccin theme, which pairs Latte (light) with Mocha (dark):

```toml
name = "Catppuccin"
url = "https://catppuccin.com"

[light]
name = "Latte"
background = "#eff1f5"
foreground = "#4c4f69"
# … the same keys as [dark]

[dark]
name = "Mocha"            # shown after the theme name: "Catppuccin Mocha"
background = "#1e1e2e"
foreground = "#cdd6f4"
surface = "#313244"
muted = "#7f849c"
cursor = "#f5e0dc"
selection = "#9399b24d"   # #rrggbbaa: overlay 2 at 30% opacity
red = "#f38ba8"
orange = "#fab387"
yellow = "#f9e2af"
green = "#a6e3a1"
cyan = "#94e2d5"
blue = "#89b4fa"
purple = "#cba6f7"
pink = "#f5c2e7"

[dark.roles]
comment = "#9399b2"
heading = "red"           # a palette colour…
heading-5 = "#74c7ec"     # …or any colour
url = "#f5e0dc"
```

## The format

### Top level

| Key | Required | Meaning |
|---|---|---|
| `name` | yes | The theme's name, shown in Preferences |
| `url` | no | Where the palette comes from |
| `[light]` | one of the two | The variant for light desktops |
| `[dark]` | one of the two | The variant for dark desktops |

A theme with only one variant uses it on both light and dark desktops.

### A variant's palette

Colours are written `#rgb`, `#rrggbb` or `#rrggbbaa`.

| Key | Required | Used for | Default |
|---|---|---|---|
| `name` | no | The variant's name, e.g. "Mocha" | none |
| `background` | yes | The editor's background | none |
| `foreground` | yes | Text | none |
| `surface` | no | Raised areas, such as inline code | 8% of the way from background to foreground |
| `muted` | no | Dim text: Markdown syntax, comments, line numbers | 45% of the way from foreground to background |
| `cursor` | no | The text cursor | `foreground` |
| `selection` | no | Behind selected text; may be translucent | `blue` at 30% opacity |
| `red` `orange` `yellow` `green` `cyan` `blue` `purple` `pink` | no | The accents roles are drawn from | `foreground` |

### Roles

Roles say what each colour is used for. Set them in a `[light.roles]` or `[dark.roles]` table. A role's value can be:

- a palette key (such as `"blue"` or `"muted"`);
- a colour;
- `"accent"`, the desktop's accent colour.

Roles you don't set use the default in the table below. A default that names another role follows that role.

**The editor**

| Role | Default | Colours |
|---|---|---|
| `text` | `foreground` | Text |
| `background` | `background` | The editor's background |
| `current-line` | 5% of the way from background to foreground | Behind the cursor's line |
| `line-number` | `muted` | Line numbers |
| `cursor` | `cursor` | The cursor |
| `selection` | `selection` | Behind selected text |
| `search-match` | 50% of the way from background to yellow | Behind search matches |

**Markdown**

| Role | Default | Colours |
|---|---|---|
| `syntax` | `muted` | Markdown syntax such as `**` and `---` |
| `heading` | `blue` | Headings |
| `heading-1` … `heading-6` | `heading` | Each heading level. Live Preview only; Source mode can't tell levels apart. |
| `link` | `blue` | Link text |
| `url` | `link` | Link destinations |
| `wikilink` | `link` | `[[Wikilinks]]` |
| `unresolved-link` | `muted` | Links to notes that don't exist (Live Preview) |
| `embed` | `link` | `![[Embeds]]` |
| `tag` | `cyan` | `#tags` |
| `highlight` | 40% of the way from background to yellow | Behind `==highlights==` |
| `code` | `green` | Code |
| `code-background` | `surface` | Behind inline code |
| `quote` | `muted` | Quote markers and bars |
| `list-marker` | `orange` | List bullets and numbers |
| `comment` | `muted` | `%%comments%%` |
| `frontmatter` | `purple` | Frontmatter |
| `math` | `orange` | `$math$` |
| `block-id` | `muted` | `^block-ids` |
| `strikethrough` | `muted` | `~~struck-through~~` text |
| `callout` | `callout-note` | Callout markers in Source mode |
| `callout-note`, `callout-tip`, `callout-success`, `callout-question`, `callout-warning`, `callout-failure`, `callout-danger`, `callout-bug`, `callout-example`, `callout-quote` | `blue`, `cyan`, `green`, `yellow`, `orange`, `red`, `red`, `red`, `purple`, `muted` | Each callout type (Live Preview) |

**Code in other languages**

| Role | Default |
|---|---|
| `keyword` | `purple` |
| `string` | `green` |
| `number` | `orange` |
| `constant` | `orange` |
| `type` | `yellow` |
| `function` | `blue` |
| `preprocessor` | `pink` |
| `special` | `cyan` |
| `error` | `red` |
| `warning` | `orange` |

**Git**

| Role | Default | Colours |
|---|---|---|
| `diff-added` | `green` | Added lines in a diff |
| `diff-removed` | `red` | Removed lines in a diff |
| `conflict` | `red` | Conflict markers (`<<<<<<<`, `=======`, `>>>>>>>`) |
| `conflict-background` | 12% of the way from background to red | Behind conflict markers |

### Problems

- **Ignored:** unknown keys and roles produce a warning but don't stop the theme from loading. That way, themes written for a newer Igneous still work.
- **Not loaded:** a theme with a missing `background` or `foreground`, a value that isn't a colour, or invalid TOML. Preferences lists these, with the reason, under "Themes That Couldn’t Be Loaded".
