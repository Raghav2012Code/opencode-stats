# Configuration

`oc-stats` reads user configuration from `~/.config/oc-stats/config.toml` on XDG-style systems, or the platform-equivalent config directory on macOS and Windows.

The file is optional. If it does not exist, `oc-stats` does not create it automatically and falls back to the built-in defaults.

## Theme Selection

Use the `[theme]` section to choose how the app resolves the active theme:

```toml
[theme]
default = "auto" # auto | dark | light
dark = "dark"
light = "light"
```

Fields:

- `default`: how `oc-stats` chooses the active theme when `--theme` is not passed
- `dark`: theme name to use when the resolved mode is dark
- `light`: theme name to use when the resolved mode is light

## Resolution Order

Theme selection uses this priority:

1. CLI flag: `oc-stats --theme auto|dark|light`
2. `config.toml` `theme.default`
3. Built-in fallback: `auto`

When `default = "auto"`, `oc-stats` tries to infer whether your terminal is using a dark or light background. It first checks explicit environment hints, then asks the terminal for its background color when supported, then falls back to `COLORFGBG`. If detection fails, it falls back to dark mode.

## Defaults

If `config.toml` is missing, these defaults are used:

```toml
[theme]
default = "auto"
dark = "dark"
light = "light"
```

The built-in `dark` and `light` theme names always exist, even if you do not create any theme files.

## Related Files

- Themes: `~/.config/oc-stats/themes/*.toml`
- Pricing cache: `~/.cache/oc-stats/models.json`

For theme file format details, see `docs/themes.md`.
