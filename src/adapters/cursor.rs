//! The Cursor importer.
//!
//! Watches two sources: the IDE's SQLite KV store (`state.vscdb` — the
//! `composerHeaders` list, `composerData:<id>` composer state with the
//! authoritative `fullConversationHeadersOnly` bubble order, and
//! `bubbleId:<composerId>:<bubbleId>` bubble values, plus content-addressed
//! `agentKv:blob:<sha256>` blobs) and the CLI agent transcripts under
//! `~/.cursor/projects/<encoded-cwd>/agent-transcripts/` (nested per
//! session, sub-agents included, or the older flat files). The KV store is a
//! hot WAL database: it is read through a snapshot copy, never in place,
//! never written. Bubble keys are unordered; order comes from the headers
//! list. Changed composers are re-detected by `lastUpdatedAt`.

use crate::importer::SkipCounter;
use crate::schema::{Agent, KnownPart, Message, Part, Provenance, Role, SubagentInfo};
use crate::session::SessionSummary;
use crate::snapshot::SnapshotConnection;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// What discovery saw in the composer headers table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CursorDiscovery {
    /// Composers found in `composerHeaders`.
    pub composers: usize,
    /// Composers whose `lastUpdatedAt` changed since the previous
    /// discovery (including composers seen for the first time).
    pub changed: Vec<String>,
}

/// Diagnostics over a Cursor KV store.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct CursorReport {
    pub store: PathBuf,
    pub composers: usize,
    pub bubbles: usize,
    pub issues: Vec<String>,
}

/// The result of one extraction pass over a composer.
#[derive(Debug, Clone, PartialEq)]
pub struct CursorExtraction {
    pub session: crate::session::Session,
    /// Messages normalized by this pass.
    pub good: usize,
    /// Bubbles skipped as malformed or of unknown type.
    pub skipped: SkipCounter,
}

/// What discovery saw under a Cursor projects root.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CursorCliDiscovery {
    pub transcripts: usize,
    /// Sub-agent transcripts, listed as sessions spawned by their parent.
    pub subagents: usize,
    pub skipped_files: usize,
}

/// The result of one extraction pass over a CLI transcript.
#[derive(Debug, Clone, PartialEq)]
pub struct CursorCliExtraction {
    pub session: crate::session::Session,
    pub good: usize,
    pub skipped: SkipCounter,
}

/// The Cursor source over one IDE KV store (`state.vscdb`).
#[derive(Debug)]
pub struct CursorSource {
    store: PathBuf,
    /// `lastUpdatedAt` observed per composer on the previous discovery, so
    /// changed composers can be re-detected.
    observed: Mutex<BTreeMap<String, String>>,
}

impl CursorSource {
    /// A source over the KV store file (e.g. `~/.cursor/state.vscdb`).
    pub fn new(store: impl Into<PathBuf>) -> Self {
        Self {
            store: store.into(),
            observed: Mutex::new(BTreeMap::new()),
        }
    }

    /// The store this source watches.
    pub fn store(&self) -> &Path {
        &self.store
    }

    /// A read-only connection to a snapshot copy of the store. The hot WAL
    /// store is never read in place.
    fn snapshot(&self) -> Result<SnapshotConnection, crate::snapshot::SnapshotError> {
        SnapshotConnection::open(&self.store)
    }

    fn read_json(kv: &rusqlite::Connection, key: &str) -> Option<Value> {
        let raw: String = kv
            .query_row(
                "SELECT value FROM cursorDiskKV WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// The composer headers: the `composerHeaders` key of `cursorDiskKV`
    /// (older stores), else the `composerHeaders` table that current
    /// stores migrated them to. Each header is normalized to
    /// `{composerId, lastUpdatedAt, name}`.
    fn composer_headers(kv: &rusqlite::Connection) -> Option<Vec<Value>> {
        if let Some(headers) =
            Self::read_json(kv, "composerHeaders").and_then(|value| value.as_array().cloned())
        {
            return Some(headers);
        }
        let mut stmt = kv
            .prepare(
                "SELECT composerId, COALESCE(lastUpdatedAt, createdAt), value FROM composerHeaders",
            )
            .ok()?;
        let rows = stmt
            .query_map([], |row| {
                let value: Option<String> = row.get(2)?;
                let name = value
                    .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                    .and_then(|value| {
                        value
                            .get("name")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .unwrap_or_default();
                Ok(serde_json::json!({
                    "composerId": row.get::<_, String>(0)?,
                    "lastUpdatedAt": row.get::<_, Option<i64>>(1)?,
                    "name": name,
                }))
            })
            .ok()?;
        Some(rows.flatten().collect())
    }

    /// Discover composers from the headers table, reporting the ones whose
    /// `lastUpdatedAt` changed since the previous discovery.
    pub fn discover(&self) -> (Vec<SessionSummary>, CursorDiscovery) {
        match self.snapshot() {
            Ok(kv) => self.discover_in(&kv),
            Err(_) => (Vec::new(), CursorDiscovery::default()),
        }
    }

    /// Discovery over an already-open snapshot, so a caller that needs the
    /// store more than once pays for one copy of it, not two.
    fn discover_in(&self, kv: &rusqlite::Connection) -> (Vec<SessionSummary>, CursorDiscovery) {
        let mut summaries = Vec::new();
        let mut stats = CursorDiscovery::default();
        let Some(headers) = Self::composer_headers(kv) else {
            return (summaries, stats);
        };
        for composer in headers {
            let Some(id) = composer
                .get("composerId")
                .and_then(Value::as_str)
                .map(str::to_string)
            else {
                continue;
            };
            let stamp = last_updated_at(&composer);
            let mut observed = self.observed.lock().expect("observed");
            if observed.get(&id) != Some(&stamp) {
                stats.changed.push(id.clone());
            }
            observed.insert(id.clone(), stamp);
            summaries.push(SessionSummary {
                id,
                agent: Agent::Cursor,
                project_id: composer
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                ..SessionSummary::default()
            });
            stats.composers += 1;
        }
        summaries.sort_by(|a, b| a.id.cmp(&b.id));
        (summaries, stats)
    }

    /// The doctor report: composers, bubbles, and issues.
    pub fn doctor(&self) -> CursorReport {
        let mut report = CursorReport {
            store: self.store.clone(),
            ..CursorReport::default()
        };
        if !self.store.is_file() {
            report.issues.push("KV store file is missing".to_string());
            return report;
        }
        let Ok(kv) = self.snapshot() else {
            report
                .issues
                .push("KV store could not be snapshotted".to_string());
            return report;
        };
        report.composers = self.discover_in(&kv).0.len();
        report.bubbles = kv
            .query_row(
                "SELECT COUNT(*) FROM cursorDiskKV WHERE key LIKE 'bubbleId:%'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(0) as usize;
        if Self::composer_headers(&kv).is_none() {
            report
                .issues
                .push("composerHeaders entry is missing or unreadable".to_string());
        }
        report
    }

    /// Extract one composer's bubbles in `fullConversationHeadersOnly`
    /// order — the bubble keys themselves are unordered. Returns `None`
    /// when no composer with that id exists.
    pub fn extract(&self, composer_id: &str) -> Option<CursorExtraction> {
        let kv = self.snapshot().ok()?;
        let composer_data = Self::read_json(&kv, &format!("composerData:{composer_id}"))?;
        let headers = composer_data
            .get("fullConversationHeadersOnly")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let mut session = crate::session::Session {
            id: composer_id.to_string(),
            agent: Agent::Cursor,
            project_id: composer_data
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            ..crate::session::Session::default()
        };
        let mut good = 0;
        let mut skipped = SkipCounter::default();
        for header in &headers {
            let bubble_id = header
                .get("bubbleId")
                .and_then(Value::as_str)
                .or_else(|| header.as_str())
                .unwrap_or_default();
            let key = format!("bubbleId:{composer_id}:{bubble_id}");
            let Some(bubble) = Self::read_json(&kv, &key) else {
                skipped.malformed += 1;
                continue;
            };
            match normalize_bubble(&bubble, bubble_id, &self.store) {
                Some(Ok(message)) => {
                    session.messages.push(message);
                    good += 1;
                }
                Some(Err(SkipReason::Malformed)) => skipped.malformed += 1,
                Some(Err(SkipReason::Unknown)) | None => skipped.unknown += 1,
            }
        }
        Some(CursorExtraction {
            session,
            good,
            skipped,
        })
    }
}

enum SkipReason {
    Malformed,
    Unknown,
}

/// The bubble's `lastUpdatedAt`, compared as a string so numeric and
/// string encodings both work.
fn last_updated_at(composer: &Value) -> String {
    match composer.get("lastUpdatedAt") {
        Some(Value::String(stamp)) => stamp.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

fn normalize_bubble(
    bubble: &Value,
    bubble_id: &str,
    store: &Path,
) -> Option<Result<Message, SkipReason>> {
    let ty = bubble.get("type").and_then(Value::as_u64)?;
    let role = match ty {
        1 => Role::User,
        2 => Role::Assistant,
        _ => return Some(Err(SkipReason::Unknown)),
    };
    let mut parts = Vec::new();
    if let Some(text) = bubble.get("text").and_then(Value::as_str) {
        if !text.is_empty() {
            parts.push(Part::Known(KnownPart::Text {
                text: text.to_string(),
                extra: None,
            }));
        }
    }
    if let Some(thinking) = bubble.get("thinking").and_then(Value::as_str) {
        parts.push(Part::Known(KnownPart::Thinking {
            text: Some(thinking.to_string()),
            signature: None,
            encrypted: None,
            extra: None,
        }));
    }
    if let Some(tool) = bubble.get("toolFormerData") {
        parts.push(Part::Known(KnownPart::ToolCall {
            id: bubble_id.to_string(),
            name: tool
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            arguments: tool
                .get("params")
                .and_then(Value::as_str)
                .and_then(|params| serde_json::from_str::<Value>(params).ok())
                .map(crate::schema::ToolArguments::from_value),
            status: None,
            extra: None,
        }));
    }
    if let Some(blob) = bubble.get("toolResultBlob").and_then(Value::as_str) {
        parts.push(Part::Known(KnownPart::Blob {
            blob_hash: blob.to_string(),
            container: "agentKv:blob".to_string(),
            extra: None,
        }));
    }
    if parts.is_empty() {
        return Some(Err(SkipReason::Malformed));
    }
    let mut message = Message {
        role,
        parts,
        ..Message::default()
    };
    message.timestamp = bubble
        .get("createdAt")
        .and_then(Value::as_str)
        .map(str::to_string);
    message.provenance = Some(Provenance {
        source_file: store.display().to_string(),
        row: None,
        record_id: Some(bubble_id.to_string()),
        record_type: Some(format!("bubbleId:{ty}")),
        ..Provenance::default()
    });
    Some(Ok(message))
}

/// Diagnostics over a Cursor projects root.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct CursorCliReport {
    pub root: PathBuf,
    pub transcripts: usize,
    pub subagents: usize,
    pub issues: Vec<String>,
}

/// Marker between the parent session id and the sub-agent file stem in a
/// sub-agent session id, the same shape Claude Code sub-agents use.
const SUBAGENT_MARK: &str = "--agent-";

/// One transcript file the CLI source found.
#[derive(Debug, Clone)]
struct CliTranscript {
    /// The session id: the file stem, or `<parent>--agent-<stem>`.
    id: String,
    path: PathBuf,
    /// The encoded-cwd project directory name.
    project: String,
    /// The session that spawned this one, for a sub-agent.
    parent: Option<String>,
}

/// The Cursor CLI source over one projects root (`~/.cursor/projects`),
/// watching `<project>/agent-transcripts/`. Two layouts are read:
///
/// - current: `<session>/<session>.jsonl`, with sub-agents beside it at
///   `<session>/subagents/<id>.jsonl`;
/// - older: a flat `<session>.jsonl`.
///
/// Sub-agents are sessions of their own, `<parent>--agent-<id>`, spawned
/// by the parent session.
#[derive(Debug)]
pub struct CursorCliSource {
    root: PathBuf,
}

impl CursorCliSource {
    /// A CLI source over the projects root.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The projects root this source watches.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn transcripts(&self) -> Vec<CliTranscript> {
        let mut found = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return found;
        };
        for project in entries.flatten() {
            let project_name = file_stem(&project.path());
            let Ok(files) = std::fs::read_dir(project.path().join("agent-transcripts")) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                let stem = file_stem(&path);
                if path.is_dir() {
                    let main = path.join(format!("{stem}.jsonl"));
                    if main.is_file() {
                        found.push(CliTranscript {
                            id: stem.clone(),
                            path: main,
                            project: project_name.clone(),
                            parent: None,
                        });
                    }
                    let Ok(agents) = std::fs::read_dir(path.join("subagents")) else {
                        continue;
                    };
                    for agent in agents.flatten() {
                        let agent_path = agent.path();
                        if agent_path.is_file()
                            && agent_path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
                        {
                            found.push(CliTranscript {
                                id: format!("{stem}{SUBAGENT_MARK}{}", file_stem(&agent_path)),
                                path: agent_path,
                                project: project_name.clone(),
                                parent: Some(stem.clone()),
                            });
                        }
                    }
                } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
                    found.push(CliTranscript {
                        id: stem,
                        path,
                        project: project_name.clone(),
                        parent: None,
                    });
                }
            }
        }
        found.sort_by(|a, b| a.id.cmp(&b.id));
        found
    }

    /// Discover CLI transcripts, one session per file; sub-agent
    /// transcripts are sessions of their own.
    pub fn discover(&self) -> (Vec<SessionSummary>, CursorCliDiscovery) {
        let mut summaries = Vec::new();
        let mut stats = CursorCliDiscovery::default();
        for transcript in self.transcripts() {
            if transcript.parent.is_some() {
                stats.subagents += 1;
            } else {
                stats.transcripts += 1;
            }
            summaries.push(SessionSummary {
                id: transcript.id,
                agent: Agent::Cursor,
                project_id: transcript.project,
                ..SessionSummary::default()
            });
        }
        (summaries, stats)
    }

    /// The doctor report: transcript and sub-agent counts, and issues.
    pub fn doctor(&self) -> CursorCliReport {
        let (_, stats) = self.discover();
        let mut report = CursorCliReport {
            root: self.root.clone(),
            transcripts: stats.transcripts,
            subagents: stats.subagents,
            issues: Vec::new(),
        };
        if !self.root.is_dir() {
            report.issues.push("projects root is missing".to_string());
        } else if stats.transcripts + stats.subagents == 0 {
            report.issues.push("no agent transcripts found".to_string());
        }
        report
    }

    /// Extract one CLI transcript from its start. A sub-agent is asked for
    /// by its `<parent>--agent-<id>` id and comes back spawned by the
    /// parent session.
    pub fn extract(&self, session_id: &str) -> Option<CursorCliExtraction> {
        let transcript = self
            .transcripts()
            .into_iter()
            .find(|transcript| transcript.id == session_id)?;
        let path = transcript.path;
        let text = std::fs::read_to_string(&path).ok()?;
        let mut session = crate::session::Session {
            id: session_id.to_string(),
            agent: Agent::Cursor,
            project_id: transcript.project,
            file_modified_at: file_modified_at(&path),
            ..crate::session::Session::default()
        };
        if let Some(parent) = transcript.parent {
            session.parent_id = Some(parent);
            session.subagent = Some(SubagentInfo {
                kind: "subagent".to_string(),
                nesting_depth: None,
            });
        }
        let mut good = 0;
        let mut skipped = SkipCounter::default();
        for (line_index, line) in text.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(record) = serde_json::from_str::<Value>(trimmed) else {
                skipped.malformed += 1;
                continue;
            };
            let Some(role) = record.get("role").and_then(Value::as_str) else {
                // Status records and the like: not part of the conversation.
                skipped.unknown += 1;
                continue;
            };
            let parts = cli_parts(&record, line_index);
            if parts.is_empty() {
                skipped.malformed += 1;
                continue;
            }
            session.messages.push(Message {
                role: match role {
                    "assistant" => Role::Assistant,
                    _ => Role::User,
                },
                parts,
                timestamp: record
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                provenance: Some(Provenance {
                    source_file: path.display().to_string(),
                    record_type: Some("message".to_string()),
                    ordinal: Some(line_index as u64),
                    ..Provenance::default()
                }),
                ..Message::default()
            });
            good += 1;
        }
        Some(CursorCliExtraction {
            session,
            good,
            skipped,
        })
    }
}

/// The parts of one CLI record: the current `{role, message: {content}}`
/// shape (content is text, or a list of `text` and `tool_use` blocks), or
/// the older flat `{role, text, thinking}` shape.
fn cli_parts(record: &Value, line_index: usize) -> Vec<Part> {
    let mut parts = Vec::new();
    let text_part = |text: &str| {
        Part::Known(KnownPart::Text {
            text: text.to_string(),
            extra: None,
        })
    };
    if let Some(text) = record.get("text").and_then(Value::as_str) {
        parts.push(text_part(text));
    }
    if let Some(thinking) = record.get("thinking").and_then(Value::as_str) {
        parts.push(Part::Known(KnownPart::Thinking {
            text: Some(thinking.to_string()),
            signature: None,
            encrypted: None,
            extra: None,
        }));
    }
    match record
        .get("message")
        .and_then(|message| message.get("content"))
    {
        Some(Value::String(text)) => parts.push(text_part(text)),
        Some(Value::Array(blocks)) => {
            for (index, block) in blocks.iter().enumerate() {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            parts.push(text_part(text));
                        }
                    }
                    Some("tool_use") => parts.push(Part::Known(KnownPart::ToolCall {
                        id: format!("line-{line_index}-{index}"),
                        name: block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        arguments: block
                            .get("input")
                            .cloned()
                            .map(crate::schema::ToolArguments::from_value),
                        status: None,
                        extra: None,
                    })),
                    _ => {}
                }
            }
        }
        _ => {}
    }
    parts
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_string()
}

/// The file's own mtime as RFC 3339 UTC, for sessions whose records carry
/// no timestamps. It is never copied into message timestamps.
fn file_modified_at(path: &Path) -> Option<String> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let millis = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis() as i64;
    Some(crate::adapters::codex::rfc3339_utc(millis)).filter(|stamp| !stamp.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::digest_files;
    use rusqlite::Connection;
    use serde_json::json;

    fn temp_store(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("botspy-cur-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("state.vscdb")
    }

    fn open_store(path: &Path) -> Connection {
        let conn = Connection::open(path).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        conn.execute(
            "CREATE TABLE IF NOT EXISTS cursorDiskKV (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
            [],
        )
        .unwrap();
        conn
    }

    fn put(conn: &Connection, key: &str, value: &Value) {
        conn.execute(
            "INSERT OR REPLACE INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, value.to_string()],
        )
        .unwrap();
    }

    #[test]
    fn discovery_lists_composers_and_reports_changes() {
        let store = temp_store("discover");
        let conn = open_store(&store);
        put(
            &conn,
            "composerHeaders",
            &json!([
                {"composerId": "composer-1", "lastUpdatedAt": 1000, "name": "demo"},
                {"composerId": "composer-2", "lastUpdatedAt": 2000, "name": "demo"},
                {"composerId": "composer-3", "lastUpdatedAt": 3000, "name": "demo"}
            ]),
        );
        for i in 1..=3 {
            put(
                &conn,
                &format!("composerData:composer-{i}"),
                &json!({"name": "demo", "fullConversationHeadersOnly": [{"bubbleId": "b1"}]}),
            );
            put(
                &conn,
                &format!("bubbleId:composer-{i}:b1"),
                &json!({"type": 1, "text": "hello"}),
            );
        }
        drop(conn);
        let source = CursorSource::new(&store);
        let (summaries, stats) = source.discover();
        assert_eq!(summaries.len(), 3);
        assert_eq!(summaries[0].id, "composer-1");
        assert_eq!(stats.composers, 3);
        assert_eq!(stats.changed.len(), 3, "first discovery reports all as new");

        // Bump only composer-2's lastUpdatedAt and append a bubble.
        let conn = open_store(&store);
        put(
            &conn,
            "composerHeaders",
            &json!([
                {"composerId": "composer-1", "lastUpdatedAt": 1000, "name": "demo"},
                {"composerId": "composer-2", "lastUpdatedAt": 9000, "name": "demo"},
                {"composerId": "composer-3", "lastUpdatedAt": 3000, "name": "demo"}
            ]),
        );
        put(
            &conn,
            "bubbleId:composer-2:b2",
            &json!({"type": 1, "text": "fresh"}),
        );
        drop(conn);
        let (_, stats) = source.discover();
        assert_eq!(stats.changed, vec!["composer-2".to_string()]);
    }

    #[test]
    fn bubbles_follow_headers_order_not_keys() {
        let store = temp_store("order");
        let conn = open_store(&store);
        put(
            &conn,
            "composerHeaders",
            &json!([{"composerId": "composer-1", "lastUpdatedAt": 1000, "name": "demo"}]),
        );
        put(
            &conn,
            "composerData:composer-1",
            &json!({
                "name": "demo",
                "fullConversationHeadersOnly": [{"bubbleId": "b1"}, {"bubbleId": "b2"}, {"bubbleId": "b3"}]
            }),
        );
        for bubble_id in ["b3", "b1", "b2"] {
            put(
                &conn,
                &format!("bubbleId:composer-1:{bubble_id}"),
                &json!({"type": 1, "text": format!("text of {bubble_id}")}),
            );
        }
        drop(conn);
        let source = CursorSource::new(&store);
        let extraction = source.extract("composer-1").expect("extract");
        let order: Vec<&str> = extraction
            .session
            .messages
            .iter()
            .map(|message| {
                message
                    .provenance
                    .as_ref()
                    .and_then(|prov| prov.record_id.as_deref())
                    .unwrap()
            })
            .collect();
        assert_eq!(order, vec!["b1", "b2", "b3"]);
        assert_eq!(extraction.good, 3);
    }

    #[test]
    fn bubbles_map_roles_thinking_and_tool_calls() {
        let store = temp_store("parts");
        let conn = open_store(&store);
        put(
            &conn,
            "composerHeaders",
            &json!([{"composerId": "composer-2", "lastUpdatedAt": 1000, "name": "demo"}]),
        );
        put(
            &conn,
            "composerData:composer-2",
            &json!({
                "name": "demo",
                "fullConversationHeadersOnly": [{"bubbleId": "b1"}, {"bubbleId": "b2"}, {"bubbleId": "b3"}]
            }),
        );
        put(
            &conn,
            "bubbleId:composer-2:b1",
            &json!({"type": 1, "text": "run the tests"}),
        );
        put(
            &conn,
            "bubbleId:composer-2:b2",
            &json!({"type": 2, "text": "On it.", "thinking": "checking the suite"}),
        );
        put(
            &conn,
            "bubbleId:composer-2:b3",
            &json!({"type": 2, "toolFormerData": {"name": "run_terminal", "params": "{\"cmd\":[\"cargo\",\"test\"]}"}}),
        );
        drop(conn);
        let source = CursorSource::new(&store);
        let extraction = source.extract("composer-2").expect("extract");
        let messages = &extraction.session.messages;
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, Role::User);
        assert_eq!(messages[1].role, Role::Assistant);
        assert!(matches!(
            messages[1].parts.get(1),
            Some(Part::Known(KnownPart::Thinking { .. }))
        ));
        assert!(matches!(
            messages[2].parts.first(),
            Some(Part::Known(KnownPart::ToolCall { .. }))
        ));
    }

    #[test]
    fn blob_tool_result_maps_to_a_blob_part() {
        let store = temp_store("blob");
        let conn = open_store(&store);
        let blob_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        put(
            &conn,
            "composerHeaders",
            &json!([{"composerId": "composer-3", "lastUpdatedAt": 1000, "name": "demo"}]),
        );
        put(
            &conn,
            "composerData:composer-3",
            &json!({"name": "demo", "fullConversationHeadersOnly": [{"bubbleId": "b1"}]}),
        );
        put(
            &conn,
            &format!("agentKv:blob:{blob_hash}"),
            &json!({"opaque": "protobuf bytes"}),
        );
        put(
            &conn,
            "bubbleId:composer-3:b1",
            &json!({"type": 2, "toolResultBlob": blob_hash}),
        );
        drop(conn);
        let source = CursorSource::new(&store);
        let extraction = source.extract("composer-3").expect("extract");
        let blob = extraction
            .session
            .messages
            .iter()
            .find_map(|message| {
                message.parts.iter().find_map(|part| match part {
                    Part::Known(KnownPart::Blob {
                        blob_hash,
                        container,
                        ..
                    }) => Some((blob_hash.clone(), container.clone())),
                    _ => None,
                })
            })
            .expect("blob part");
        assert_eq!(blob.0, blob_hash);
        assert_eq!(blob.1, "agentKv:blob");
    }

    #[test]
    fn doctor_flags_missing_store() {
        let store = temp_store("doctor");
        let source = CursorSource::new(&store);
        let report = source.doctor();
        assert_eq!(report.composers, 0);
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.contains("KV store file is missing")));
    }

    #[test]
    fn cli_transcripts_are_discovered_and_extracted() {
        let root = std::env::temp_dir().join(format!("botspy-cur-cli-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (project, transcript) in [("proj-a", "a.jsonl"), ("proj-b", "b.jsonl")] {
            let dir = root.join(project).join("agent-transcripts");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join(transcript),
                "{\"role\":\"user\",\"text\":\"hello from the CLI\"}\n{\"role\":\"assistant\",\"text\":\"On it.\",\"thinking\":\"planning\"}\n",
            )
            .unwrap();
        }
        std::fs::write(root.join("proj-a").join("notes.txt"), "junk").unwrap();
        let source = CursorCliSource::new(&root);
        let (summaries, stats) = source.discover();
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].id, "a");
        assert_eq!(summaries[0].project_id, "proj-a");
        assert_eq!(stats.transcripts, 2);
        let extraction = source.extract("a").expect("extract");
        assert_eq!(extraction.good, 2);
        assert!(matches!(
            extraction.session.messages[1].parts.get(1),
            Some(Part::Known(KnownPart::Thinking { .. }))
        ));
        let _ = digest_files(&[&root]);
    }

    fn nested_root(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("botspy-cur-nest-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    const NESTED_RECORDS: &str = concat!(
        r#"{"role":"user","message":{"content":[{"type":"text","text":"hello"}]}}"#,
        "\n",
        r#"{"role":"assistant","message":{"content":[{"type":"text","text":"On it."},{"type":"tool_use","name":"Shell","input":{"command":"ls"}}]}}"#,
        "\n",
        r#"{"type":"turn","status":"done"}"#,
        "\n",
    );

    #[test]
    fn nested_cli_layout_and_subagents_are_sessions() {
        let root = nested_root("layout");
        let dir = root.join("proj-a").join("agent-transcripts").join("s1");
        std::fs::create_dir_all(dir.join("subagents")).unwrap();
        std::fs::write(dir.join("s1.jsonl"), NESTED_RECORDS).unwrap();
        std::fs::write(dir.join("subagents").join("c1.jsonl"), NESTED_RECORDS).unwrap();
        std::fs::write(dir.join("subagents").join("notes.txt"), "junk").unwrap();
        let source = CursorCliSource::new(&root);
        let (summaries, stats) = source.discover();
        let ids: Vec<&str> = summaries.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["s1", "s1--agent-c1"]);
        assert_eq!((stats.transcripts, stats.subagents), (1, 1));
        assert_eq!(summaries[1].project_id, "proj-a");
        let doctor = source.doctor();
        assert_eq!((doctor.transcripts, doctor.subagents), (1, 1));
        assert!(doctor.issues.is_empty());

        let parent = source.extract("s1").unwrap();
        assert_eq!(parent.session.parent_id, None);
        assert_eq!(parent.session.subagent, None);
        assert_eq!(parent.good, 2);
        assert_eq!(
            parent.skipped.unknown, 1,
            "the status record is not a message"
        );
        let child = source.extract("s1--agent-c1").unwrap().session;
        assert_eq!(child.parent_id.as_deref(), Some("s1"));
        let info = child.subagent.unwrap();
        assert_eq!(info.kind, "subagent");
        assert_eq!(info.nesting_depth, None);
        assert_eq!(child.project_id, "proj-a");
    }

    #[test]
    fn current_records_map_text_and_tool_use_blocks() {
        let root = nested_root("records");
        let dir = root.join("proj-a").join("agent-transcripts").join("s1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("s1.jsonl"), NESTED_RECORDS).unwrap();
        let session = CursorCliSource::new(&root).extract("s1").unwrap().session;
        assert_eq!(session.messages[0].role, Role::User);
        assert_eq!(session.messages[1].role, Role::Assistant);
        assert!(matches!(
            session.messages[1].parts.as_slice(),
            [
                Part::Known(KnownPart::Text { .. }),
                Part::Known(KnownPart::ToolCall { name, .. })
            ] if name == "Shell"
        ));
    }

    #[test]
    fn sessions_without_timestamps_carry_the_file_mtime_only() {
        let root = nested_root("mtime");
        let dir = root.join("proj-a").join("agent-transcripts").join("s1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("s1.jsonl"), NESTED_RECORDS).unwrap();
        let session = CursorCliSource::new(&root).extract("s1").unwrap().session;
        let mtime = session.file_modified_at.expect("file mtime recorded");
        assert!(mtime.starts_with("20") && mtime.ends_with('Z'), "{mtime}");
        assert!(session.messages.iter().all(|m| m.timestamp.is_none()));
        assert!(session.started_at.is_empty());
    }

    #[test]
    fn flat_and_nested_layouts_can_share_a_root() {
        let root = nested_root("mixed");
        let flat = root.join("proj-a").join("agent-transcripts");
        std::fs::create_dir_all(flat.join("n1")).unwrap();
        std::fs::write(
            flat.join("old.jsonl"),
            "{\"role\":\"user\",\"text\":\"hi\"}\n",
        )
        .unwrap();
        std::fs::write(flat.join("n1").join("n1.jsonl"), NESTED_RECORDS).unwrap();
        // A session directory without its main transcript is not a session.
        std::fs::create_dir_all(flat.join("empty")).unwrap();
        let source = CursorCliSource::new(&root);
        let ids: Vec<String> = source.discover().0.into_iter().map(|s| s.id).collect();
        assert_eq!(ids, ["n1", "old"]);
        assert_eq!(source.extract("old").unwrap().good, 1);
        assert_eq!(source.extract("n1").unwrap().good, 2);
        assert!(source.extract("empty").is_none());
    }

    #[test]
    fn composers_are_discovered_from_the_migrated_headers_table() {
        let store = temp_store("table");
        let conn = open_store(&store);
        conn.execute(
            "CREATE TABLE composerHeaders (composerId TEXT PRIMARY KEY, workspaceId TEXT, \
             createdAt INTEGER, lastUpdatedAt INTEGER, isSubagent INTEGER, value TEXT)",
            [],
        )
        .unwrap();
        for (id, created, updated, name) in [
            ("composer-1", 100, Some(500), "named"),
            ("composer-2", 200, None, ""),
        ] {
            conn.execute(
                "INSERT INTO composerHeaders VALUES (?1, 'w', ?2, ?3, 0, ?4)",
                rusqlite::params![
                    id,
                    created,
                    updated,
                    json!({"composerId": id, "name": name}).to_string()
                ],
            )
            .unwrap();
        }
        put(
            &conn,
            "composerData:composer-1",
            &json!({"name": "named", "fullConversationHeadersOnly": [{"bubbleId": "b1"}]}),
        );
        put(
            &conn,
            "bubbleId:composer-1:b1",
            &json!({"type": 1, "text": "hi"}),
        );
        drop(conn);
        let source = CursorSource::new(&store);
        let (summaries, stats) = source.discover();
        assert_eq!(stats.composers, 2);
        assert_eq!(summaries[0].project_id, "named");
        assert_eq!(stats.changed.len(), 2);
        assert_eq!(source.extract("composer-1").unwrap().good, 1);
        assert!(source.doctor().issues.is_empty());
    }
}
