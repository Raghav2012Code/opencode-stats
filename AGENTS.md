# AGENTS.md

**Rust edition: 2024**

## Architecture

Data flow: `db::queries::load_app_data` → `analytics::build_snapshot` → `ui::app::App::run`

## Key Rules

- Usage counts `assistant` and `compaction` messages (both are billed model calls); `user` messages count as prompts only
- Default database paths: `%APPDATA%/opencode/opencode.db` (Windows), `~/.local/share/opencode/opencode.db` (Linux), `~/Library/Application Support/opencode/opencode.db` (macOS)

### Error Handling

- Domain modules (`db/`, `cache/`, `config/`) use `thiserror` with their own `errors.rs` and `Result<T>` alias
- User-facing code (`main.rs`, `app.rs`) uses `color_eyre::eyre::Result` with `.wrap_err()`

### Pricing

- Cache: `~/.cache/oc-stats/models.json` (TTL: 1 hour)
- Remote: `https://models.dev/api.json`
- OpenCode config overrides take priority
- Fallback: `cacheWrite = input`, `cacheRead = input * 0.1`
- Prefer stored cost from database when available

### Theme

- Config: `<config dir>/oc-stats/config.toml` (`~/.config/oc-stats` on Linux/macOS, `%APPDATA%\oc-stats` on Windows)
- Index: same directory, `themes.toml`
- Overrides: same directory, `themes/*.toml`
- `[app]` section: `page`, `range`, `db` (all optional; absent means Overview, all time, discovered database)

## Agent skills

### Issue tracker

Issues live in the GitHub repo `Raghav2012Code/opencode-stats` (a fork; upstream is `Cateds/opencode-stats`), driven by the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

Default five-role vocabulary, label strings unchanged. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `CONTEXT.md` plus `docs/adr/` at the repo root. See `docs/agents/domain.md`.
