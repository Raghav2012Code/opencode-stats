//! The extracted rows of a database, kept beside the pricing cache.
//!
//! Reading the payload column is what startup spends its time on: hundreds of megabytes of
//! message JSON for a large history, read on every launch and every `R`. When nothing has
//! been written since the last read, the parsed result is the same, so the second launch
//! reads a few megabytes instead of scanning the database again.
//!
//! The file is one header line and then one row per line, so the rows can be split at line
//! boundaries and parsed on worker threads. That matters more than the size: deserializing a
//! row spends most of its time turning timestamps into local datetimes, and doing that on a
//! single thread costs as much as the whole parallel database read it replaces.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::db::models::{AppData, ImportStats, MessageRecord, SessionRecord, UsageEvent};
use crate::db::queries::parser_threads;

/// Bump when `AppData`'s shape changes: an older file then misses instead of failing.
const FORMAT_VERSION: u32 = 1;

const CACHE_FILE: &str = "usage-cache.jsonl";

/// What the rows were read from. The database is live and in WAL mode, so a write moves the
/// length or mtime of the sidecar long before it touches the main file: both are the key.
#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Signature {
    path: String,
    main_len: u64,
    main_modified_ms: u64,
    wal_len: u64,
    wal_modified_ms: u64,
}

/// The first line: what the rest of the file was read from, and how much of it to expect.
#[derive(Debug, Deserialize, Serialize)]
struct Header {
    version: u32,
    signature: Signature,
    events: usize,
    messages: usize,
    sessions: usize,
    import_stats: ImportStats,
}

/// One row of the body. The tag keeps the three kinds apart while a chunk is parsed on its
/// own; the order within each kind is the order they were written in.
#[derive(Debug, Deserialize)]
enum Row {
    #[serde(rename = "e")]
    Event(UsageEvent),
    #[serde(rename = "m")]
    Message(MessageRecord),
    #[serde(rename = "s")]
    Session(SessionRecord),
}

#[derive(Serialize)]
enum RowRef<'a> {
    #[serde(rename = "e")]
    Event(&'a UsageEvent),
    #[serde(rename = "m")]
    Message(&'a MessageRecord),
    #[serde(rename = "s")]
    Session(&'a SessionRecord),
}

#[derive(Default)]
struct Rows {
    events: Vec<UsageEvent>,
    messages: Vec<MessageRecord>,
    sessions: Vec<SessionRecord>,
}

/// The cached rows for this database, or `None` when the file is missing, stale, unreadable,
/// or written by another format. The cache is an accelerator: every failure is a miss.
pub fn load(database: &Path) -> Option<AppData> {
    load_at(&default_cache_path()?, database)
}

/// Replaces the cached rows. A failure loses nothing but the shortcut.
pub fn store(database: &Path, data: &AppData) {
    if let Some(path) = default_cache_path() {
        store_at(&path, database, data);
    }
}

fn load_at(cache_path: &Path, database: &Path) -> Option<AppData> {
    let signature = signature(database)?;
    let bytes = fs::read(cache_path).ok()?;
    let newline = bytes.iter().position(|byte| *byte == b'\n')?;
    let header: Header = serde_json::from_slice(&bytes[..newline]).ok()?;
    if header.version != FORMAT_VERSION || header.signature != signature {
        return None;
    }

    let rows = parse_rows(&bytes[newline + 1..])?;
    // A body that does not hold what its header promised was truncated or replaced.
    if rows.events.len() != header.events
        || rows.messages.len() != header.messages
        || rows.sessions.len() != header.sessions
    {
        return None;
    }

    Some(AppData {
        events: rows.events,
        messages: rows.messages,
        session_records: rows.sessions,
        import_stats: header.import_stats,
    })
}

fn store_at(cache_path: &Path, database: &Path, data: &AppData) {
    let Some(signature) = signature(database) else {
        return;
    };
    let Some(parent) = cache_path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    // A per-process temp name, so two oc-stats never write into the same file and the
    // rename always publishes one whole copy.
    let mut temp = cache_path.to_path_buf();
    temp.set_extension(format!("{}.tmp", std::process::id()));
    let Ok(file) = fs::File::create(&temp) else {
        return;
    };
    let mut writer = std::io::BufWriter::new(file);

    let header = Header {
        version: FORMAT_VERSION,
        signature,
        events: data.events.len(),
        messages: data.messages.len(),
        sessions: data.session_records.len(),
        import_stats: data.import_stats.clone(),
    };
    let wrote = write_line(&mut writer, &header)
        && data
            .events
            .iter()
            .all(|row| write_line(&mut writer, &RowRef::Event(row)))
        && data
            .messages
            .iter()
            .all(|row| write_line(&mut writer, &RowRef::Message(row)))
        && data
            .session_records
            .iter()
            .all(|row| write_line(&mut writer, &RowRef::Session(row)));
    if !wrote || writer.flush().is_err() {
        drop(writer);
        let _ = fs::remove_file(&temp);
        return;
    }
    drop(writer);
    let _ = fs::rename(&temp, cache_path);
}

fn write_line(writer: &mut impl Write, row: &impl Serialize) -> bool {
    serde_json::to_writer(&mut *writer, row).is_ok() && writer.write_all(b"\n").is_ok()
}

/// Parses the body on worker threads, one line each, keeping the written order.
fn parse_rows(body: &[u8]) -> Option<Rows> {
    if body.iter().all(|byte| byte.is_ascii_whitespace()) {
        return Some(Rows::default());
    }
    let chunks = split_at_lines(body, parser_threads());
    let parts = std::thread::scope(|scope| {
        let handles = chunks
            .into_iter()
            .map(|chunk| scope.spawn(move || parse_chunk(chunk)))
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| handle.join().ok().flatten())
            .collect::<Option<Vec<_>>>()
    })?;

    let mut rows = Rows::default();
    for part in parts {
        rows.events.extend(part.events);
        rows.messages.extend(part.messages);
        rows.sessions.extend(part.sessions);
    }
    Some(rows)
}

fn parse_chunk(chunk: &[u8]) -> Option<Rows> {
    let mut rows = Rows::default();
    for line in chunk.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        match serde_json::from_slice(line).ok()? {
            Row::Event(event) => rows.events.push(event),
            Row::Message(message) => rows.messages.push(message),
            Row::Session(session) => rows.sessions.push(session),
        }
    }
    Some(rows)
}

/// Splits the body at newlines into at most `parts` contiguous pieces, so no line is cut.
fn split_at_lines(body: &[u8], parts: usize) -> Vec<&[u8]> {
    let parts = parts.max(1).min(body.len().max(1));
    let mut bounds = vec![0];
    for part in 1..parts {
        let at = body.len() * part / parts;
        if let Some(offset) = body[at..].iter().position(|byte| *byte == b'\n') {
            let start = at + offset + 1;
            if start > *bounds.last().expect("bounds starts with one entry") && start < body.len() {
                bounds.push(start);
            }
        }
    }
    bounds.push(body.len());
    bounds
        .windows(2)
        .map(|pair| &body[pair[0]..pair[1]])
        .collect()
}

fn default_cache_path() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("oc-stats").join(CACHE_FILE))
}

fn signature(database: &Path) -> Option<Signature> {
    let (main_len, main_modified_ms) = stamp(database);
    let (wal_len, wal_modified_ms) = stamp(&wal_path(database));
    Some(Signature {
        path: database.display().to_string(),
        main_len,
        main_modified_ms,
        wal_len,
        wal_modified_ms,
    })
}

/// SQLite's write-ahead log beside the database: `<file>-wal`.
fn wal_path(database: &Path) -> PathBuf {
    let mut name = database.as_os_str().to_owned();
    name.push("-wal");
    PathBuf::from(name)
}

/// `(len, mtime in ms)`, both zero when the file is absent: an added or removed log is a
/// change like any other.
fn stamp(path: &Path) -> (u64, u64) {
    let Ok(metadata) = fs::metadata(path) else {
        return (0, 0);
    };
    let modified_ms = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0);
    (metadata.len(), modified_ms)
}

#[cfg(test)]
mod tests {
    use super::{load_at, store_at};
    use crate::db::models::{
        AppData, ImportStats, MessageRecord, SessionRecord, TokenUsage, UsageEvent,
    };
    use crate::test_support::unique_temp_dir;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::SystemTime;

    fn sample_data() -> AppData {
        let when = chrono::Local::now();
        let mut events = Vec::new();
        let mut messages = Vec::new();
        for index in 0..40 {
            events.push(UsageEvent {
                session_id: format!("ses_{index}"),
                model_id: "gpt-5".into(),
                provider_id: Some("openai".into()),
                tokens: TokenUsage {
                    input: 100 + index,
                    output: 20,
                    cache_read: 3,
                    cache_write: 4,
                },
                created_at: Some(when),
                ..UsageEvent::default()
            });
            messages.push(MessageRecord {
                session_id: format!("ses_{index}"),
                role: Some("assistant".into()),
                created_at: Some(when),
                ..MessageRecord::default()
            });
        }
        AppData {
            events,
            messages,
            session_records: vec![SessionRecord {
                session_id: "ses_0".into(),
                created_at: when,
                updated_at: when,
            }],
            import_stats: ImportStats {
                skipped_sqlite_messages: 2,
                ..ImportStats::default()
            },
        }
    }

    /// A database file that is never opened: only its metadata is part of the key.
    fn database_file(dir: &Path) -> PathBuf {
        let path = dir.join("opencode.db");
        fs::write(&path, b"not really a database").unwrap();
        path
    }

    #[test]
    fn an_unchanged_database_is_a_hit_that_keeps_every_row() {
        let dir = unique_temp_dir("usage-cache-hit");
        let database = database_file(&dir);
        let cache = dir.join("usage-cache.jsonl");
        let data = sample_data();

        store_at(&cache, &database, &data);
        let loaded = load_at(&cache, &database).expect("an unchanged database is a hit");

        assert_eq!(loaded.events.len(), 40);
        assert_eq!(loaded.messages.len(), 40);
        assert_eq!(loaded.session_records.len(), 1);
        assert_eq!(loaded.import_stats.skipped_sqlite_messages, 2);
        // The rows keep the order they were written in.
        assert_eq!(loaded.events[0].tokens.input, 100);
        assert_eq!(loaded.events[39].tokens.input, 139);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_database_written_since_the_read_is_a_miss() {
        let dir = unique_temp_dir("usage-cache-stale");
        let database = database_file(&dir);
        let cache = dir.join("usage-cache.jsonl");

        store_at(&cache, &database, &sample_data());
        // A later write, even with the same length, must invalidate: the WAL moves too.
        fs::File::options()
            .write(true)
            .open(&database)
            .unwrap()
            .set_modified(SystemTime::now() + std::time::Duration::from_secs(1))
            .unwrap();

        assert!(
            load_at(&cache, &database).is_none(),
            "a database that changed must be read again"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_corrupt_truncated_or_absent_cache_is_a_miss() {
        let dir = unique_temp_dir("usage-cache-junk");
        let database = database_file(&dir);
        let cache = dir.join("usage-cache.jsonl");

        assert!(load_at(&cache, &database).is_none(), "absent is a miss");

        fs::write(&cache, b"not json").unwrap();
        assert!(load_at(&cache, &database).is_none(), "junk is a miss");

        store_at(&cache, &database, &sample_data());
        let whole = fs::read(&cache).unwrap();
        fs::write(&cache, &whole[..whole.len() / 2]).unwrap();
        assert!(
            load_at(&cache, &database).is_none(),
            "a body shorter than its header promises is a miss"
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_database_with_no_sidecar_stamps_the_wal_as_absent() {
        let dir = unique_temp_dir("usage-cache-wal");
        let database = database_file(&dir);
        let cache = dir.join("usage-cache.jsonl");

        store_at(&cache, &database, &sample_data());
        assert!(load_at(&cache, &database).is_some());

        // The log appearing is a change in the source, so the key no longer matches.
        fs::write(dir.join("opencode.db-wal"), b"wal").unwrap();
        assert!(load_at(&cache, &database).is_none());

        let _ = fs::remove_dir_all(dir);
    }
}
