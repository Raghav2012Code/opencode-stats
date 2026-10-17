use std::env;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::db::errors::{Error, Result};

/// Where the user could have meant when no path is named. With a named path this is that
/// path alone: the standard locations are not consulted, so the error report cannot imply
/// that a working database down the list was rejected.
pub fn database_candidates(custom_path: Option<&Path>) -> Vec<PathBuf> {
    match named_database_path(
        custom_path,
        env::var("OC_STATS_DB_PATH").ok().map(PathBuf::from),
    ) {
        Some(named) => vec![named],
        None => default_locations(),
    }
}

/// A path the user named: the flag first, the environment variable second. Naming a path
/// is an instruction, not a hint.
fn named_database_path(flag: Option<&Path>, env_value: Option<PathBuf>) -> Option<PathBuf> {
    flag.map(Path::to_path_buf).or(env_value)
}

fn default_locations() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(home) = dirs::home_dir() {
        candidates.push(
            home.join(".local")
                .join("share")
                .join("opencode")
                .join("opencode.db"),
        );
    }

    if cfg!(target_os = "windows") {
        if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
            candidates.push(
                PathBuf::from(local_app_data)
                    .join("opencode")
                    .join("opencode.db"),
            );
        }
        if let Ok(appdata) = env::var("APPDATA") {
            candidates.push(PathBuf::from(appdata).join("opencode").join("opencode.db"));
        }
    } else if cfg!(target_os = "macos") {
        if let Some(home) = dirs::home_dir() {
            candidates.push(
                home.join("Library")
                    .join("Application Support")
                    .join("opencode")
                    .join("opencode.db"),
            );
        }
    } else if let Some(data_dir) = dirs::data_local_dir() {
        candidates.push(data_dir.join("opencode").join("opencode.db"));
    }

    dedupe_preserve_order(candidates)
}

/// A named path is used or reported missing; it never falls back. Silently reporting a
/// different database's numbers looks like success and is worse than reporting none.
pub fn discover_database_path(custom_path: Option<&Path>) -> Option<PathBuf> {
    resolve_database_path(named_database_path(
        custom_path,
        env::var("OC_STATS_DB_PATH").ok().map(PathBuf::from),
    ))
}

fn resolve_database_path(named: Option<PathBuf>) -> Option<PathBuf> {
    match named {
        Some(path) => usable_database(path),
        None => default_locations().into_iter().find_map(usable_database),
    }
}

fn usable_database(path: PathBuf) -> Option<PathBuf> {
    (path.exists() && database_has_expected_tables(&path).unwrap_or(false)).then_some(path)
}

pub fn open_database(path: &Path) -> Result<Connection> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| Error::database_open(path, e))
}

pub fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [table],
        |row| row.get::<_, i64>(0),
    )
    .map(|exists| exists == 1)
    .map_err(Error::database_query)
}

pub fn database_has_expected_tables(path: &Path) -> Result<bool> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| Error::database_open(path, e))?;

    if !table_exists(&conn, "project")? {
        return Ok(false);
    }

    // Either the pre-migration tables or the modern ones must be present.
    let legacy = table_exists(&conn, "session")? && table_exists(&conn, "message")?;
    let modern = table_exists(&conn, "session_v2")? && table_exists(&conn, "session_message")?;

    Ok(legacy || modern)
}

fn dedupe_preserve_order(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    paths
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        database_candidates, database_has_expected_tables, named_database_path,
        resolve_database_path,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use rusqlite::Connection;

    fn temp_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("oc-stats-{label}-{nonce}.db"))
    }

    /// A synthetic database the discovery check accepts, so tests need no real OpenCode data.
    fn synthetic_database(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE project (id TEXT PRIMARY KEY);
             CREATE TABLE session (id TEXT PRIMARY KEY);
             CREATE TABLE message (id TEXT PRIMARY KEY);",
        )
        .unwrap();
    }

    #[test]
    fn named_path_is_the_only_candidate() {
        let named = Path::new("named.db");
        assert_eq!(database_candidates(Some(named)), vec![named.to_path_buf()]);
    }

    #[test]
    fn unnamed_discovery_still_offers_the_standard_locations() {
        let candidates = database_candidates(None);
        assert!(!candidates.is_empty(), "expected standard locations");
    }

    #[test]
    fn flag_beats_environment_variable() {
        let flag = PathBuf::from("from-flag.db");
        let env = PathBuf::from("from-env.db");

        assert_eq!(
            named_database_path(Some(&flag), Some(env.clone())),
            Some(flag)
        );
        assert_eq!(named_database_path(None, Some(env.clone())), Some(env));
        assert_eq!(named_database_path(None, None), None);
    }

    #[test]
    fn a_missing_named_path_is_never_replaced_by_a_default_database() {
        // This machine has a real OpenCode database in the standard locations; a named
        // path that does not exist must still be an error rather than a silent fallback.
        let missing = temp_path("missing");

        assert_eq!(resolve_database_path(Some(missing)), None);
    }

    #[test]
    fn a_named_file_that_is_not_a_database_is_rejected() {
        let junk = temp_path("junk");
        fs::write(&junk, b"this is not sqlite").unwrap();

        assert_eq!(resolve_database_path(Some(junk.clone())), None);
        let _ = fs::remove_file(junk);
    }

    #[test]
    fn a_named_valid_database_is_used() {
        let db_path = temp_path("named-valid");
        synthetic_database(&db_path);

        assert_eq!(
            resolve_database_path(Some(db_path.clone())),
            Some(db_path.clone())
        );
        let _ = fs::remove_file(db_path);
    }

    #[test]
    fn rejects_sqlite_without_expected_schema() {
        let db_path = temp_path("schema");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute("CREATE TABLE only_one(id INTEGER)", [])
            .unwrap();
        drop(conn);

        assert!(!database_has_expected_tables(&db_path).unwrap());
        let _ = fs::remove_file(db_path);
    }
}
