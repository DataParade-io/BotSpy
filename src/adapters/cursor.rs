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
use crate::schema::{
    Agent, KnownPart, Message, Origin, Part, Provenance, Role, SessionMetadata, SubagentInfo,
};
use crate::session::SessionSummary;
use crate::snapshot::SnapshotConnection;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
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
    /// The one snapshot copy of the store, kept while the store is unchanged.
    cache: Mutex<Option<CachedSnapshot>>,
    /// How many snapshot copies this source has taken.
    copies_taken: AtomicUsize,
    /// `workspaceId` to workspace folder, filled from `workspaceStorage` the
    /// first time each id is met. Only hits are kept: a folder never changes
    /// for an id, but a missing directory may appear later.
    workspaces: Mutex<BTreeMap<String, String>>,
}

/// The project every composer falls under when Cursor does not say which
/// workspace it belongs to. It is never derived from a title.
pub const NO_WORKSPACE_PROJECT: &str = "cursor-no-workspace";

/// The project of one composer: a stable, non-identifying id, and the
/// workspace folder when one is known.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ComposerProject {
    project_id: String,
    cwd: Option<String>,
}

/// What identifies the state of the store on disk: size and modification
/// time of the database and of its write-ahead log, when each exists.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StoreFingerprint {
    database: Option<(u64, std::time::SystemTime)>,
    wal: Option<(u64, std::time::SystemTime)>,
}

impl StoreFingerprint {
    fn of(store: &Path) -> Self {
        let mut wal = store.as_os_str().to_owned();
        wal.push("-wal");
        Self {
            database: Self::stat(store),
            // Opening the store read-only can itself create an empty WAL, so
            // an empty log means the same as no log: neither holds data.
            wal: Self::stat(Path::new(&wal)).filter(|(len, _)| *len > 0),
        }
    }

    fn stat(path: &Path) -> Option<(u64, std::time::SystemTime)> {
        let metadata = std::fs::metadata(path).ok()?;
        Some((metadata.len(), metadata.modified().ok()?))
    }
}

/// A snapshot copy and the store state it was taken from.
#[derive(Debug)]
struct CachedSnapshot {
    fingerprint: StoreFingerprint,
    connection: SnapshotConnection,
}

impl CursorSource {
    /// A source over the KV store file (e.g. `~/.cursor/state.vscdb`).
    pub fn new(store: impl Into<PathBuf>) -> Self {
        Self {
            store: store.into(),
            observed: Mutex::new(BTreeMap::new()),
            cache: Mutex::new(None),
            copies_taken: AtomicUsize::new(0),
            workspaces: Mutex::new(BTreeMap::new()),
        }
    }

    /// How many snapshot copies of the store this source has taken. Calls
    /// over an unchanged store share one copy, so this only grows when the
    /// store changes.
    pub fn snapshots_taken(&self) -> usize {
        self.copies_taken.load(Ordering::SeqCst)
    }

    /// The directory holding the current snapshot copy, if one exists. It
    /// is removed when the copy is replaced or the source is dropped.
    pub fn snapshot_directory(&self) -> Option<PathBuf> {
        let cache = self.cache.lock().expect("snapshot cache");
        cache
            .as_ref()
            .map(|cached| cached.connection.directory().to_path_buf())
    }

    /// The store this source watches.
    pub fn store(&self) -> &Path {
        &self.store
    }

    /// Run `read` over a read-only connection to a snapshot copy of the
    /// store; the hot WAL store is never read in place. The copy is kept
    /// and reused for as long as the store's fingerprint is unchanged. When
    /// it changes, the old copy is removed before a fresh one is taken, so
    /// at most one copy exists at a time.
    fn with_snapshot<R>(
        &self,
        read: impl FnOnce(&rusqlite::Connection) -> R,
    ) -> Result<R, crate::snapshot::SnapshotError> {
        let mut cache = self.cache.lock().expect("snapshot cache");
        let fingerprint = StoreFingerprint::of(&self.store);
        if cache
            .as_ref()
            .is_none_or(|cached| cached.fingerprint != fingerprint)
        {
            *cache = None;
            let connection = SnapshotConnection::open(&self.store)?;
            self.copies_taken.fetch_add(1, Ordering::SeqCst);
            *cache = Some(CachedSnapshot {
                fingerprint,
                connection,
            });
        }
        let cached = cache.as_ref().expect("snapshot cached");
        Ok(read(&cached.connection))
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
    /// `{composerId, lastUpdatedAt, title, workspaceId, isSubagent,
    /// subagentTypeName}`. The title is the conversation title: it must
    /// only ever reach `metadata.title`, never a project.
    fn composer_headers(kv: &rusqlite::Connection) -> Option<Vec<Value>> {
        if let Some(headers) =
            Self::read_json(kv, "composerHeaders").and_then(|value| value.as_array().cloned())
        {
            return Some(headers.iter().map(normalize_key_header).collect());
        }
        Self::table_headers(kv, None)
    }

    /// One composer's normalized header, from whichever layout holds it.
    fn composer_header(kv: &rusqlite::Connection, composer_id: &str) -> Option<Value> {
        if let Some(headers) =
            Self::read_json(kv, "composerHeaders").and_then(|value| value.as_array().cloned())
        {
            return headers.iter().map(normalize_key_header).find(|header| {
                header.get("composerId").and_then(Value::as_str) == Some(composer_id)
            });
        }
        Self::table_headers(kv, Some(composer_id))?
            .into_iter()
            .next()
    }

    /// Rows of the `composerHeaders` table. Columns that an older table
    /// lacks read as null, so one query serves every version.
    fn table_headers(kv: &rusqlite::Connection, only: Option<&str>) -> Option<Vec<Value>> {
        let columns: Vec<String> = kv
            .prepare("SELECT name FROM pragma_table_info('composerHeaders')")
            .ok()?
            .query_map([], |row| row.get::<_, String>(0))
            .ok()?
            .flatten()
            .collect();
        let has = |name: &str| columns.iter().any(|column| column == name);
        if !has("composerId") {
            return None;
        }
        let column = |name: &'static str| if has(name) { name } else { "NULL" };
        let updated = match (has("lastUpdatedAt"), has("createdAt")) {
            (true, true) => "COALESCE(lastUpdatedAt, createdAt)",
            (true, false) => "lastUpdatedAt",
            (false, true) => "createdAt",
            (false, false) => "NULL",
        };
        let sql = format!(
            "SELECT composerId, {updated}, {}, {}, {}, {} FROM composerHeaders{}",
            column("value"),
            column("workspaceId"),
            column("isSubagent"),
            column("subagentTypeName"),
            if only.is_some() {
                " WHERE composerId = ?1"
            } else {
                ""
            }
        );
        let mut stmt = kv.prepare(&sql).ok()?;
        let map_row = |row: &rusqlite::Row| {
            let value: Option<String> = row.get(2)?;
            let title = value
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
                "title": title,
                "workspaceId": row.get::<_, Option<String>>(3).ok().flatten(),
                "isSubagent": row.get::<_, Option<i64>>(4).ok().flatten().is_some_and(|flag| flag != 0),
                "subagentTypeName": row.get::<_, Option<String>>(5).ok().flatten(),
            }))
        };
        let rows = match only {
            Some(id) => stmt.query_map([id], map_row).ok()?.flatten().collect(),
            None => stmt.query_map([], map_row).ok()?.flatten().collect(),
        };
        Some(rows)
    }

    /// The `User/workspaceStorage` directory beside this store, when the
    /// store sits in a `globalStorage` directory. The store path may be a
    /// symlink, so the real location decides.
    fn workspace_storage(&self) -> Option<PathBuf> {
        let real = std::fs::canonicalize(&self.store).unwrap_or_else(|_| self.store.clone());
        let global = real.parent()?;
        if global.file_name()? != "globalStorage" {
            return None;
        }
        Some(global.parent()?.join("workspaceStorage"))
    }

    /// The project of a composer, from its header. Only the workspace
    /// decides; a composer with no usable workspace falls under
    /// [`NO_WORKSPACE_PROJECT`].
    fn project_of(&self, header: Option<&Value>) -> ComposerProject {
        let neutral = ComposerProject {
            project_id: NO_WORKSPACE_PROJECT.to_string(),
            cwd: None,
        };
        let Some(id) = header
            .and_then(|header| header.get("workspaceId"))
            .and_then(Value::as_str)
            .filter(|id| is_workspace_id(id))
        else {
            return neutral;
        };
        let mut known = self.workspaces.lock().expect("workspaces");
        if let Some(folder) = known.get(id) {
            return ComposerProject {
                project_id: id.to_string(),
                cwd: Some(folder.clone()),
            };
        }
        let folder = self
            .workspace_storage()
            .map(|dir| dir.join(id).join("workspace.json"))
            .and_then(|file| std::fs::read_to_string(file).ok())
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .and_then(|json| workspace_folder(&json));
        match folder {
            Some(folder) => {
                known.insert(id.to_string(), folder.clone());
                ComposerProject {
                    project_id: id.to_string(),
                    cwd: Some(folder),
                }
            }
            None => neutral,
        }
    }

    /// Discover composers from the headers table, reporting the ones whose
    /// `lastUpdatedAt` changed since the previous discovery.
    pub fn discover(&self) -> (Vec<SessionSummary>, CursorDiscovery) {
        self.with_snapshot(|kv| self.discover_in(kv))
            .unwrap_or_else(|_| (Vec::new(), CursorDiscovery::default()))
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
            let project = self.project_of(Some(&composer));
            summaries.push(SessionSummary {
                id,
                agent: Agent::Cursor,
                project_id: project.project_id,
                metadata: SessionMetadata {
                    title: header_title(&composer),
                    cwd: project.cwd,
                    ..SessionMetadata::default()
                },
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
        let read = self.with_snapshot(|kv| {
            let composers = self.discover_in(kv).0.len();
            let bubbles = kv
                .query_row(
                    "SELECT COUNT(*) FROM cursorDiskKV WHERE key LIKE 'bubbleId:%'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap_or(0) as usize;
            (composers, bubbles, Self::composer_headers(kv).is_none())
        });
        let Ok((composers, bubbles, headers_missing)) = read else {
            report
                .issues
                .push("KV store could not be snapshotted".to_string());
            return report;
        };
        report.composers = composers;
        report.bubbles = bubbles;
        if headers_missing {
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
        self.with_snapshot(|kv| self.extract_in(kv, composer_id))
            .ok()?
    }

    /// Extraction over an already-open snapshot.
    fn extract_in(&self, kv: &rusqlite::Connection, composer_id: &str) -> Option<CursorExtraction> {
        let composer_data = Self::read_json(kv, &format!("composerData:{composer_id}"))?;
        let headers = composer_data
            .get("fullConversationHeadersOnly")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let header = Self::composer_header(kv, composer_id);
        let project = self.project_of(header.as_ref());
        let title = composer_data
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .or_else(|| header.as_ref().and_then(header_title));
        let mut session = crate::session::Session {
            id: composer_id.to_string(),
            agent: Agent::Cursor,
            project_id: project.project_id,
            metadata: SessionMetadata {
                title,
                cwd: project.cwd,
                ..SessionMetadata::default()
            },
            ..crate::session::Session::default()
        };
        if header
            .as_ref()
            .and_then(|header| header.get("isSubagent"))
            .and_then(Value::as_bool)
            == Some(true)
        {
            let kind = header
                .as_ref()
                .and_then(|header| header.get("subagentTypeName"))
                .and_then(Value::as_str)
                .filter(|kind| !kind.is_empty())
                .unwrap_or("subagent");
            session.subagent = Some(SubagentInfo {
                kind: kind.to_string(),
                nesting_depth: None,
            });
        }
        let mut good = 0;
        let mut skipped = SkipCounter::default();
        for header in &headers {
            let bubble_id = header
                .get("bubbleId")
                .and_then(Value::as_str)
                .or_else(|| header.as_str())
                .unwrap_or_default();
            let key = format!("bubbleId:{composer_id}:{bubble_id}");
            let Some(bubble) = Self::read_json(kv, &key) else {
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

/// Header from the older `composerHeaders` key, in the normalized shape.
/// The conversation title (`name`) moves to `title` so nothing downstream
/// can mistake it for a project.
fn normalize_key_header(header: &Value) -> Value {
    serde_json::json!({
        "composerId": header.get("composerId").cloned().unwrap_or(Value::Null),
        "lastUpdatedAt": header.get("lastUpdatedAt").cloned().unwrap_or(Value::Null),
        "title": header.get("name").cloned().unwrap_or(Value::Null),
        "workspaceId": header.get("workspaceId").cloned().unwrap_or(Value::Null),
        "isSubagent": header.get("isSubagent").and_then(Value::as_bool).unwrap_or(false),
        "subagentTypeName": header.get("subagentTypeName").cloned().unwrap_or(Value::Null),
    })
}

fn header_title(header: &Value) -> Option<String> {
    header
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| !title.is_empty())
        .map(str::to_string)
}

/// Workspace ids are 32 hex characters (an md5 of the folder). Anything
/// else (`empty-window`, a path-like string) has no directory, and is never
/// joined onto a path.
fn is_workspace_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The folder a `workspace.json` names: its `folder` URI, or the directory
/// holding its `.code-workspace` file (the file's parent reads as the
/// project, where the file stem is often just "workspace"). Remote URIs,
/// Cursor's own untitled `workspace.json` files, and anything unreadable
/// give `None`.
fn workspace_folder(json: &Value) -> Option<String> {
    if let Some(folder) = json.get("folder").and_then(Value::as_str) {
        return file_uri_path(folder);
    }
    let file = file_uri_path(json.get("workspace").and_then(Value::as_str)?)?;
    let path = Path::new(&file);
    if !file.ends_with(".code-workspace") {
        return None;
    }
    let parent = path.parent()?.to_str()?;
    (!parent.is_empty()).then(|| parent.to_string())
}

/// The path of a `file://` URI, percent-decoded. Other schemes, and hosts
/// other than local, give `None`.
fn file_uri_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let path = rest.strip_prefix("localhost").unwrap_or(rest);
    if !path.starts_with('/') {
        return None;
    }
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' && at + 2 < bytes.len() {
            if let Some(byte) = path
                .get(at + 1..at + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
            {
                decoded.push(byte);
                at += 3;
                continue;
            }
        }
        decoded.push(bytes[at]);
        at += 1;
    }
    let mut text = String::from_utf8_lossy(&decoded).into_owned();
    // `file:///c%3A/dir` is a Windows drive path: drop the leading slash.
    let drive = text.as_bytes();
    if drive.len() >= 3 && drive[0] == b'/' && drive[1].is_ascii_alphabetic() && drive[2] == b':' {
        text.remove(0);
    }
    (text.len() > 1).then_some(text)
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
    // `isSimulatedMsg`: Cursor wrote this user bubble itself (a plan run,
    // an injected follow-up), the human did not type it.
    let simulated = role == Role::User
        && bubble
            .get("isSimulatedMsg")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    let mut message = Message {
        role,
        parts,
        origin: simulated.then_some(Origin::Simulated),
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
            let is_user = role != "assistant";
            let (parts, origin, extra) = if is_user {
                split_user_context(parts)
            } else {
                (parts, None, None)
            };
            session.messages.push(Message {
                role: if is_user { Role::User } else { Role::Assistant },
                parts,
                origin,
                extra,
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

/// What the human typed, when the CLI wrapped it with context: the text
/// between `<user_query>` and `</user_query>`, and the text around them.
fn user_query(text: &str) -> Option<(&str, String)> {
    let open = text.find("<user_query>")?;
    let start = open + "<user_query>".len();
    let end = text.rfind("</user_query>").filter(|end| *end >= start)?;
    let around = format!("{}{}", &text[..open], &text[end + "</user_query>".len()..]);
    Some((text[start..end].trim(), around.trim().to_string()))
}

/// Whether a CLI user text with no `<user_query>` is context the CLI
/// injected: nothing after a leading `<timestamp>` wrapper, or a tag-led
/// block (tool catalogs, command lists, sub-agent context). Plain text,
/// as the older flat transcripts hold, is the human's.
fn is_injected_cli_text(text: &str) -> bool {
    let mut rest = text.trim_start();
    if let Some(after) = rest.strip_prefix("<timestamp>") {
        rest = after
            .split_once("</timestamp>")
            .map_or("", |(_, tail)| tail);
    }
    let rest = rest.trim_start();
    rest.is_empty() || rest.starts_with('<')
}

/// Separate what the human typed from what the CLI added around it.
///
/// - A text with `<user_query>` is the human's: the text part becomes the
///   query alone, and the rest of the text is kept in `extra.context`.
/// - A text with no query that is only a timestamp or a tag-led block is
///   injected context.
/// - A message with no human text at all is marked [`Origin::Harness`].
fn split_user_context(parts: Vec<Part>) -> (Vec<Part>, Option<Origin>, Option<Value>) {
    let mut human = Vec::new();
    let mut context = Vec::new();
    let mut other = Vec::new();
    for part in parts {
        let Part::Known(KnownPart::Text { text, extra }) = &part else {
            other.push(part);
            continue;
        };
        if let Some((query, around)) = user_query(text) {
            if !around.is_empty() {
                context.push(Value::String(around));
            }
            human.push(Part::Known(KnownPart::Text {
                text: query.to_string(),
                extra: extra.clone(),
            }));
        } else if is_injected_cli_text(text) {
            context.push(Value::String(text.clone()));
        } else {
            human.push(part);
        }
    }
    if human.is_empty() {
        // Nothing the human said: keep the injected text as it came.
        let kept = context
            .into_iter()
            .filter_map(|value| match value {
                Value::String(text) => Some(Part::Known(KnownPart::Text { text, extra: None })),
                _ => None,
            })
            .chain(other)
            .collect();
        return (kept, Some(Origin::Harness), None);
    }
    human.extend(other);
    let extra = (!context.is_empty()).then(|| serde_json::json!({ "context": context }));
    (human, None, extra)
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
        // The workspace id "w" is not a real one, and the title is not a
        // project: both composers fall under the neutral project.
        assert_eq!(summaries[0].project_id, NO_WORKSPACE_PROJECT);
        assert_eq!(summaries[0].metadata.title.as_deref(), Some("named"));
        assert_eq!(stats.changed.len(), 2);
        assert_eq!(source.extract("composer-1").unwrap().good, 1);
        assert!(source.doctor().issues.is_empty());
    }

    const WS_A: &str = "0123456789abcdef0123456789abcdef";
    const WS_B: &str = "fedcba9876543210fedcba9876543210";

    /// A Cursor application-data layout in a tempdir: the store at
    /// `User/globalStorage/state.vscdb`, and one `workspace.json` per entry
    /// under `User/workspaceStorage/<id>/`.
    fn layout(tag: &str, workspaces: &[(&str, Value)]) -> (PathBuf, Connection) {
        let root = std::env::temp_dir().join(format!("botspy-curws-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let global = root.join("User/globalStorage");
        std::fs::create_dir_all(&global).unwrap();
        for (id, json) in workspaces {
            let dir = root.join("User/workspaceStorage").join(id);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("workspace.json"), json.to_string()).unwrap();
        }
        let store = global.join("state.vscdb");
        let conn = open_store(&store);
        conn.execute(
            "CREATE TABLE composerHeaders (composerId TEXT PRIMARY KEY, workspaceId TEXT, \
             createdAt INTEGER, lastUpdatedAt INTEGER, isArchived INTEGER, isSubagent INTEGER, \
             recency INTEGER, checkpointAt INTEGER, value TEXT, subagentTypeName TEXT)",
            [],
        )
        .unwrap();
        (store, conn)
    }

    fn add_composer(
        conn: &Connection,
        id: &str,
        workspace: Option<&str>,
        title: &str,
        subagent: Option<&str>,
    ) {
        conn.execute(
            "INSERT INTO composerHeaders (composerId, workspaceId, createdAt, lastUpdatedAt, \
             isSubagent, value, subagentTypeName) VALUES (?1, ?2, 1, 2, ?3, ?4, ?5)",
            rusqlite::params![
                id,
                workspace,
                subagent.is_some() as i64,
                json!({"composerId": id, "name": title}).to_string(),
                subagent.filter(|kind| !kind.is_empty())
            ],
        )
        .unwrap();
        put(
            conn,
            &format!("composerData:{id}"),
            &json!({"name": title, "fullConversationHeadersOnly": [{"bubbleId": "b1"}]}),
        );
        put(
            conn,
            &format!("bubbleId:{id}:b1"),
            &json!({"type": 1, "text": "hi"}),
        );
    }

    fn project_of_composer(source: &CursorSource, id: &str) -> (String, Option<String>) {
        let session = source.extract(id).unwrap().session;
        let summary = source
            .discover()
            .0
            .into_iter()
            .find(|summary| summary.id == id)
            .unwrap();
        assert_eq!(summary.project_id, session.project_id);
        assert_eq!(summary.metadata.cwd, session.metadata.cwd);
        (session.project_id, session.metadata.cwd)
    }

    #[test]
    fn a_composer_is_grouped_by_its_workspace_folder_never_its_title() {
        let (store, conn) = layout(
            "folder",
            &[
                (WS_A, json!({"folder": "file:///work/demo"})),
                (WS_B, json!({"folder": "file:///work/other"})),
            ],
        );
        add_composer(&conn, "c1", Some(WS_A), "Secret plan", None);
        add_composer(&conn, "c2", Some(WS_A), "Another title", None);
        add_composer(&conn, "c3", Some(WS_B), "Secret plan", None);
        drop(conn);
        let source = CursorSource::new(&store);
        let (p1, cwd1) = project_of_composer(&source, "c1");
        let (p2, _) = project_of_composer(&source, "c2");
        let (p3, cwd3) = project_of_composer(&source, "c3");
        assert_eq!((p1.as_str(), cwd1.as_deref()), (WS_A, Some("/work/demo")));
        assert_eq!(p1, p2);
        assert_ne!(p1, p3);
        assert_eq!(cwd3.as_deref(), Some("/work/other"));
        let session = source.extract("c1").unwrap().session;
        assert_eq!(session.metadata.title.as_deref(), Some("Secret plan"));
        assert!(!session.project_id.contains("Secret"));
    }

    #[test]
    fn folder_uris_are_percent_decoded() {
        let (store, conn) = layout(
            "encoded",
            &[(
                WS_A,
                json!({"folder": "file:///work/my%20demo/caf%C3%A9%2Bx"}),
            )],
        );
        add_composer(&conn, "c1", Some(WS_A), "t", None);
        drop(conn);
        let source = CursorSource::new(&store);
        assert_eq!(
            project_of_composer(&source, "c1").1.as_deref(),
            Some("/work/my demo/café+x")
        );
    }

    #[test]
    fn a_code_workspace_file_names_its_parent_directory() {
        let (store, conn) = layout(
            "codews",
            &[(
                WS_A,
                json!({"workspace": "file:///work/demo/main.code-workspace"}),
            )],
        );
        add_composer(&conn, "c1", Some(WS_A), "t", None);
        drop(conn);
        let source = CursorSource::new(&store);
        assert_eq!(
            project_of_composer(&source, "c1"),
            (WS_A.to_string(), Some("/work/demo".to_string()))
        );
    }

    #[test]
    fn composers_without_a_usable_workspace_share_the_neutral_project() {
        let (store, conn) = layout(
            "neutral",
            &[
                (WS_A, json!({})),
                (
                    WS_B,
                    json!({"folder": "vscode-remote://ssh-remote+host/work/x"}),
                ),
                (
                    "cccccccccccccccccccccccccccccccc",
                    json!({"workspace": "file:///cursor/Workspaces/1/workspace.json"}),
                ),
            ],
        );
        add_composer(&conn, "none", None, "Alpha", None);
        add_composer(&conn, "empty", Some("empty-window"), "Beta", None);
        add_composer(
            &conn,
            "missing",
            Some("11111111111111111111111111111111"),
            "Gamma",
            None,
        );
        add_composer(&conn, "traversal", Some("../../../../etc"), "Delta", None);
        add_composer(&conn, "nofolder", Some(WS_A), "Epsilon", None);
        add_composer(&conn, "remote", Some(WS_B), "Zeta", None);
        add_composer(
            &conn,
            "untitled",
            Some("cccccccccccccccccccccccccccccccc"),
            "Eta",
            None,
        );
        drop(conn);
        let source = CursorSource::new(&store);
        for id in [
            "none",
            "empty",
            "missing",
            "traversal",
            "nofolder",
            "remote",
            "untitled",
        ] {
            assert_eq!(
                project_of_composer(&source, id),
                (NO_WORKSPACE_PROJECT.to_string(), None),
                "{id}"
            );
        }
    }

    #[test]
    fn the_older_headers_key_without_workspace_ids_uses_the_neutral_project() {
        let store = temp_store("oldkey");
        let conn = open_store(&store);
        put(
            &conn,
            "composerHeaders",
            &json!([{"composerId": "c1", "lastUpdatedAt": 1, "name": "A title"}]),
        );
        put(
            &conn,
            "composerData:c1",
            &json!({"name": "A title", "fullConversationHeadersOnly": []}),
        );
        drop(conn);
        let source = CursorSource::new(&store);
        let summary = source.discover().0.remove(0);
        assert_eq!(summary.project_id, NO_WORKSPACE_PROJECT);
        let session = source.extract("c1").unwrap().session;
        assert_eq!(session.project_id, NO_WORKSPACE_PROJECT);
        assert_eq!(session.metadata.cwd, None);
        assert_eq!(session.metadata.title.as_deref(), Some("A title"));
    }

    #[test]
    fn workspace_lookup_follows_a_symlinked_store_and_is_read_only() {
        let (store, conn) = layout("link", &[(WS_A, json!({"folder": "file:///work/demo"}))]);
        add_composer(&conn, "c1", Some(WS_A), "t", None);
        drop(conn);
        let linked_root =
            std::env::temp_dir().join(format!("botspy-curws-linkroot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&linked_root);
        std::fs::create_dir_all(linked_root.join("cursor/store")).unwrap();
        let link = linked_root.join("cursor/store/state.vscdb");
        std::os::unix::fs::symlink(&store, &link).unwrap();
        let workspace_json = store
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("workspaceStorage")
            .join(WS_A)
            .join("workspace.json");
        let before = digest_files(&[workspace_json.as_path()]);
        let source = CursorSource::new(&link);
        assert_eq!(
            project_of_composer(&source, "c1").1.as_deref(),
            Some("/work/demo")
        );
        assert_eq!(digest_files(&[workspace_json.as_path()]), before);
        let _ = std::fs::remove_dir_all(&linked_root);
    }

    #[test]
    fn workspace_folders_are_read_once_per_source() {
        let (store, conn) = layout("cache", &[(WS_A, json!({"folder": "file:///work/demo"}))]);
        add_composer(&conn, "c1", Some(WS_A), "t", None);
        drop(conn);
        let source = CursorSource::new(&store);
        assert_eq!(
            project_of_composer(&source, "c1").1.as_deref(),
            Some("/work/demo")
        );
        let workspace_dir = store
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("workspaceStorage");
        std::fs::remove_dir_all(&workspace_dir).unwrap();
        assert_eq!(
            project_of_composer(&source, "c1").1.as_deref(),
            Some("/work/demo")
        );
    }

    #[test]
    fn a_subagent_composer_carries_its_kind_and_no_parent() {
        let (store, conn) = layout("sub", &[(WS_A, json!({"folder": "file:///work/demo"}))]);
        add_composer(&conn, "typed", Some(WS_A), "t", Some("explore"));
        add_composer(&conn, "plain", Some(WS_A), "t", Some(""));
        add_composer(&conn, "main", Some(WS_A), "t", None);
        drop(conn);
        let source = CursorSource::new(&store);
        let typed = source.extract("typed").unwrap().session;
        let info = typed.subagent.unwrap();
        assert_eq!((info.kind.as_str(), info.nesting_depth), ("explore", None));
        assert_eq!(typed.parent_id, None);
        let plain = source.extract("plain").unwrap().session;
        assert_eq!(plain.subagent.unwrap().kind, "subagent");
        assert_eq!(source.extract("main").unwrap().session.subagent, None);
    }

    #[test]
    fn file_uris_decode_and_reject_other_hosts() {
        assert_eq!(file_uri_path("file:///a/b%20c").as_deref(), Some("/a/b c"));
        assert_eq!(file_uri_path("file:///c%3A/dir").as_deref(), Some("c:/dir"));
        assert_eq!(file_uri_path("file://localhost/a").as_deref(), Some("/a"));
        assert_eq!(file_uri_path("file://server/share"), None);
        assert_eq!(file_uri_path("vscode-remote://x/a"), None);
        assert_eq!(file_uri_path("file:///a/%zz%"), Some("/a/%zz%".to_string()));
    }

    fn seeded_source(tag: &str, composers: usize) -> (CursorSource, PathBuf) {
        let store = temp_store(tag);
        let conn = open_store(&store);
        let headers: Vec<Value> = (1..=composers)
            .map(|i| json!({"composerId": format!("c{i}"), "lastUpdatedAt": i, "name": "demo"}))
            .collect();
        put(&conn, "composerHeaders", &json!(headers));
        for i in 1..=composers {
            put(
                &conn,
                &format!("composerData:c{i}"),
                &json!({"name": "demo", "fullConversationHeadersOnly": [{"bubbleId": "b1"}]}),
            );
            put(
                &conn,
                &format!("bubbleId:c{i}:b1"),
                &json!({"type": 1, "text": "hello"}),
            );
        }
        drop(conn);
        (CursorSource::new(&store), store)
    }

    #[test]
    fn discover_extract_and_doctor_share_one_snapshot() {
        let (source, _store) = seeded_source("one-copy", 5);
        assert_eq!(source.snapshots_taken(), 0);
        assert!(source.snapshot_directory().is_none());
        let (summaries, _) = source.discover();
        for summary in &summaries {
            assert_eq!(source.extract(&summary.id).unwrap().good, 1);
        }
        assert!(source.doctor().issues.is_empty());
        assert_eq!(summaries.len(), 5);
        assert_eq!(source.snapshots_taken(), 1);
    }

    #[test]
    fn a_changed_store_gets_a_fresh_snapshot_and_the_old_copy_goes() {
        let (source, store) = seeded_source("fresh-copy", 2);
        assert_eq!(source.extract("c1").unwrap().good, 1);
        let first = source.snapshot_directory().unwrap();
        assert!(first.exists());

        let conn = open_store(&store);
        put(
            &conn,
            "bubbleId:c1:b2",
            &json!({"type": 1, "text": "fresh"}),
        );
        put(
            &conn,
            "composerData:c1",
            &json!({"name": "demo", "fullConversationHeadersOnly": [
                {"bubbleId": "b1"}, {"bubbleId": "b2"}]}),
        );
        // Keep the writer open so the change sits in the WAL.
        assert_eq!(source.extract("c1").unwrap().good, 2);
        drop(conn);
        assert_eq!(source.snapshots_taken(), 2);
        let second = source.snapshot_directory().unwrap();
        assert_ne!(first, second);
        assert!(!first.exists(), "the stale copy must be removed");
        assert!(second.exists());
    }

    #[test]
    fn a_wal_only_change_is_noticed() {
        let (source, store) = seeded_source("wal-change", 1);
        let writer = open_store(&store);
        put(&writer, "unrelated", &json!(1));
        source.discover();
        let wal = {
            let mut wal = store.as_os_str().to_owned();
            wal.push("-wal");
            PathBuf::from(wal)
        };
        assert!(wal.exists(), "the open writer keeps the WAL");
        put(&writer, "unrelated", &json!(2));
        source.discover();
        assert_eq!(source.snapshots_taken(), 2);
    }

    #[test]
    fn the_copy_is_removed_when_the_source_drops() {
        let (source, _store) = seeded_source("drop-copy", 1);
        source.discover();
        let directory = source.snapshot_directory().unwrap();
        assert!(directory.join("snapshot.db").exists());
        drop(source);
        assert!(!directory.exists());
    }

    #[test]
    fn a_missing_store_takes_no_copy() {
        let source = CursorSource::new(temp_store("missing"));
        assert!(source.discover().0.is_empty());
        assert!(source.extract("c1").is_none());
        assert_eq!(source.snapshots_taken(), 0);
    }

    fn text_part_of(text: &str) -> Vec<Part> {
        vec![Part::Known(KnownPart::Text {
            text: text.to_string(),
            extra: None,
        })]
    }

    #[test]
    fn a_cli_query_is_the_text_and_its_context_goes_to_extra() {
        let wrapped = "<timestamp>Monday</timestamp>\n<user_query>\nadd a test\n</user_query>";
        let (parts, origin, extra) = split_user_context(text_part_of(wrapped));
        assert_eq!(origin, None);
        assert_eq!(parts, text_part_of("add a test"));
        assert_eq!(
            extra,
            Some(serde_json::json!({"context": ["<timestamp>Monday</timestamp>"]}))
        );
    }

    #[test]
    fn cli_records_without_human_words_are_marked_and_kept_whole() {
        for text in [
            "<timestamp>Monday</timestamp>",
            "<dynamic_tools>x</dynamic_tools>",
        ] {
            let (parts, origin, extra) = split_user_context(text_part_of(text));
            assert_eq!(origin, Some(Origin::Harness), "{text}");
            assert_eq!(parts, text_part_of(text));
            assert_eq!(extra, None);
        }
        let (parts, origin, _) = split_user_context(text_part_of("plain older text"));
        assert_eq!(origin, None);
        assert_eq!(parts, text_part_of("plain older text"));
    }

    #[test]
    fn simulated_user_bubbles_are_marked_and_typed_ones_are_not() {
        let mark = |bubble: Value| {
            normalize_bubble(&bubble, "b1", Path::new("store"))
                .unwrap()
                .ok()
                .unwrap()
                .origin
        };
        assert_eq!(
            mark(json!({"type": 1, "text": "go", "isSimulatedMsg": true})),
            Some(Origin::Simulated)
        );
        assert_eq!(mark(json!({"type": 1, "text": "go"})), None);
        assert_eq!(
            mark(json!({"type": 2, "text": "ok", "isSimulatedMsg": true})),
            None
        );
    }
}
