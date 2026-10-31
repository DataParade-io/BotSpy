//! Steps for the Claude Code importer (spec 03_importers/claude_code/adapter.feature).

use crate::steps::BotSpyWorld;
use botspy::adapters::claude_code::ClaudeCodeSource;
use cucumber::{given, then, when};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static UNIQUE_DIR: AtomicUsize = AtomicUsize::new(0);

const PARTIAL_PREFIX: &str = r#"{"type":"user","uuid":"uuid-3","parentUuid":"","timestamp":"2026-10-01T09:00:30Z","message":{"role":"user","content":"later""#;
const PARTIAL_REST: &str = "}}\n";

fn new_root(world: &mut BotSpyWorld) -> PathBuf {
    let base = std::env::temp_dir().join("botspy-cc").join(format!(
        "{}-{}",
        std::process::id(),
        UNIQUE_DIR.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&base).expect("create Claude Code source root");
    world.cc_root = Some(base.clone());
    base
}

fn project_dir(root: &Path) -> PathBuf {
    let dir = root.join("proj-a");
    std::fs::create_dir_all(&dir).expect("create Claude Code project dir");
    dir
}

fn user_record(uuid: &str, parent: &str, timestamp: &str, text: &str) -> String {
    format!(
        r#"{{"type":"user","uuid":"{uuid}","parentUuid":"{parent}","timestamp":"{timestamp}","message":{{"role":"user","content":"{text}"}}}}"#
    )
}

fn stamp(index: usize) -> String {
    format!("2026-10-01T09:00:{:02}Z", index % 60)
}

fn write_transcript(path: &Path, lines: &[String], partial: Option<&str>) {
    let mut body: String = lines.iter().map(|line| format!("{line}\n")).collect();
    if let Some(partial_line) = partial {
        body.push_str(partial_line);
    }
    std::fs::write(path, body).expect("write Claude Code transcript");
}

fn append_to(path: &Path, text: &str) {
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .expect("open Claude Code transcript for append");
    file.write_all(text.as_bytes())
        .expect("append to Claude Code transcript");
}

fn transcript_path(world: &BotSpyWorld) -> PathBuf {
    let root = world.cc_root.as_ref().expect("no Claude Code source root");
    let name = world
        .cc_transcript
        .as_deref()
        .expect("no Claude Code fixture transcript");
    project_dir(root).join(name)
}

fn session_id_of(world: &BotSpyWorld) -> String {
    world
        .cc_transcript
        .as_deref()
        .expect("no Claude Code fixture transcript")
        .trim_end_matches(".jsonl")
        .to_string()
}

fn finish_setup(world: &mut BotSpyWorld, transcript: String) {
    let root = world.cc_root.clone().expect("no Claude Code source root");
    world.cc_transcript = Some(transcript);
    world.cc_source = Some(ClaudeCodeSource::new(root));
}

#[given(regex = r#"^a Claude Code source root with project transcripts "([^"]+)" and "([^"]+)"$"#)]
fn root_with_transcripts(world: &mut BotSpyWorld, a: String, b: String) {
    let dir = project_dir(&new_root(world));
    write_transcript(
        &dir.join(&a),
        &[user_record("uuid-1", "", &stamp(0), "hello from a")],
        None,
    );
    write_transcript(
        &dir.join(&b),
        &[user_record("uuid-2", "", &stamp(5), "hello from b")],
        None,
    );
    world.cc_source = Some(ClaudeCodeSource::new(world.cc_root.clone().expect("root")));
}

#[given(regex = r#"^a Claude Code transcript "([^"]+)" with ([0-9]+) records$"#)]
fn transcript_with_records(world: &mut BotSpyWorld, name: String, count: usize) {
    let dir = project_dir(&new_root(world));
    let lines: Vec<String> = (0..count)
        .map(|i| user_record(&format!("uuid-{i}"), "", &stamp(i), &format!("record {i}")))
        .collect();
    write_transcript(&dir.join(&name), &lines, None);
    finish_setup(world, name);
}

#[given(
    regex = r#"^a Claude Code transcript "([^"]+)" holding a user record "([^"]+)" with parent "([^"]+)"$"#
)]
fn transcript_with_parent(world: &mut BotSpyWorld, name: String, uuid: String, parent: String) {
    let dir = project_dir(&new_root(world));
    write_transcript(
        &dir.join(&name),
        &[user_record(&uuid, &parent, &stamp(0), "hello")],
        None,
    );
    finish_setup(world, name);
}

#[given(
    regex = r#"^a Claude Code transcript "([^"]+)" holding "([^"]*)", "([^"]*)", and "([^"]*)" records$"#
)]
fn transcript_with_aux_records(
    world: &mut BotSpyWorld,
    name: String,
    first: String,
    second: String,
    third: String,
) {
    let dir = project_dir(&new_root(world));
    let mut lines = Vec::new();
    for ty in [&first, &second, &third] {
        lines.push(match ty.as_str() {
            "cost-state" => {
                r#"{"type":"cost-state","totalCostUsd":0.42,"totalApiDurationMs":1200}"#.to_string()
            }
            "pr-link" => r#"{"type":"pr-link","url":"https://github.com/x/y/pull/7"}"#.to_string(),
            "custom-title" => r#"{"type":"custom-title","title":"Fix parser"}"#.to_string(),
            other => panic!("unknown auxiliary record type {other}"),
        });
    }
    write_transcript(&dir.join(&name), &lines, None);
    finish_setup(world, name);
}

#[given(
    regex = r#"^a Claude Code transcript "([^"]+)" with ([0-9]+) records ending in a partial trailing line$"#
)]
fn transcript_with_partial(world: &mut BotSpyWorld, name: String, count: usize) {
    let dir = project_dir(&new_root(world));
    let lines: Vec<String> = (0..count)
        .map(|i| user_record(&format!("uuid-{i}"), "", &stamp(i), &format!("record {i}")))
        .collect();
    write_transcript(&dir.join(&name), &lines, Some(PARTIAL_PREFIX));
    finish_setup(world, name);
}

#[when(regex = r#"^the Claude Code importer detects changes$"#)]
fn detect_changes(world: &mut BotSpyWorld) {
    let source = world.cc_source.as_ref().expect("no Claude Code source");
    let (summaries, _) = source.discover();
    world.discovered = summaries;
}

#[when(regex = r#"^the Claude Code importer extracts the transcript(?: again)?$"#)]
fn extract_transcript(world: &mut BotSpyWorld) {
    let source = world.cc_source.as_ref().expect("no Claude Code source");
    let id = session_id_of(world);
    let extraction = source
        .extract(&id)
        .expect("no transcript for the Claude Code session");
    world.cc_extraction = Some(extraction);
}

#[when(regex = r#"^([0-9]+) more records are appended to the transcript$"#)]
fn append_records(world: &mut BotSpyWorld, count: usize) {
    let path = transcript_path(world);
    let lines: Vec<String> = (0..count)
        .map(|i| {
            user_record(
                &format!("uuid-appended-{i}"),
                "",
                &stamp(i + 40),
                &format!("appended {i}"),
            )
        })
        .collect();
    let body: String = lines.iter().map(|line| format!("{line}\n")).collect();
    append_to(&path, &body);
}

#[when(regex = r#"^the file is completed with the rest of the record$"#)]
fn complete_partial_record(world: &mut BotSpyWorld) {
    append_to(&transcript_path(world), PARTIAL_REST);
}

#[then(regex = r#"^extraction yields ([0-9]+) messages?$"#)]
fn extraction_yields_messages(world: &mut BotSpyWorld, count: usize) {
    let extraction = world
        .cc_extraction
        .as_ref()
        .expect("no Claude Code extraction");
    assert_eq!(
        extraction.good, count,
        "extraction mapped the wrong messages"
    );
}

#[then(regex = r#"^extraction yields ([0-9]+) more messages?$"#)]
fn extraction_yields_more_messages(world: &mut BotSpyWorld, count: usize) {
    extraction_yields_messages(world, count);
}

#[then(
    regex = r#"^the first message's provenance has record_id "([^"]+)" and parent record "([^"]+)"$"#
)]
fn first_message_provenance(world: &mut BotSpyWorld, record_id: String, parent: String) {
    let extraction = world
        .cc_extraction
        .as_ref()
        .expect("no Claude Code extraction");
    let message = extraction.session.messages.first().expect("no messages");
    let provenance = message.provenance.as_ref().expect("no provenance");
    assert_eq!(provenance.record_id.as_deref(), Some(record_id.as_str()));
    assert_eq!(provenance.parent_record.as_deref(), Some(parent.as_str()));
}

#[then(regex = r#"^the session carries the cost, the PR link, and the title$"#)]
fn session_carries_aux_state(world: &mut BotSpyWorld) {
    let extraction = world
        .cc_extraction
        .as_ref()
        .expect("no Claude Code extraction");
    let session = &extraction.session;
    assert!(
        session.cost.is_some(),
        "the cost-state record did not land in session state"
    );
    assert!(
        session.metadata.pr_url.is_some(),
        "the pr-link record did not land in session state"
    );
    assert!(
        session.metadata.title.is_some(),
        "the custom-title record did not land in session state"
    );
}

#[then(regex = r#"^([0-9]+) partial trailing lines? (?:is|are) held back$"#)]
fn partial_lines_held_back(world: &mut BotSpyWorld, count: usize) {
    let extraction = world
        .cc_extraction
        .as_ref()
        .expect("no Claude Code extraction");
    assert_eq!(
        extraction.pending_partial, count,
        "the wrong number of partial trailing lines was held back"
    );
}

// ---- sub-agents ----------------------------------------------------------

/// Write `<project>/<parent>/subagents/agent-<agent>.jsonl` (two sidechain
/// records: the parent's prompt, then the sub-agent's reply) and, when
/// given, its `.meta.json` with the agent type and spawn depth.
pub fn write_subagent(project: &Path, parent: &str, agent: &str, meta: Option<(&str, u64)>) {
    let dir = project.join(parent).join("subagents");
    std::fs::create_dir_all(&dir).expect("create subagents dir");
    let prompt = format!(
        r#"{{"isSidechain":true,"agentId":"{agent}","sessionId":"{parent}","type":"user","uuid":"{agent}-1","parentUuid":"","timestamp":"2026-10-01T09:01:00Z","message":{{"role":"user","content":"the prompt the parent sent"}}}}"#
    );
    let reply = format!(
        r#"{{"isSidechain":true,"agentId":"{agent}","sessionId":"{parent}","type":"assistant","uuid":"{agent}-2","parentUuid":"{agent}-1","timestamp":"2026-10-01T09:02:00Z","message":{{"role":"assistant","content":[{{"type":"text","text":"finished"}}]}}}}"#
    );
    std::fs::write(
        dir.join(format!("agent-{agent}.jsonl")),
        format!("{prompt}\n{reply}\n"),
    )
    .expect("write sub-agent transcript");
    if let Some((kind, depth)) = meta {
        std::fs::write(
            dir.join(format!("agent-{agent}.meta.json")),
            format!(
                r#"{{"agentType":"{kind}","description":"look around","toolUseId":"toolu_1","spawnDepth":{depth},"model":"model-1"}}"#
            ),
        )
        .expect("write sub-agent meta");
    }
}

/// The parent transcript `<name>` for sub-agent scenarios: one human
/// message then one reply, bracketing the sub-agent's work.
pub fn write_parent_transcript(project: &Path, name: &str) {
    let reply = r#"{"type":"assistant","uuid":"p-2","parentUuid":"p-1","timestamp":"2026-10-01T09:10:00Z","message":{"role":"assistant","content":[{"type":"text","text":"all done"}]}}"#;
    write_transcript(
        &project.join(name),
        &[
            user_record("p-1", "", "2026-10-01T09:00:00Z", "start"),
            reply.to_string(),
        ],
        None,
    );
}

#[given(
    regex = r#"^a Claude Code session transcript "([^"]+)" with sub-agent "([^"]+)" of type "([^"]+)" at depth ([0-9]+)$"#
)]
fn transcript_with_subagent(
    world: &mut BotSpyWorld,
    name: String,
    agent: String,
    kind: String,
    depth: u64,
) {
    let dir = project_dir(&new_root(world));
    write_transcript(
        &dir.join(&name),
        &[user_record("uuid-1", "", &stamp(0), "mainline work")],
        None,
    );
    let parent = name.trim_end_matches(".jsonl");
    write_subagent(&dir, parent, &agent_id(&agent), Some((&kind, depth)));
    finish_setup(world, name);
}

#[given(
    regex = r#"^a Claude Code session transcript "([^"]+)" with sub-agent "([^"]+)" and no meta file$"#
)]
fn transcript_with_subagent_no_meta(world: &mut BotSpyWorld, name: String, agent: String) {
    let dir = project_dir(&new_root(world));
    write_transcript(
        &dir.join(&name),
        &[user_record("uuid-1", "", &stamp(0), "mainline work")],
        None,
    );
    let parent = name.trim_end_matches(".jsonl");
    write_subagent(&dir, parent, &agent_id(&agent), None);
    finish_setup(world, name);
}

/// The scenario names a sub-agent by its file stem, `agent-<id>`.
fn agent_id(stem: &str) -> String {
    stem.trim_start_matches("agent-").to_string()
}

#[given(
    regex = r#"^a Claude Code transcript "([^"]+)" holding a user record and a stray sidechain record$"#
)]
fn transcript_with_stray_sidechain(world: &mut BotSpyWorld, name: String) {
    let dir = project_dir(&new_root(world));
    let stray = user_record("uuid-2", "", &stamp(1), "side work").replacen(
        "{\"type\"",
        "{\"isSidechain\":true,\"type\"",
        1,
    );
    write_transcript(
        &dir.join(&name),
        &[user_record("uuid-1", "", &stamp(0), "mainline work"), stray],
        None,
    );
    finish_setup(world, name);
}

#[when(regex = r#"^the Claude Code importer extracts the sub-agent session "([^"]+)"$"#)]
fn extract_subagent(world: &mut BotSpyWorld, id: String) {
    let source = world.cc_source.as_ref().expect("no Claude Code source");
    world.cc_extraction = Some(source.extract(&id).expect("no such sub-agent session"));
}

fn extracted(world: &BotSpyWorld) -> &botspy::Session {
    &world
        .cc_extraction
        .as_ref()
        .expect("no Claude Code extraction")
        .session
}

#[then(regex = r#"^discovery lists the session "([^"]+)" in project "([^"]+)"$"#)]
fn discovery_lists_session(world: &mut BotSpyWorld, id: String, project: String) {
    let found = world
        .discovered
        .iter()
        .find(|summary| summary.id == id)
        .unwrap_or_else(|| panic!("discovery did not list {id}"));
    assert_eq!(found.project_id, project);
}

#[then(regex = r#"^the doctor report counts ([0-9]+) sub-agents?$"#)]
fn doctor_counts_subagents(world: &mut BotSpyWorld, count: usize) {
    let report = world.cc_source.as_ref().expect("source").doctor();
    assert_eq!(report.subagents, count, "{report:?}");
}

#[then(regex = r#"^the doctor report counts ([0-9]+) transcripts?$"#)]
fn doctor_counts_transcripts(world: &mut BotSpyWorld, count: usize) {
    let report = world.cc_source.as_ref().expect("source").doctor();
    assert_eq!(report.transcripts, count, "{report:?}");
}

#[then(regex = r#"^the session is spawned by "([^"]+)"$"#)]
fn session_spawned_by(world: &mut BotSpyWorld, parent: String) {
    assert_eq!(extracted(world).parent_id.as_deref(), Some(parent.as_str()));
}

#[then(regex = r#"^the session is a "([^"]+)" sub-agent at depth ([0-9]+)$"#)]
fn session_is_subagent(world: &mut BotSpyWorld, kind: String, depth: u64) {
    let info = extracted(world).subagent.clone().expect("a sub-agent");
    assert_eq!(info.kind, kind);
    assert_eq!(info.nesting_depth, Some(depth));
}

#[then(regex = r#"^the session is a "([^"]+)" sub-agent of unknown depth$"#)]
fn session_is_subagent_unknown_depth(world: &mut BotSpyWorld, kind: String) {
    let info = extracted(world).subagent.clone().expect("a sub-agent");
    assert_eq!(info.kind, kind);
    assert_eq!(info.nesting_depth, None);
}

#[then(regex = r#"^the session is titled "([^"]+)"$"#)]
fn session_titled(world: &mut BotSpyWorld, title: String) {
    assert_eq!(
        extracted(world).metadata.title.as_deref(),
        Some(title.as_str())
    );
}

#[then(regex = r#"^the session holds ([0-9]+) messages?$"#)]
fn session_holds_messages(world: &mut BotSpyWorld, count: usize) {
    assert_eq!(extracted(world).messages.len(), count);
}

#[then(regex = r#"^the session has no parent and is not a sub-agent$"#)]
fn session_is_a_root(world: &mut BotSpyWorld) {
    let session = extracted(world);
    assert_eq!(session.parent_id, None);
    assert_eq!(session.subagent, None);
}

#[then(regex = r#"^([0-9]+) records? (?:is|are) skipped as unknown$"#)]
fn records_skipped_unknown(world: &mut BotSpyWorld, count: u64) {
    let extraction = world.cc_extraction.as_ref().expect("extraction");
    assert_eq!(extraction.skipped.unknown, count);
}
