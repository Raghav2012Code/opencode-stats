use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};

use crate::db::connection::{self, discover_database_path, open_database};
use crate::db::errors::{Error, Result};
use crate::db::models::{
    AppData, ImportStats, InputOptions, JsonMessageRecord, MessageRecord, SessionRecord,
    TokenUsage, UsageEvent,
};
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
    load_from_sqlite(&db_path)
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

fn load_messages_sqlite(
    conn: &rusqlite::Connection,
    sessions: &BTreeMap<&str, &SessionRow>,
    schema: Schema,
) -> Result<SqliteMessageLoad> {
    let mut stmt = conn
        .prepare(messages_sql(schema))
        .map_err(Error::database_query)?;

    let mut messages = Vec::new();
    let mut skipped_messages = 0usize;
    let mut rows = stmt.query([]).map_err(Error::database_query)?;
    while let Some(row) = rows.next().map_err(Error::database_query)? {
        let session_id: String = row.get("session_id").map_err(Error::database_query)?;
        let Some(session) = sessions.get(session_id.as_str()) else {
            skipped_messages = skipped_messages.saturating_add(1);
            continue;
        };
        let role_hint: Option<String> = row.get("role").map_err(Error::database_query)?;

        let data = match row.get_ref("data").map_err(Error::database_query)? {
            rusqlite::types::ValueRef::Text(bytes) => bytes,
            _ => {
                skipped_messages = skipped_messages.saturating_add(1);
                continue;
            }
        };

        let mut record: JsonMessageRecord = match serde_json::from_slice(data) {
            Ok(record) => record,
            Err(_) => {
                skipped_messages = skipped_messages.saturating_add(1);
                continue;
            }
        };

        // Modern messages keep the role in the `type` column, not in the payload.
        if record.role.is_none() {
            record.role = role_hint;
        }

        if let Some(message) = parse_json_record(record, session) {
            messages.push(message);
        }
    }
    Ok(SqliteMessageLoad {
        messages,
        skipped_messages,
    })
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
    use super::{load_from_sqlite, parse_json_record};
    use crate::db::models::{
        JsonCacheTokensRecord, JsonMessageRecord, JsonPathRecord, JsonTimeRecord, JsonTokensRecord,
    };
    use rusqlite::Connection;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

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
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let db_path = std::env::temp_dir().join(format!("oc-stats-modern-only-{nonce}.db"));
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
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let db_path = std::env::temp_dir().join(format!("oc-stats-v2-test-{nonce}.db"));
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
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let db_path = std::env::temp_dir().join(format!("oc-stats-load-test-{nonce}.db"));
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
}
