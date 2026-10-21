# Configuration

`oc-stats` reads user configuration from `config.toml` in its config directory:
`%APPDATA%\oc-stats\config.toml` on Windows, `~/.config/oc-stats/config.toml` on
Linux and macOS.

The file is optional. If it does not exist, `oc-stats` does not create it
automatically and falls back to the built-in defaults. Every section is
optional, and unknown keys are rejected: a typo fails at startup naming the key
rather than being silently ignored.

## `[app]`

What the dashboard opens with, and which database it reads.

```toml
[app]
page = "sessions"   # overview | models | providers | sessions
range = "7d"        # all | today | 7d | 30d
db = "D:\\snapshots\\opencode.db"
reload_every = "5m" # seconds, or a duration like "30s", "5m", "1h"
```

Fields:

- `page`: the page the app opens on. Absent means `overview`.
- `range`: the range the list pages (`Models`, `Providers`, `Sessions`) open on.
  Absent means `all`. The `Overview` page always shows all time and ignores this.
- `db`: the database to read. Absent means auto-discovery.
- `reload_every`: re-read the database on this interval. Absent means no
  automatic reload; reload manually with `R`.

An invalid value stops the program at startup and names the key and the values
it accepts:

```toml
[app]
page = "graphs"
```

```text
error: invalid value 'graphs' for config key [app] page; expected one of overview, models, providers, sessions
```

### Database precedence

Most specific first:

1. `--db PATH` command-line flag
2. `[app] db` in `config.toml`
3. `OC_STATS_DB_PATH` environment variable
4. Auto-discovery of the standard OpenCode locations

A named database — the flag or the config value — is used or reported missing.
It is never quietly replaced by a database found elsewhere.

## `[view]`

What to leave out of the picture. Every filter is off unless asked for, and an
absent section produces exactly the unfiltered output.

```toml
[view]
project = "volt-ledger"
pinned_models = ["space-bunny-free"]
pinned_providers = ["ollama"]
```

Fields:

- `project`: show only this project's sessions, matched the way the Sessions
  page names them — the worktree's file name first, then the stored project
  name, without regard to case. A project with no activity renders the empty
  state naming it.
- `pinned_models`, `pinned_providers`: these rows come first, in the given
  order; everything else keeps its order. Names that match nothing are ignored.

Free models are never filtered out: a model that cost nothing is still a row on
the `Models` page, and its tokens count towards every total. A model with no
price at all is kept and marked as missing — unknown is not zero.

Totals, percentages and row counts always describe the rows that remain, on
every surface: the TUI, `stats` and the export all build from the same
snapshot. While a filter is active, the rule above the content says so.

## `[budget]`

A monthly spending cap in US dollars.

```toml
[budget]
monthly = 50
```

When set, the Overview shows what the month cost so far, the cap, and what is
left; the over state reads `OVER by $X`. The month is always the calendar
month, so changing the range selector changes what is listed but never whether
you are over. A negative cap fails at startup. Absent means no budget line.

## `[brand]`

Your own words on the dashboard. Unset values render exactly as today.

```toml
[brand]
tagline = "Ship it"
comparison = false
exit_art = false
```

Fields:

- `tagline`: replaces the Overview's `Favorite model:` line and leads the
  share-card text summary, so the header and the card cannot disagree. An empty
  tagline fails at startup.
- `comparison`: `false` hides the fun-comparison sentence (`About 6.04 library
  sections, ...`). Unset or `true` shows it.
- `exit_art`: `false` skips the art printed on exit.

## `[theme]`

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

Theme selection uses this priority:

1. CLI flag: `oc-stats --theme auto|dark|light`
2. `config.toml` `theme.default`
3. Built-in fallback: `auto`

When `default = "auto"`, `oc-stats` tries to infer whether your terminal is using a dark or light background. It first checks explicit environment hints, then asks the terminal for its background color when supported, then falls back to `COLORFGBG`. If detection fails, it falls back to dark mode.

If `config.toml` is missing, these defaults are used:

```toml
[theme]
default = "auto"
dark = "dark"
light = "light"
```

The built-in `dark` and `light` theme names always exist, even if you do not create any theme files. Custom themes live in `themes/*.toml` beside `config.toml`, one file per theme, named by the file name. For theme file format details, see `docs/themes.md`.

## Recipes

These need no code and no config; they compose the shipped commands.

**Live usage in a prompt or statusline.** One line, no colour or terminal codes:

```bash
oc-stats stats --range today --format line
```

**A weekly digest from a scheduled task.** JSON is easy to feed to anything else;
`--out` writes the file and echoes its path:

```bash
oc-stats stats --range 7d --format json --out usage-week.json
```

**An export where you are working.** In the TUI, `e` drops timestamped CSV and
JSON files into the working directory, built from the same snapshot the screen
shows — including any active `[view]` filter.

**One run against a bright terminal**, without touching the config:

```bash
oc-stats --theme light
```

## Related Files

- Themes: `themes/*.toml` beside `config.toml`
- Pricing cache: `%LOCALAPPDATA%\oc-stats\models.json` on Windows,
  `~/.cache/oc-stats/models.json` on Linux and macOS
