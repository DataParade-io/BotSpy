//! The Codex importer.
//!
//! Watches rollout JSONL event logs under `~/.codex/sessions/YYYY/MM/DD/`,
//! indexed by `state_5.sqlite` — the `threads` table (with the thread
//! metadata, and no parent column) plus the `thread_spawn_edges` edge
//! table — and `thread_history_1.sqlite` (turn and item projections with
//! rollout byte offsets). The index is a live database: it is read
//! through a snapshot copy, never in place, and a schema mismatch
//! surfaces as an issue instead of a silently empty result. The thread
//! metadata (title, model, git branch and origin, cwd, created/updated
//! instants) maps into `SessionMetadata` (R11). Every rollout
//! record carries an explicit `ordinal` — the authoritative order, so
//! extraction sorts by ordinal instead of file order. `response_item`
//! records become messages (user/assistant/developer roles, reasoning
//! summaries, tool calls with outputs); `token_usage_record` parks the
//! token counts in session usage; `compacted` records chain compaction
//! windows; `task_complete` records turn timing. Rollouts are appended
//! live: extraction resumes from the stored byte offset, partial trailing
//! lines are retried on the next read, and malformed or unknown records
//! are skipped with counters. The source tree is only ever read.

use crate::importer::SkipCounter;
use crate::schema::{
    Agent, CompactionWindow, KnownPart, Message, Origin, Part, Provenance, Role, ToolArguments,
    Usage,
};
use crate::session::SessionSummary;
use crate::snapshot::SnapshotConnection;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// One row of the threads index (`state_5.sqlite`). The real table has no
/// parent column: spawn edges live in `thread_spawn_edges`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ThreadRow {
    id: String,
    rollout_path: String,
    cwd: String,
    title: Option<String>,
    model: Option<String>,
    git_branch: Option<String>,
    git_origin_url: Option<String>,
    git_sha: Option<String>,
    /// Creation instant in epoch milliseconds (from `created_at_ms`, or
    /// `created_at` seconds scaled up).
    created_at_ms: Option<i64>,
    /// Last-update instant in epoch milliseconds (same fallback rule).
    updated_at_ms: Option<i64>,
    archived: Option<i64>,
}

impl ThreadRow {
    /// The R11 metadata the threads index carries.
    fn metadata(&self) -> crate::schema::SessionMetadata {
        let models: Vec<String> = self
            .model
            .clone()
            .filter(|model| !model.is_empty())
            .into_iter()
            .collect();
        crate::schema::SessionMetadata {
            title: self.title.clone().filter(|title| !title.is_empty()),
            git_branch: self.git_branch.clone(),
            git_commit: self.git_sha.clone(),
            git_origin_url: self.git_origin_url.clone(),
            cwd: Some(self.cwd.clone()).filter(|cwd| !cwd.is_empty()),
            archived: self.archived.map(|flag| flag != 0),
            models,
            ..crate::schema::SessionMetadata::default()
        }
    }

    fn started_at(&self) -> String {
        self.created_at_ms
            .filter(|ms| *ms > 0)
            .map(rfc3339_utc)
            .unwrap_or_default()
    }

    fn last_activity_at(&self) -> String {
        self.updated_at_ms
            .filter(|ms| *ms > 0)
            .map(rfc3339_utc)
            .unwrap_or_default()
    }
}

/// Format an epoch-milliseconds instant as RFC3339 UTC (second
/// precision). Non-positive instants map to the empty timestamp.
fn rfc3339_utc(epoch_ms: i64) -> String {
    if epoch_ms <= 0 {
        return String::new();
    }
    let secs = epoch_ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    // Inverse of Howard Hinnant's days_from_civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60
    )
}

/// What discovery saw in the threads index.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodexDiscovery {
    /// Threads whose rollout file exists.
    pub threads: usize,
    /// Threads whose rollout file is missing, skipped by discovery.
    pub missing_rollouts: usize,
    /// Rollout paths that are missing (doctor detail).
    pub missing_rollout_paths: Vec<String>,
    /// Spawn edges: child thread id -> parent thread id.
    pub parent_links: BTreeMap<String, String>,
    /// Read failures: schema drift, unreadable index, and similar.
    /// Discovery never swallows a broken threads index silently.
    pub issues: Vec<String>,
}

/// Diagnostics over a Codex home root.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct CodexReport {
    pub root: PathBuf,
    pub threads: usize,
    pub missing_rollouts: usize,
    pub issues: Vec<String>,
}

/// The result of one extraction pass over a rollout.
#[derive(Debug, Clone, PartialEq)]
pub struct CodexExtraction {
    pub session: crate::session::Session,
    /// Messages normalized by this pass.
    pub good: usize,
    /// Records skipped as malformed, unknown, or held-back partials.
    pub skipped: SkipCounter,
    /// Partial trailing lines held back for the next read.
    pub pending_partial: usize,
}

/// The Codex source over one home root (e.g. `~/.codex`).
#[derive(Debug)]
pub struct CodexSource {
    root: PathBuf,
    /// Per-thread rollout byte offsets, so extraction resumes where it
    /// stopped (thread_history_1.sqlite stores these).
    offsets: Mutex<BTreeMap<String, u64>>,
}

impl CodexSource {
    /// A source over the Codex home root.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            offsets: Mutex::new(BTreeMap::new()),
        }
    }

    /// The home root this source watches.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Seed the stored rollout byte offset for one thread (as
    /// thread_history_1.sqlite would have recorded it).
    pub fn set_offset(&self, session_id: &str, offset: u64) {
        self.offsets
            .lock()
            .expect("offsets")
            .insert(session_id.to_string(), offset);
    }

    /// A read-only connection to a snapshot copy of the threads index.
    /// The live index is never read in place.
    fn snapshot_threads_index(&self) -> Result<SnapshotConnection, crate::snapshot::SnapshotError> {
        SnapshotConnection::open(&self.root.join("state_5.sqlite"))
    }

    /// The threads rows on an open snapshot connection. Any query failure
    /// (a missing column is schema drift) is returned, never swallowed.
    fn query_threads(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<ThreadRow>> {
        let mut stmt = conn.prepare(
            "SELECT id, rollout_path, cwd, title, model, git_branch, git_origin_url, git_sha, \
             created_at_ms, updated_at_ms, created_at, updated_at, archived FROM threads",
        )?;
        let rows = stmt.query_map([], |row| {
            let created_ms: Option<i64> = row.get(8)?;
            let updated_ms: Option<i64> = row.get(9)?;
            let created_s: i64 = row.get(10)?;
            let updated_s: i64 = row.get(11)?;
            Ok(ThreadRow {
                id: row.get(0)?,
                rollout_path: row.get(1)?,
                cwd: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                title: row.get(3)?,
                model: row.get(4)?,
                git_branch: row.get(5)?,
                git_origin_url: row.get(6)?,
                git_sha: row.get(7)?,
                created_at_ms: Some(created_ms.unwrap_or(created_s * 1000)),
                updated_at_ms: Some(updated_ms.unwrap_or(updated_s * 1000)),
                archived: row.get(12)?,
            })
        })?;
        rows.collect()
    }

    /// The spawn edges (child -> parent) on an open snapshot connection.
    fn query_edges(conn: &rusqlite::Connection) -> rusqlite::Result<BTreeMap<String, String>> {
        let mut stmt =
            conn.prepare("SELECT child_thread_id, parent_thread_id FROM thread_spawn_edges")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect()
    }

    /// Scan the threads index: summaries, spawn edges, missing rollouts,
    /// and issues. A broken or drifted index is reported as an issue, not
    /// silently swallowed.
    fn scan(&self) -> (Vec<SessionSummary>, CodexDiscovery) {
        let mut summaries = Vec::new();
        let mut stats = CodexDiscovery::default();
        if !self.root.join("state_5.sqlite").is_file() {
            stats
                .issues
                .push("threads index state_5.sqlite is missing".to_string());
            return (summaries, stats);
        }
        let conn = match self.snapshot_threads_index() {
            Ok(conn) => conn,
            Err(err) => {
                stats.issues.push(format!(
                    "threads index could not be read through a snapshot: {err}"
                ));
                return (summaries, stats);
            }
        };
        let threads = match Self::query_threads(&conn) {
            Ok(threads) => threads,
            Err(err) => {
                stats
                    .issues
                    .push(format!("threads index could not be read: {err}"));
                return (summaries, stats);
            }
        };
        let edges = match Self::query_edges(&conn) {
            Ok(edges) => edges,
            Err(err) => {
                stats
                    .issues
                    .push(format!("spawn edge table could not be read: {err}"));
                BTreeMap::new()
            }
        };
        for thread in threads {
            let path = self.resolve_rollout(&thread.rollout_path);
            if !path.is_file() {
                stats.missing_rollouts += 1;
                stats
                    .missing_rollout_paths
                    .push(thread.rollout_path.clone());
                continue;
            }
            if let Some(parent) = edges.get(&thread.id) {
                stats.parent_links.insert(thread.id.clone(), parent.clone());
            }
            summaries.push(SessionSummary {
                id: thread.id.clone(),
                agent: Agent::Codex,
                project_id: thread.cwd.clone(),
                started_at: thread.started_at(),
                last_activity_at: thread.last_activity_at(),
                metadata: thread.metadata(),
                ..SessionSummary::default()
            });
            stats.threads += 1;
        }
        summaries.sort_by(|a, b| a.id.cmp(&b.id));
        (summaries, stats)
    }

    /// `rollout_path` may be absolute or relative to the home root; both
    /// are resolved explicitly.
    fn resolve_rollout(&self, rollout_path: &str) -> PathBuf {
        let path = Path::new(rollout_path);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        }
    }

    /// Discover sessions via the threads index. Threads whose rollout file
    /// is missing are skipped and counted; spawn edges are reported as
    /// parent links; read failures surface as issues.
    pub fn discover(&self) -> (Vec<SessionSummary>, CodexDiscovery) {
        self.scan()
    }

    /// The doctor report: threads, missing rollouts, and issues.
    pub fn doctor(&self) -> CodexReport {
        let (_, stats) = self.scan();
        let mut report = CodexReport {
            root: self.root.clone(),
            threads: stats.threads,
            missing_rollouts: stats.missing_rollouts,
            issues: stats.issues,
        };
        for path in &stats.missing_rollout_paths {
            report.issues.push(format!("rollout {path} is missing"));
        }
        report
    }

    /// The thread row for one session id, with its spawn-edge parent.
    fn thread_row(&self, session_id: &str) -> Option<(ThreadRow, Option<String>)> {
        if !self.root.join("state_5.sqlite").is_file() {
            return None;
        }
        let conn = self.snapshot_threads_index().ok()?;
        let threads = Self::query_threads(&conn).ok()?;
        // A missing or drifted edge table must not hide the rollout:
        // discovery and the doctor report the drift instead.
        let parent = Self::query_edges(&conn)
            .ok()
            .and_then(|edges| edges.get(session_id).cloned());
        threads
            .into_iter()
            .find(|thread| thread.id == session_id)
            .map(|thread| (thread, parent))
    }

    /// Extract one rollout, resuming from the stored byte offset. Records
    /// are ordered by their explicit `ordinal`, not by file order. Returns
    /// `None` when no thread with that id exists.
    pub fn extract(&self, session_id: &str) -> Option<CodexExtraction> {
        let (row, parent) = self.thread_row(session_id)?;
        let path = self.resolve_rollout(&row.rollout_path);
        let mut file = File::open(&path).ok()?;
        let offset = *self
            .offsets
            .lock()
            .expect("offsets")
            .get(session_id)
            .unwrap_or(&0);
        file.seek(SeekFrom::Start(offset)).ok()?;
        let mut text = String::new();
        file.read_to_string(&mut text).ok()?;

        let mut session = crate::session::Session {
            id: session_id.to_string(),
            agent: Agent::Codex,
            project_id: row.cwd.clone(),
            metadata: row.metadata(),
            parent_id: parent,
            ..crate::session::Session::default()
        };
        let mut good = 0;
        let mut skipped = SkipCounter::default();
        let mut pending_partial = 0;
        let mut consumed = 0usize;
        let mut records: Vec<(Value, u64)> = Vec::new();

        let mut lines = text.split('\n').peekable();
        while let Some(line) = lines.next() {
            let is_last = lines.peek().is_none();
            if is_last && line.is_empty() {
                // The text ended with a newline: nothing left to consume.
                break;
            }
            if is_last && !line.is_empty() {
                // A trailing line without its newline: hold it back for
                // the next read; the offset stays at its start.
                pending_partial += 1;
                skipped.partial += 1;
                break;
            }
            consumed += line.len() + 1;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let line_no = records.len() as u64 + 1;
            match serde_json::from_str::<Value>(trimmed) {
                Ok(record) if record.get("ordinal").and_then(Value::as_u64).is_some() => {
                    records.push((record, line_no));
                }
                Ok(_) => skipped.malformed += 1,
                Err(_) => skipped.malformed += 1,
            }
        }

        // The explicit ordinal is the authoritative order.
        records.sort_by_key(|(record, _)| {
            record
                .get("ordinal")
                .and_then(Value::as_u64)
                .unwrap_or(u64::MAX)
        });

        for (record, line_no) in records {
            let ty = record.get("type").and_then(Value::as_str).unwrap_or("");
            match ty {
                "response_item" => {
                    if let Some(message) =
                        normalize_response_item(&record, &path, line_no, &mut session)
                    {
                        session.messages.push(message);
                        good += 1;
                    }
                }
                "token_usage_record" => apply_usage(&mut session, record.get("payload")),
                "compacted" => apply_compaction(&mut session, record.get("payload")),
                "task_complete" => apply_task_complete(&mut session, record.get("payload")),
                _ => skipped.unknown += 1,
            }
        }

        // The threads index carries the session bounds; record timestamps
        // win when present, index instants fill the gaps.
        if session.started_at.is_empty() {
            session.started_at = row.started_at();
        }
        if session.last_activity_at.is_empty() {
            session.last_activity_at = row.last_activity_at();
        }

        self.offsets
            .lock()
            .expect("offsets")
            .insert(session_id.to_string(), offset + consumed as u64);
        Some(CodexExtraction {
            session,
            good,
            skipped,
            pending_partial,
        })
    }
}

fn normalize_response_item(
    record: &Value,
    source_file: &Path,
    line_no: u64,
    session: &mut crate::session::Session,
) -> Option<Message> {
    let payload = record.get("payload")?;
    let payload_ty = payload.get("type").and_then(Value::as_str)?;
    let (role, parts, origin) = match payload_ty {
        "message" => {
            let role = payload.get("role").and_then(Value::as_str)?;
            let (role, origin) = match role {
                "user" => (Role::User, None),
                "assistant" => (Role::Assistant, None),
                "developer" => (Role::System, Some(Origin::Developer)),
                "system" => (Role::System, None),
                _ => return None,
            };
            (role, message_parts(payload.get("content")?), origin)
        }
        "reasoning" => {
            let texts: Vec<&str> = payload
                .get("summary")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.get("text").and_then(Value::as_str))
                        .collect()
                })
                .unwrap_or_default();
            if texts.is_empty() {
                return None;
            }
            (
                Role::Assistant,
                Some(vec![Part::Known(KnownPart::Thinking {
                    text: Some(texts.join("\n")),
                    signature: None,
                    encrypted: None,
                    extra: None,
                })]),
                None,
            )
        }
        "function_call" => {
            let call_id = payload
                .get("call_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let name = payload
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let arguments = payload
                .get("arguments")
                .and_then(Value::as_str)
                .and_then(|arguments| serde_json::from_str::<Value>(arguments).ok())
                .map(ToolArguments::from_value);
            (
                Role::Assistant,
                Some(vec![Part::Known(KnownPart::ToolCall {
                    id: call_id.to_string(),
                    name: name.to_string(),
                    arguments,
                    status: None,
                    extra: None,
                })]),
                None,
            )
        }
        "function_call_output" => {
            let call_id = payload
                .get("call_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let text = match payload.get("output") {
                Some(Value::String(text)) => Some(text.clone()),
                Some(value) => Some(value.to_string()),
                None => None,
            };
            (
                Role::Tool,
                Some(vec![Part::Known(KnownPart::ToolResult {
                    call_id: call_id.to_string(),
                    text,
                    status: None,
                    extra: None,
                })]),
                None,
            )
        }
        _ => return None,
    };
    let parts = parts?;
    if parts.is_empty() {
        return None;
    }
    let mut message = Message {
        role,
        parts,
        origin,
        ..Message::default()
    };
    message.timestamp = record
        .get("timestamp")
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Some(timestamp) = &message.timestamp {
        if session.started_at.is_empty() {
            session.started_at = timestamp.clone();
        }
        session.last_activity_at = timestamp.clone();
    }
    message.provenance = Some(Provenance {
        source_file: source_file.display().to_string(),
        line: Some(line_no),
        record_id: payload
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string),
        record_type: Some(payload_ty.to_string()),
        ordinal: record.get("ordinal").and_then(Value::as_u64),
        ..Provenance::default()
    });
    Some(message)
}

fn message_parts(content: &Value) -> Option<Vec<Part>> {
    match content {
        Value::String(text) => Some(vec![text_part(text)]),
        Value::Array(items) => {
            let mut parts = Vec::new();
            for item in items {
                let item_ty = item.get("type").and_then(Value::as_str).unwrap_or("");
                if matches!(item_ty, "input_text" | "output_text" | "text") {
                    if let Some(text) = item.get("text").and_then(Value::as_str) {
                        parts.push(text_part(text));
                    }
                }
            }
            Some(parts)
        }
        _ => None,
    }
}

fn text_part(text: &str) -> Part {
    Part::Known(KnownPart::Text {
        text: text.to_string(),
        extra: None,
    })
}

fn apply_usage(session: &mut crate::session::Session, payload: Option<&Value>) {
    let payload = match payload {
        Some(payload) => payload,
        None => return,
    };
    session.usage = Some(Usage {
        input_tokens: payload.get("input_tokens").and_then(Value::as_u64),
        output_tokens: payload.get("output_tokens").and_then(Value::as_u64),
        cached_input_tokens: payload.get("cached_input_tokens").and_then(Value::as_u64),
        reasoning_tokens: payload.get("reasoning_tokens").and_then(Value::as_u64),
        model: payload
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string),
        ..Usage::default()
    });
}

fn apply_compaction(session: &mut crate::session::Session, payload: Option<&Value>) {
    let Some(payload) = payload else { return };
    let window_id = payload
        .get("window_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    session.compaction_windows.insert(
        window_id,
        CompactionWindow {
            previous_window: payload
                .get("previous_window")
                .and_then(Value::as_str)
                .map(str::to_string),
            retained_context: payload
                .get("retained_context")
                .and_then(Value::as_str)
                .map(str::to_string),
        },
    );
}

fn apply_task_complete(session: &mut crate::session::Session, payload: Option<&Value>) {
    let Some(payload) = payload else { return };
    let turn_id = payload
        .get("turn_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| "turn".to_string());
    session.turns.insert(
        turn_id.clone(),
        crate::schema::Turn {
            id: turn_id,
            duration_ms: payload.get("duration_ms").and_then(Value::as_u64),
            time_to_first_token_ms: payload
                .get("time_to_first_token_ms")
                .and_then(Value::as_u64),
            ..crate::schema::Turn::default()
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{KnownPart, Part, Role};

    fn write_rollout(root: &Path, path: &str, lines: &[String]) {
        let full = root.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(
            full,
            lines
                .iter()
                .map(|line| format!("{line}\n"))
                .collect::<String>(),
        )
        .unwrap();
    }

    fn write_threads_index(root: &Path, rows: &[(&str, &str, &str)]) {
        let conn = rusqlite::Connection::open(root.join("state_5.sqlite")).unwrap();
        conn.execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                rollout_path TEXT NOT NULL,
                cwd TEXT NOT NULL,
                title TEXT NOT NULL DEFAULT '',
                source TEXT NOT NULL DEFAULT '',
                model_provider TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0,
                created_at_ms INTEGER,
                updated_at_ms INTEGER,
                archived INTEGER NOT NULL DEFAULT 0,
                git_sha TEXT,
                git_branch TEXT,
                git_origin_url TEXT,
                model TEXT
            );
            CREATE TABLE thread_spawn_edges (
                parent_thread_id TEXT NOT NULL,
                child_thread_id TEXT NOT NULL PRIMARY KEY,
                status TEXT NOT NULL
            );",
        )
        .unwrap();
        for (id, rollout_path, cwd) in rows {
            conn.execute(
                "INSERT INTO threads (id, rollout_path, cwd) VALUES (?1, ?2, ?3)",
                rusqlite::params![id, rollout_path, cwd],
            )
            .unwrap();
        }
    }

    fn write_spawn_edge(root: &Path, parent: &str, child: &str) {
        let conn = rusqlite::Connection::open(root.join("state_5.sqlite")).unwrap();
        conn.execute(
            "INSERT INTO thread_spawn_edges (parent_thread_id, child_thread_id, status) \
             VALUES (?1, ?2, 'open')",
            rusqlite::params![parent, child],
        )
        .unwrap();
    }

    fn set_thread_metadata(
        root: &Path,
        id: &str,
        metadata: (&str, &str, &str, &str),
        created_ms: i64,
        updated_ms: i64,
    ) {
        let (title, model, branch, origin) = metadata;
        let conn = rusqlite::Connection::open(root.join("state_5.sqlite")).unwrap();
        conn.execute(
            "UPDATE threads SET title = ?2, model = ?3, git_branch = ?4, git_origin_url = ?5, \
             created_at = ?6 / 1000, updated_at = ?7 / 1000, created_at_ms = ?6, updated_at_ms = ?7 \
             WHERE id = ?1",
            rusqlite::params![id, title, model, branch, origin, created_ms, updated_ms],
        )
        .unwrap();
    }

    fn user_record(ordinal: u64, timestamp: &str, text: &str) -> String {
        format!(
            r#"{{"ordinal":{ordinal},"timestamp":"{timestamp}","type":"response_item","payload":{{"type":"message","id":"m{ordinal}","role":"user","content":[{{"type":"input_text","text":"{text}"}}]}}}}"#
        )
    }

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("botspy-cx-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn discovery_orders_threads_and_reports_spawn_edges() {
        let root = temp_root("discover");
        write_threads_index(
            &root,
            &[
                ("t2", "sessions/2026/10/01/rollout-t2.jsonl", "/repo/demo"),
                ("t1", "sessions/2026/10/01/rollout-t1.jsonl", "/repo/demo"),
            ],
        );
        write_spawn_edge(&root, "t1", "t2");
        write_rollout(
            &root,
            "sessions/2026/10/01/rollout-t1.jsonl",
            &[user_record(1, "2026-10-01T09:00:00Z", "hi")],
        );
        write_rollout(
            &root,
            "sessions/2026/10/01/rollout-t2.jsonl",
            &[user_record(1, "2026-10-01T09:00:05Z", "sub")],
        );
        write_rollout(
            &root,
            "sessions/2026/10/01/rollout-t3.jsonl",
            &[user_record(1, "2026-10-01T09:00:10Z", "orphan")],
        );
        let source = CodexSource::new(&root);
        let (summaries, stats) = source.discover();
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].id, "t1");
        assert_eq!(summaries[0].project_id, "/repo/demo");
        assert_eq!(stats.threads, 2);
        assert_eq!(stats.parent_links.get("t2").map(String::as_str), Some("t1"));
    }

    #[test]
    fn doctor_flags_missing_rollouts() {
        let root = temp_root("doctor");
        write_threads_index(
            &root,
            &[("t1", "sessions/2026/10/01/rollout-t1.jsonl", "/repo")],
        );
        let source = CodexSource::new(&root);
        let report = source.doctor();
        assert_eq!(report.threads, 0);
        assert_eq!(report.missing_rollouts, 1);
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.contains("rollout-t1.jsonl")));
    }

    #[test]
    fn ordinal_is_the_authoritative_order() {
        let root = temp_root("ordinal");
        write_threads_index(
            &root,
            &[("t1", "sessions/2026/10/01/rollout-t1.jsonl", "/repo")],
        );
        write_rollout(
            &root,
            "sessions/2026/10/01/rollout-t1.jsonl",
            &[
                user_record(3, "2026-10-01T09:00:15Z", "third"),
                user_record(1, "2026-10-01T09:00:05Z", "first"),
                user_record(2, "2026-10-01T09:00:10Z", "second"),
            ],
        );
        let source = CodexSource::new(&root);
        let extraction = source.extract("t1").expect("extract");
        let ordinals: Vec<u64> = extraction
            .session
            .messages
            .iter()
            .map(|message| {
                message
                    .provenance
                    .as_ref()
                    .and_then(|prov| prov.ordinal)
                    .expect("ordinal")
            })
            .collect();
        assert_eq!(ordinals, vec![1, 2, 3]);
        assert_eq!(extraction.good, 3);
    }

    #[test]
    fn extraction_resumes_from_the_stored_offset() {
        let root = temp_root("offset");
        write_threads_index(
            &root,
            &[("t1", "sessions/2026/10/01/rollout-t1.jsonl", "/repo")],
        );
        write_rollout(
            &root,
            "sessions/2026/10/01/rollout-t1.jsonl",
            &[
                user_record(1, "2026-10-01T09:00:05Z", "first"),
                user_record(2, "2026-10-01T09:00:10Z", "second"),
                user_record(3, "2026-10-01T09:00:15Z", "third"),
                user_record(4, "2026-10-01T09:00:20Z", "fourth"),
            ],
        );
        let path = root.join("sessions/2026/10/01/rollout-t1.jsonl");
        let content = std::fs::read_to_string(&path).unwrap();
        let after_two: usize = content.lines().take(2).map(|line| line.len() + 1).sum();
        let source = CodexSource::new(&root);
        source.set_offset("t1", after_two as u64);
        let resumed = source.extract("t1").expect("extract from stored offset");
        assert_eq!(resumed.good, 2);
        let ordinals: Vec<u64> = resumed
            .session
            .messages
            .iter()
            .map(|message| {
                message
                    .provenance
                    .as_ref()
                    .and_then(|prov| prov.ordinal)
                    .expect("ordinal")
            })
            .collect();
        assert_eq!(ordinals, vec![3, 4]);
    }

    #[test]
    fn response_items_map_roles_reasoning_and_tool_kinds() {
        let root = temp_root("parts");
        write_threads_index(
            &root,
            &[("t1", "sessions/2026/10/01/rollout-t1.jsonl", "/repo")],
        );
        write_rollout(
            &root,
            "sessions/2026/10/01/rollout-t1.jsonl",
            &[
                user_record(1, "2026-10-01T09:00:00Z", "run the tests"),
                r#"{"ordinal":2,"timestamp":"2026-10-01T09:00:05Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"On it."}]}}"#.to_string(),
                r#"{"ordinal":3,"timestamp":"2026-10-01T09:00:06Z","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"Be concise."}]}}"#.to_string(),
                r#"{"ordinal":4,"timestamp":"2026-10-01T09:00:07Z","type":"response_item","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"thinking it through"}]}}"#.to_string(),
                r#"{"ordinal":5,"timestamp":"2026-10-01T09:00:08Z","type":"response_item","payload":{"type":"function_call","call_id":"call_1","name":"shell","arguments":"{\"cmd\":[\"cargo\",\"test\"]}"}}"#.to_string(),
                r#"{"ordinal":6,"timestamp":"2026-10-01T09:00:20Z","type":"response_item","payload":{"type":"function_call_output","call_id":"call_1","output":"all tests pass"}}"#.to_string(),
            ],
        );
        let source = CodexSource::new(&root);
        let extraction = source.extract("t1").expect("extract");
        let messages = &extraction.session.messages;
        assert_eq!(messages.len(), 6);
        assert_eq!(messages[0].role, Role::User);
        assert_eq!(messages[1].role, Role::Assistant);
        assert_eq!(messages[2].role, Role::System);
        assert_eq!(messages[2].origin, Some(Origin::Developer));
        assert!(matches!(
            messages[3].parts.first(),
            Some(Part::Known(KnownPart::Thinking { .. }))
        ));
        assert!(matches!(
            messages[4].parts.first(),
            Some(Part::Known(KnownPart::ToolCall { .. }))
        ));
        assert!(matches!(
            messages[5].parts.first(),
            Some(Part::Known(KnownPart::ToolResult { .. }))
        ));
        assert_eq!(messages[5].role, Role::Tool);
    }

    #[test]
    fn usage_compaction_and_task_complete_map_to_session_state() {
        let root = temp_root("aux");
        write_threads_index(
            &root,
            &[("t1", "sessions/2026/10/01/rollout-t1.jsonl", "/repo")],
        );
        write_rollout(
            &root,
            "sessions/2026/10/01/rollout-t1.jsonl",
            &[
                r#"{"ordinal":1,"type":"token_usage_record","payload":{"input_tokens":1200,"cached_input_tokens":300,"output_tokens":800,"reasoning_tokens":150,"model":"gpt-5-codex"}}"#.to_string(),
                r#"{"ordinal":2,"type":"compacted","payload":{"window_id":"w2","previous_window":"w1","retained_context":"summary so far"}}"#.to_string(),
                r#"{"ordinal":3,"type":"task_complete","payload":{"turn_id":"turn-1","duration_ms":14300,"time_to_first_token_ms":850}}"#.to_string(),
            ],
        );
        let source = CodexSource::new(&root);
        let extraction = source.extract("t1").expect("extract");
        let session = extraction.session;
        let usage = session.usage.as_ref().expect("session usage");
        assert_eq!(usage.input_tokens, Some(1200));
        assert_eq!(usage.cached_input_tokens, Some(300));
        assert_eq!(usage.output_tokens, Some(800));
        assert_eq!(usage.reasoning_tokens, Some(150));
        assert_eq!(usage.model.as_deref(), Some("gpt-5-codex"));
        let window = session.compaction_windows.get("w2").expect("window");
        assert_eq!(window.previous_window.as_deref(), Some("w1"));
        assert_eq!(window.retained_context.as_deref(), Some("summary so far"));
        let turn = session.turns.get("turn-1").expect("turn");
        assert_eq!(turn.duration_ms, Some(14300));
        assert_eq!(turn.time_to_first_token_ms, Some(850));
    }

    #[test]
    fn partial_trailing_and_malformed_are_skipped_not_normalized() {
        let root = temp_root("partial");
        write_threads_index(
            &root,
            &[("t1", "sessions/2026/10/01/rollout-t1.jsonl", "/repo")],
        );
        let full = root.join("sessions/2026/10/01/rollout-t1.jsonl");
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(
            &full,
            format!(
                "this line is {{ not json\n{}",
                r#"{"ordinal":2,"type":"response_item","payload":{"type":"message","role":"user","content":"cut off"#
            ),
        )
        .unwrap();
        let source = CodexSource::new(&root);
        let extraction = source.extract("t1").expect("extract");
        assert_eq!(extraction.good, 0);
        assert_eq!(extraction.skipped.malformed, 1);
        assert_eq!(extraction.skipped.partial, 1);
        assert_eq!(extraction.skipped.total(), 2);
        assert_eq!(extraction.pending_partial, 1);

        // Completing the append recovers the held-back record.
        std::fs::write(
            &full,
            format!(
                "this line is {{ not json\n{}\n",
                r#"{"ordinal":2,"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"complete now"}]}}"#
            ),
        )
        .unwrap();
        let recovered = source.extract("t1").expect("extract again");
        assert_eq!(recovered.good, 1);
        assert_eq!(recovered.pending_partial, 0);
    }

    #[test]
    fn rfc3339_utc_formats_epoch_milliseconds() {
        assert_eq!(rfc3339_utc(0), "");
        assert_eq!(rfc3339_utc(-5), "");
        assert_eq!(rfc3339_utc(1767225600000), "2026-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(1767229200000), "2026-01-01T01:00:00Z");
        assert_eq!(rfc3339_utc(1704067200000), "2024-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(1735689599999), "2024-12-31T23:59:59Z");
    }

    #[test]
    fn thread_metadata_maps_into_summaries_and_extractions() {
        let root = temp_root("metadata");
        write_threads_index(
            &root,
            &[("t1", "sessions/2026/10/01/rollout-t1.jsonl", "/repo/demo")],
        );
        set_thread_metadata(
            &root,
            "t1",
            (
                "Alpha build fix",
                "synthetic-model",
                "feat/alpha",
                "https://git.example.com/acme/alpha.git",
            ),
            1767225600000,
            1767229200000,
        );
        write_rollout(
            &root,
            "sessions/2026/10/01/rollout-t1.jsonl",
            &[user_record(1, "2026-10-01T09:00:00Z", "hi")],
        );
        let source = CodexSource::new(&root);
        let (summaries, _) = source.discover();
        assert_eq!(summaries.len(), 1);
        let summary = &summaries[0];
        assert_eq!(summary.metadata.title.as_deref(), Some("Alpha build fix"));
        assert_eq!(summary.metadata.models, vec!["synthetic-model".to_string()]);
        assert_eq!(summary.metadata.git_branch.as_deref(), Some("feat/alpha"));
        assert_eq!(
            summary.metadata.git_origin_url.as_deref(),
            Some("https://git.example.com/acme/alpha.git")
        );
        assert_eq!(summary.metadata.cwd.as_deref(), Some("/repo/demo"));
        assert_eq!(summary.started_at, "2026-01-01T00:00:00Z");
        assert_eq!(summary.last_activity_at, "2026-01-01T01:00:00Z");

        let extraction = source.extract("t1").expect("extract");
        let session = &extraction.session;
        assert_eq!(session.metadata.title.as_deref(), Some("Alpha build fix"));
        assert_eq!(
            session.metadata.git_origin_url.as_deref(),
            Some("https://git.example.com/acme/alpha.git")
        );
        assert_eq!(session.project_id, "/repo/demo");
    }

    #[test]
    fn index_instants_fill_missing_record_timestamps() {
        let root = temp_root("instants");
        write_threads_index(
            &root,
            &[("t1", "sessions/2026/10/01/rollout-t1.jsonl", "/repo/demo")],
        );
        set_thread_metadata(&root, "t1", ("", "", "", ""), 1767225600000, 1767229200000);
        write_rollout(
            &root,
            "sessions/2026/10/01/rollout-t1.jsonl",
            &[r#"{"ordinal":1,"type":"response_item","payload":{"type":"message","id":"m1","role":"user","content":[{"type":"input_text","text":"no timestamp here"}]}}"#.to_string()],
        );
        let source = CodexSource::new(&root);
        let extraction = source.extract("t1").expect("extract");
        assert_eq!(extraction.session.started_at, "2026-01-01T00:00:00Z");
        assert_eq!(extraction.session.last_activity_at, "2026-01-01T01:00:00Z");
    }

    #[test]
    fn absolute_rollout_paths_resolve_to_themselves() {
        let root = temp_root("absolute");
        write_threads_index(&root, &[("t1", "elsewhere/rollout-t1.jsonl", "/repo/demo")]);
        write_rollout(
            &root,
            "elsewhere/rollout-t1.jsonl",
            &[user_record(1, "2026-10-01T09:00:00Z", "absolute")],
        );
        let absolute = root
            .join("elsewhere/rollout-t1.jsonl")
            .display()
            .to_string();
        let conn = rusqlite::Connection::open(root.join("state_5.sqlite")).unwrap();
        conn.execute(
            "UPDATE threads SET rollout_path = ?1 WHERE id = 't1'",
            rusqlite::params![absolute],
        )
        .unwrap();
        drop(conn);
        let source = CodexSource::new(&root);
        let (summaries, stats) = source.discover();
        assert_eq!(summaries.len(), 1);
        assert_eq!(stats.threads, 1);
        assert_eq!(stats.missing_rollouts, 0);
    }

    #[test]
    fn legacy_schema_drift_is_reported_not_swallowed() {
        let root = temp_root("drift");
        let conn = rusqlite::Connection::open(root.join("state_5.sqlite")).unwrap();
        conn.execute(
            "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL, parent_id TEXT, cwd TEXT)",
            [],
        )
        .unwrap();
        drop(conn);
        write_rollout(
            &root,
            "sessions/2026/10/01/rollout-t1.jsonl",
            &[user_record(1, "2026-10-01T09:00:00Z", "hi")],
        );
        let source = CodexSource::new(&root);
        let (summaries, stats) = source.discover();
        assert!(summaries.is_empty(), "drifted index yielded sessions");
        assert!(
            stats
                .issues
                .iter()
                .any(|issue| issue.contains("threads index")),
            "drift was not reported: {:?}",
            stats.issues
        );
        let report = source.doctor();
        assert_eq!(report.threads, 0);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.contains("threads index")),
            "doctor did not report the drift: {:?}",
            report.issues
        );
    }

    #[test]
    fn missing_edge_table_is_reported_but_threads_still_index() {
        let root = temp_root("no-edges");
        write_threads_index(
            &root,
            &[("t1", "sessions/2026/10/01/rollout-t1.jsonl", "/repo/demo")],
        );
        let conn = rusqlite::Connection::open(root.join("state_5.sqlite")).unwrap();
        conn.execute("DROP TABLE thread_spawn_edges", []).unwrap();
        drop(conn);
        write_rollout(
            &root,
            "sessions/2026/10/01/rollout-t1.jsonl",
            &[user_record(1, "2026-10-01T09:00:00Z", "hi")],
        );
        let source = CodexSource::new(&root);
        let (summaries, stats) = source.discover();
        assert_eq!(summaries.len(), 1, "threads stopped indexing");
        assert!(
            stats
                .issues
                .iter()
                .any(|issue| issue.contains("spawn edge")),
            "missing edge table was not reported: {:?}",
            stats.issues
        );
        let extraction = source.extract("t1").expect("extract without edge table");
        assert_eq!(extraction.good, 1);
    }
}
