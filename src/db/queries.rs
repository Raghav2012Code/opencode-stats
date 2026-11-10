use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};

use crate::cache::usage_cache;
use crate::db::connection::{self, discover_database_path, open_database};
use crate::db::errors::{Error, Result};
use crate::db::hermes;
use crate::db::models::{
    AppData, ImportStats, InputOptions, JsonMessageRecord, MessageRecord, SessionRecord,
    TokenUsage, UsageEvent,
};
use crate::db::pi;
use crate::utils::time::timestamp_ms_to_local;

#[derive(Clone, Debug)]
struct SessionRow {
    id: String,
    parent_id: Option<String>,
    project_name: Option<String>,
    project_worktree: Option<PathBuf>,
    title: Option<String>,
    time_created: Option<DateTime<Local>>,
    time_updated: Option<DateTime<Local>>,
    time_archived: Option<DateTime<Local>>,
}

pub fn load_app_data(options: &InputOptions) -> Result<AppData> {
    let db_path = discover_database_path(options.database_path.as_deref()).ok_or_else(|| {
        Error::database_not_found(connection::database_candidates(
            options.database_path.as_deref(),
        ))
    })?;
    // Reading the payload column is what a launch spends its time on; with nothing written
    // since the last read the parsed rows are the same, so reuse them instead of scanning.
    let mut data = match usage_cache::load(&db_path) {
        Some(data) => data,
        None => {
            let data = load_from_sqlite(&db_path)?;
            usage_cache::store(&db_path, &data);
            data
        }
    };

    if let Some(path) = options.hermes_path.as_deref() {
        merge_hermes(&mut data, path);
    }
    if let Some(dir) = options.pi_sessions_dir.as_deref() {
        merge_pi(&mut data, dir);
    }
    data.events.sort_by_key(|event| event.created_at);

    Ok(data)
}

/// A second source only ever adds usage and sessions; a store that cannot be read is
/// reported instead of failing the run, so one tool's upgrade cannot hide OpenCode's data.
fn merge_hermes(data: &mut AppData, path: &Path) {
    match hermes::load(path) {
        Ok(load) => {
            data.events.extend(load.events);
            data.session_records.extend(load.sessions);
        }
        Err(err) => {
            data.import_stats.hermes_notice = Some(format!("Hermes usage unavailable: {err}"));
        }
    }
}

/// Pi tolerates individual files; only the count of unreadable ones is reported.
fn merge_pi(data: &mut AppData, dir: &Path) {
    let load = pi::load(dir);
    data.events.extend(load.events);
    data.session_records.extend(load.sessions);
    if load.skipped_files > 0 {
        let noun = if load.skipped_files == 1 {
            "file"
        } else {
            "files"
        };
        data.import_stats.pi_notice = Some(format!(
            "Skipped {} unreadable Pi session {noun}",
            load.skipped_files
        ));
    }
}

pub fn load_from_sqlite(db_path: &Path) -> Result<AppData> {
    let conn = open_database(db_path)?;
    let schema = detect_schema(&conn)?;

    let mut session_stmt = conn
        .prepare(sessions_sql(schema))
        .map_err(Error::database_query)?;

    let sessions_iter = session_stmt
        .query_map([], |row| {
            Ok(SessionRow {
                id: row.get("id")?,
                parent_id: row.get("parent_id")?,
                project_name: row.get("project_name")?,
                project_worktree: row
                    .get::<_, Option<String>>("project_worktree")?
                    .map(PathBuf::from),
                title: row.get("title")?,
                time_created: row
                    .get::<_, Option<i64>>("time_created")?
                    .and_then(timestamp_ms_to_local),
                time_updated: row
                    .get::<_, Option<i64>>("time_updated")?
                    .and_then(timestamp_ms_to_local),
                time_archived: row
                    .get::<_, Option<i64>>("time_archived")?
                    .and_then(timestamp_ms_to_local),
            })
        })
        .map_err(Error::database_query)?;

    let mut sessions = Vec::new();
    for session in sessions_iter {
        sessions.push(session.map_err(Error::database_query)?);
    }

    let session_lookup = sessions
        .iter()
        .map(|session| (session.id.as_str(), session))
        .collect::<BTreeMap<_, _>>();
    let sqlite_messages = load_messages_sqlite(&conn, &session_lookup, schema)?;

    let mut all_events = Vec::new();
    let mut all_messages = Vec::new();
    let mut session_records = Vec::new();
    for message in sqlite_messages.messages {
        if let Some(event) = message.event {
            all_events.push(event);
        }
        all_messages.push(message.record);
    }
    for session in sessions {
        if let (Some(created_at), Some(updated_at)) = (session.time_created, session.time_updated) {
            session_records.push(SessionRecord {
                session_id: session.id.clone(),
                created_at,
                updated_at,
            });
        }
    }

    finalize_app_data(
        all_events,
        all_messages,
        session_records,
        ImportStats {
            skipped_sqlite_messages: sqlite_messages.skipped_messages,
            hermes_notice: None,
            pi_notice: None,
        },
    )
}

/// Which table sets this database has. A fresh OpenCode install has only the modern
/// pair; a database mid-migration has both; an old one has only the legacy pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Schema {
    modern: bool,
    legacy: bool,
}

fn detect_schema(conn: &rusqlite::Connection) -> Result<Schema> {
    Ok(Schema {
        modern: connection::table_exists(conn, "session_v2")?
            && connection::table_exists(conn, "session_message")?,
        legacy: connection::table_exists(conn, "session")?
            && connection::table_exists(conn, "message")?,
    })
}

fn sessions_sql(schema: Schema) -> &'static str {
    match schema {
        // A migrated session keeps a copy in both tables, so take v2 and add only the
        // legacy sessions v2 does not have.
        Schema {
            modern: true,
            legacy: true,
        } => {
            "
        SELECT s.id, s.parent_id, s.title, s.time_created, s.time_updated, s.time_archived,
               p.name as project_name, p.worktree as project_worktree
        FROM session_v2 s
        LEFT JOIN project p ON s.project_id = p.id
        UNION ALL
        SELECT s.id, s.parent_id, s.title, s.time_created, s.time_updated, s.time_archived,
               p.name as project_name, p.worktree as project_worktree
        FROM session s
        LEFT JOIN project p ON s.project_id = p.id
        WHERE s.id NOT IN (SELECT id FROM session_v2)
        "
        }
        Schema {
            modern: true,
            legacy: false,
        } => {
            "
        SELECT s.id, s.parent_id, s.title, s.time_created, s.time_updated, s.time_archived,
               p.name as project_name, p.worktree as project_worktree
        FROM session_v2 s
        LEFT JOIN project p ON s.project_id = p.id
        "
        }
        Schema {
            modern: false,
            legacy: true,
        } => {
            "
        SELECT s.id, s.parent_id, s.title, s.time_created, s.time_updated, s.time_archived,
               p.name as project_name, p.worktree as project_worktree
        FROM session s
        LEFT JOIN project p ON s.project_id = p.id
        "
        }
        // Discovery rejects a database with neither table set, so this is unreachable;
        // the legacy query keeps it harmless if that ever changes.
        Schema {
            modern: false,
            legacy: false,
        } => {
            "
        SELECT s.id, s.parent_id, s.title, s.time_created, s.time_updated, s.time_archived,
               p.name as project_name, p.worktree as project_worktree
        FROM session s
        LEFT JOIN project p ON s.project_id = p.id
        "
        }
    }
}

fn messages_sql(schema: Schema) -> &'static str {
    match schema {
        Schema {
            modern: true,
            legacy: true,
        } => {
            "
        SELECT session_id, type AS role, data
        FROM session_message
        WHERE type IN ('assistant', 'compaction', 'user')
        UNION ALL
        SELECT session_id, NULL AS role, data
        FROM message
        WHERE id NOT IN (SELECT id FROM session_message)
        "
        }
        Schema {
            modern: true,
            legacy: false,
        } => {
            "
        SELECT session_id, type AS role, data
        FROM session_message
        WHERE type IN ('assistant', 'compaction', 'user')
        "
        }
        Schema {
            modern: false,
            legacy: true,
        } => {
            "
        SELECT session_id, NULL AS role, data
        FROM message
        "
        }
        Schema {
            modern: false,
            legacy: false,
        } => {
            "
        SELECT session_id, NULL AS role, data
        FROM message
        "
        }
    }
}

struct ParsedMessage {
    record: MessageRecord,
    event: Option<UsageEvent>,
}

struct SqliteMessageLoad {
    messages: Vec<ParsedMessage>,
    skipped_messages: usize,
}

/// One row as SQLite hands it over: small columns copied, payload bytes owned. Parsing
/// happens on worker threads; fetching stays a single sequential scan, which is the
/// fastest pattern for a multi-hundred-megabyte table on any disk.
struct RawMessage {
    session_id: String,
    role: Option<String>,
    data: Vec<u8>,
}

struct ParsedBatch {
    messages: Vec<Option<ParsedMessage>>,
    skipped: usize,
}

/// Rows per batch: large enough that thread handoffs stay rare, small enough that the
/// in-flight batches fit comfortably next to the parsed output.
const PARSE_BATCH_ROWS: usize = 512;
/// Batches in flight at once, bounding the extra memory to a few tens of megabytes.
const PARSE_CHANNEL_BATCHES: usize = 4;

pub(crate) fn parser_threads() -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(4)
        .clamp(1, 8)
}

fn parse_message_batch(
    batch: Vec<RawMessage>,
    sessions: &BTreeMap<&str, &SessionRow>,
) -> ParsedBatch {
    let mut messages = Vec::with_capacity(batch.len());
    let mut skipped = 0usize;

    for raw in batch {
        let Some(session) = sessions.get(raw.session_id.as_str()) else {
            skipped = skipped.saturating_add(1);
            continue;
        };

        let mut record: JsonMessageRecord = match serde_json::from_slice(&raw.data) {
            Ok(record) => record,
            Err(_) => {
                skipped = skipped.saturating_add(1);
                continue;
            }
        };

        // Modern messages keep the role in the `type` column, not in the payload.
        if record.role.is_none() {
            record.role = raw.role.clone();
        }

        messages.push(parse_json_record(record, session));
    }

    ParsedBatch { messages, skipped }
}

fn load_messages_sqlite(
    conn: &rusqlite::Connection,
    sessions: &BTreeMap<&str, &SessionRow>,
    schema: Schema,
) -> Result<SqliteMessageLoad> {
    let mut stmt = conn
        .prepare(messages_sql(schema))
        .map_err(Error::database_query)?;

    // Batches flow from the fetching main thread to the parsers; results land in one
    // shared vector and are ordered by batch index afterwards, so the rows come out in
    // exactly the order the old single-threaded loop produced them.
    let (work_tx, work_rx) =
        std::sync::mpsc::sync_channel::<(usize, Vec<RawMessage>)>(PARSE_CHANNEL_BATCHES);
    let work_rx = std::sync::Arc::new(std::sync::Mutex::new(work_rx));
    let parsed_batches = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut skipped_messages = 0usize;

    std::thread::scope(|scope| {
        for _ in 0..parser_threads() {
            let work_rx = std::sync::Arc::clone(&work_rx);
            let parsed_batches = std::sync::Arc::clone(&parsed_batches);
            scope.spawn(move || {
                while let Ok((index, batch)) = work_rx
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .recv()
                {
                    let parsed = parse_message_batch(batch, sessions);
                    parsed_batches
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push((index, parsed));
                }
            });
        }

        let mut rows = stmt.query([]).map_err(Error::database_query)?;
        let mut batch = Vec::with_capacity(PARSE_BATCH_ROWS);
        let mut batch_index = 0usize;
        loop {
            match rows.next() {
                Err(err) => {
                    drop(work_tx);
                    return Err(Error::database_query(err));
                }
                Ok(None) => break,
                Ok(Some(row)) => match read_raw_message(row, sessions) {
                    Ok(Some(raw)) => batch.push(raw),
                    Ok(None) => skipped_messages = skipped_messages.saturating_add(1),
                    Err(err) => {
                        drop(work_tx);
                        return Err(err);
                    }
                },
            }
            if batch.len() >= PARSE_BATCH_ROWS {
                let full = std::mem::replace(&mut batch, Vec::with_capacity(PARSE_BATCH_ROWS));
                if work_tx.send((batch_index, full)).is_err() {
                    break;
                }
                batch_index += 1;
            }
        }
        if !batch.is_empty() {
            // The parsers are gone only if one panicked, in which case the scope
            // re-raises it on the way out; dropping the batch is then moot.
            let _ = work_tx.send((batch_index, batch));
        }
        drop(work_tx);
        // Every sent batch is parsed: receivers only stop once the sender is dropped
        // and the queue is drained, and the scope joins every parser.
        Ok(())
    })?;

    let mut batches = parsed_batches
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .drain(..)
        .collect::<Vec<_>>();
    batches.sort_by_key(|(index, _)| *index);

    let mut messages = Vec::new();
    for (_, batch) in batches {
        skipped_messages = skipped_messages.saturating_add(batch.skipped);
        messages.extend(batch.messages.into_iter().flatten());
    }

    Ok(SqliteMessageLoad {
        messages,
        skipped_messages,
    })
}

/// Reads one row's small columns and copies its payload, without parsing it. Rows for
/// unknown sessions and non-text payloads are counted as skipped here, exactly as the
/// old loop did before it ever reached the JSON parser.
fn read_raw_message(
    row: &rusqlite::Row<'_>,
    sessions: &BTreeMap<&str, &SessionRow>,
) -> Result<Option<RawMessage>> {
    let session_id: String = row.get("session_id").map_err(Error::database_query)?;
    if !sessions.contains_key(session_id.as_str()) {
        return Ok(None);
    }
    let role: Option<String> = row.get("role").map_err(Error::database_query)?;

    match row.get_ref("data").map_err(Error::database_query)? {
        rusqlite::types::ValueRef::Text(bytes) => Ok(Some(RawMessage {
            session_id,
            role,
            data: bytes.to_owned(),
        })),
        _ => Ok(None),
    }
}

fn parse_json_record(record: JsonMessageRecord, session: &SessionRow) -> Option<ParsedMessage> {
    let role = normalize_optional_text(record.role.clone());
    let provider_id = normalize_optional_text(record.provider_id.clone()).or_else(|| {
        record
            .model
            .as_ref()
            .and_then(|model| normalize_optional_text(model.provider_id.clone()))
    });
    let model_id = normalize_optional_text(record.model_id.clone()).or_else(|| {
        record
            .model
            .as_ref()
            .and_then(|model| normalize_optional_text(model.model_id.clone()))
    });
    let created_at = record
        .time
        .as_ref()
        .and_then(|time| time.created)
        .and_then(timestamp_ms_to_local);

    let message = MessageRecord {
        session_id: session.id.clone(),
        role: role.clone(),
        provider_id: provider_id.clone(),
        model_id: model_id.clone(),
        created_at,
    };

    // `compaction` is a model call that produces a session digest, so it carries
    // billable usage but is neither a user prompt nor an assistant turn.
    let carries_usage = matches!(role.as_deref(), Some("assistant") | Some("compaction"));
    if !carries_usage {
        return Some(ParsedMessage {
            record: message,
            event: None,
        });
    }

    let model_id = model_id?;

    let tokens = TokenUsage {
        input: record
            .tokens
            .as_ref()
            .and_then(|value| value.input)
            .unwrap_or(0),
        output: record
            .tokens
            .as_ref()
            .and_then(|value| value.output)
            .unwrap_or(0),
        cache_read: record
            .tokens
            .as_ref()
            .and_then(|value| value.cache.as_ref())
            .and_then(|value| value.read)
            .unwrap_or(0),
        cache_write: record
            .tokens
            .as_ref()
            .and_then(|value| value.cache.as_ref())
            .and_then(|value| value.write)
            .unwrap_or(0),
    };

    let completed_at = record
        .time
        .as_ref()
        .and_then(|time| time.completed)
        .and_then(timestamp_ms_to_local);

    let event = UsageEvent {
        session_id: session.id.clone(),
        parent_session_id: session.parent_id.clone(),
        session_title: session.title.clone(),
        session_started_at: session.time_created,
        session_archived_at: session.time_archived,
        project_name: session.project_name.clone(),
        project_path: record
            .path
            .as_ref()
            .and_then(|path| path.cwd.clone().or(path.root.clone()))
            .or_else(|| session.project_worktree.clone()),
        provider_id,
        model_id,
        agent: record.agent,
        finish_reason: record.finish,
        tokens,
        created_at,
        completed_at,
        stored_cost_usd: record.cost,
    };

    Some(ParsedMessage {
        record: message,
        event: (event.tokens.total() > 0).then_some(event),
    })
}

fn normalize_optional_text(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

fn finalize_app_data(
    mut events: Vec<UsageEvent>,
    messages: Vec<MessageRecord>,
    session_records: Vec<SessionRecord>,
    import_stats: ImportStats,
) -> Result<AppData> {
    events.sort_by_key(|event| event.created_at);

    Ok(AppData {
        events,
        messages,
        session_records,
        import_stats,
    })
}

#[cfg(test)]
mod tests {
    use super::{load_app_data, load_from_sqlite, parse_json_record};
    use crate::cache::models_cache::{PricingAvailability, PricingCatalog};
    use crate::db::models::{
        InputOptions, JsonCacheTokensRecord, JsonMessageRecord, JsonPathRecord, JsonTimeRecord,
        JsonTokensRecord,
    };
    use crate::test_support::{unique_temp_dir, unique_temp_path};
    use crate::utils::pricing::ZeroCostBehavior;
    use crate::utils::time::TimeRange;
    use rusqlite::Connection;
    use std::fs;
    use std::path::{Path, PathBuf};

    #[test]
    fn parses_assistant_json_record() {
        let record = JsonMessageRecord {
            role: Some("assistant".to_string()),
            provider_id: Some("anthropic".to_string()),
            model_id: Some("claude-sonnet-4.5".to_string()),
            tokens: Some(JsonTokensRecord {
                input: Some(10),
                output: Some(20),
                cache: Some(JsonCacheTokensRecord {
                    read: Some(1),
                    write: Some(2),
                }),
            }),
            time: Some(JsonTimeRecord {
                created: Some(1_710_000_000_000),
                completed: Some(1_710_000_001_000),
            }),
            path: Some(JsonPathRecord {
                cwd: Some("C:/repo".into()),
                root: None,
            }),
            agent: Some("build".to_string()),
            finish: Some("stop".to_string()),
            cost: None,
            model: None,
        };

        let session = super::SessionRow {
            id: "ses_1".to_string(),
            parent_id: None,
            project_name: None,
            project_worktree: None,
            title: None,
            time_created: None,
            time_updated: None,
            time_archived: None,
        };

        let parsed = parse_json_record(record, &session).unwrap();
        let event = parsed.event.unwrap();
        assert_eq!(event.tokens.total(), 33);
        assert_eq!(event.model_id, "claude-sonnet-4.5");
        assert_eq!(event.provider_id.as_deref(), Some("anthropic"));
    }

    #[test]
    fn reads_a_database_that_has_only_the_modern_tables() {
        // A fresh OpenCode install has session_v2 and session_message and no legacy pair.
        // Queries that name the legacy tables must not be issued at all.
        let db_path = unique_temp_path("modern-only", ".db");
        let conn = Connection::open(&db_path).unwrap();

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
            INSERT INTO project (id, name, worktree) VALUES ('proj_1', 'fresh', '/tmp/fresh');
            INSERT INTO session_v2 (id, project_id, title, time_created, time_updated)
                VALUES ('ses_fresh', 'proj_1', 'Fresh install', 1710000000000, 1710000001000);
            ",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_message (id, session_id, type, seq, data) VALUES ('msg_1', 'ses_fresh', 'assistant', 1, ?1)",
            [r#"{"model":{"providerID":"openai","id":"fresh-model"},"cost":0.25,"tokens":{"input":100,"output":10,"cache":{"read":0,"write":0}},"time":{"created":1710000000000,"completed":1710000001000}}"#],
        )
        .unwrap();
        drop(conn);

        let data = load_from_sqlite(&db_path).unwrap();

        assert_eq!(data.events.len(), 1);
        assert_eq!(data.messages.len(), 1);
        assert_eq!(data.events[0].model_id, "fresh-model");
        assert_eq!(data.events[0].provider_id.as_deref(), Some("openai"));
        assert_eq!(data.session_records.len(), 1);

        let _ = fs::remove_file(db_path);
    }

    #[test]
    fn reads_modern_tables_without_double_counting_migrated_sessions() {
        let db_path = unique_temp_path("v2-test", ".db");
        let conn = Connection::open(&db_path).unwrap();

        conn.execute_batch(
            "
            CREATE TABLE project (id TEXT PRIMARY KEY, name TEXT, worktree TEXT);
            CREATE TABLE session (
                id TEXT PRIMARY KEY,
                project_id TEXT,
                parent_id TEXT,
                title TEXT,
                time_created INTEGER,
                time_updated INTEGER,
                time_archived INTEGER
            );
            CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT,
                data TEXT,
                time_created INTEGER
            );
            CREATE TABLE session_v2 (
                id TEXT PRIMARY KEY,
                project_id TEXT,
                parent_id TEXT,
                title TEXT,
                time_created INTEGER,
                time_updated INTEGER,
                time_archived INTEGER
            );
            CREATE TABLE session_message (
                id TEXT PRIMARY KEY,
                session_id TEXT,
                type TEXT,
                seq INTEGER,
                time_created INTEGER,
                time_updated INTEGER,
                data TEXT
            );
            ",
        )
        .unwrap();

        conn.execute(
            "INSERT INTO project (id, name, worktree) VALUES ('proj_1', 'demo', '/tmp/demo')",
            [],
        )
        .unwrap();

        let session_insert = "INSERT INTO {} (id, project_id, title, time_created, time_updated) \
                              VALUES (?1, 'proj_1', ?2, 1710000000000, 1710000001000)";
        for (table, id, title) in [
            ("session", "ses_migrated", "Migrated"),
            ("session_v2", "ses_migrated", "Migrated"),
            ("session", "ses_legacy", "Legacy only"),
            ("session_v2", "ses_new", "Modern only"),
        ] {
            conn.execute(&session_insert.replace("{}", table), (id, title))
                .unwrap();
        }

        // A migrated row is kept in both tables under the same id: counted once.
        conn.execute(
            "INSERT INTO message (id, session_id, data) VALUES ('msg_1', 'ses_migrated', ?1)",
            [r#"{"role":"assistant","providerID":"opencode","modelID":"muse-spark","cost":0.10,"tokens":{"input":100,"output":10},"time":{"created":1710000000000,"completed":1710000001000}}"#],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_message (id, session_id, type, seq, data) \
             VALUES ('msg_1', 'ses_migrated', 'assistant', 1, ?1)",
            [r#"{"model":{"providerID":"opencode","id":"muse-spark"},"cost":0.10,"tokens":{"input":100,"output":10,"reasoning":5,"cache":{"read":0,"write":0}},"time":{"created":1710000000000,"completed":1710000001000}}"#],
        )
        .unwrap();

        // A row the migration never copied: real usage that only the legacy table has.
        conn.execute(
            "INSERT INTO message (id, session_id, data) VALUES ('msg_orphan', 'ses_migrated', ?1)",
            [r#"{"role":"assistant","providerID":"opencode","modelID":"muse-spark","cost":0.05,"tokens":{"input":50,"output":5},"time":{"created":1710000000000,"completed":1710000001000}}"#],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO message (id, session_id, data) VALUES ('msg_2', 'ses_legacy', ?1)",
            [r#"{"role":"assistant","providerID":"opencode","modelID":"muse-spark","cost":0.40,"tokens":{"input":100,"output":10},"time":{"created":1710000000000,"completed":1710000001000}}"#],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_message (id, session_id, type, seq, data) \
             VALUES ('msg_3', 'ses_new', 'assistant', 1, ?1)",
            [r#"{"model":{"providerID":"opencode","id":"glm-5.3-flash"},"cost":1.25,"tokens":{"input":100,"output":10},"time":{"created":1710000000000,"completed":1710000001000}}"#],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_message (id, session_id, type, seq, data) \
             VALUES ('msg_4', 'ses_new', 'user', 2, ?1)",
            [r#"{"content":[]}"#],
        )
        .unwrap();
        // A compaction digest is a billed model call with no user or assistant turn.
        conn.execute(
            "INSERT INTO session_message (id, session_id, type, seq, data) \
             VALUES ('msg_5', 'ses_new', 'compaction', 3, ?1)",
            [r#"{"model":{"providerID":"opencode-go","id":"space-bunny-free"},"cost":0.25,"summary":"Objective ...","tokens":{"input":70,"output":7,"cache":{"read":0,"write":0}},"time":{"created":1710000000000}}"#],
        )
        .unwrap();
        drop(conn);

        let data = load_from_sqlite(&db_path).unwrap();
        let total_cost: rust_decimal::Decimal = data
            .events
            .iter()
            .map(|event| event.stored_cost_usd.unwrap())
            .sum();

        assert_eq!(data.events.len(), 5);
        assert_eq!(data.session_records.len(), 3);
        assert_eq!(total_cost, rust_decimal::Decimal::new(205, 2));
        assert_eq!(data.messages.len(), 6);
        let modern = data
            .events
            .iter()
            .find(|event| event.model_id == "glm-5.3-flash")
            .unwrap();
        assert_eq!(modern.provider_id.as_deref(), Some("opencode"));
        assert_eq!(modern.session_id, "ses_new");
        let compaction = data
            .events
            .iter()
            .find(|event| event.model_id == "space-bunny-free")
            .unwrap();
        assert_eq!(
            compaction.stored_cost_usd,
            Some(rust_decimal::Decimal::new(25, 2))
        );
        assert_eq!(
            data.events
                .iter()
                .filter(|event| event.session_id == "ses_migrated")
                .count(),
            2
        );

        let _ = fs::remove_file(db_path);
    }

    #[test]
    fn tracks_skipped_sqlite_messages_during_import() {
        let db_path = unique_temp_path("load-test", ".db");
        let conn = Connection::open(&db_path).unwrap();

        conn.execute_batch(
            "
            CREATE TABLE project (id TEXT PRIMARY KEY, name TEXT, worktree TEXT);
            CREATE TABLE session (
                id TEXT PRIMARY KEY,
                project_id TEXT,
                parent_id TEXT,
                title TEXT,
                time_created INTEGER,
                time_updated INTEGER,
                time_archived INTEGER
            );
            CREATE TABLE message (
                session_id TEXT,
                data TEXT,
                time_created INTEGER
            );
            ",
        )
        .unwrap();

        conn.execute(
            "INSERT INTO project (id, name, worktree) VALUES (?1, ?2, ?3)",
            ("proj_1", "demo", "/tmp/demo"),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, title, time_created, time_updated) VALUES (?1, ?2, ?3, ?4, ?5)",
            ("ses_1", "proj_1", "Demo", 1_710_000_000_000i64, 1_710_000_001_000i64),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (session_id, data, time_created) VALUES (?1, ?2, ?3)",
            (
                "ses_1",
                r#"{"role":"assistant","providerID":"openai","modelID":"gpt-5","tokens":{"input":10,"output":20},"time":{"created":1710000000000,"completed":1710000001000}}"#,
                1_710_000_000_000i64,
            ),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (session_id, data, time_created) VALUES (?1, ?2, ?3)",
            ("ses_1", "not-json", 1_710_000_000_001i64),
        )
        .unwrap();
        drop(conn);

        let data = load_from_sqlite(&db_path).unwrap();

        assert_eq!(data.import_stats.skipped_sqlite_messages, 1);
        assert_eq!(data.messages.len(), 1);
        assert_eq!(data.events.len(), 1);

        let _ = fs::remove_file(db_path);
    }

    fn opencode_fixture(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "
            CREATE TABLE project (id TEXT PRIMARY KEY, name TEXT, worktree TEXT);
            CREATE TABLE session (
                id TEXT PRIMARY KEY, project_id TEXT, parent_id TEXT, title TEXT,
                time_created INTEGER, time_updated INTEGER, time_archived INTEGER
            );
            CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, data TEXT, time_created INTEGER);
            INSERT INTO project (id, name, worktree) VALUES ('proj_1', 'demo', '/tmp/demo');
            INSERT INTO session (id, project_id, title, time_created, time_updated)
                VALUES ('ses_open', 'proj_1', 'OpenCode', 1710000000000, 1710000001000);
            ",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, data) VALUES ('msg_open', 'ses_open', ?1)",
            [r#"{"role":"assistant","providerID":"opencode-go","modelID":"deepseek-v4.1-flash","cost":0.25,"tokens":{"input":100,"output":10},"time":{"created":1710000000000,"completed":1710000001000}}"#],
        )
        .unwrap();
    }

    fn hermes_fixture(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "
            CREATE TABLE sessions (
                id TEXT PRIMARY KEY, parent_session_id TEXT, title TEXT, cwd TEXT,
                git_repo_root TEXT, started_at REAL NOT NULL, ended_at REAL,
                last_activity_at REAL, billing_provider TEXT, model TEXT
            );
            CREATE TABLE session_model_usage (
                session_id TEXT, model TEXT, billing_provider TEXT,
                input_tokens INTEGER, output_tokens INTEGER,
                cache_read_tokens INTEGER, cache_write_tokens INTEGER,
                estimated_cost_usd REAL, actual_cost_usd REAL,
                first_seen REAL, last_seen REAL
            );
            INSERT INTO sessions (id, parent_session_id, title, cwd, started_at, ended_at, billing_provider)
                VALUES ('20261020_224127_bbdb38', 'parent_x', 'Hermes work',
                        'C:/repo/hermes-project', 1710000100.5, 1710000200.5, 'opencode-go');
            INSERT INTO sessions (id, title, started_at, ended_at, billing_provider)
                VALUES ('20261021_000000_aaaaaa', NULL, 1710000300.0, 1710000400.0, 'opencode-go');
            INSERT INTO sessions (id, title, started_at)
                VALUES ('20261022_000000_bbbbbb', 'No usage', 1710000500.0);
            ",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_model_usage VALUES
             ('20261020_224127_bbdb38', 'deepseek-v4.1-flash', 'opencode-go',
              200, 20, 0, 0, 0.0, 0.5, 1710000100.0, 1710000200.0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_model_usage VALUES
             ('20261021_000000_aaaaaa', 'glm-5.3-flash', '',
              300, 30, 0, 0, 0.0, 0.0, 1710000300.0, 1710000400.0)",
            [],
        )
        .unwrap();
        // A usage row without a model still proves its session did work.
        conn.execute(
            "INSERT INTO session_model_usage VALUES
             ('20261021_000000_aaaaaa', '', 'opencode-go',
              5, 5, 0, 0, 0.0, 0.0, 1710000300.0, 1710000400.0)",
            [],
        )
        .unwrap();
    }

    fn empty_catalog() -> PricingCatalog {
        PricingCatalog {
            models: Default::default(),
            cache_path: PathBuf::from("/tmp/models.json"),
            refresh_needed: false,
            availability: PricingAvailability::Empty,
            load_notice: None,
        }
    }

    #[test]
    fn hermes_usage_joins_totals_sessions_and_attribution() {
        let opencode = unique_temp_path("merge-open", ".db");
        let hermes = unique_temp_path("merge-hermes", ".db");
        opencode_fixture(&opencode);
        hermes_fixture(&hermes);

        let data = load_app_data(&InputOptions {
            database_path: Some(opencode.clone()),
            hermes_path: Some(hermes.clone()),
            pi_sessions_dir: None,
        })
        .unwrap();

        assert_eq!(data.events.len(), 3);
        assert_eq!(
            data.session_records.len(),
            3,
            "two Hermes sessions have usage"
        );
        assert!(data.import_stats.hermes_notice.is_none());

        let hermes_events: Vec<_> = data
            .events
            .iter()
            .filter(|event| event.session_id.starts_with("hermes:"))
            .collect();
        assert_eq!(hermes_events.len(), 2);

        let recorded = hermes_events
            .iter()
            .find(|event| event.model_id == "deepseek-v4.1-flash")
            .unwrap();
        assert_eq!(recorded.session_id, "hermes:20261020_224127_bbdb38");
        assert_eq!(
            recorded.parent_session_id.as_deref(),
            Some("hermes:parent_x")
        );
        assert_eq!(recorded.session_title.as_deref(), Some("Hermes work"));
        assert_eq!(
            recorded.project_path.as_deref(),
            Some(Path::new("C:/repo/hermes-project"))
        );
        assert_eq!(
            recorded.stored_cost_usd,
            Some(rust_decimal::Decimal::new(5, 1))
        );

        let estimated = hermes_events
            .iter()
            .find(|event| event.model_id == "glm-5.3-flash")
            .unwrap();
        assert_eq!(
            estimated.provider_id.as_deref(),
            Some("opencode-go"),
            "the session's provider wins when the row has none"
        );
        assert_eq!(estimated.stored_cost_usd, None, "unknown is not zero");
        assert_eq!(estimated.session_title.as_deref(), Some("Hermes 20261021"));

        let snapshot = crate::analytics::build_snapshot(
            &data,
            &empty_catalog(),
            TimeRange::All,
            ZeroCostBehavior::KeepZero,
        );
        assert_eq!(snapshot.overview.total_tokens, 660);
        assert_eq!(snapshot.overview.sessions, 3);
        assert_eq!(snapshot.overview.models_used, 2);
        assert_eq!(snapshot.overview.messages, 1, "Hermes adds no messages");
        assert_eq!(snapshot.overview.prompts, 0, "Hermes adds no prompts");
        assert_eq!(
            snapshot.overview.total_cost.known,
            rust_decimal::Decimal::new(75, 2)
        );

        let _ = fs::remove_file(opencode);
        let _ = fs::remove_file(hermes);
    }

    #[test]
    fn merged_events_stay_ordered_by_creation() {
        let opencode = unique_temp_path("order-open", ".db");
        let hermes = unique_temp_path("order-hermes", ".db");
        opencode_fixture(&opencode);
        hermes_fixture(&hermes);

        let data = load_app_data(&InputOptions {
            database_path: Some(opencode.clone()),
            hermes_path: Some(hermes.clone()),
            pi_sessions_dir: None,
        })
        .unwrap();

        let order: Vec<_> = data
            .events
            .iter()
            .map(|event| event.created_at.unwrap())
            .collect();
        assert!(order.windows(2).all(|pair| pair[0] <= pair[1]));

        let _ = fs::remove_file(opencode);
        let _ = fs::remove_file(hermes);
    }

    #[test]
    fn no_hermes_path_leaves_the_database_only_numbers() {
        let opencode = unique_temp_path("no-hermes", ".db");
        opencode_fixture(&opencode);

        let data = load_app_data(&InputOptions {
            database_path: Some(opencode.clone()),
            hermes_path: None,
            pi_sessions_dir: None,
        })
        .unwrap();

        assert_eq!(data.events.len(), 1);
        assert_eq!(data.session_records.len(), 1);
        assert!(data.import_stats.summary().is_none());

        let _ = fs::remove_file(opencode);
    }

    #[test]
    fn an_unusable_hermes_store_is_reported_and_changes_nothing() {
        let opencode = unique_temp_path("junk-open", ".db");
        let hermes = unique_temp_path("junk-hermes", ".db");
        opencode_fixture(&opencode);
        fs::write(&hermes, b"this is not sqlite").unwrap();

        let data = load_app_data(&InputOptions {
            database_path: Some(opencode.clone()),
            hermes_path: Some(hermes.clone()),
            pi_sessions_dir: None,
        })
        .unwrap();

        assert_eq!(data.events.len(), 1);
        assert_eq!(data.session_records.len(), 1);
        let summary = data.import_stats.summary().expect("a notice is surfaced");
        assert!(summary.contains("Hermes"), "{summary}");

        let _ = fs::remove_file(opencode);
        let _ = fs::remove_file(hermes);
    }

    fn pi_fixture(dir: &Path) {
        fs::write(
            dir.join("2026-09-29_uuid-1.jsonl"),
            concat!(
                r#"{"type":"session","version":3,"id":"uuid-1","timestamp":"2026-09-29T13:33:02.776Z","cwd":"C:/repo/pi-project"}"#,
                "\n",
                r#"{"type":"model_change","id":"m0","timestamp":"2026-09-29T13:33:03.000Z","provider":"opencode-go","modelId":"deepseek-v4.1-flash"}"#,
                "\n",
                r#"{"type":"message","timestamp":"2026-09-29T13:33:05.000Z","message":{"role":"user","content":[{"type":"text","text":"Fix the flaky test\nsecond line"}]}}"#,
                "\n",
                r#"{"type":"message","timestamp":"2026-09-29T13:38:45.992Z","message":{"role":"assistant","provider":"opencode-go","model":"deepseek-v4.1-flash","usage":{"input":100,"output":10,"cacheRead":5,"cacheWrite":0,"reasoning":2,"totalTokens":115,"cost":{"total":0.5}}}}"#,
            ),
        )
        .unwrap();

        fs::write(
            dir.join("2026-10-01_uuid-2.jsonl"),
            concat!(
                r#"{"type":"session","version":3,"id":"uuid-2","timestamp":"2026-10-01T08:00:00.000Z"}"#,
                "\n",
                r#"{"type":"message","timestamp":"2026-10-01T08:00:05.000Z","message":{"role":"assistant","provider":"opencode-go","model":"space-bunny-free","usage":{"input":50,"output":5,"cacheRead":0,"cacheWrite":0,"cost":{"total":0}}}}"#,
                "\n",
                // A file being appended to can end mid-line: the earlier call must survive.
                r#"{"type":"message","timestamp":"2026-10-01T08:00:06.000Z","message":{"role":"assistant""#,
            ),
        )
        .unwrap();

        fs::write(
            dir.join("2026-10-02_empty.jsonl"),
            r#"{"type":"session","version":3,"id":"uuid-3","timestamp":"2026-10-02T08:00:00.000Z"}"#,
        )
        .unwrap();
        fs::write(dir.join("junk.jsonl"), "this is not json").unwrap();
    }

    #[test]
    fn pi_usage_joins_totals_sessions_and_titles() {
        let opencode = unique_temp_path("pi-open", ".db");
        let sessions = unique_temp_dir("pi-sessions");
        opencode_fixture(&opencode);
        pi_fixture(&sessions);

        let data = load_app_data(&InputOptions {
            database_path: Some(opencode.clone()),
            hermes_path: None,
            pi_sessions_dir: Some(sessions.clone()),
        })
        .unwrap();

        assert_eq!(data.events.len(), 3, "one OpenCode call plus two Pi calls");
        assert_eq!(
            data.session_records.len(),
            3,
            "the usage-less Pi file is not a session"
        );

        let pi_events: Vec<_> = data
            .events
            .iter()
            .filter(|event| event.session_id.starts_with("pi:"))
            .collect();
        assert_eq!(pi_events.len(), 2);

        let titled = pi_events
            .iter()
            .find(|event| event.model_id == "deepseek-v4.1-flash")
            .unwrap();
        assert_eq!(titled.session_id, "pi:uuid-1");
        assert_eq!(titled.session_title.as_deref(), Some("Fix the flaky test"));
        assert_eq!(
            titled.project_path.as_deref(),
            Some(Path::new("C:/repo/pi-project"))
        );
        assert_eq!(titled.tokens.total(), 115);
        assert_eq!(
            titled.stored_cost_usd,
            Some(rust_decimal::Decimal::new(5, 1))
        );

        let free = pi_events
            .iter()
            .find(|event| event.model_id == "space-bunny-free")
            .unwrap();
        assert_eq!(free.session_title.as_deref(), Some("Pi 2026-10-01"));
        assert_eq!(free.stored_cost_usd, Some(rust_decimal::Decimal::ZERO));
        assert_eq!(free.project_path, None);

        let summary = data.import_stats.summary().expect("a notice is surfaced");
        assert!(summary.contains("Pi"), "{summary}");

        let snapshot = crate::analytics::build_snapshot(
            &data,
            &empty_catalog(),
            TimeRange::All,
            ZeroCostBehavior::KeepZero,
        );
        assert_eq!(snapshot.overview.total_tokens, 280);
        assert_eq!(snapshot.overview.sessions, 3);
        assert_eq!(snapshot.overview.messages, 1, "Pi adds no messages");
        assert_eq!(snapshot.overview.prompts, 0, "Pi adds no prompts");
        assert_eq!(
            snapshot.overview.total_cost.known,
            rust_decimal::Decimal::new(75, 2)
        );
        assert!(!snapshot.overview.total_cost.missing);

        let _ = fs::remove_file(opencode);
        let _ = fs::remove_dir_all(sessions);
    }

    #[test]
    fn pi_events_stay_ordered_across_sources() {
        let opencode = unique_temp_path("pi-order-open", ".db");
        let sessions = unique_temp_dir("pi-order-sessions");
        opencode_fixture(&opencode);
        pi_fixture(&sessions);

        let data = load_app_data(&InputOptions {
            database_path: Some(opencode.clone()),
            hermes_path: None,
            pi_sessions_dir: Some(sessions.clone()),
        })
        .unwrap();

        let order: Vec<_> = data
            .events
            .iter()
            .map(|event| event.created_at.unwrap())
            .collect();
        assert!(order.windows(2).all(|pair| pair[0] <= pair[1]));

        let _ = fs::remove_file(opencode);
        let _ = fs::remove_dir_all(sessions);
    }
}
