use std::io::BufRead;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::Value;

use crate::db::models::{SessionRecord, TokenUsage, UsageEvent};
use crate::utils::time::timestamp_ms_to_local;

/// Pi session ids are UUIDs; the prefix keeps them clear of OpenCode's `ses_` ids.
const SESSION_PREFIX: &str = "pi:";

pub struct PiLoad {
    pub events: Vec<UsageEvent>,
    pub sessions: Vec<SessionRecord>,
    /// Files that looked like sessions but had no usable header; reported, not fatal.
    pub skipped_files: usize,
}

/// Pi stores one JSONL file per session under the user's home directory. The directory
/// is a source only when it holds at least one session file.
pub fn discover() -> Option<PathBuf> {
    let dir = dirs::home_dir()?.join(".pi").join("agent").join("sessions");
    (!session_files(&dir).is_empty()).then_some(dir)
}

pub fn load(dir: &Path) -> PiLoad {
    let mut load = PiLoad {
        events: Vec::new(),
        sessions: Vec::new(),
        skipped_files: 0,
    };

    for path in session_files(dir) {
        match load_file(&path) {
            Some((events, session)) => {
                load.events.extend(events);
                if let Some(session) = session {
                    load.sessions.push(session);
                }
            }
            None => load.skipped_files = load.skipped_files.saturating_add(1),
        }
    }

    load
}

fn session_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];

    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "jsonl")
            {
                files.push(path);
            }
        }
    }

    files.sort();
    files
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum Line {
    #[serde(rename = "session")]
    Session(SessionHeader),
    #[serde(rename = "message")]
    Message(MessageLine),
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct SessionHeader {
    id: String,
    timestamp: Option<Value>,
    cwd: Option<String>,
}

#[derive(Deserialize)]
struct MessageLine {
    timestamp: Option<Value>,
    message: MessageBody,
}

#[derive(Deserialize)]
struct MessageBody {
    role: String,
    provider: Option<String>,
    model: Option<String>,
    usage: Option<Usage>,
    content: Option<Value>,
    timestamp: Option<i64>,
}

#[derive(Deserialize)]
struct Usage {
    input: Option<u64>,
    output: Option<u64>,
    #[serde(rename = "cacheRead", alias = "cache_read")]
    cache_read: Option<u64>,
    #[serde(rename = "cacheWrite", alias = "cache_write")]
    cache_write: Option<u64>,
    cost: Option<Cost>,
}

#[derive(Deserialize)]
struct Cost {
    total: Option<f64>,
}

/// `None` means the file has no usable header, so it is counted as skipped rather than
/// guessed at. A file with a valid header but no usage is a valid, empty session.
fn load_file(path: &Path) -> Option<(Vec<UsageEvent>, Option<SessionRecord>)> {
    let file = std::fs::File::open(path).ok()?;
    let mut header: Option<SessionHeader> = None;
    let mut title: Option<String> = None;
    let mut events = Vec::new();
    let mut last_moment: Option<DateTime<Local>> = None;

    for line in std::io::BufReader::new(file).lines() {
        // A file being appended to can end mid-line; that line is ignored, not fatal.
        let Ok(line) = line else { continue };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(parsed) = serde_json::from_str::<Line>(&line) else {
            continue;
        };

        match parsed {
            Line::Session(session) => header = Some(session),
            Line::Message(message) => {
                let moment = message
                    .timestamp
                    .as_ref()
                    .and_then(parse_moment)
                    .or_else(|| message.message.timestamp.and_then(timestamp_ms_to_local));
                if let Some(moment) = moment {
                    last_moment = Some(last_moment.map_or(moment, |last| last.max(moment)));
                }

                if message.message.role == "user" && title.is_none() {
                    title = user_text(message.message.content.as_ref());
                }
                if message.message.role != "assistant" {
                    continue;
                }

                let Some(header) = header.as_ref() else {
                    continue;
                };
                let Some(model_id) = normalize(message.message.model.clone()) else {
                    continue;
                };
                let Some(usage) = message.message.usage.as_ref() else {
                    continue;
                };

                let tokens = TokenUsage {
                    input: usage.input.unwrap_or(0),
                    output: usage.output.unwrap_or(0),
                    cache_read: usage.cache_read.unwrap_or(0),
                    cache_write: usage.cache_write.unwrap_or(0),
                };
                if tokens.total() == 0 {
                    continue;
                }

                let recorded = usage.cost.as_ref().and_then(|cost| cost.total);
                let session_title = title.clone().or_else(|| {
                    header
                        .timestamp
                        .as_ref()
                        .and_then(parse_moment)
                        .map(|start| format!("Pi {}", start.format("%Y-%m-%d")))
                });

                events.push(UsageEvent {
                    session_id: format!("{SESSION_PREFIX}{}", header.id),
                    parent_session_id: None,
                    session_title,
                    session_started_at: header.timestamp.as_ref().and_then(parse_moment),
                    session_archived_at: None,
                    project_name: None,
                    project_path: normalize(header.cwd.clone()).map(PathBuf::from),
                    provider_id: normalize(message.message.provider.clone()),
                    model_id: model_id.clone(),
                    agent: None,
                    finish_reason: None,
                    tokens,
                    created_at: moment,
                    completed_at: None,
                    stored_cost_usd: recorded.and_then(decimal_cost),
                });
            }
            Line::Other => {}
        }
    }

    let header = header?;
    let created_at = header.timestamp.as_ref().and_then(parse_moment)?;
    let session = (!events.is_empty()).then(|| SessionRecord {
        session_id: format!("{SESSION_PREFIX}{}", header.id),
        created_at,
        updated_at: last_moment.unwrap_or(created_at),
    });

    Some((events, session))
}

fn parse_moment(value: &Value) -> Option<DateTime<Local>> {
    match value {
        Value::String(text) => DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|moment| moment.with_timezone(&Local)),
        Value::Number(number) => number.as_i64().and_then(timestamp_ms_to_local),
        _ => None,
    }
}

fn normalize(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn user_text(content: Option<&Value>) -> Option<String> {
    let first = match content? {
        Value::String(text) => text.as_str(),
        Value::Array(blocks) => blocks
            .iter()
            .find_map(|block| block.get("text").and_then(Value::as_str))?,
        _ => return None,
    };
    let line = first.lines().map(str::trim).find(|line| !line.is_empty())?;
    let shortened = line.chars().take(60).collect::<String>();
    (!shortened.is_empty()).then_some(shortened)
}

/// Pi always writes a cost object for an assistant call, so its value is authoritative
/// even when it is zero: a free call really is free, and re-pricing it would disagree
/// with Pi's own accounting.
fn decimal_cost(recorded: f64) -> Option<Decimal> {
    if !recorded.is_finite() || recorded < 0.0 {
        return None;
    }
    format!("{recorded:.12}").parse::<Decimal>().ok()
}

#[cfg(test)]
mod tests {
    use super::decimal_cost;
    use rust_decimal::Decimal;

    #[test]
    fn cost_trusts_the_recorded_value_including_zero() {
        assert_eq!(decimal_cost(0.25), Some(Decimal::new(25, 2)));
        assert_eq!(decimal_cost(0.0), Some(Decimal::ZERO));
        assert_eq!(decimal_cost(f64::NAN), None);
        assert_eq!(decimal_cost(-1.0), None);
    }
}
