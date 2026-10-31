//! The Cursor importer.
//!
//! Watches two sources: the IDE's SQLite KV store (`state.vscdb` — the
//! `composerHeaders` list, `composerData:<id>` composer state with the
//! authoritative `fullConversationHeadersOnly` bubble order, and
//! `bubbleId:<composerId>:<bubbleId>` bubble values, plus content-addressed
//! `agentKv:blob:<sha256>` blobs) and the CLI agent transcripts under
//! `~/.cursor/projects/<encoded-cwd>/agent-transcripts/`. The KV store is a
//! hot WAL database: it is read through a snapshot copy, never in place,
//! never written. Bubble keys are unordered; order comes from the headers
//! list. Changed composers are re-detected by `lastUpdatedAt`.

use crate::importer::SkipCounter;
use crate::schema::{Agent, KnownPart, Message, Part, Provenance, Role};
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

    /// Discover composers from the headers table, reporting the ones whose
    /// `lastUpdatedAt` changed since the previous discovery.
    pub fn discover(&self) -> (Vec<SessionSummary>, CursorDiscovery) {
        let mut summaries = Vec::new();
        let mut stats = CursorDiscovery::default();
        let Ok(kv) = self.snapshot() else {
            return (summaries, stats);
        };
        let Some(headers) =
            Self::read_json(&kv, "composerHeaders").and_then(|value| value.as_array().cloned())
        else {
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
        let (summaries, _) = self.discover();
        let mut report = CursorReport {
            store: self.store.clone(),
            composers: summaries.len(),
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
        report.bubbles = kv
            .query_row(
                "SELECT COUNT(*) FROM cursorDiskKV WHERE key LIKE 'bubbleId:%'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(0) as usize;
        if Self::read_json(&kv, "composerHeaders").is_none() {
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

/// The Cursor CLI source over one projects root
/// (`~/.cursor/projects`), watching `agent-transcripts/*.jsonl`.
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

    fn transcripts(&self) -> Vec<(String, PathBuf)> {
        let mut found = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return found;
        };
        for project in entries.flatten() {
            let transcripts = project.path().join("agent-transcripts");
            let Ok(files) = std::fs::read_dir(&transcripts) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
                    let id = path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .unwrap_or_default()
                        .to_string();
                    found.push((id, path));
                }
            }
        }
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found
    }

    /// Discover CLI transcripts, one session per file.
    pub fn discover(&self) -> (Vec<SessionSummary>, CursorCliDiscovery) {
        let mut summaries = Vec::new();
        let mut stats = CursorCliDiscovery::default();
        for (id, path) in self.transcripts() {
            summaries.push(SessionSummary {
                id,
                agent: Agent::Cursor,
                project_id: path
                    .parent()
                    .and_then(Path::parent)
                    .and_then(|project| project.file_stem())
                    .and_then(|stem| stem.to_str())
                    .unwrap_or_default()
                    .to_string(),
                ..SessionSummary::default()
            });
            stats.transcripts += 1;
        }
        (summaries, stats)
    }

    /// Extract one CLI transcript from its start.
    pub fn extract(&self, session_id: &str) -> Option<CursorCliExtraction> {
        let path = self
            .transcripts()
            .into_iter()
            .find(|(id, _)| id == session_id)
            .map(|(_, path)| path)?;
        let text = std::fs::read_to_string(&path).ok()?;
        let mut session = crate::session::Session {
            id: session_id.to_string(),
            agent: Agent::Cursor,
            project_id: path
                .parent()
                .and_then(Path::parent)
                .and_then(|project| project.file_stem())
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_string(),
            ..crate::session::Session::default()
        };
        let mut good = 0;
        let mut skipped = SkipCounter::default();
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(record) = serde_json::from_str::<Value>(trimmed) else {
                skipped.malformed += 1;
                continue;
            };
            let Some(role) = record.get("role").and_then(Value::as_str) else {
                skipped.unknown += 1;
                continue;
            };
            let Some(text) = record.get("text").and_then(Value::as_str) else {
                skipped.malformed += 1;
                continue;
            };
            let mut parts = vec![Part::Known(KnownPart::Text {
                text: text.to_string(),
                extra: None,
            })];
            if let Some(thinking) = record.get("thinking").and_then(Value::as_str) {
                parts.push(Part::Known(KnownPart::Thinking {
                    text: Some(thinking.to_string()),
                    signature: None,
                    encrypted: None,
                    extra: None,
                }));
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
}
