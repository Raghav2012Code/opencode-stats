//! Baseline benchmarks for the slow-open / slow-typing complaint (#43).
//!
//! Builds a synthetic database of a known size, then times the two paths that
//! the performance tickets measure against:
//!   - cold start: `load_app_data` + `build_snapshot` (app launch to interactive UI)
//!   - find filter: `SearchState::update_filter` per keystroke on a list page
//!
//! Run with `cargo test baseline -- --nocapture`.
//!
//! Size is configurable through the environment so a machine can scale it:
//!   OC_STATS_BENCH_SESSIONS   number of sessions     (default 500)
//!   OC_STATS_BENCH_MSGS       messages per session   (default 30)
//!
//! A real database can be measured instead of the synthetic one; the find test
//! then types over its session titles, as the Sessions page does:
//!   OC_STATS_BENCH_DB         path to an opencode.db

use crate::analytics::build_snapshot;
use crate::analytics::view::View;
use crate::cache::models_cache::{PricingAvailability, PricingCatalog};
use crate::config::app_config::Brand;
use crate::db::models::InputOptions;
use crate::db::queries::load_app_data;
use crate::ui::app::{App, Page};
use crate::ui::models::SearchState;
use crate::ui::theme::Theme;
use crate::utils::pricing::ZeroCostBehavior;
use crate::utils::time::TimeRange;
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

const MODELS: &[&str] = &[
    "gpt-5",
    "claude-sonnet-4.5",
    "gemini-2.5-pro",
    "llama-3.3-70b",
    "mistral-large-2",
    "deepseek-v3",
    "qwen-3-235b",
    "phi-4",
    "gpt-4o",
    "claude-3.5-haiku",
];

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(default)
}

#[allow(clippy::too_many_arguments)]
fn message_json(
    role: &str,
    model: Option<&str>,
    provider: Option<&str>,
    input: u64,
    output: u64,
    created_ms: i64,
    completed_ms: i64,
    cost: Option<f64>,
) -> String {
    use serde_json::json;
    let mut msg = json!({
        "tokens": {
            "input": input,
            "output": output,
            "cache": { "read": 0, "write": 0 }
        },
        "time": {
            "created": created_ms,
            "completed": completed_ms
        },
        "finish": "stop"
    });
    if let Some(m) = model {
        msg["model"] = json!({ "providerID": provider, "id": m });
    }
    if let Some(c) = cost {
        msg["cost"] = json!(c);
    }
    // Modern messages keep the role in the `type` column, but including it
    // in the payload is harmless and keeps the JSON self-contained.
    msg.as_object_mut()
        .unwrap()
        .insert("role".into(), json!(role));
    msg.to_string()
}

fn create_synthetic_database(sessions: usize, messages_per_session: usize) -> PathBuf {
    let path = crate::test_support::unique_temp_path("baseline", ".db");
    let mut conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "
        CREATE TABLE project (id TEXT PRIMARY KEY, name TEXT, worktree TEXT);
        CREATE TABLE session_v2 (
            id TEXT PRIMARY KEY, project_id TEXT, parent_id TEXT, title TEXT,
            time_created INTEGER, time_updated INTEGER, time_archived INTEGER
        );
        CREATE TABLE session_message (
            id TEXT PRIMARY KEY, session_id TEXT, type TEXT, seq INTEGER,
            time_created INTEGER, time_updated INTEGER, data TEXT
        );
        ",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO project (id, name, worktree) VALUES ('proj_1', 'demo', '/tmp/demo')",
        [],
    )
    .unwrap();

    let now = chrono::Local::now().timestamp_millis();
    let day_ms = 86_400_000i64;

    // One transaction: without it every INSERT is its own implicit transaction
    // with an fsync, and a few thousand rows takes minutes in debug builds.
    let tx = conn.transaction().unwrap();
    {
        let mut session_stmt = tx.prepare(
            "INSERT INTO session_v2 (id, project_id, title, time_created, time_updated) \
             VALUES (?1, 'proj_1', ?2, ?3, ?4)",
        ).unwrap();
        let mut message_stmt = tx.prepare(
            "INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        ).unwrap();
        for s in 0..sessions {
            let session_id = format!("ses_{s}");
            let created = now - ((s % 30) as i64) * day_ms;
            let updated = created + 3_600_000;
            session_stmt.execute(rusqlite::params![
                session_id,
                format!("Session {s}"),
                created,
                updated,
            ]).unwrap();

            for m in 0..messages_per_session {
                let msg_id = format!("msg_{s}_{m}");
                let seq = (m as i32) + 1;
                let is_assistant = m % 3 != 0;
                let role = if is_assistant { "assistant" } else { "user" };
                let (model, provider, input, output, cost) = if is_assistant {
                    let model = MODELS[(s + m) % MODELS.len()];
                    let provider = match model {
                        "gpt-5" | "gpt-4o" => "openai",
                        "claude-sonnet-4.5" | "claude-3.5-haiku" => "anthropic",
                        "gemini-2.5-pro" => "google",
                        "llama-3.3-70b" => "groq",
                        "mistral-large-2" => "mistral",
                        "deepseek-v3" => "deepseek",
                        "qwen-3-235b" => "alibaba",
                        "phi-4" => "microsoft",
                        _ => "opencode",
                    };
                    let input = 100 + (((s * 7) + (m * 13)) % 900) as u64;
                    let output = 50 + (((s * 3) + (m * 5)) % 450) as u64;
                    let cost = input as f64 * 0.000002 + output as f64 * 0.000015;
                    (
                        Some(model.to_string()),
                        Some(provider.to_string()),
                        input,
                        output,
                        Some(cost),
                    )
                } else {
                    (None, None, 0, 0, None)
                };
                let msg_created = created + (m as i64) * 60_000;
                let msg_completed = msg_created + 1_200_000;
                let data = message_json(
                    role,
                    model.as_deref(),
                    provider.as_deref(),
                    input,
                    output,
                    msg_created,
                    msg_completed,
                    cost,
                );
                message_stmt.execute(rusqlite::params![
                    msg_id, session_id, role, seq, msg_created, msg_completed, data,
                ]).unwrap();
            }
        }
        Ok::<_, rusqlite::Error>(())
    }
    .unwrap();
    tx.commit().unwrap();
    drop(conn);
    path
}

fn empty_pricing() -> PricingCatalog {
    PricingCatalog {
        models: BTreeMap::new(),
        cache_path: PathBuf::from("/tmp/models.json"),
        refresh_needed: false,
        availability: PricingAvailability::Empty,
        load_notice: None,
    }
}

/// Synthetic search items representative of a large list page.
fn synthetic_search_items(count: usize) -> Vec<String> {
    (0..count)
        .map(|i| format!("{}/{}", MODELS[i % MODELS.len()], i))
        .collect()
}

#[test]
fn baseline_cold_start_and_find_filter_timing() {
    let sessions = env_usize("OC_STATS_BENCH_SESSIONS", 500);
    let msgs = env_usize("OC_STATS_BENCH_MSGS", 30);

    // With OC_STATS_BENCH_DB set, the run measures that database and leaves it
    // alone; otherwise a synthetic one is built and removed afterwards.
    let real_db = std::env::var("OC_STATS_BENCH_DB")
        .ok()
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from);
    let db_path = match &real_db {
        Some(path) => path.clone(),
        None => create_synthetic_database(sessions, msgs),
    };
    let size_bytes = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);

    // Cold start: load + snapshot, exactly what the app does before the first frame.
    let load_start = Instant::now();
    let data = load_app_data(&InputOptions {
        database_path: Some(db_path.clone()),
    })
    .expect("the benchmark database must load");
    let load_ms = load_start.elapsed().as_secs_f64() * 1000.0;

    let snap_start = Instant::now();
    let snapshot = build_snapshot(
        &data,
        &empty_pricing(),
        TimeRange::All,
        ZeroCostBehavior::KeepZero,
    );
    let snap_ms = snap_start.elapsed().as_secs_f64() * 1000.0;
    let total_ms = load_ms + snap_ms;

    // Shell availability (#46): constructing the loading shell, which is all the
    // first frame waits for. The database read lands afterwards, in the background.
    let shell_start = Instant::now();
    let _shell = App::loading(
        empty_pricing(),
        Theme::builtin_dark(),
        ZeroCostBehavior::KeepZero,
        InputOptions {
            database_path: None,
        },
        Page::Models,
        TimeRange::Last7Days,
        View::default(),
        None,
        Brand::default(),
        None,
    );
    let shell_ms = shell_start.elapsed().as_secs_f64() * 1000.0;

    // Find filter: time each keystroke over the page's list. One carried `SearchState`
    // is typed through the whole sequence, exactly like the real find path calls
    // `update_filter` per keystroke: appends (`g` to `gpt-5`) exercise the
    // incremental subset, the jump to `cla` a replacement, and `x` to `""` a deletion.
    // On the Sessions page the list is the session titles.
    let items = match &real_db {
        Some(_) => snapshot
            .sessions
            .iter()
            .map(|row| row.title.clone())
            .collect(),
        None => synthetic_search_items(sessions),
    };
    let queries = ["g", "gp", "gpt-", "gpt-5", "cla", "clau", "claude", "x", ""];
    let mut search = SearchState::new(items, 0);
    let item_count = search.ids.len();
    let mut keystrokes = Vec::new();
    for q in &queries {
        search.query = q.to_string();
        let start = Instant::now();
        search.update_filter();
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        keystrokes.push((q, elapsed_ms, search.filtered_indices.len()));
    }

    // Write report to temp file for reproducibility.
    let report = serde_json::json!({
        "database": {
            "path": db_path.display().to_string(),
            "source": if real_db.is_some() { "real" } else { "synthetic" },
            "size_bytes": size_bytes,
            "sessions": data.session_records.len(),
            "messages": data.messages.len(),
            "events": data.events.len(),
        },
        "cold_start": {
            "load_app_data_ms": load_ms,
            "build_snapshot_ms": snap_ms,
            "total_ms": total_ms,
        },
        "shell": {
            "construct_ms": shell_ms,
        },
        "find_filter": {
            "items": item_count,
            "keystrokes": keystrokes.iter().map(|(q, ms, n)| serde_json::json!({
                "query": q,
                "elapsed_ms": ms,
                "results": n,
            })).collect::<Vec<_>>(),
        },
    });

    let report_path = crate::test_support::unique_temp_path("baseline-report", ".json");
    std::fs::write(&report_path, serde_json::to_string_pretty(&report).unwrap()).unwrap();

    println!(
        "\n=== baseline ({}): {} sessions, {} messages, {} events, {} bytes ===",
        if real_db.is_some() {
            "real database"
        } else {
            "synthetic"
        },
        data.session_records.len(),
        data.messages.len(),
        data.events.len(),
        size_bytes
    );
    println!(
        "cold start: load {:.2}ms + snapshot {:.2}ms = {:.2}ms",
        load_ms, snap_ms, total_ms
    );
    println!("shell: construct {shell_ms:.3}ms");

    for (q, ms, n) in &keystrokes {
        println!("find {:>10}: {:.3}ms -> {} results", q, ms, n);
    }
    println!("report: {}", report_path.display());

    // Cleanup: only the database this run created.
    if real_db.is_none() {
        let _ = std::fs::remove_file(&db_path);
    }
}
