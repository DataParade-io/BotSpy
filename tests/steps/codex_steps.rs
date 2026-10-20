//! Steps for the Codex importer (spec 03_importers/codex/adapter.feature).

use crate::steps::BotSpyWorld;
use botspy::adapters::codex::{CodexDiscovery, CodexReport, CodexSource};
use botspy::snapshot::digest_files;
use cucumber::{given, then, when};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static UNIQUE_DIR: AtomicUsize = AtomicUsize::new(0);

fn new_root(world: &mut BotSpyWorld) -> PathBuf {
    let base = std::env::temp_dir().join("botspy-cx").join(format!(
        "{}-{}",
        std::process::id(),
        UNIQUE_DIR.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&base).expect("create Codex home root");
    world.cx_root = Some(base.clone());
    base
}

/// The threads-index shape the importer reads: no parent column — spawn
/// edges live in `thread_spawn_edges` — and the metadata columns Codex
/// really carries.
fn open_index(root: &Path, with_edges: bool) -> rusqlite::Connection {
    let conn =
        rusqlite::Connection::open(root.join("state_5.sqlite")).expect("open Codex threads index");
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
        );",
    )
    .expect("create threads table");
    if with_edges {
        conn.execute_batch(
            "CREATE TABLE thread_spawn_edges (
                parent_thread_id TEXT NOT NULL,
                child_thread_id TEXT NOT NULL PRIMARY KEY,
                status TEXT NOT NULL
            );",
        )
        .expect("create thread_spawn_edges table");
    }
    conn
}

/// One synthetic row for the threads index.
#[derive(Default)]
struct ThreadRow {
    id: String,
    rollout_path: String,
    cwd: String,
    title: Option<String>,
    model: Option<String>,
    git_branch: Option<String>,
    git_origin_url: Option<String>,
    git_sha: Option<String>,
    created_at_ms: Option<i64>,
    updated_at_ms: Option<i64>,
}

fn insert_thread(conn: &rusqlite::Connection, row: &ThreadRow) {
    conn.execute(
        "INSERT INTO threads (id, rollout_path, cwd, title, model, git_branch, git_origin_url, \
         git_sha, created_at, updated_at, created_at_ms, updated_at_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        rusqlite::params![
            row.id,
            row.rollout_path,
            row.cwd,
            row.title.clone().unwrap_or_default(),
            row.model,
            row.git_branch,
            row.git_origin_url,
            row.git_sha,
            row.created_at_ms.map(|ms| ms / 1000).unwrap_or(0),
            row.updated_at_ms.map(|ms| ms / 1000).unwrap_or(0),
            row.created_at_ms,
            row.updated_at_ms,
        ],
    )
    .expect("insert thread row");
}

fn insert_edge(conn: &rusqlite::Connection, parent: &str, child: &str) {
    conn.execute(
        "INSERT INTO thread_spawn_edges (parent_thread_id, child_thread_id, status) \
         VALUES (?1, ?2, 'open')",
        rusqlite::params![parent, child],
    )
    .expect("insert spawn edge");
}

fn write_rollout(root: &Path, path: &str, lines: &[String], partial: Option<&str>) {
    let full = root.join(path);
    std::fs::create_dir_all(full.parent().expect("rollout parent dir"))
        .expect("create rollout dirs");
    let mut body: String = lines.iter().map(|line| format!("{line}\n")).collect();
    if let Some(partial_line) = partial {
        body.push_str(partial_line);
    }
    std::fs::write(full, body).expect("write Codex rollout");
}

fn response_message(ordinal: u64, timestamp: &str, text: &str) -> String {
    format!(
        r#"{{"ordinal":{ordinal},"timestamp":"{timestamp}","type":"response_item","payload":{{"type":"message","id":"m{ordinal}","role":"user","content":[{{"type":"input_text","text":"{text}"}}]}}}}"#
    )
}

fn index_for_single_rollout(root: &Path) {
    let conn = open_index(root, false);
    insert_thread(&conn, &index_thread("t1"));
    drop(conn);
}

fn finish_setup(world: &mut BotSpyWorld, thread: String) {
    let root = world.cx_root.clone().expect("no Codex home root");
    world.cx_thread = Some(thread);
    world.cx_source = Some(CodexSource::new(root));
}

fn index_thread(id: &str) -> ThreadRow {
    ThreadRow {
        id: id.to_string(),
        rollout_path: format!("sessions/2026/10/01/rollout-{id}.jsonl"),
        cwd: "/workspace/alpha".to_string(),
        ..ThreadRow::default()
    }
}

fn setup_threads(world: &mut BotSpyWorld, rows: &[ThreadRow], edges: &[(&str, &str)]) {
    let root = new_root(world);
    let conn = open_index(&root, !edges.is_empty());
    for row in rows {
        insert_thread(&conn, row);
        write_rollout(
            &root,
            &row.rollout_path,
            &[response_message(
                1,
                "2026-10-01T09:00:00Z",
                &format!("thread {}", row.id),
            )],
            None,
        );
    }
    for (parent, child) in edges {
        insert_edge(&conn, parent, child);
    }
    drop(conn);
    world.cx_source = Some(CodexSource::new(root));
}

const ROLLOUT: &str = "sessions/2026/10/01/rollout-t1.jsonl";

#[given(regex = r#"^a Codex threads index with ([0-9]+) threads pointing at rollout files$"#)]
fn threads_index(world: &mut BotSpyWorld, count: usize) {
    let rows: Vec<ThreadRow> = (1..=count)
        .map(|i| index_thread(&format!("t{i}")))
        .collect();
    setup_threads(world, &rows, &[]);
}

#[given(regex = r#"^a Codex threads index where thread "([^"]+)" is spawned by thread "([^"]+)"$"#)]
fn threads_with_spawn_edge(world: &mut BotSpyWorld, child: String, parent: String) {
    let mut parent_row = index_thread(&parent);
    parent_row.rollout_path = "sessions/2026/10/01/rollout-t1.jsonl".to_string();
    let mut child_row = index_thread(&child);
    child_row.rollout_path = "sessions/2026/10/01/rollout-t2.jsonl".to_string();
    setup_threads(
        world,
        &[parent_row, child_row],
        &[(parent.as_str(), child.as_str())],
    );
    world.cx_thread = Some(child);
}

#[given(
    regex = r#"^a Codex threads index where thread "([^"]+)" carries title "([^"]*)", model "([^"]*)", git branch "([^"]*)", git origin "([^"]*)", and cwd "([^"]*)"$"#
)]
fn threads_with_metadata(
    world: &mut BotSpyWorld,
    id: String,
    title: String,
    model: String,
    git_branch: String,
    git_origin: String,
    cwd: String,
) {
    let row = ThreadRow {
        id: id.clone(),
        rollout_path: format!("sessions/2026/10/01/rollout-{id}.jsonl"),
        cwd,
        title: Some(title),
        model: Some(model),
        git_branch: Some(git_branch),
        git_origin_url: Some(git_origin),
        git_sha: Some("abcdef1234567890".to_string()),
        ..ThreadRow::default()
    };
    setup_threads(world, &[row], &[]);
    world.cx_thread = Some(id);
}

#[given(
    regex = r#"^a Codex threads index where thread "([^"]+)" was created at ([0-9]+) and last updated at ([0-9]+)$"#
)]
fn threads_with_instants(world: &mut BotSpyWorld, id: String, created_ms: i64, updated_ms: i64) {
    let row = ThreadRow {
        id: id.clone(),
        rollout_path: format!("sessions/2026/10/01/rollout-{id}.jsonl"),
        cwd: "/workspace/alpha".to_string(),
        created_at_ms: Some(created_ms),
        updated_at_ms: Some(updated_ms),
        ..ThreadRow::default()
    };
    setup_threads(world, &[row], &[]);
    world.cx_thread = Some(id);
}

#[given(
    regex = r#"^a Codex threads index where thread "([^"]+)" points at an absolute rollout path$"#
)]
fn threads_with_absolute_rollout(world: &mut BotSpyWorld, id: String) {
    let root = new_root(world);
    let absolute = root
        .join("sessions/2026/10/01/rollout-abs.jsonl")
        .display()
        .to_string();
    write_rollout(
        &root,
        &absolute,
        &[response_message(
            1,
            "2026-10-01T09:00:00Z",
            "absolute path rollout",
        )],
        None,
    );
    let conn = open_index(&root, false);
    insert_thread(
        &conn,
        &ThreadRow {
            id: id.clone(),
            rollout_path: absolute,
            cwd: "/workspace/alpha".to_string(),
            ..ThreadRow::default()
        },
    );
    drop(conn);
    world.cx_source = Some(CodexSource::new(root));
}

#[given(regex = r#"^a Codex threads index with a legacy schema carrying a parent column$"#)]
fn threads_index_legacy_schema(world: &mut BotSpyWorld) {
    let root = new_root(world);
    let conn =
        rusqlite::Connection::open(root.join("state_5.sqlite")).expect("open Codex threads index");
    conn.execute(
        "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL, parent_id TEXT, cwd TEXT)",
        [],
    )
    .expect("create legacy threads table");
    for (id, parent) in [("t1", None), ("t2", Some("t1"))] {
        conn.execute(
            "INSERT INTO threads (id, rollout_path, parent_id, cwd) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                id,
                format!("sessions/2026/10/01/rollout-{id}.jsonl"),
                parent,
                "/workspace/alpha"
            ],
        )
        .expect("insert legacy thread row");
    }
    drop(conn);
    for id in ["t1", "t2"] {
        write_rollout(
            &root,
            &format!("sessions/2026/10/01/rollout-{id}.jsonl"),
            &[response_message(
                1,
                "2026-10-01T09:00:00Z",
                &format!("thread {id}"),
            )],
            None,
        );
    }
    world.cx_source = Some(CodexSource::new(root));
}

#[given(regex = r#"^a Codex threads index whose spawn edge table is missing$"#)]
fn threads_index_without_edges(world: &mut BotSpyWorld) {
    let root = new_root(world);
    let conn = open_index(&root, false);
    for id in ["t1", "t2"] {
        insert_thread(&conn, &index_thread(id));
    }
    drop(conn);
    for id in ["t1", "t2"] {
        write_rollout(
            &root,
            &format!("sessions/2026/10/01/rollout-{id}.jsonl"),
            &[response_message(
                1,
                "2026-10-01T09:00:00Z",
                &format!("thread {id}"),
            )],
            None,
        );
    }
    world.cx_source = Some(CodexSource::new(root));
}

#[given(
    regex = r#"^a Codex rollout with records "ordinal ([0-9]+)", "ordinal ([0-9]+)", and "ordinal ([0-9]+)"$"#
)]
fn rollout_with_shuffled_ordinals(world: &mut BotSpyWorld, first: u64, second: u64, third: u64) {
    let root = new_root(world);
    index_for_single_rollout(&root);
    write_rollout(
        &root,
        ROLLOUT,
        &[
            response_message(first, "2026-10-01T09:00:15Z", "written first"),
            response_message(second, "2026-10-01T09:00:05Z", "written second"),
            response_message(third, "2026-10-01T09:00:10Z", "written third"),
        ],
        None,
    );
    finish_setup(world, "t1".to_string());
}

#[given(
    regex = r#"^a Codex rollout with ([0-9]+) records and a stored offset after record ([0-9]+)$"#
)]
fn rollout_with_stored_offset(world: &mut BotSpyWorld, count: usize, after: usize) {
    let root = new_root(world);
    index_for_single_rollout(&root);
    let lines: Vec<String> = (1..=count)
        .map(|i| response_message(i as u64, "2026-10-01T09:00:00Z", &format!("record {i}")))
        .collect();
    write_rollout(&root, ROLLOUT, &lines, None);
    let offset: usize = lines.iter().take(after).map(|line| line.len() + 1).sum();
    finish_setup(world, "t1".to_string());
    world
        .cx_source
        .as_ref()
        .expect("Codex source")
        .set_offset("t1", offset as u64);
}

#[given(
    regex = r#"^a Codex rollout holding "response_item" records with roles "user", "assistant", "developer", a reasoning summary, and a tool call with output$"#
)]
fn rollout_with_response_items(world: &mut BotSpyWorld) {
    let root = new_root(world);
    index_for_single_rollout(&root);
    write_rollout(
        &root,
        ROLLOUT,
        &[
            response_message(1, "2026-10-01T09:00:00Z", "run the tests"),
            r#"{"ordinal":2,"timestamp":"2026-10-01T09:00:05Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"On it."}]}}"#.to_string(),
            r#"{"ordinal":3,"timestamp":"2026-10-01T09:00:06Z","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"Be concise."}]}}"#.to_string(),
            r#"{"ordinal":4,"timestamp":"2026-10-01T09:00:07Z","type":"response_item","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"thinking it through"}]}}"#.to_string(),
            r#"{"ordinal":5,"timestamp":"2026-10-01T09:00:08Z","type":"response_item","payload":{"type":"function_call","call_id":"call_1","name":"shell","arguments":"{\"cmd\":[\"cargo\",\"test\"]}"}}"#.to_string(),
            r#"{"ordinal":6,"timestamp":"2026-10-01T09:00:20Z","type":"response_item","payload":{"type":"function_call_output","call_id":"call_1","output":"all tests pass"}}"#.to_string(),
        ],
        None,
    );
    finish_setup(world, "t1".to_string());
}

#[given(regex = r#"^a Codex rollout holding a "token_usage_record" and a "compacted" record$"#)]
fn rollout_with_usage_and_compaction(world: &mut BotSpyWorld) {
    let root = new_root(world);
    index_for_single_rollout(&root);
    write_rollout(
        &root,
        ROLLOUT,
        &[
            r#"{"ordinal":1,"type":"token_usage_record","payload":{"input_tokens":1200,"cached_input_tokens":300,"output_tokens":800,"reasoning_tokens":150,"model":"gpt-5-codex"}}"#.to_string(),
            r#"{"ordinal":2,"type":"compacted","payload":{"window_id":"w2","previous_window":"w1","retained_context":"summary so far"}}"#.to_string(),
        ],
        None,
    );
    finish_setup(world, "t1".to_string());
}

#[given(
    regex = r#"^a Codex rollout holding a "task_complete" record with duration_ms ([0-9]+) and time_to_first_token_ms ([0-9]+)$"#
)]
fn rollout_with_task_complete(
    world: &mut BotSpyWorld,
    duration_ms: u64,
    time_to_first_token_ms: u64,
) {
    let root = new_root(world);
    index_for_single_rollout(&root);
    write_rollout(
        &root,
        ROLLOUT,
        &[format!(
            r#"{{"ordinal":1,"type":"task_complete","payload":{{"turn_id":"turn-1","duration_ms":{duration_ms},"time_to_first_token_ms":{time_to_first_token_ms}}}}}"#
        )],
        None,
    );
    finish_setup(world, "t1".to_string());
}

#[given(
    regex = r#"^a Codex rollout holding a malformed line and ending in a partial trailing line$"#
)]
fn rollout_with_malformed_and_partial(world: &mut BotSpyWorld) {
    let root = new_root(world);
    index_for_single_rollout(&root);
    write_rollout(
        &root,
        ROLLOUT,
        &["this line is { not json".to_string()],
        Some(
            r#"{"ordinal":2,"type":"response_item","payload":{"type":"message","role":"user","content":"cut off"#,
        ),
    );
    finish_setup(world, "t1".to_string());
}

fn index_paths(world: &BotSpyWorld) -> Vec<PathBuf> {
    let root = world.cx_root.as_ref().expect("no Codex home root");
    let db = root.join("state_5.sqlite");
    let mut wal = db.as_os_str().to_owned();
    wal.push("-wal");
    vec![db, PathBuf::from(wal)]
}

#[when(regex = r#"^the importer detects changes$"#)]
fn detect_changes(world: &mut BotSpyWorld) {
    let source = world.cx_source.as_ref().expect("no Codex source");
    let (summaries, discovery) = source.discover();
    world.discovered = summaries;
    world.cx_discovery = Some(discovery);
}

#[when(regex = r#"^the importer reads the threads index through a snapshot copy$"#)]
fn read_through_snapshot(world: &mut BotSpyWorld) {
    let paths = index_paths(world);
    world.cx_digest_before = Some(digest_files(
        &paths.iter().map(|path| path.as_path()).collect::<Vec<_>>(),
    ));
    let source = world.cx_source.as_ref().expect("no Codex source");
    let (summaries, discovery) = source.discover();
    world.discovered = summaries;
    world.cx_discovery = Some(discovery);
    world.cx_digest_after = Some(digest_files(
        &paths.iter().map(|path| path.as_path()).collect::<Vec<_>>(),
    ));
}

#[when(regex = r#"^the doctor checks the Codex source$"#)]
fn doctor_checks(world: &mut BotSpyWorld) {
    let source = world.cx_source.as_ref().expect("no Codex source");
    world.cx_report = Some(source.doctor());
}

#[when(regex = r#"^the importer extracts the rollout(?: from the stored offset)?$"#)]
fn extract_rollout(world: &mut BotSpyWorld) {
    let source = world.cx_source.as_ref().expect("no Codex source");
    let thread = world.cx_thread.as_deref().expect("no Codex thread");
    let extraction = source
        .extract(thread)
        .expect("no rollout for the Codex thread");
    world.skipped = extraction.skipped.clone();
    world.cx_extraction = Some(extraction);
}

#[then(regex = r#"^session "([^"]+)" links to parent session "([^"]+)"$"#)]
fn session_links_to_parent(world: &mut BotSpyWorld, child: String, parent: String) {
    let discovery: &CodexDiscovery = world
        .cx_discovery
        .as_ref()
        .expect("no Codex discovery result");
    assert_eq!(
        discovery.parent_links.get(&child).map(String::as_str),
        Some(parent.as_str()),
        "spawn edge {child} -> {parent} missing"
    );
}

#[then(regex = r#"^session "([^"]+)" carries the title, model, git branch, git origin, and cwd$"#)]
fn session_carries_metadata(world: &mut BotSpyWorld, id: String) {
    let summary = world
        .discovered
        .iter()
        .find(|summary| summary.id == id)
        .expect("no summary for the session");
    let metadata = &summary.metadata;
    assert_eq!(metadata.title.as_deref(), Some("Alpha build fix"));
    assert_eq!(metadata.models, vec!["synthetic-model".to_string()]);
    assert_eq!(metadata.git_branch.as_deref(), Some("feat/alpha"));
    assert_eq!(
        metadata.git_origin_url.as_deref(),
        Some("https://git.example.com/acme/alpha.git")
    );
    assert_eq!(metadata.cwd.as_deref(), Some("/workspace/alpha"));
    assert_eq!(summary.project_id, "/workspace/alpha");
}

#[then(regex = r#"^session "([^"]+)" starts at "([^"]+)" and was last active at "([^"]+)"$"#)]
fn session_has_instants(world: &mut BotSpyWorld, id: String, started: String, active: String) {
    let summary = world
        .discovered
        .iter()
        .find(|summary| summary.id == id)
        .expect("no summary for the session");
    assert_eq!(summary.started_at, started, "wrong started instant");
    assert_eq!(summary.last_activity_at, active, "wrong last activity");
}

#[then(
    regex = r#"^the session carries the title, model, git branch, git origin, and cwd as project$"#
)]
fn extraction_carries_metadata(world: &mut BotSpyWorld) {
    let extraction = world.cx_extraction.as_ref().expect("no Codex extraction");
    let session = &extraction.session;
    let metadata = &session.metadata;
    assert_eq!(metadata.title.as_deref(), Some("Alpha build fix"));
    assert_eq!(metadata.models, vec!["synthetic-model".to_string()]);
    assert_eq!(metadata.git_branch.as_deref(), Some("feat/alpha"));
    assert_eq!(
        metadata.git_origin_url.as_deref(),
        Some("https://git.example.com/acme/alpha.git")
    );
    assert_eq!(metadata.cwd.as_deref(), Some("/workspace/alpha"));
    assert_eq!(session.project_id, "/workspace/alpha");
}

#[then(regex = r#"^the threads index was never mutated$"#)]
fn index_unmutated(world: &mut BotSpyWorld) {
    let before = world.cx_digest_before.as_ref().expect("digest before");
    let after = world.cx_digest_after.as_ref().expect("digest after");
    assert_eq!(before, after, "reading the threads index mutated it");
}

#[then(regex = r#"^discovery reports an issue about the threads index$"#)]
fn discovery_reports_index_issue(world: &mut BotSpyWorld) {
    let discovery = world.cx_discovery.as_ref().expect("no Codex discovery");
    assert!(
        discovery
            .issues
            .iter()
            .any(|issue| issue.contains("threads index")),
        "no threads index issue reported: {:?}",
        discovery.issues
    );
}

#[then(regex = r#"^discovery reports an issue about the spawn edges$"#)]
fn discovery_reports_edge_issue(world: &mut BotSpyWorld) {
    let discovery = world.cx_discovery.as_ref().expect("no Codex discovery");
    assert!(
        discovery
            .issues
            .iter()
            .any(|issue| issue.contains("spawn edge")),
        "no spawn edge issue reported: {:?}",
        discovery.issues
    );
}

#[then("discovery still yields the indexed sessions")]
fn discovery_preserves_indexed_sessions(world: &mut BotSpyWorld) {
    let mut ids: Vec<&str> = world
        .discovered
        .iter()
        .map(|summary| summary.id.as_str())
        .collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        ["t1", "t2"],
        "a missing spawn-edge table must not hide either indexed thread"
    );
}

#[then(regex = r#"^the doctor reports the schema drift issue$"#)]
fn doctor_reports_drift(world: &mut BotSpyWorld) {
    let report: &CodexReport = world.cx_report.as_ref().expect("no Codex doctor report");
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.contains("threads index")),
        "doctor did not report the schema drift: {:?}",
        report.issues
    );
}

#[then(regex = r#"^the normalized messages appear in ordinal order ([0-9]+(?:, [0-9]+)*)$"#)]
fn messages_in_ordinal_order(world: &mut BotSpyWorld, expected: String) {
    let expected_ordinals: Vec<u64> = expected
        .split(", ")
        .map(|part| part.parse().expect("ordinal number"))
        .collect();
    let extraction = world.cx_extraction.as_ref().expect("no Codex extraction");
    let ordinals: Vec<u64> = extraction
        .session
        .messages
        .iter()
        .map(|message| {
            message
                .provenance
                .as_ref()
                .and_then(|prov| prov.ordinal)
                .expect("message provenance ordinal")
        })
        .collect();
    assert_eq!(
        ordinals, expected_ordinals,
        "messages did not follow the explicit ordinal order"
    );
}

#[then(regex = r#"^extraction yields ([0-9]+) more good records?$"#)]
fn extraction_yields_good(world: &mut BotSpyWorld, count: usize) {
    let extraction = world.cx_extraction.as_ref().expect("no Codex extraction");
    assert_eq!(
        extraction.good, count,
        "extraction mapped the wrong records"
    );
}

#[then(
    regex = r#"^the normalized messages map the roles, reasoning kinds, and tool call/result kinds$"#
)]
fn messages_map_part_kinds(world: &mut BotSpyWorld) {
    let extraction = world.cx_extraction.as_ref().expect("no Codex extraction");
    let messages = &extraction.session.messages;
    assert_eq!(messages.len(), 6, "wrong number of normalized messages");
    assert_eq!(messages[0].role, botspy::Role::User);
    assert_eq!(messages[1].role, botspy::Role::Assistant);
    assert_eq!(messages[2].role, botspy::Role::System);
    assert_eq!(messages[2].origin, Some(botspy::Origin::Developer));
    assert!(
        matches!(
            messages[3].parts.first(),
            Some(botspy::Part::Known(botspy::KnownPart::Thinking { .. }))
        ),
        "the reasoning summary did not map to a thinking part"
    );
    assert!(
        matches!(
            messages[4].parts.first(),
            Some(botspy::Part::Known(botspy::KnownPart::ToolCall { .. }))
        ),
        "the function call did not map to a tool call part"
    );
    assert!(
        matches!(
            messages[5].parts.first(),
            Some(botspy::Part::Known(botspy::KnownPart::ToolResult { .. }))
        ),
        "the function output did not map to a tool result part"
    );
}

#[then(regex = r#"^the session usage carries the token counts$"#)]
fn session_usage_carries_tokens(world: &mut BotSpyWorld) {
    let extraction = world.cx_extraction.as_ref().expect("no Codex extraction");
    let usage = extraction
        .session
        .usage
        .as_ref()
        .expect("the token_usage_record did not land in session usage");
    assert_eq!(usage.input_tokens, Some(1200));
    assert_eq!(usage.cached_input_tokens, Some(300));
    assert_eq!(usage.output_tokens, Some(800));
    assert_eq!(usage.reasoning_tokens, Some(150));
    assert_eq!(usage.model.as_deref(), Some("gpt-5-codex"));
}

#[then(regex = r#"^the compaction window carries the previous window and the retained context$"#)]
fn compaction_window_carries_boundary(world: &mut BotSpyWorld) {
    let extraction = world.cx_extraction.as_ref().expect("no Codex extraction");
    let window = extraction
        .session
        .compaction_windows
        .get("w2")
        .expect("the compacted record did not land in the window chain");
    assert_eq!(window.previous_window.as_deref(), Some("w1"));
    assert_eq!(window.retained_context.as_deref(), Some("summary so far"));
}

#[then(regex = r#"^the turn has duration_ms ([0-9]+) and time_to_first_token_ms ([0-9]+)$"#)]
fn turn_has_timing(world: &mut BotSpyWorld, duration_ms: u64, time_to_first_token_ms: u64) {
    let extraction = world.cx_extraction.as_ref().expect("no Codex extraction");
    let turn = extraction
        .session
        .turns
        .get("turn-1")
        .expect("the task_complete record did not land in the turn registry");
    assert_eq!(turn.duration_ms, Some(duration_ms));
    assert_eq!(turn.time_to_first_token_ms, Some(time_to_first_token_ms));
}

#[then(regex = r#"^no partial or malformed record is normalized$"#)]
fn no_partial_or_malformed_normalized(world: &mut BotSpyWorld) {
    let extraction = world.cx_extraction.as_ref().expect("no Codex extraction");
    assert_eq!(extraction.good, 0, "a skipped record was normalized");
    assert_eq!(extraction.pending_partial, 1);
}
