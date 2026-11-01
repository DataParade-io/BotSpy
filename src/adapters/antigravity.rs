//! The Antigravity importer.
//!
//! Watches `conversation_summaries.db` for new and updated conversations
//! (the current `conversation_summaries` table, or the older
//! `conversations(id, name, updated_at)` one: discovery picks whichever
//! table exists), prefers each conversation's readable
//! `brain/<id>/.system_generated/logs/transcript.jsonl`, and falls back to
//! the per-conversation SQLite DB (protobuf payload blobs) for linkage the
//! JSONL lacks — e.g. tool-call ids, which live in the DB's `tool_links`
//! table keyed by `step_index`. Per-conversation DBs are hot WAL
//! databases: they are read through a snapshot copy, never in place,
//! never written. Transcript records carry `USER_INPUT`,
//! `PLANNER_RESPONSE`, `CHECKPOINT`, `ERROR_MESSAGE`, `SYSTEM_MESSAGE`,
//! `EPHEMERAL_MESSAGE`, `GENERIC`, the legacy `TOOL_CALL`, `step_index`
//! (the authoritative order), and `created_at`. A conversation with a
//! `parent_conversation_id` is a sub-agent spawned by that parent.
//! `brain/<id>/payload.db` is optional.

use crate::importer::SkipCounter;
use crate::schema::{
    Agent, KnownPart, Message, Part, Provenance, Role, SessionMetadata, SubagentInfo, ToolArguments,
};
use crate::session::SessionSummary;
use crate::snapshot::SnapshotConnection;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// What discovery saw in the summaries index.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AntigravityDiscovery {
    pub conversations: usize,
}

/// Diagnostics over an Antigravity data root.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct AntigravityReport {
    pub root: PathBuf,
    pub conversations: usize,
    pub transcripts: usize,
    pub issues: Vec<String>,
}

/// The result of one extraction pass over a conversation.
#[derive(Debug, Clone, PartialEq)]
pub struct AntigravityExtraction {
    pub session: crate::session::Session,
    /// Messages normalized by this pass.
    pub good: usize,
    /// Steps skipped as malformed or of unknown type.
    pub skipped: SkipCounter,
}

/// One row of the summaries index, whichever schema it came from.
#[derive(Debug, Clone, Default)]
struct ConversationRow {
    id: String,
    project: String,
    title: Option<String>,
    last_activity: String,
    parent_id: Option<String>,
    nesting_depth: Option<u64>,
    agent_name: Option<String>,
}

/// `2026-10-01 09:00:00.5+00:00` as `2026-10-01T09:00:00.5+00:00`: the
/// RFC 3339 shape, so activity strings compare and parse like every
/// other source's. Anything that is not date-then-space is left alone.
fn rfc3339_shape(value: &str) -> String {
    match value.as_bytes() {
        [_, _, _, _, b'-', _, _, b'-', _, _, b' ', ..] => {
            format!("{}T{}", &value[..10], &value[11..])
        }
        _ => value.to_string(),
    }
}

/// A tool-call linkage row from the payload DB.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolLink {
    call_id: String,
    result: String,
}

/// The Antigravity source over one data root.
#[derive(Debug)]
pub struct AntigravitySource {
    root: PathBuf,
}

impl AntigravitySource {
    /// A source over the Antigravity data root (the directory holding
    /// `conversation_summaries.db` and the `brain/` tree).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The data root this source watches.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn snapshot_connection(
        &self,
        db: &Path,
    ) -> Result<SnapshotConnection, crate::snapshot::SnapshotError> {
        SnapshotConnection::open(db)
    }

    /// The summaries index, from whichever schema the database has: the
    /// current `conversation_summaries` table, else the older
    /// `conversations` one.
    fn summaries(&self) -> Vec<ConversationRow> {
        let db = self.root.join("conversation_summaries.db");
        let Ok(kv) = self.snapshot_connection(&db) else {
            return Vec::new();
        };
        let has_table = |name: &str| {
            kv.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [name],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count > 0)
            .unwrap_or(false)
        };
        if has_table("conversation_summaries") {
            let Ok(mut stmt) = kv.prepare(
                "SELECT conversation_id, project_id, title, last_modified_time, \
                 parent_conversation_id, nesting_depth, agent_name \
                 FROM conversation_summaries ORDER BY conversation_id",
            ) else {
                return Vec::new();
            };
            let rows = stmt.query_map([], |row| {
                let text = |index: usize| -> rusqlite::Result<Option<String>> {
                    Ok(row
                        .get::<_, Option<String>>(index)?
                        .filter(|v| !v.is_empty()))
                };
                Ok(ConversationRow {
                    id: row.get::<_, String>(0)?,
                    project: text(1)?.unwrap_or_default(),
                    title: text(2)?,
                    last_activity: text(3)?.map(|v| rfc3339_shape(&v)).unwrap_or_default(),
                    parent_id: text(4)?,
                    nesting_depth: row.get::<_, Option<i64>>(5)?.map(|d| d.max(0) as u64),
                    agent_name: text(6)?,
                })
            });
            return rows
                .map(|rows| rows.flatten().collect())
                .unwrap_or_default();
        }
        if has_table("conversations") {
            let Ok(mut stmt) =
                kv.prepare("SELECT id, name, updated_at FROM conversations ORDER BY id")
            else {
                return Vec::new();
            };
            let rows = stmt.query_map([], |row| {
                Ok(ConversationRow {
                    id: row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    project: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    last_activity: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    ..ConversationRow::default()
                })
            });
            return rows
                .map(|rows| rows.flatten().collect())
                .unwrap_or_default();
        }
        Vec::new()
    }

    /// Discover conversations from the summaries index.
    pub fn discover(&self) -> (Vec<SessionSummary>, AntigravityDiscovery) {
        let rows = self.summaries();
        let summaries = rows
            .iter()
            .map(|row| SessionSummary {
                id: row.id.clone(),
                agent: Agent::Antigravity,
                project_id: row.project.clone(),
                last_activity_at: row.last_activity.clone(),
                metadata: SessionMetadata {
                    title: row.title.clone(),
                    ..SessionMetadata::default()
                },
                ..SessionSummary::default()
            })
            .collect();
        (
            summaries,
            AntigravityDiscovery {
                conversations: rows.len(),
            },
        )
    }

    /// The doctor report: conversations, transcripts, and issues.
    pub fn doctor(&self) -> AntigravityReport {
        let mut report = AntigravityReport {
            root: self.root.clone(),
            conversations: 0,
            transcripts: 0,
            issues: Vec::new(),
        };
        if !self.root.join("conversation_summaries.db").is_file() {
            report
                .issues
                .push("conversation_summaries.db is missing".to_string());
        }
        for row in self.summaries() {
            report.conversations += 1;
            if self.transcript_path(&row.id).is_file() {
                report.transcripts += 1;
            } else {
                report.issues.push(format!(
                    "conversation {} has no readable transcript",
                    row.id
                ));
            }
        }
        report
    }

    fn transcript_path(&self, conversation_id: &str) -> PathBuf {
        self.root
            .join("brain")
            .join(conversation_id)
            .join(".system_generated")
            .join("logs")
            .join("transcript.jsonl")
    }

    fn payload_db(&self, conversation_id: &str) -> PathBuf {
        self.root
            .join("brain")
            .join(conversation_id)
            .join("payload.db")
    }

    /// Tool-call linkage from the payload DB, keyed by step_index. The DB
    /// is read through a snapshot copy.
    fn tool_links(&self, conversation_id: &str) -> BTreeMap<u64, ToolLink> {
        let db = self.payload_db(conversation_id);
        if !db.is_file() {
            return BTreeMap::new();
        }
        let Ok(kv) = self.snapshot_connection(&db) else {
            return BTreeMap::new();
        };
        let Ok(mut stmt) = kv.prepare("SELECT step_index, call_id, result FROM tool_links") else {
            return BTreeMap::new();
        };
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)? as u64,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            ))
        });
        rows.map(|rows| {
            rows.flatten()
                .map(|(step_index, call_id, result)| (step_index, ToolLink { call_id, result }))
                .collect()
        })
        .unwrap_or_default()
    }

    /// Extract one conversation. The transcript is the preferred source;
    /// records are ordered by their explicit `step_index`; the payload DB
    /// (read through a snapshot copy) supplies tool-call linkage the
    /// transcript lacks.
    pub fn extract(&self, conversation_id: &str) -> Option<AntigravityExtraction> {
        let transcript_path = self.transcript_path(conversation_id);
        let transcript = std::fs::read_to_string(&transcript_path).ok()?;
        let row = self
            .summaries()
            .into_iter()
            .find(|row| row.id == conversation_id)
            .unwrap_or_default();
        let mut session = crate::session::Session {
            id: conversation_id.to_string(),
            agent: Agent::Antigravity,
            project_id: row.project.clone(),
            ..crate::session::Session::default()
        };
        session.metadata.title = row.title.clone();
        if let Some(parent) = &row.parent_id {
            session.parent_id = Some(parent.clone());
            session.subagent = Some(SubagentInfo {
                kind: row
                    .agent_name
                    .clone()
                    .unwrap_or_else(|| "subagent".to_string()),
                nesting_depth: row.nesting_depth,
            });
        }

        let mut records: Vec<(Value, u64)> = Vec::new();
        let mut skipped = SkipCounter::default();
        for line in transcript.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(trimmed) {
                Ok(record) => match record.get("step_index").and_then(Value::as_u64) {
                    Some(step_index) => records.push((record, step_index)),
                    None => skipped.malformed += 1,
                },
                Err(_) => skipped.malformed += 1,
            }
        }
        // The explicit step_index is the authoritative order.
        records.sort_by_key(|(_, step_index)| *step_index);

        let links = self.tool_links(conversation_id);
        let mut good = 0;
        for (record, step_index) in records {
            let ty = record.get("type").and_then(Value::as_str).unwrap_or("");
            let mut messages = match ty {
                "USER_INPUT" | "PLANNER_RESPONSE" | "CHECKPOINT" | "ERROR_MESSAGE"
                | "SYSTEM_MESSAGE" | "EPHEMERAL_MESSAGE" | "GENERIC" => {
                    vec![normalize_step(&record, ty, &transcript_path, step_index)]
                }
                "TOOL_CALL" => match links.get(&step_index) {
                    Some(link) => {
                        let call = tool_call_message(&record, link, &transcript_path, step_index);
                        let result = tool_result_message(link, &transcript_path, step_index);
                        vec![call, result]
                    }
                    None => {
                        skipped.malformed += 1;
                        continue;
                    }
                },
                _ => {
                    skipped.unknown += 1;
                    continue;
                }
            };
            for message in messages.drain(..) {
                if let Some(timestamp) = &message.timestamp {
                    if session.started_at.is_empty() {
                        session.started_at = timestamp.clone();
                    }
                    session.last_activity_at = timestamp.clone();
                }
                session.messages.push(message);
                good += 1;
            }
        }
        Some(AntigravityExtraction {
            session,
            good,
            skipped,
        })
    }
}

fn step_text(record: &Value) -> String {
    let text = |key: &str| match record.get(key) {
        Some(Value::String(text)) if !text.is_empty() => Some(text.clone()),
        Some(Value::Null) | None | Some(Value::String(_)) => None,
        Some(other) => Some(other.to_string()),
    };
    text("content")
        .or_else(|| text("summary"))
        .or_else(|| text("error"))
        .unwrap_or_default()
}

fn normalize_step(record: &Value, ty: &str, source_file: &Path, step_index: u64) -> Message {
    let (role, is_error, is_compaction_summary) = match ty {
        "USER_INPUT" => (Role::User, false, false),
        "CHECKPOINT" => (Role::Assistant, false, true),
        "ERROR_MESSAGE" => (Role::System, true, false),
        "SYSTEM_MESSAGE" | "EPHEMERAL_MESSAGE" => (Role::System, false, false),
        // A step result the model's tooling reported back.
        "GENERIC" => (Role::Tool, record.get("error").is_some(), false),
        _ => (Role::Assistant, false, false),
    };
    let text = step_text(record);
    let mut parts = Vec::new();
    if !text.is_empty() {
        parts.push(Part::Known(KnownPart::Text { text, extra: None }));
    }
    if let Some(thinking) = record
        .get("thinking")
        .and_then(Value::as_str)
        .filter(|thinking| !thinking.is_empty())
    {
        parts.push(Part::Known(KnownPart::Thinking {
            text: Some(thinking.to_string()),
            signature: None,
            encrypted: None,
            extra: None,
        }));
    }
    for (index, call) in record
        .get("tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        parts.push(Part::Known(KnownPart::ToolCall {
            id: format!("step-{step_index}-{index}"),
            name: call
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            arguments: call.get("args").map(|args| match args {
                Value::String(raw) => ToolArguments::from_value(
                    serde_json::from_str::<Value>(raw).unwrap_or_else(|_| args.clone()),
                ),
                other => ToolArguments::from_value(other.clone()),
            }),
            status: None,
            extra: None,
        }));
    }
    if parts.is_empty() {
        parts.push(Part::Known(KnownPart::Text {
            text: String::new(),
            extra: None,
        }));
    }
    let mut message = Message {
        role,
        is_error,
        is_compaction_summary,
        parts,
        ..Message::default()
    };
    message.timestamp = record
        .get("created_at")
        .and_then(Value::as_str)
        .map(str::to_string);
    message.provenance = Some(Provenance {
        source_file: source_file.display().to_string(),
        record_id: record.get("id").and_then(Value::as_str).map(str::to_string),
        record_type: Some(ty.to_string()),
        ordinal: Some(step_index),
        ..Provenance::default()
    });
    message
}

fn tool_call_message(
    record: &Value,
    link: &ToolLink,
    source_file: &Path,
    step_index: u64,
) -> Message {
    let mut message = Message {
        role: Role::Assistant,
        parts: vec![Part::Known(KnownPart::ToolCall {
            id: link.call_id.clone(),
            name: record
                .get("toolName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            arguments: record
                .get("arguments")
                .and_then(Value::as_str)
                .and_then(|arguments| serde_json::from_str::<Value>(arguments).ok())
                .map(ToolArguments::from_value),
            status: None,
            extra: None,
        })],
        ..Message::default()
    };
    message.timestamp = record
        .get("created_at")
        .and_then(Value::as_str)
        .map(str::to_string);
    message.provenance = Some(Provenance {
        source_file: source_file.display().to_string(),
        record_type: Some("TOOL_CALL".to_string()),
        ordinal: Some(step_index),
        ..Provenance::default()
    });
    message
}

fn tool_result_message(link: &ToolLink, source_file: &Path, step_index: u64) -> Message {
    let mut message = Message {
        role: Role::Tool,
        parts: vec![Part::Known(KnownPart::ToolResult {
            call_id: link.call_id.clone(),
            text: Some(link.result.clone()),
            status: None,
            extra: None,
        })],
        ..Message::default()
    };
    message.provenance = Some(Provenance {
        source_file: source_file.display().to_string(),
        record_type: Some("TOOL_RESULT".to_string()),
        ordinal: Some(step_index),
        ..Provenance::default()
    });
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::digest_files;
    use rusqlite::Connection;

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("botspy-ag-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn open_store(path: &Path) -> Connection {
        let conn = Connection::open(path).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        conn
    }

    fn write_summaries(root: &Path, ids: &[&str]) {
        let conn = open_store(&root.join("conversation_summaries.db"));
        conn.execute(
            "CREATE TABLE conversations (id TEXT PRIMARY KEY, name TEXT, updated_at TEXT)",
            [],
        )
        .unwrap();
        for (i, id) in ids.iter().enumerate() {
            conn.execute(
                "INSERT INTO conversations (id, name, updated_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![id, format!("demo-{i}"), "2026-10-01T09:00:00Z"],
            )
            .unwrap();
        }
    }

    /// (id, parent, nesting_depth, agent_name)
    type SummaryRowSpec<'a> = (&'a str, Option<&'a str>, i64, &'a str);

    fn write_current_summaries(root: &Path, rows: &[SummaryRowSpec]) {
        let conn = open_store(&root.join("conversation_summaries.db"));
        conn.execute(
            "CREATE TABLE conversation_summaries (conversation_id TEXT PRIMARY KEY, title TEXT, \
             preview TEXT, step_count INTEGER, last_modified_time TEXT, workspace_uris TEXT, \
             status TEXT, source TEXT, project_id TEXT, agent_name TEXT, \
             parent_conversation_id TEXT, nesting_depth INTEGER, raw_summary BLOB)",
            [],
        )
        .unwrap();
        for (id, parent, depth, agent) in rows {
            conn.execute(
                "INSERT INTO conversation_summaries (conversation_id, title, last_modified_time, \
                 project_id, agent_name, parent_conversation_id, nesting_depth) \
                 VALUES (?1, ?2, '2026-10-01 09:00:00.123456+00:00', 'demo-project', ?3, ?4, ?5)",
                rusqlite::params![
                    id,
                    format!("title of {id}"),
                    agent,
                    parent.unwrap_or(""),
                    depth
                ],
            )
            .unwrap();
        }
    }

    fn write_transcript(root: &Path, id: &str, records: &[String]) {
        let path = root
            .join("brain")
            .join(id)
            .join(".system_generated")
            .join("logs")
            .join("transcript.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            records
                .iter()
                .map(|line| format!("{line}\n"))
                .collect::<String>(),
        )
        .unwrap();
    }

    fn user_input(step_index: u64, content: &str) -> String {
        format!(
            r#"{{"type":"USER_INPUT","step_index":{step_index},"created_at":"2026-10-01T09:00:00Z","content":"{content}"}}"#
        )
    }

    fn write_payload_db(root: &Path, id: &str, links: &[(u64, &str, &str)]) {
        let db = root.join("brain").join(id).join("payload.db");
        let conn = open_store(&db);
        conn.execute(
            "CREATE TABLE tool_links (step_index INTEGER PRIMARY KEY, call_id TEXT NOT NULL, result TEXT)",
            [],
        )
        .unwrap();
        for (step_index, call_id, result) in links {
            conn.execute(
                "INSERT INTO tool_links (step_index, call_id, result) VALUES (?1, ?2, ?3)",
                rusqlite::params![*step_index as i64, call_id, result],
            )
            .unwrap();
        }
    }

    #[test]
    fn discovery_lists_conversations() {
        let root = temp_root("discover");
        write_summaries(&root, &["conv-2", "conv-1"]);
        let source = AntigravitySource::new(&root);
        let (summaries, stats) = source.discover();
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].id, "conv-1");
        assert_eq!(summaries[0].project_id, "demo-1");
        assert_eq!(stats.conversations, 2);
    }

    #[test]
    fn doctor_flags_missing_transcripts() {
        let root = temp_root("doctor");
        write_summaries(&root, &["conv-1"]);
        let source = AntigravitySource::new(&root);
        let report = source.doctor();
        assert_eq!(report.conversations, 1);
        assert_eq!(report.transcripts, 0);
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.contains("conv-1 has no readable transcript")));
    }

    #[test]
    fn transcript_types_map_to_chapter_1_kinds() {
        let root = temp_root("types");
        write_summaries(&root, &["conv-1"]);
        write_transcript(
            &root,
            "conv-1",
            &[
                user_input(1, "run the tests"),
                r#"{"type":"PLANNER_RESPONSE","step_index":2,"created_at":"2026-10-01T09:00:05Z","content":"On it."}"#.to_string(),
                r#"{"type":"CHECKPOINT","step_index":3,"created_at":"2026-10-01T09:00:10Z","summary":"checkpoint state"}"#.to_string(),
                r#"{"type":"ERROR_MESSAGE","step_index":4,"created_at":"2026-10-01T09:00:15Z","content":"boom"}"#.to_string(),
            ],
        );
        let source = AntigravitySource::new(&root);
        let extraction = source.extract("conv-1").expect("extract");
        let messages = &extraction.session.messages;
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].role, Role::User);
        assert_eq!(messages[1].role, Role::Assistant);
        assert!(messages[2].is_compaction_summary);
        assert!(messages[3].is_error);
        assert_eq!(messages[3].role, Role::System);
        assert_eq!(extraction.skipped.total(), 0);
    }

    #[test]
    fn step_index_is_the_authoritative_order() {
        let root = temp_root("order");
        write_summaries(&root, &["conv-1"]);
        write_transcript(
            &root,
            "conv-1",
            &[
                user_input(3, "third"),
                user_input(1, "first"),
                user_input(2, "second"),
            ],
        );
        let source = AntigravitySource::new(&root);
        let extraction = source.extract("conv-1").expect("extract");
        let steps: Vec<u64> = extraction
            .session
            .messages
            .iter()
            .map(|message| {
                message
                    .provenance
                    .as_ref()
                    .and_then(|prov| prov.ordinal)
                    .unwrap()
            })
            .collect();
        assert_eq!(steps, vec![1, 2, 3]);
    }

    #[test]
    fn payload_db_supplies_tool_call_linkage() {
        let root = temp_root("linkage");
        write_summaries(&root, &["conv-1"]);
        write_transcript(
            &root,
            "conv-1",
            &[
                user_input(1, "run the tests"),
                r#"{"type":"TOOL_CALL","step_index":2,"created_at":"2026-10-01T09:00:05Z","toolName":"run_terminal","arguments":"{\"cmd\":[\"cargo\",\"test\"]}"}"#.to_string(),
            ],
        );
        write_payload_db(&root, "conv-1", &[(2, "call_5", "all tests pass")]);
        let source = AntigravitySource::new(&root);
        let extraction = source.extract("conv-1").expect("extract");
        let messages = &extraction.session.messages;
        assert_eq!(messages.len(), 3);
        let call_id = messages[1]
            .parts
            .iter()
            .find_map(|part| match part {
                Part::Known(KnownPart::ToolCall { id, name, .. }) => {
                    Some((id.clone(), name.clone()))
                }
                _ => None,
            })
            .expect("tool call part");
        assert_eq!(call_id.0, "call_5");
        assert_eq!(call_id.1, "run_terminal");
        let result_call_id = messages[2]
            .parts
            .iter()
            .find_map(|part| match part {
                Part::Known(KnownPart::ToolResult { call_id, .. }) => Some(call_id.clone()),
                _ => None,
            })
            .expect("tool result part");
        assert_eq!(result_call_id, "call_5");
        assert_eq!(messages[2].role, Role::Tool);
    }

    #[test]
    fn hot_payload_dbs_are_read_through_snapshots() {
        let root = temp_root("wal");
        write_summaries(&root, &["conv-1"]);
        write_transcript(&root, "conv-1", &[user_input(1, "hello")]);
        let db = root.join("brain").join("conv-1").join("payload.db");
        let writer = open_store(&db);
        writer
            .execute(
                "CREATE TABLE tool_links (step_index INTEGER PRIMARY KEY, call_id TEXT NOT NULL, result TEXT)",
                [],
            )
            .unwrap();
        writer
            .execute(
                "INSERT INTO tool_links (step_index, call_id, result) VALUES (1, 'call_1', 'ok')",
                [],
            )
            .unwrap();
        let wal = {
            let mut wal = db.as_os_str().to_owned();
            wal.push("-wal");
            PathBuf::from(wal)
        };
        let before = digest_files(&[&db, &wal]);
        let source = AntigravitySource::new(&root);
        let extraction = source.extract("conv-1").expect("extract");
        assert_eq!(extraction.good, 1);
        let after = digest_files(&[&db, &wal]);
        assert_eq!(before, after, "reading the payload DB mutated it");
        drop(writer);
    }

    #[test]
    fn malformed_steps_are_skipped_and_counted() {
        let root = temp_root("malformed");
        write_summaries(&root, &["conv-1"]);
        write_transcript(
            &root,
            "conv-1",
            &[
                user_input(1, "good 0"),
                "this line is { not json".to_string(),
                r#"{"type":"UNKNOWN_EVENT","step_index":2}"#.to_string(),
            ],
        );
        let source = AntigravitySource::new(&root);
        let extraction = source.extract("conv-1").expect("extract");
        assert_eq!(extraction.good, 1);
        assert_eq!(extraction.skipped.malformed, 1);
        assert_eq!(extraction.skipped.unknown, 1);
    }

    #[test]
    fn the_current_summaries_schema_is_discovered_with_parent_and_nesting() {
        let root = temp_root("current");
        write_current_summaries(
            &root,
            &[
                ("conv-1", None, 0, "self"),
                ("conv-2", Some("conv-1"), 1, "research"),
                ("conv-3", Some("conv-1"), 1, ""),
            ],
        );
        write_transcript(&root, "conv-1", &[user_input(1, "hello")]);
        write_transcript(&root, "conv-2", &[user_input(1, "child")]);
        write_transcript(&root, "conv-3", &[user_input(1, "child")]);
        let source = AntigravitySource::new(&root);
        let (summaries, stats) = source.discover();
        assert_eq!(stats.conversations, 3);
        assert_eq!(summaries[0].project_id, "demo-project");
        assert_eq!(
            summaries[0].metadata.title.as_deref(),
            Some("title of conv-1")
        );
        assert_eq!(
            summaries[0].last_activity_at,
            "2026-10-01T09:00:00.123456+00:00"
        );
        assert_eq!(source.doctor().transcripts, 3);

        let parent = source.extract("conv-1").unwrap().session;
        assert_eq!(parent.parent_id, None);
        assert_eq!(parent.subagent, None);
        let child = source.extract("conv-2").unwrap().session;
        assert_eq!(child.parent_id.as_deref(), Some("conv-1"));
        let info = child.subagent.unwrap();
        assert_eq!(info.kind, "research");
        assert_eq!(info.nesting_depth, Some(1));
        let unnamed = source.extract("conv-3").unwrap().session;
        assert_eq!(unnamed.subagent.unwrap().kind, "subagent");
    }

    #[test]
    fn the_old_summaries_schema_is_still_read() {
        let root = temp_root("old");
        write_summaries(&root, &["conv-1"]);
        write_transcript(&root, "conv-1", &[user_input(1, "hello")]);
        let source = AntigravitySource::new(&root);
        let session = source.extract("conv-1").unwrap().session;
        assert_eq!(session.project_id, "demo-0");
        assert_eq!(session.parent_id, None);
        assert_eq!(
            source.discover().0[0].last_activity_at,
            "2026-10-01T09:00:00Z"
        );
    }

    #[test]
    fn current_transcript_types_and_keys_map_to_messages() {
        let root = temp_root("current-types");
        write_current_summaries(&root, &[("conv-1", None, 0, "self")]);
        write_transcript(
            &root,
            "conv-1",
            &[
                r#"{"type":"USER_INPUT","step_index":0,"source":"USER_EXPLICIT","status":"DONE","created_at":"2026-10-01T09:00:00Z","content":"go"}"#.to_string(),
                r#"{"type":"PLANNER_RESPONSE","step_index":1,"source":"MODEL","status":"DONE","created_at":"2026-10-01T09:00:01Z","thinking":"hmm","tool_calls":[{"name":"run","args":{"cmd":"ls"}}]}"#.to_string(),
                r#"{"type":"GENERIC","step_index":2,"source":"MODEL","status":"DONE","created_at":"2026-10-01T09:00:02Z","content":"output"}"#.to_string(),
                r#"{"type":"SYSTEM_MESSAGE","step_index":3,"source":"SYSTEM","status":"DONE","created_at":"2026-10-01T09:00:03Z","content":"note"}"#.to_string(),
                r#"{"type":"ERROR_MESSAGE","step_index":4,"source":"SYSTEM","status":"ERROR","created_at":"2026-10-01T09:00:04Z","error":"boom"}"#.to_string(),
                r#"{"type":"CHECKPOINT","step_index":5,"source":"SYSTEM","status":"DONE","created_at":"2026-10-01T09:00:05Z","content":"state"}"#.to_string(),
            ],
        );
        let extraction = AntigravitySource::new(&root).extract("conv-1").unwrap();
        assert_eq!(extraction.skipped.total(), 0);
        let messages = &extraction.session.messages;
        let roles: Vec<Role> = messages.iter().map(|m| m.role).collect();
        assert_eq!(
            roles,
            [
                Role::User,
                Role::Assistant,
                Role::Tool,
                Role::System,
                Role::System,
                Role::Assistant
            ]
        );
        assert_eq!(
            messages[0].timestamp.as_deref(),
            Some("2026-10-01T09:00:00Z")
        );
        assert!(matches!(
            messages[1].parts.as_slice(),
            [
                Part::Known(KnownPart::Thinking { .. }),
                Part::Known(KnownPart::ToolCall { .. })
            ]
        ));
        assert!(messages[4].is_error);
        assert!(matches!(
            messages[4].parts.first(),
            Some(Part::Known(KnownPart::Text { text, .. })) if text == "boom"
        ));
        assert!(messages[5].is_compaction_summary);
        assert_eq!(extraction.session.started_at, "2026-10-01T09:00:00Z");
        assert_eq!(extraction.session.last_activity_at, "2026-10-01T09:00:05Z");
    }
}
