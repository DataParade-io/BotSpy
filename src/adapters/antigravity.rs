//! The Antigravity importer.
//!
//! Watches `conversation_summaries.db` for new and updated conversations,
//! prefers each conversation's readable
//! `brain/<id>/.system_generated/logs/transcript.jsonl`, and falls back to
//! the per-conversation SQLite DB (protobuf payload blobs) for linkage the
//! JSONL lacks — e.g. tool-call ids, which live in the DB's `tool_links`
//! table keyed by `step_index`. Per-conversation DBs are hot WAL
//! databases: they are read through a snapshot copy, never in place,
//! never written. Transcript records carry `USER_INPUT`,
//! `PLANNER_RESPONSE`, `CHECKPOINT`, `ERROR_MESSAGE`, `TOOL_CALL`,
//! `step_index` (the authoritative order), and `created_at`.

use crate::importer::SkipCounter;
use crate::schema::{Agent, KnownPart, Message, Part, Provenance, Role, ToolArguments};
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

    /// The summaries index: conversation id, name, updated_at.
    fn summaries(&self) -> Vec<(String, String, String)> {
        let db = self.root.join("conversation_summaries.db");
        let Ok(kv) = self.snapshot_connection(&db) else {
            return Vec::new();
        };
        let Ok(mut stmt) = kv.prepare("SELECT id, name, updated_at FROM conversations ORDER BY id")
        else {
            return Vec::new();
        };
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0),
                row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            ))
        });
        rows.map(|rows| {
            rows.flatten()
                .map(|(id, name, updated_at)| (id.unwrap_or_default(), name, updated_at))
                .collect()
        })
        .unwrap_or_default()
    }

    /// Discover conversations from the summaries index.
    pub fn discover(&self) -> (Vec<SessionSummary>, AntigravityDiscovery) {
        let rows = self.summaries();
        let summaries = rows
            .iter()
            .map(|(id, name, updated_at)| SessionSummary {
                id: id.clone(),
                agent: Agent::Antigravity,
                project_id: name.clone(),
                last_activity_at: updated_at.clone(),
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
        for (id, _, _) in self.summaries() {
            report.conversations += 1;
            if self.transcript_path(&id).is_file() {
                report.transcripts += 1;
            } else {
                report
                    .issues
                    .push(format!("conversation {id} has no readable transcript"));
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
        let name = self
            .summaries()
            .into_iter()
            .find(|(id, _, _)| id == conversation_id)
            .map(|(_, name, _)| name)
            .unwrap_or_default();
        let mut session = crate::session::Session {
            id: conversation_id.to_string(),
            agent: Agent::Antigravity,
            project_id: name,
            ..crate::session::Session::default()
        };

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
                "USER_INPUT" | "PLANNER_RESPONSE" | "CHECKPOINT" | "ERROR_MESSAGE" => {
                    vec![normalize_step(&record, ty, &transcript_path, step_index)?]
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
    record
        .get("content")
        .or_else(|| record.get("summary"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn normalize_step(
    record: &Value,
    ty: &str,
    source_file: &Path,
    step_index: u64,
) -> Option<Message> {
    let (role, is_error, is_compaction_summary) = match ty {
        "USER_INPUT" => (Role::User, false, false),
        "PLANNER_RESPONSE" => (Role::Assistant, false, false),
        "CHECKPOINT" => (Role::Assistant, false, true),
        "ERROR_MESSAGE" => (Role::System, true, false),
        _ => return None,
    };
    let mut message = Message {
        role,
        is_error,
        is_compaction_summary,
        parts: vec![Part::Known(KnownPart::Text {
            text: step_text(record),
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
        record_id: record.get("id").and_then(Value::as_str).map(str::to_string),
        record_type: Some(ty.to_string()),
        ordinal: Some(step_index),
        ..Provenance::default()
    });
    Some(message)
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
}
