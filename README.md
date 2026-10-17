<h1 align="center">OpenCode Stats</h1>

<p align="center">
  <a href="https://ratatui.rs/"><img src="https://img.shields.io/badge/Built_With_Ratatui-000?logo=ratatui&logoColor=fff" alt="Built With Ratatui"></a>
</p>

<p align="center">
  <a href="./README.md">English</a> | 
  <a href="./README_CN.md">中文</a>
</p>

A terminal dashboard for OpenCode usage statistics.

<img src="images/overview.png" alt="screenshot" style="zoom:50%;" />

`oc-stats` reads your local OpenCode SQLite database and displays token usage, cost estimates, model and provider distributions, and a 365-day activity heatmap directly in your terminal. Inspired by Claude Code's `/stats` command, this is an independent implementation focused on local execution, export, and shareability.

> If you're already using OpenCode and want a quick view of your usage, costs, and activity trends, this tool is ready to go.
>
> This is an unofficial community project and is not affiliated with, endorsed by, or maintained by the OpenCode team.

## Features

- Terminal dashboard UI built on `ratatui`
- Automatically reads the OpenCode local database
- Displays total tokens, costs, session count, message count, prompt count, and other overview metrics
- View usage breakdown by model and provider
- Supports four time ranges (all time, today, last 7 days, last 30 days) on the `Models`, `Providers` and `Sessions` pages; `Overview` always shows all time
- Built-in 365-day activity heatmap for observing long-term usage trends
- Supports dark / light theme via command-line argument
- Copy the current view to the clipboard as a share card, with the card also written as a PNG beside your exports and an automatic fallback to a text summary (`Ctrl+S` or `s`)
- Local model pricing cache with update / clean commands
- Output data calculation aligns with `opencode stats` and `opencode stats --models` for consistency

## Preview

OpenCode Stats provides three data perspectives:

| Year view                                                     | Model usage                                                 | Provider usage                                                    |
| ------------------------------------------------------------- | ----------------------------------------------------------- | ----------------------------------------------------------------- |
| <img src="images/overview.png" alt="screenshot" width="300"/> | <img src="images/models.png" alt="model uses" width="300"/> | <img src="images/providers.png" alt="provider uses" width="300"/> |

Each page also supports exporting a transparent-background share card directly to clipboard:

<img src="images/card.png" alt="card" style="zoom:50%;" />

## Installation

### Install from crates.io

```bash
cargo install opencode-stats
```

After installation, run:

```bash
oc-stats
```

### Download pre-built binaries from GitHub Releases

Download the archive for your platform from the Releases page, extract it, and run `oc-stats` directly.

The current release workflow builds for:

- Windows `x86_64-pc-windows-msvc`
- macOS `x86_64-apple-darwin`
- macOS `aarch64-apple-darwin`
- Linux `x86_64-unknown-linux-gnu`
- Linux `x86_64-unknown-linux-musl`

### Build from source

```bash
git clone https://github.com/Cateds/opencode-stats.git
cd opencode-stats
cargo build --release
```

The compiled binary will be located at:

```bash
target/release/oc-stats
```

Or build directly using the git path:

```bash
cargo install --git https://github.com/Cateds/opencode-stats.git
```

## Usage

### Default launch

By default, the program automatically locates your OpenCode local database and loads the data:

```bash
oc-stats
```

### Specify database path

```bash
oc-stats --db /path/to/opencode.db
```

### Specify theme

```bash
oc-stats --theme auto
oc-stats --theme dark
oc-stats --theme light
```

### Ignore placeholder zero-cost values

By default, `oc-stats` keeps stored costs as-is, including `cost: 0`, to preserve compatibility with existing behavior. If your OpenCode setup stores `cost: 0` as a placeholder for responses that still have token usage, use `--ignore-zero` to treat those zero values as missing and estimate the cost instead.

```bash
oc-stats --ignore-zero
```

### Scriptable stats

Print what the dashboard shows, for scripts and statuslines. Same numbers as the TUI, one line by default:

```bash
oc-stats stats --range today --format line
oc-stats stats --range 7d --format json
oc-stats stats --range 30d --format csv
```

Ranges are `all`, `today`, `7d` and `30d`. The line format is stable `key=value` pairs; JSON carries the overview, model rows, provider rows, session rows and the heatmap window with exact decimal costs; CSV opens directly in a spreadsheet. Output is plain text with no terminal codes, so it stays pipeable, and a missing database exits non-zero instead of reporting zeros.

Add `--out <PATH>` to write the output to a file instead of stdout; the written path is echoed so a script can chain on it. The file holds exactly the bytes stdout would have produced.

### Cache management commands

View the local pricing cache path:

```bash
oc-stats cache path
```

Update the local pricing cache:

```bash
oc-stats cache update
```

Clean the local pricing cache:

```bash
oc-stats cache clean
```

## Interaction

Once the program is running, you can quickly navigate pages and time ranges using the keyboard:

- `Tab` / `Left` / `Right` / `h` / `l` — Switch pages
- `Up` / `Down` / `j` / `k` — Move focus within `Models` / `Providers` pages
- `r` — Cycle through time ranges (on `Models`, `Providers` and `Sessions`)
- `1` / `2` / `3` / `4` — Quickly switch time ranges (same pages)
- `Ctrl+S` or `s` — Copy the current view to the clipboard as a share card, and write it as a PNG beside your exports (`s` is there for terminals that take `Ctrl+S` as XOFF)
- `e` — Export the current view as CSV and JSON
- `R` — Reload the database
- `f` / `/` — Search within the current page
- `q` / `Esc` / `Ctrl+C` — Exit the program

Pages:

- `Overview` — Overall usage summary
- `Models` — Usage statistics by model
- `Providers` — Usage statistics by provider

Time ranges:

- `All time`
- `Today`
- `Last 7 days`
- `Last 30 days`

The range applies to the `Models`, `Providers` and `Sessions` pages, where the selector appears in the header. `Overview` is the whole picture by design: it always shows all time, offers no range selector, and its exports and share card are named for all time.

## Data sources and pricing

### Data input

`oc-stats` reads the OpenCode local SQLite database.

Default database locations:

- Windows: `%APPDATA%/opencode/opencode.db`
- Linux: `~/.local/share/opencode/opencode.db`
- macOS: `~/Library/Application Support/opencode/opencode.db`

### Pricing data

Model pricing is read from local cache first and refreshed from remote when needed:

- Local cache path: `~/.cache/oc-stats/models.json`
- Remote source: `https://models.dev/api.json`
- Cache TTL: 1 hour

If local overrides exist in your OpenCode configuration, they take precedence.

When complete pricing information is unavailable, the program falls back to estimated cache read/write costs. If the database already contains actual costs, those values are prioritized.

If the database stores `cost: 0` for a nonzero-token response, `oc-stats` keeps that stored zero by default. Pass `--ignore-zero` to estimate the cost instead.

## Use cases

- Quickly view your OpenCode token consumption
- Analyze usage preferences by model or provider
- Understand recent and long-term usage trends
- Export statistics as images or text for easy sharing

## License

MIT

## Acknowledgments

- Font: [Cascadia Code](https://github.com/microsoft/cascadia-code), under SIL Open Font License
- Experience inspired by Claude Code's `/stats` command
- Reference project: [ocmonitor-share](https://github.com/Shlomob/ocmonitor-share)
