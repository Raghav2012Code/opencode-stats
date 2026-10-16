# Themes

`oc-stats` supports built-in themes and user-defined themes loaded from the XDG config directory, or the platform-equivalent config directory on macOS and Windows.

Theme files are optional. If you do not create any, `oc-stats` keeps using the built-in `dark` and `light` themes.

## Theme Sources

User themes are loaded from:

- `~/.config/oc-stats/themes.toml`
- `~/.config/oc-stats/themes/*.toml`

Load order:

1. Built-in themes
2. `themes.toml`
3. `themes/*.toml`

If a theme name appears in both places, the file in `themes/*.toml` overrides the entry from `themes.toml`.

## `themes.toml`

Use `[[theme]]` entries when you want multiple themes in one file:

```toml
[[theme]]
name = "nord-sea"
type = "dark"

[theme.base]
foreground = "#E5E9F0"
muted = "#808698"

[theme.card]
background = "#1C212B"
border = "#78829B"
shadow = "#000000"

[theme.accent]
primary = "#88C0D0"
comparison = "#B4BEFE"

[theme.tab]
active_fg = "#000000"
active_bg = "#88C0D0"

[theme.heatmap]
empty = "#5E6273"
active = "#88C0D0"

[theme.series]
model = ["#BF616A", "#D08770", "#EBCB8B", "#A3BE8C", "#88C0D0", "#81A1C1", "#B48EAD", "#AB7967", "#5E81AC", "#8FBCBB", "#D8DEE9", "#4C566A"]
```

## `themes/*.toml`

Use a single file when you want one theme per file:

`~/.config/oc-stats/themes/paper.toml`

```toml
type = "light"

[base]
foreground = "#252933"
muted = "#5A6273"

[card]
background = "#FCFDFF"
border = "#ADB7C9"
shadow = "#606B80"

[accent]
primary = "#007AA3"
comparison = "#5E5CE6"

[tab]
active_fg = "#FFFFFF"
active_bg = "#007AA3"

[heatmap]
empty = "#A0AABA"
active = "#007AA3"

[series]
model = ["#A72828", "#AF5E00", "#916C00", "#337A44", "#007AA3", "#3258A0", "#7E4C8E", "#784E34", "#486A9A", "#46969A", "#222222", "#5A6273"]
```

For files in `themes/`, the theme name comes from the file name. For example, `paper.toml` defines the `paper` theme.

## Required Fields

Every custom theme must include:

- `type = "dark"` or `type = "light"`
- `[base]`
- `[card]`
- `[accent]`
- `[tab]`
- `[heatmap]`
- `[series]`

`[series].model` must contain exactly 12 colors.

## Field Reference

### `type`

- `dark`: the theme can be selected by the dark theme slot
- `light`: the theme can be selected by the light theme slot

### `[base]`

- `foreground`: main text color
- `muted`: secondary text color

### `[card]`

- `background`: main panel background
- `border`: panel border color
- `shadow`: export card shadow color

### `[accent]`

- `primary`: highlight color for emphasized content
- `comparison`: color for comparison text

### `[tab]`

- `active_fg`: active tab text color
- `active_bg`: active tab background color

### `[heatmap]`

- `empty`: low-activity heatmap color
- `active`: active heatmap color

The map has five intensity levels and you configure two of them: `empty` is a day with no
usage and `active` is the fullest level. The three levels in between are interpolated
between those endpoints, so a theme never lists them.

### `[series]`

- `model`: 12-color palette used by model and provider charts

## Color Format

All colors must use `#RRGGBB` format.

Examples:

- `#000000`
- `#FFFFFF`
- `#88C0D0`

Unknown fields are rejected, so old or unused theme keys should be removed instead of kept around.

## Connecting Themes To `config.toml`

After defining custom themes, select them in `~/.config/oc-stats/config.toml`:

```toml
[theme]
default = "auto"
dark = "nord-sea"
light = "paper"
```
