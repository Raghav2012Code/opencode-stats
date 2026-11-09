use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use rust_decimal::Decimal;

use crate::db::connection::{self, open_database};
use crate::db::errors::{Error, Result};
use crate::db::models::{SessionRecord, TokenUsage, UsageEvent};
use crate::utils::time::timestamp_secs_to_local;

/// Session ids from another tool must never collide with OpenCode's own `ses_` ids.
const SESSION_PREFIX: &str = "hermes:";

pub struct HermesLoad {
    pub events: Vec<UsageEvent>,
    pub sessions: Vec<SessionRecord>,
}

/// The first Hermes store that looks like one. A missing or foreign file is simply not
/// a source, so detection is silent; only a store that exists but fails to load is
/// reported, by the caller, through the import summary.
pub fn discover() -> Option<PathBuf> {
    candidates().into_iter().find(|path| usable(path))
}

fn candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        paths.push(
            PathBuf::from(local_app_data)
                .join("hermes")
                .join("state.db"),
        );
    }
    if let Some(home) = dirs::home_dir() {
        paths.push(
            home.join(".local")
                .join("share")
                .join("hermes")
                .join("state.db"),
        );
        paths.push(home.join(".hermes").join("state.db"));
    }

    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

/// Hermes has no stable public schema, so a store counts only when the two tables this
/// loader reads are both there. A rename disables the source instead of producing zeros.
fn usable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let Ok(conn) = connection::open_database(path) else {
        return false;
    };
    connection::table_exists(&conn, "sessions").unwrap_or(false)
        && connection::table_exists(&conn, "session_model_usage").unwrap_or(false)
}

const ROWS_SQL: &str = "
    SELECT s.id, s.parent_session_id, s.title, s.cwd, s.git_repo_root,
           s.started_at, s.ended_at, s.last_activity_at,
           COALESCE(NULLIF(u.billing_provider, ''), NULLIF(s.billing_provider, '')) AS provider,
           u.model, u.input_tokens, u.output_tokens,
           u.cache_read_tokens, u.cache_write_tokens,
           u.estimated_cost_usd, u.actual_cost_usd,
           u.first_seen, u.last_seen
    FROM session_model_usage u
    JOIN sessions s ON s.id = u.session_id
";

pub fn load(path: &Path) -> Result<HermesLoad> {
    let conn = open_database(path)?;
    let mut stmt = conn.prepare(ROWS_SQL).map_err(Error::database_query)?;
    let mut rows = stmt.query([]).map_err(Error::database_query)?;

    let mut events = Vec::new();
    let mut sessions: BTreeMap<String, SessionRecord> = BTreeMap::new();

    while let Some(row) = rows.next().map_err(Error::database_query)? {
        let raw_id: String = field(row, "id")?;
        let session_id = format!("{SESSION_PREFIX}{raw_id}");
        let parent_session_id = field::<Option<String>>(row, "parent_session_id")?
            .map(|parent| format!("{SESSION_PREFIX}{parent}"));
        let title = field::<Option<String>>(row, "title")?;
        let cwd = field::<Option<String>>(row, "cwd")?;
        let git_root = field::<Option<String>>(row, "git_repo_root")?;
        let started_at = field::<Option<f64>>(row, "started_at")?;
        let ended_at = field::<Option<f64>>(row, "ended_at")?;
        let last_activity_at = field::<Option<f64>>(row, "last_activity_at")?;
        let provider = field::<Option<String>>(row, "provider")?;
        let model = field::<Option<String>>(row, "model")?;
        let input = field::<Option<i64>>(row, "input_tokens")?;
        let output = field::<Option<i64>>(row, "output_tokens")?;
        let cache_read = field::<Option<i64>>(row, "cache_read_tokens")?;
        let cache_write = field::<Option<i64>>(row, "cache_write_tokens")?;
        let estimated_cost = field::<Option<f64>>(row, "estimated_cost_usd")?;
        let actual_cost = field::<Option<f64>>(row, "actual_cost_usd")?;
        let first_seen = field::<Option<f64>>(row, "first_seen")?;
        let last_seen = field::<Option<f64>>(row, "last_seen")?;

        // A session counts once it has any usage row, even one whose model is unusable:
        // it did work, and the Sessions page should say so.
        if let Some(created_at) = started_at.and_then(timestamp_secs_to_local) {
            let updated_at = ended_at
                .and_then(timestamp_secs_to_local)
                .or_else(|| last_activity_at.and_then(timestamp_secs_to_local))
                .or_else(|| last_seen.and_then(timestamp_secs_to_local))
                .unwrap_or(created_at);
            sessions
                .entry(session_id.clone())
                .and_modify(|record| {
                    if updated_at > record.updated_at {
                        record.updated_at = updated_at;
                    }
                })
                .or_insert(SessionRecord {
                    session_id: session_id.clone(),
                    created_at,
                    updated_at,
                });
        }

        let Some(model_id) = normalize(model) else {
            continue;
        };

        let tokens = TokenUsage {
            input: non_negative(input),
            output: non_negative(output),
            cache_read: non_negative(cache_read),
            cache_write: non_negative(cache_write),
        };
        if tokens.total() == 0 {
            continue;
        }

        let session_title =
            normalize(title).or_else(|| Some(format!("Hermes {}", short_id(&raw_id))));
        let project_path = normalize(cwd)
            .or_else(|| normalize(git_root))
            .map(PathBuf::from);
        let stored_cost_usd = stored_cost(
            actual_cost.unwrap_or(0.0),
            estimated_cost.unwrap_or(0.0),
            &model_id,
        );

        events.push(UsageEvent {
            session_id: session_id.clone(),
            parent_session_id,
            session_title,
            session_started_at: started_at.and_then(timestamp_secs_to_local),
            session_archived_at: None,
            project_name: None,
            project_path,
            provider_id: normalize(provider),
            model_id,
            agent: None,
            finish_reason: None,
            tokens,
            created_at: first_seen.or(started_at).and_then(timestamp_secs_to_local),
            completed_at: last_seen.or(ended_at).and_then(timestamp_secs_to_local),
            stored_cost_usd,
        });
    }

    Ok(HermesLoad {
        events,
        sessions: sessions.into_values().collect(),
    })
}

fn field<T: rusqlite::types::FromSql>(row: &rusqlite::Row<'_>, name: &str) -> Result<T> {
    row.get(name).map_err(Error::database_query)
}

fn normalize(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn non_negative(value: Option<i64>) -> u64 {
    value
        .and_then(|value| u64::try_from(value.max(0)).ok())
        .unwrap_or(0)
}

/// Hermes stores `0.0` alongside an unknown cost status; that zero is a placeholder,
/// not a free call, so only a positive value is trusted as recorded cost. A model that
/// is named as a free tier is the exception: its price really is zero.
fn stored_cost(actual: f64, estimated: f64, model_id: &str) -> Option<Decimal> {
    let value = if actual > 0.0 {
        actual
    } else if estimated > 0.0 {
        estimated
    } else if model_id.ends_with("-free") {
        return Some(Decimal::ZERO);
    } else {
        return None;
    };
    format!("{value:.6}").parse::<Decimal>().ok()
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::stored_cost;
    use rust_decimal::Decimal;

    #[test]
    fn cost_prefers_actual_and_ignores_placeholders() {
        assert_eq!(
            stored_cost(0.25, 0.5, "deepseek-v4.1-flash"),
            Some(Decimal::new(25, 2))
        );
        assert_eq!(
            stored_cost(0.0, 0.5, "deepseek-v4.1-flash"),
            Some(Decimal::new(5, 1))
        );
        assert_eq!(stored_cost(0.0, 0.0, "glm-5.3-flash"), None);
        assert_eq!(
            stored_cost(0.0, 0.0, "muse-spark-1.2-contributor-free"),
            Some(Decimal::ZERO)
        );
    }

    #[test]
    fn a_nan_cost_is_not_a_cost() {
        assert_eq!(stored_cost(f64::NAN, 0.0, "glm-5.3-flash"), None);
    }
}
