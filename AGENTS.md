# AGENTS.md

**Rust edition: 2024**

## Architecture

Data flow: `db::queries::load_app_data` → `analytics::build_snapshot` → `ui::app::App::run`

## Key Rules

- Usage counts `assistant` and `compaction` messages (both are billed model calls)
- Messages count `user` and `assistant` turns; prompts count `user` turns. `compaction` is usage only, never a message or a prompt
- A turn with no model/provider attached counts toward its session's dominant model/provider; turns in sessions without usage stay overview- and sessions-only
- Free models are never filtered out: a model that cost nothing is still a row, and its tokens count towards every total
- Default database paths, checked in this order: `~/.local/share/opencode/opencode.db` (every platform), then on Windows `%LOCALAPPDATA%\opencode\opencode.db` and `%APPDATA%\opencode\opencode.db`, on macOS `~/Library/Application Support/opencode/opencode.db`

### Error Handling

- Domain modules (`db/`, `cache/`, `config/`) use `thiserror` with their own `errors.rs` and `Result<T>` alias
- User-facing code (`main.rs`, `app.rs`) uses `color_eyre::eyre::Result` with `.wrap_err()`

### Pricing

- Cache: `dirs::cache_dir()/oc-stats/models.json` — `%LOCALAPPDATA%\oc-stats\models.json` on Windows, `~/.cache/oc-stats/models.json` on Linux and macOS (TTL: 1 hour)
- Remote: `https://models.dev/api.json`
- OpenCode config overrides take priority
- Fallback: `cacheWrite = input`, `cacheRead = input * 0.1`
- Prefer stored cost from database when available

### Config

- Config: `<config dir>/oc-stats/config.toml` (`~/.config/oc-stats` on Linux/macOS, `%APPDATA%\oc-stats` on Windows)
- Themes: same directory, `themes/*.toml` (name from the file name; overrides a built-in of the same name)
- `[app]` section: `page`, `range`, `db` (all optional; absent means Overview, all time, discovered database)
- `[view]` section: `project`, `pinned_models`, `pinned_providers` (all optional; no filter unless asked for)
- `[budget]` section: `monthly` (a spending cap in USD, measured over the last 30 days)
- Full reference: `docs/config.md`

## Agent skills

### Issue tracker

Issues live in the GitHub repo `Raghav2012Code/opencode-stats` (a fork; upstream is `Cateds/opencode-stats`), driven by the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

Default five-role vocabulary, label strings unchanged. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `CONTEXT.md` plus `docs/adr/` at the repo root. See `docs/agents/domain.md`.
