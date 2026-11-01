//! Steps for the message-origin specs (03_importers/*/message_origin.feature)
//! and the meter's use of them (04_examples/consumers.feature).
//!
//! Every transcript, rollout and store is synthetic and built here, in a
//! temporary directory, then read through the real adapter. The session
//! that comes back is kept in `world.opened`.

use crate::steps::BotSpyWorld;
use botspy::adapters::claude_code::ClaudeCodeSource;
use botspy::adapters::codex::CodexSource;
use botspy::adapters::cursor::{CursorCliSource, CursorSource};
use botspy::{KnownPart, Message, Origin, Part, Role, Session};
use cucumber::{given, then, when};
use serde_json::{json, Value};

fn last_message(world: &BotSpyWorld) -> &Message {
    let session = world
        .opened
        .as_ref()
        .and_then(|opened| opened.as_ref().ok())
        .expect("no session was read");
    session.messages.last().expect("the session has a message")
}

/// Text in a feature spells a newline as `\n`.
fn unescape(text: &str) -> String {
    text.replace("\\n", "\n")
}

fn texts_of(message: &Message) -> Vec<&str> {
    message
        .parts
        .iter()
        .filter_map(|part| match part {
            Part::Known(KnownPart::Text { text, .. }) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

// ---- Claude Code ----------------------------------------------------------

/// One Claude Code user record. `mark` names what the record carries
/// besides its text: `none`, `isMeta`, `isCompactSummary`, or
/// `origin:<kind>`.
pub fn claude_user_record(mark: &str, text: &str) -> Value {
    let mut record = json!({
        "type": "user",
        "uuid": "u1",
        "parentUuid": "",
        "timestamp": "2026-10-01T09:00:00Z",
        "message": {"role": "user", "content": text},
    });
    match mark {
        "none" => {}
        "isMeta" | "isCompactSummary" => record[mark] = json!(true),
        other => {
            let kind = other.strip_prefix("origin:").expect("a known mark");
            record["origin"] = json!({"kind": kind});
        }
    }
    record
}

/// The session Claude Code's importer reads from the given records.
pub fn claude_session(records: &[Value]) -> Session {
    let root = tempfile::tempdir().expect("scratch dir");
    let project = root.path().join("proj-a");
    std::fs::create_dir_all(&project).expect("project dir");
    let body: String = records.iter().map(|record| format!("{record}\n")).collect();
    std::fs::write(project.join("s1.jsonl"), body).expect("write transcript");
    ClaudeCodeSource::new(root.path())
        .extract("s1")
        .expect("the transcript extracts")
        .session
}

#[when(
    regex = r#"^the Claude Code importer reads a user record marked "([^"]+)" with the text "([^"]*)"$"#
)]
fn claude_reads_record(world: &mut BotSpyWorld, mark: String, text: String) {
    let session = claude_session(&[claude_user_record(&mark, &unescape(&text))]);
    world.opened = Some(Ok(session));
}

// ---- Codex ----------------------------------------------------------------

pub fn codex_session(messages: &[(&str, &str)]) -> Session {
    let root = tempfile::tempdir().expect("scratch dir");
    let conn = rusqlite::Connection::open(root.path().join("state_5.sqlite")).expect("index");
    conn.execute_batch(
        "CREATE TABLE threads (
            id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL, cwd TEXT NOT NULL,
            title TEXT NOT NULL DEFAULT '', source TEXT NOT NULL DEFAULT '',
            model_provider TEXT NOT NULL DEFAULT '', created_at INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL DEFAULT 0, created_at_ms INTEGER, updated_at_ms INTEGER,
            archived INTEGER NOT NULL DEFAULT 0, git_sha TEXT, git_branch TEXT,
            git_origin_url TEXT, model TEXT
        );
        CREATE TABLE thread_spawn_edges (
            parent_thread_id TEXT NOT NULL, child_thread_id TEXT NOT NULL PRIMARY KEY,
            status TEXT NOT NULL
        );
        INSERT INTO threads (id, rollout_path, cwd) VALUES ('t1', 'rollout-t1.jsonl', '/repo');",
    )
    .expect("create the index");
    drop(conn);
    let body: String = messages
        .iter()
        .enumerate()
        .map(|(index, (role, text))| {
            let kind = if *role == "assistant" {
                "output_text"
            } else {
                "input_text"
            };
            format!(
                "{}\n",
                json!({
                    "ordinal": index + 1,
                    "timestamp": "2026-10-01T09:00:00Z",
                    "type": "response_item",
                    "payload": {"type": "message", "role": role,
                                "content": [{"type": kind, "text": text}]},
                })
            )
        })
        .collect();
    std::fs::write(root.path().join("rollout-t1.jsonl"), body).expect("write rollout");
    CodexSource::new(root.path())
        .extract("t1")
        .expect("the rollout extracts")
        .session
}

#[when(regex = r#"^the Codex importer reads a user message with the text "([^"]*)"$"#)]
fn codex_reads_user_message(world: &mut BotSpyWorld, text: String) {
    world.opened = Some(Ok(codex_session(&[("user", &unescape(&text))])));
}

// ---- Cursor CLI -----------------------------------------------------------

pub fn cursor_cli_session(texts: &[(&str, &str)]) -> Session {
    let root = tempfile::tempdir().expect("scratch dir");
    let dir = root.path().join("proj-a/agent-transcripts");
    std::fs::create_dir_all(&dir).expect("transcripts dir");
    let body: String = texts
        .iter()
        .map(|(role, text)| {
            format!(
                "{}\n",
                json!({"role": role, "message": {"content": [{"type": "text", "text": text}]}})
            )
        })
        .collect();
    std::fs::write(dir.join("s1.jsonl"), body).expect("write transcript");
    CursorCliSource::new(root.path())
        .extract("s1")
        .expect("the transcript extracts")
        .session
}

#[when(regex = r#"^the Cursor CLI importer reads a user record with the text "([^"]*)"$"#)]
fn cursor_cli_reads_user_record(world: &mut BotSpyWorld, text: String) {
    world.opened = Some(Ok(cursor_cli_session(&[("user", &unescape(&text))])));
}

// ---- Cursor IDE -----------------------------------------------------------

/// The session the IDE importer reads from one user bubble.
pub fn cursor_ide_session(text: &str, simulated: bool) -> Session {
    let root = tempfile::tempdir().expect("scratch dir");
    let store = root.path().join("state.vscdb");
    let conn = rusqlite::Connection::open(&store).expect("store");
    conn.execute(
        "CREATE TABLE cursorDiskKV (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        [],
    )
    .expect("create the table");
    let mut bubble = json!({"type": 1, "text": text});
    if simulated {
        bubble["isSimulatedMsg"] = json!(true);
    }
    for (key, value) in [
        (
            "composerHeaders".to_string(),
            json!([{"composerId": "c1", "lastUpdatedAt": 1000, "name": "demo"}]),
        ),
        (
            "composerData:c1".to_string(),
            json!({"name": "demo", "fullConversationHeadersOnly": [{"bubbleId": "b1"}]}),
        ),
        ("bubbleId:c1:b1".to_string(), bubble),
    ] {
        conn.execute(
            "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, value.to_string()],
        )
        .expect("write the row");
    }
    drop(conn);
    CursorSource::new(&store)
        .extract("c1")
        .expect("the composer extracts")
        .session
}

#[when(
    regex = r#"^the Cursor importer reads a user bubble with the text "([^"]*)"(, flagged simulated)?$"#
)]
fn cursor_ide_reads_bubble(world: &mut BotSpyWorld, text: String, simulated: String) {
    world.opened = Some(Ok(cursor_ide_session(&text, !simulated.is_empty())));
}

// ---- Then -----------------------------------------------------------------

#[then(regex = r#"^the message is a user message$"#)]
fn message_is_user(world: &mut BotSpyWorld) {
    assert_eq!(last_message(world).role, Role::User);
}

#[then(regex = r#"^the message has origin "([^"]+)"$"#)]
fn message_has_origin(world: &mut BotSpyWorld, name: String) {
    let expected = Origin::parse(&name).unwrap_or_else(|| panic!("unknown origin {name}"));
    assert_eq!(last_message(world).origin, Some(expected));
}

#[then(regex = r#"^the message has no origin, so it is the human's$"#)]
fn message_is_human(world: &mut BotSpyWorld) {
    assert_eq!(last_message(world).origin, None);
}

#[then(regex = r#"^the message text is "([^"]*)"$"#)]
fn message_text_is(world: &mut BotSpyWorld, text: String) {
    assert_eq!(texts_of(last_message(world)), vec![unescape(&text)]);
}

#[then(regex = r#"^the message keeps the surrounding context "([^"]*)" apart from its text$"#)]
fn message_keeps_context(world: &mut BotSpyWorld, context: String) {
    let extra = last_message(world).extra.clone().expect("message extra");
    assert_eq!(extra["context"], json!([unescape(&context)]));
}

// ---- The meter ------------------------------------------------------------

fn f_bomb() -> String {
    // Joined at run time: the censored spelling.
    format!("what the f{} is this", "***")
}

/// A thank-you and an F-bomb, appended to injected text.
fn charged(lead: &str) -> String {
    format!("{lead} thanks, {}", f_bomb())
}

fn push_session(world: &mut BotSpyWorld, session: Session) {
    world.example_sessions.push(session);
}

#[given(
    regex = r#"^a Claude Code session where I typed "([^"]+)" and the harness injected a record marked "([^"]+)" led by "([^"]*)"$"#
)]
fn meter_claude(world: &mut BotSpyWorld, typed: String, mark: String, lead: String) {
    let session = claude_session(&[
        claude_user_record("origin:human", &typed),
        claude_user_record(&mark, &charged(&lead)),
    ]);
    push_session(world, session);
}

#[given(
    regex = r#"^a Codex session where I typed "([^"]+)" and the harness injected the user message led by "([^"]*)"$"#
)]
fn meter_codex(world: &mut BotSpyWorld, typed: String, lead: String) {
    let injected = charged(&lead);
    push_session(
        world,
        codex_session(&[("user", &typed), ("user", &injected)]),
    );
}

#[given(
    regex = r#"^a Cursor CLI session where I typed "([^"]+)" and the harness injected the user record led by "([^"]*)"$"#
)]
fn meter_cursor_cli(world: &mut BotSpyWorld, typed: String, lead: String) {
    let wrapped = format!("<timestamp>Thursday</timestamp>\n<user_query>\n{typed}\n</user_query>");
    let injected = charged(&lead);
    push_session(
        world,
        cursor_cli_session(&[("user", &wrapped), ("user", &injected)]),
    );
}

#[given(
    regex = r#"^a Cursor session where I typed "([^"]+)" and Cursor simulated a user bubble carrying thanks and an F-bomb$"#
)]
fn meter_cursor_ide(world: &mut BotSpyWorld, typed: String) {
    let human = cursor_ide_session(&typed, false);
    let mut session = cursor_ide_session(&charged("carry on"), true);
    session.messages.splice(0..0, human.messages);
    push_session(world, session);
}

#[then(regex = r#"^the message is a compaction summary$"#)]
fn message_is_compaction_summary(world: &mut BotSpyWorld) {
    assert!(last_message(world).is_compaction_summary);
}
