//! Steps for the local store (spec 02_querying/04_store.feature and the
//! store specs that follow it in features/02_querying/).

use crate::steps::{adapter_for, parse_agent, BotSpyWorld};
use botspy::store::embed::Embedder;
use botspy::{
    Adapter, Agent, KnownPart, Message, MessageFilter, Part, Role, SearchHit, Session,
    SessionFilter,
};
use botspy::{IngestOptions, Store};
use cucumber::{given, then, when};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Store fixture paths live under the test target dir, so scenarios never
/// litter the crate and never collide across reruns.
fn store_path(path: &str) -> PathBuf {
    std::env::var_os("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(path)
}

/// Monotonic scope for derived store paths: the runner executes scenarios
/// concurrently, so two scenarios that registered the same adapters would
/// otherwise derive the same file name and delete each other's store
/// mid-scenario. One increment per store creation keeps every scenario's
/// store in its own file (the `scratch(tag)` pattern from the CLI steps).
fn scenario_scope() -> usize {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static STORE_SEQ: AtomicUsize = AtomicUsize::new(0);
    STORE_SEQ.fetch_add(1, Ordering::SeqCst)
}

/// A plain fixture session with one text message.
fn fixture_session(id: &str, agent: Agent, project: &str, last_activity: &str) -> Session {
    Session {
        id: id.to_string(),
        agent,
        project_id: project.to_string(),
        started_at: last_activity.to_string(),
        last_activity_at: last_activity.to_string(),
        messages: vec![Message {
            role: Role::User,
            parts: vec![Part::Known(KnownPart::Text {
                text: format!("hello from {id}"),
                extra: None,
            })],
            timestamp: Some(last_activity.to_string()),
            ..Message::default()
        }],
        ..Session::default()
    }
}

/// The store file plus its WAL sidecars, so a scenario can start clean.
fn store_files(path: &Path) -> Vec<PathBuf> {
    let stem = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut files = vec![path.to_path_buf()];
    for suffix in ["-wal", "-shm"] {
        files.push(path.with_file_name(format!("{stem}{suffix}")));
    }
    files
}

/// Remove the store file and its WAL sidecars.
fn remove_store_files_at(path: &Path) {
    for candidate in store_files(path) {
        let _ = std::fs::remove_file(candidate);
    }
}

fn remove_store_files(path: &str) {
    remove_store_files_at(&store_path(path));
}

/// Every fixture session known to the world's adapters, in stable order.
fn all_fixture_sessions(world: &BotSpyWorld) -> Vec<Session> {
    let mut sessions = Vec::new();
    for adapter in world.adapters.values() {
        for summary in adapter.discover() {
            if let Some(session) = adapter.open(&summary.id) {
                sessions.push(session);
            }
        }
    }
    sessions
}

// 04_store.feature — opening, persistence, concurrent readers, errors.

#[given(regex = r#"^no store exists at "([^"]+)"$"#)]
fn no_store_exists_at(_world: &mut BotSpyWorld, path: String) {
    remove_store_files(&path);
}

/// Open a store at the given path (relative to the scenario temp dir),
/// recording a typed open error instead of panicking.
fn open_store_recording_error(world: &mut BotSpyWorld, path: &str) {
    world.local_open_error = None;
    match Store::open(store_path(path)) {
        Ok(store) => world.local_store = Some(store),
        Err(err) => world.local_open_error = Some(err),
    }
}

#[when(regex = r#"^I open a store at "([^"]+)"$"#)]
fn open_store_at(world: &mut BotSpyWorld, path: String) {
    open_store_recording_error(world, &path);
}

#[then(regex = r#"^the store file exists at "([^"]+)"$"#)]
fn store_file_exists_at(world: &mut BotSpyWorld, path: String) {
    assert!(world.local_store.is_some(), "no store was opened");
    assert!(
        store_path(&path).exists(),
        "the store file does not exist at {path}"
    );
}

#[given(regex = r#"^an empty fixture adapter for "([^"]+)"$"#)]
fn empty_fixture_adapter(world: &mut BotSpyWorld, agent: String) {
    adapter_for(world, &agent);
}

#[given(regex = r#"^fixture sessions "([^"]+)" from "([^"]+)" and "([^"]+)" from "([^"]+)"$"#)]
fn fixture_sessions_two_agents(
    world: &mut BotSpyWorld,
    id1: String,
    agent1: String,
    id2: String,
    agent2: String,
) {
    for (id, agent) in [(&id1, &agent1), (&id2, &agent2)] {
        let adapter = adapter_for(world, agent);
        adapter.add_session(fixture_session(
            id,
            parse_agent(agent),
            "",
            "2026-10-01T09:00:00Z",
        ));
    }
}

#[given(regex = r#"^fixture sessions "([^"]+)", "([^"]+)", and "([^"]+)" from "([^"]+)"$"#)]
fn fixture_sessions_three(
    world: &mut BotSpyWorld,
    id1: String,
    id2: String,
    id3: String,
    agent: String,
) {
    for id in [&id1, &id2, &id3] {
        let adapter = adapter_for(world, &agent);
        adapter.add_session(fixture_session(
            id,
            parse_agent(&agent),
            "",
            "2026-10-01T09:00:00Z",
        ));
    }
}

#[given(regex = r#"^a store at "([^"]+)" with those sessions ingested$"#)]
fn store_with_sessions_ingested(world: &mut BotSpyWorld, path: String) {
    remove_store_files(&path);
    let store = Store::open(store_path(&path)).expect("store opens for ingest");
    store
        .ingest_sessions(all_fixture_sessions(world))
        .expect("fixture sessions ingest");
    world.local_store = Some(store);
}

#[when(regex = r#"^I reopen a store at "([^"]+)"$"#)]
fn reopen_store_at(world: &mut BotSpyWorld, path: String) {
    open_store_recording_error(world, &path);
}

#[when(regex = r#"^I iterate the store's sessions$"#)]
fn iterate_store_sessions(world: &mut BotSpyWorld) {
    let store = world.local_store.as_ref().expect("no store is open");
    world.local_sessions = store.sessions().collect();
}

#[then(regex = r#"^the store iteration yields ([0-9]+) sessions?$"#)]
fn store_iteration_yields_count(world: &mut BotSpyWorld, count: usize) {
    assert_eq!(
        world.local_sessions.len(),
        count,
        "the store iteration yielded the wrong number of sessions"
    );
}

/// The quoted tokens of `"a", "b", and "c"` in order.
fn quoted_list(text: &str) -> Vec<String> {
    text.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

#[then(regex = r#"^the store iteration yields sessions (.+)$"#)]
fn store_iteration_yields_ids(world: &mut BotSpyWorld, ids: String) {
    let expected = quoted_list(&ids);
    let actual: Vec<String> = world
        .local_sessions
        .iter()
        .map(|summary| summary.id.clone())
        .collect();
    assert_eq!(actual, expected, "the store iteration is out of order");
}

#[given(
    regex = r#"^a second reader is open on the store at "([^"]+)" and has taken its first session$"#
)]
fn second_reader_took_first(world: &mut BotSpyWorld, path: String) {
    let reader = Store::open(store_path(&path)).expect("second reader opens");
    let mut iter = reader.sessions();
    let first = iter.next().expect("the reader sees the ingested sessions");
    world.local_reader = Some(reader);
    world.local_reader_iter = Some(iter);
    world.local_sessions = vec![first];
    world.local_reader_finished = false;
}

#[when(regex = r#"^a new fixture session "([^"]+)" is ingested through the writer$"#)]
fn ingest_new_session_through_writer(world: &mut BotSpyWorld, id: String) {
    let agent = "claude_code";
    let adapter = adapter_for(world, agent);
    let session = fixture_session(&id, parse_agent(agent), "", "2026-10-01T12:00:00Z");
    adapter.add_session(session.clone());
    let writer = world.local_store.as_ref().expect("no writer store is open");
    writer
        .ingest_sessions(vec![session])
        .expect("the writer ingests while the reader is mid-iteration");
}

#[when(regex = r#"^the reader continues its iteration$"#)]
fn reader_continues_iteration(world: &mut BotSpyWorld) {
    let iter = world
        .local_reader_iter
        .as_mut()
        .expect("the reader has no open iteration");
    for summary in iter.by_ref() {
        world.local_sessions.push(summary);
    }
    world.local_reader_finished = true;
}

#[then(regex = r#"^the reader finishes its iteration without error$"#)]
fn reader_finishes_without_error(world: &mut BotSpyWorld) {
    assert!(
        world.local_reader_finished,
        "the reader's iteration did not run to completion"
    );
}

#[given(regex = r#"^a file "([^"]+)" containing "([^"]+)"$"#)]
fn file_containing(_world: &mut BotSpyWorld, path: String, content: String) {
    let path = store_path(&path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent directory");
    }
    std::fs::write(path, content).expect("write the non-store file");
}

#[then(regex = r#"^opening fails with a store error, not a panic$"#)]
fn opening_fails_with_store_error(world: &mut BotSpyWorld) {
    assert!(
        world.local_store.is_none(),
        "opening a non-store file succeeded"
    );
    let err = world
        .local_open_error
        .as_ref()
        .expect("opening a non-store file should fail");
    assert!(
        matches!(err, botspy::store::StoreError::NotADatabase { .. }),
        "expected a typed not-a-database error, got {err:?}"
    );
}

#[given(regex = r#"^BOTSPY_HOME is "([^"]+)"$"#)]
fn botspy_home_is(world: &mut BotSpyWorld, home: String) {
    world.saved_botspy_home = std::env::var_os("BOTSPY_HOME");
    std::env::set_var("BOTSPY_HOME", store_path(&home));
}

#[when(regex = r#"^I open the default store$"#)]
fn open_default_store(world: &mut BotSpyWorld) {
    world.local_store = Some(Store::open_default().expect("the default store opens"));
    if let Some(previous) = world.saved_botspy_home.take() {
        std::env::set_var("BOTSPY_HOME", previous);
    } else {
        std::env::remove_var("BOTSPY_HOME");
    }
}

// 05_ingest.feature — adapter-driven ingestion into the store (red until
// BOTSPY-a48fb1), plus the 01_iteration.feature steps (red until
// BOTSPY-a927fc): fixture adapters → store → ingest → iterate.

#[given(regex = r#"^a store at "([^"]+)"$"#)]
fn a_store_at(world: &mut BotSpyWorld, path: String) {
    let file = store_path(&format!("stores/{}-{path}", scenario_scope()));
    remove_store_files_at(&file);
    world.local_store = Some(Store::open(&file).expect("store opens"));
}

#[given(regex = r#"^I ingest the registered adapters into the store$"#)]
#[when(regex = r#"^I ingest the registered adapters into the store$"#)]
fn ingest_registered_adapters(world: &mut BotSpyWorld) {
    ensure_store_ingested(world);
    run_ingest_with(world, IngestOptions::default());
}

/// Every registered fixture adapter, as the store's ingestion surface
/// receives them.
fn registered_adapters(world: &BotSpyWorld) -> Vec<std::sync::Arc<dyn Adapter>> {
    world
        .adapters
        .values()
        .map(|adapter| {
            let owned: std::sync::Arc<dyn Adapter> = adapter.clone();
            owned
        })
        .collect()
}

#[then(regex = r#"^the ingest reports ([0-9]+) new sessions?$"#)]
fn ingest_reports_new_sessions(world: &mut BotSpyWorld, count: usize) {
    let report = world.ingest_report.expect("an ingest report");
    assert_eq!(report.new, count, "ingest report new-session count");
}

#[given(regex = r#"^fixture session "([^"]+)" from "([^"]+)" and "([^"]+)" from "([^"]+)"$"#)]
fn fixture_session_two_adapters(
    world: &mut BotSpyWorld,
    id1: String,
    agent1: String,
    id2: String,
    agent2: String,
) {
    adapter_for(world, &agent1).add_session(fixture_session(
        &id1,
        parse_agent(&agent1),
        "demo",
        "2026-10-01T09:00:00Z",
    ));
    adapter_for(world, &agent2).add_session(fixture_session(
        &id2,
        parse_agent(&agent2),
        "demo",
        "2026-10-01T09:00:00Z",
    ));
}

#[when(regex = r#"^I open the stored session "([^"]+)"$"#)]
fn open_session_from_store(world: &mut BotSpyWorld, id: String) {
    let store = world.local_store.as_ref().expect("a store");
    world.local_opened = Some(store.open_session(&id));
}

#[then(regex = r#"^it equals the session the adapter returns$"#)]
fn opened_equals_adapter_session(world: &mut BotSpyWorld) {
    let opened = world
        .local_opened
        .as_ref()
        .expect("an opened session")
        .as_ref()
        .expect("opening succeeds");
    let id = world.current_session.as_deref().expect("a current session");
    let expected = crate::steps::find_session(world, id).expect("fixture session");
    assert_eq!(opened, &expected, "lossless round-trip");
}

fn adapters_sessions_digest(world: &BotSpyWorld) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for adapter in world.adapters.values() {
        for summary in adapter.discover() {
            if let Some(session) = adapter.open(&summary.id) {
                let json = serde_json::to_string(&session).expect("session serializes");
                hasher.update(json.as_bytes());
            }
        }
    }
    format!("{:x}", hasher.finalize())
}

#[when(regex = r#"^I snapshot the adapters' sessions digest$"#)]
fn snapshot_sessions_digest(world: &mut BotSpyWorld) {
    world.local_digest_before = Some(adapters_sessions_digest(world));
}

#[when(regex = r#"^I snapshot the adapters' sessions digest again$"#)]
fn snapshot_sessions_digest_again(world: &mut BotSpyWorld) {
    world.local_digest_after = Some(adapters_sessions_digest(world));
}

#[then(regex = r#"^the sources are unchanged by the ingest$"#)]
fn sources_unchanged_by_ingest(world: &mut BotSpyWorld) {
    let before = world.local_digest_before.as_ref().expect("digest before");
    let after = world.local_digest_after.as_ref().expect("digest after");
    assert_eq!(before, after, "sources must stay read-only");
}

// 01_iteration.feature — cross-source iteration through the store, plus
// 03_laziness.feature — streaming discipline, proven with the query
// counters (the importer protocol's peak_buffered/SkipCounter precedent).

/// A store holding every fixture session so far, opened on first use: the
/// iteration specs describe behavior, the harness hides the plumbing. The
/// path is derived from the registered adapters so parallel scenarios with
/// different fixtures never collide.
fn ensure_store_ingested(world: &mut BotSpyWorld) {
    if world.local_store.is_some() {
        return;
    }
    let key: String = world.adapters.keys().cloned().collect::<Vec<_>>().join("-");
    let path = format!("iteration/{}-{key}.db", scenario_scope());
    remove_store_files(&path);
    let store = open_scenario_store(world, &path);
    store
        .ingest_sessions(all_fixture_sessions(world))
        .expect("fixture sessions ingest");
    world.local_store = Some(store);
}

/// Open the scenario's store, honoring an embedder a prior step asked
/// for (the shipped MiniLM, loaded from the models cache setup fetched).
fn open_scenario_store(world: &mut BotSpyWorld, path: &str) -> Store {
    match world.local_embedder_model.take() {
        Some(model) => {
            assert_eq!(
                model,
                botspy::store::assets::MODEL_ID,
                "the specs exercise the shipped embedder"
            );
            let embedder = botspy::store::embed_minilm::MiniLMEmbedder::open().expect(
                "the MiniLM embedder loads from the models cache \
                 (run `cargo run --example setup_models` once)",
            );
            Store::open_with_embedder(store_path(path), Arc::new(embedder))
                .expect("the store opens with its embedder")
        }
        None => Store::open(store_path(path)).expect("store opens"),
    }
}

#[given(
    regex = r#"^fixture sessions "([^"]+)" from "([^"]+)" and "([^"]+)" from "([^"]+)" and "([^"]+)" from "([^"]+)"$"#
)]
#[allow(clippy::too_many_arguments)]
fn fixture_sessions_three_agents(
    world: &mut BotSpyWorld,
    id1: String,
    agent1: String,
    id2: String,
    agent2: String,
    id3: String,
    agent3: String,
) {
    for (id, agent) in [(&id1, &agent1), (&id2, &agent2), (&id3, &agent3)] {
        let adapter = adapter_for(world, agent);
        adapter.add_session(fixture_session(
            id,
            parse_agent(agent),
            "demo",
            "2026-10-01T09:00:00Z",
        ));
    }
}

#[when(regex = r#"^I iterate the sessions$"#)]
fn iterate_the_sessions(world: &mut BotSpyWorld) {
    ensure_store_ingested(world);
    let store = world.local_store.as_ref().expect("no store is open");
    let query = store.query();
    world.local_sessions = query.sessions().collect();
    world.local_query_counters = Some(query.counters());
}

#[then(regex = r#"^the iteration yields ([0-9]+) sessions?$"#)]
fn iteration_yields_count(world: &mut BotSpyWorld, count: usize) {
    assert_eq!(
        world.local_sessions.len(),
        count,
        "the session iteration yielded the wrong number of sessions"
    );
}

#[then(regex = r#"^the iteration yields sessions (.+)$"#)]
fn iteration_yields_ids(world: &mut BotSpyWorld, ids: String) {
    let expected = quoted_list(&ids);
    let actual: Vec<String> = world
        .local_sessions
        .iter()
        .map(|summary| summary.id.clone())
        .collect();
    assert_eq!(actual, expected, "the session iteration is out of order");
}

#[given(regex = r#"^the session has messages (.+)$"#)]
fn session_has_messages(world: &mut BotSpyWorld, texts: String) {
    let mut session = crate::steps::current_session(world);
    for text in quoted_list(&texts) {
        session.messages.push(Message {
            role: Role::User,
            parts: vec![Part::Known(KnownPart::Text { text, extra: None })],
            timestamp: Some("2026-10-01T09:00:00Z".to_string()),
            ..Message::default()
        });
    }
    crate::steps::save_session(world, session);
}

#[when(regex = r#"^I iterate the messages of session "([^"]+)"$"#)]
fn iterate_messages_of_session(world: &mut BotSpyWorld, id: String) {
    ensure_store_ingested(world);
    let store = world.local_store.as_ref().expect("no store is open");
    let query = store.query();
    world.local_messages = query.messages(&id).collect();
    world.local_query_counters = Some(query.counters());
}

fn first_text(message: &Message) -> String {
    message
        .parts
        .iter()
        .find_map(|part| match part {
            Part::Known(KnownPart::Text { text, .. }) => Some(text.clone()),
            _ => None,
        })
        .expect("the message has a text part")
}

#[then(regex = r#"^the iteration yields the texts (.+)(?: in that order)?$"#)]
fn iteration_yields_texts(world: &mut BotSpyWorld, texts: String) {
    let expected = quoted_list(&texts);
    let actual: Vec<String> = world.local_messages.iter().map(first_text).collect();
    assert_eq!(actual, expected, "the message iteration is out of order");
}

#[given(regex = r#"^the session has a message with parts (.+)$"#)]
fn session_has_message_with_parts(world: &mut BotSpyWorld, kinds: String) {
    let parts: Vec<Part> = quoted_list(&kinds)
        .iter()
        .map(|kind| part_of_kind(kind))
        .collect();
    let mut session = crate::steps::current_session(world);
    session.messages.push(Message {
        role: Role::Assistant,
        parts,
        timestamp: Some("2026-10-01T09:00:00Z".to_string()),
        ..Message::default()
    });
    crate::steps::save_session(world, session);
}

#[when(regex = r#"^I iterate the parts of the last message of session "([^"]+)"$"#)]
fn iterate_parts_of_last_message(world: &mut BotSpyWorld, id: String) {
    ensure_store_ingested(world);
    let store = world.local_store.as_ref().expect("no store is open");
    let query = store.query();
    let message_count = query.messages(&id).count();
    let last_ordinal = message_count - 1;
    world.local_message_filter = None;
    world.local_parts = query
        .parts(&id, last_ordinal)
        .expect("the last message exists")
        .collect();
    world.local_query_counters = Some(query.counters());
}

#[then(regex = r#"^the iteration yields the kinds (.+)(?: in that order)?$"#)]
fn iteration_yields_kinds(world: &mut BotSpyWorld, kinds: String) {
    let expected = quoted_list(&kinds);
    let actual: Vec<String> = match &world.local_message_filter {
        // A filtered message iteration: each yielded message carries the
        // part kind it was selected by.
        Some(filter) => world
            .local_messages
            .iter()
            .map(|message| {
                message
                    .parts
                    .iter()
                    .find_map(|part| {
                        let kind = part.kind_name()?;
                        (kind == *filter).then(|| kind.to_string())
                    })
                    .expect("the yielded message has the filtered kind")
            })
            .collect(),
        // A parts iteration over one message.
        None => world
            .local_parts
            .iter()
            .map(|part| part.kind_name().unwrap_or("extra").to_string())
            .collect(),
    };
    assert_eq!(actual, expected, "the iteration is out of order");
}

// 03_laziness.feature — only what was consumed was touched.

/// Consume the first `count` sessions of a fresh iteration, opening each:
/// the laziness specs assert on the query counters either way.
fn take_and_open_sessions(world: &mut BotSpyWorld, count: usize) {
    ensure_store_ingested(world);
    let store = world.local_store.as_ref().expect("no store is open");
    let (summaries, opened_id, counters) = {
        let query = store.query();
        let mut summaries = Vec::new();
        let mut opened_id = None;
        for summary in query.sessions().take(count) {
            let session = query.open(&summary.id).expect("the session opens");
            opened_id = Some(session.id);
            summaries.push(summary);
        }
        (summaries, opened_id, query.counters())
    };
    world.local_sessions = summaries;
    world.local_opened_id = opened_id;
    world.local_query_counters = Some(counters);
}

#[when(regex = r#"^I take the first ([0-9]+) sessions? from the iteration$"#)]
fn take_first_sessions(world: &mut BotSpyWorld, count: usize) {
    take_and_open_sessions(world, count);
}

#[when(regex = r#"^I iterate ([0-9]+) sessions? and drop the iterator$"#)]
fn iterate_and_drop(world: &mut BotSpyWorld, count: usize) {
    take_and_open_sessions(world, count);
}

#[then(regex = r#"^only session "([^"]+)" was opened$"#)]
fn only_session_was_opened(world: &mut BotSpyWorld, id: String) {
    let counters = world.local_query_counters.expect("query counters");
    assert_eq!(counters.opens, 1, "exactly one session was opened");
    assert_eq!(
        world.local_opened_id.as_deref(),
        Some(id.as_str()),
        "the opened session"
    );
}

#[then(regex = r#"^the store performed only ([0-9]+) session opens?$"#)]
fn store_performed_session_opens(world: &mut BotSpyWorld, count: usize) {
    let counters = world.local_query_counters.expect("query counters");
    assert_eq!(counters.opens, count, "session materializations");
}

#[given(regex = r#"^a fixture session "([^"]+)" from "([^"]+)" with ([0-9]+) messages$"#)]
fn fixture_session_with_n_messages(
    world: &mut BotSpyWorld,
    id: String,
    agent: String,
    count: usize,
) {
    let messages = (0..count)
        .map(|ordinal| Message {
            role: if ordinal % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            },
            parts: vec![Part::Known(KnownPart::Text {
                text: format!("message {ordinal}"),
                extra: None,
            })],
            timestamp: Some("2026-10-01T09:00:00Z".to_string()),
            ..Message::default()
        })
        .collect();
    let session = Session {
        id: id.clone(),
        agent: parse_agent(&agent),
        project_id: "demo".to_string(),
        started_at: "2026-10-01T09:00:00Z".to_string(),
        last_activity_at: "2026-10-01T09:00:00Z".to_string(),
        messages,
        ..Session::default()
    };
    adapter_for(world, &agent).add_session(session);
    world.current_session = Some(id);
}

#[when(regex = r#"^I iterate the first ([0-9]+) messages of session "([^"]+)"$"#)]
fn iterate_first_messages(world: &mut BotSpyWorld, count: usize, id: String) {
    ensure_store_ingested(world);
    let store = world.local_store.as_ref().expect("no store is open");
    let query = store.query();
    world.local_messages = query.messages(&id).take(count).collect();
    world.local_query_counters = Some(query.counters());
}

#[then(regex = r#"^the iteration yields ([0-9]+) messages?$"#)]
fn iteration_yields_messages(world: &mut BotSpyWorld, count: usize) {
    assert_eq!(
        world.local_messages.len(),
        count,
        "the message iteration yielded the wrong number of messages"
    );
}

#[then(regex = r#"^the session was not fully materialized$"#)]
fn session_not_fully_materialized(world: &mut BotSpyWorld) {
    let counters = world.local_query_counters.expect("query counters");
    let expected = world.local_messages.len();
    assert_eq!(
        counters.rows_fetched, expected,
        "only the consumed messages were read from the store"
    );
}

// 06_refresh.feature — incremental refresh (red until BOTSPY-d5b2c4).

#[when(regex = r#"^a fixture session "([^"]+)" from "([^"]+)" is registered$"#)]
fn fixture_session_registered(world: &mut BotSpyWorld, id: String, agent: String) {
    adapter_for(world, &agent).add_session(fixture_session(
        &id,
        parse_agent(&agent),
        "demo",
        "2026-10-01T09:00:00Z",
    ));
}

#[when(regex = r#"^I refresh the store$"#)]
fn refresh_the_store(world: &mut BotSpyWorld) {
    run_refresh_with(
        world,
        IngestOptions {
            prune: true,
            ..IngestOptions::default()
        },
    );
}

#[when(regex = r#"^I refresh only the "([^"]+)" adapter( without writing)?$"#)]
fn refresh_selected_adapter(world: &mut BotSpyWorld, agent: String, dry_run: String) {
    let adapters: Vec<Arc<dyn Adapter>> = registered_adapters(world)
        .into_iter()
        .filter(|adapter| adapter.agent() == parse_agent(&agent))
        .collect();
    assert_eq!(adapters.len(), 1, "exactly one selected fixture adapter");
    let report = world
        .local_store
        .as_ref()
        .expect("a store")
        .refresh_with(
            &adapters,
            &IngestOptions {
                prune: true,
                dry_run: !dry_run.is_empty(),
                ..IngestOptions::default()
            },
        )
        .expect("selected-source refresh succeeds");
    world.detailed_ingest_report = Some(report.clone());
    world.ingest_report = Some(report.totals);
}

#[when("I refresh with no adapters")]
fn refresh_no_adapters(world: &mut BotSpyWorld) {
    world.ingest_report = Some(
        world
            .local_store
            .as_ref()
            .expect("a store")
            .refresh(&[])
            .expect("empty refresh succeeds"),
    );
}

#[then(
    regex = r#"^the refresh reports ([0-9]+) new sessions?, ([0-9]+) updated, and ([0-9]+) pruned$"#
)]
fn refresh_reports(world: &mut BotSpyWorld, new: usize, updated: usize, pruned: usize) {
    let report = world.ingest_report.expect("a refresh report");
    assert_eq!(report.new, new, "refresh report new sessions");
    assert_eq!(report.updated, updated, "refresh report updated sessions");
    assert_eq!(report.pruned, pruned, "refresh report pruned sessions");
}

#[when(regex = r#"^session "([^"]+)" gets another message at "([^"]+)"$"#)]
fn session_gets_another_message(world: &mut BotSpyWorld, id: String, at: String) {
    let mut session = crate::steps::find_session(world, &id).expect("fixture session");
    session.messages.push(Message {
        role: Role::User,
        parts: vec![Part::Known(KnownPart::Text {
            text: "and one more".to_string(),
            extra: None,
        })],
        timestamp: Some(at.clone()),
        ..Message::default()
    });
    session.last_activity_at = at;
    crate::steps::save_session_by_id(world, session);
}

#[when(regex = r#"^session "([^"]+)" is removed from its adapter$"#)]
fn session_removed_from_adapter(world: &mut BotSpyWorld, id: String) {
    let adapter = crate::steps::adapter_owning(world, &id).expect("the session's adapter");
    assert!(adapter.remove_session(&id), "the session was removed");
}

#[when(regex = r#"^I snapshot the adapters' open count$"#)]
fn snapshot_open_count(world: &mut BotSpyWorld) {
    world.local_open_count_before = Some(
        world
            .adapters
            .values()
            .map(|adapter| adapter.open_count())
            .sum(),
    );
}

#[when(regex = r#"^I snapshot the adapters' open count again$"#)]
fn snapshot_open_count_again(world: &mut BotSpyWorld) {
    world.local_open_count_after = Some(
        world
            .adapters
            .values()
            .map(|adapter| adapter.open_count())
            .sum(),
    );
}

#[then(regex = r#"^the adapters served no session opens during the refresh$"#)]
fn adapters_served_no_opens(world: &mut BotSpyWorld) {
    let before = world.local_open_count_before.expect("open count before");
    let after = world.local_open_count_after.expect("open count after");
    assert_eq!(before, after, "unchanged sessions must not be re-opened");
}

// 05_ingest.feature and 06_refresh.feature option-carrying passes — a
// `since` cutoff, dry-run passes, prune-less refreshes — and the
// per-adapter report (BOTSPY-8ef5a5).

/// Run an ingest pass with options, recording both report forms.
fn run_ingest_with(world: &mut BotSpyWorld, options: IngestOptions) {
    let store = world.local_store.as_ref().expect("a store");
    let adapters = registered_adapters(world);
    let report = store
        .ingest_with(&adapters, &options)
        .expect("the ingest succeeds");
    world.detailed_ingest_report = Some(report.clone());
    world.ingest_report = Some(report.totals);
}

/// Run a refresh pass with options, recording both report forms.
fn run_refresh_with(world: &mut BotSpyWorld, options: IngestOptions) {
    let store = world.local_store.as_ref().expect("a store");
    let adapters = registered_adapters(world);
    let report = store
        .refresh_with(&adapters, &options)
        .expect("the refresh succeeds");
    world.detailed_ingest_report = Some(report.clone());
    world.ingest_report = Some(report.totals);
}

#[given(regex = r#"^I ingest the registered adapters into the store with a cutoff "([^"]+)"$"#)]
#[when(regex = r#"^I ingest the registered adapters into the store with a cutoff "([^"]+)"$"#)]
fn ingest_with_cutoff(world: &mut BotSpyWorld, cutoff: String) {
    run_ingest_with(
        world,
        IngestOptions {
            since: Some(cutoff),
            ..IngestOptions::default()
        },
    );
}

#[given(regex = r#"^I ingest the registered adapters into the store without writing$"#)]
#[when(regex = r#"^I ingest the registered adapters into the store without writing$"#)]
fn ingest_without_writing(world: &mut BotSpyWorld) {
    run_ingest_with(
        world,
        IngestOptions {
            dry_run: true,
            ..IngestOptions::default()
        },
    );
}

#[when(regex = r#"^I refresh the store with a cutoff "([^"]+)"$"#)]
fn refresh_with_cutoff(world: &mut BotSpyWorld, cutoff: String) {
    run_refresh_with(
        world,
        IngestOptions {
            since: Some(cutoff),
            ..IngestOptions::default()
        },
    );
}

#[when(regex = r#"^I refresh the store without pruning$"#)]
fn refresh_without_pruning(world: &mut BotSpyWorld) {
    run_refresh_with(world, IngestOptions::default());
}

#[when(regex = r#"^I refresh the store without writing$"#)]
fn refresh_without_writing(world: &mut BotSpyWorld) {
    run_refresh_with(
        world,
        IngestOptions {
            dry_run: true,
            ..IngestOptions::default()
        },
    );
}

#[when(regex = r#"^a fixture session "([^"]+)" last active at "([^"]+)" is registered$"#)]
fn fixture_session_registered_at(world: &mut BotSpyWorld, id: String, at: String) {
    adapter_for(world, "claude_code").add_session(fixture_session(
        &id,
        parse_agent("claude_code"),
        "demo",
        &at,
    ));
}

#[given(regex = r#"^session "([^"]+)" cannot be opened from its adapter$"#)]
fn block_session_open(world: &mut BotSpyWorld, id: String) {
    let adapter = crate::steps::adapter_owning(world, &id).expect("the session's adapter");
    adapter.block_open(&id);
}

fn detailed_report(world: &BotSpyWorld) -> &botspy::DetailedIngestReport {
    world
        .detailed_ingest_report
        .as_ref()
        .expect("a detailed ingest or refresh report")
}

fn adapter_row(world: &BotSpyWorld, agent: &str) -> botspy::AdapterIngestReport {
    let agent = parse_agent(agent);
    *detailed_report(world)
        .adapters
        .iter()
        .find(|row| row.agent == agent)
        .unwrap_or_else(|| panic!("no adapter row for {agent:?}"))
}

#[then(regex = r#"^the (ingest|refresh) reports ([0-9]+) skipped sessions?$"#)]
fn pass_reports_skipped(world: &mut BotSpyWorld, _pass: String, count: String) {
    let expected = count.parse::<usize>().expect("numeric count");
    assert_eq!(
        detailed_report(world).totals.skipped,
        expected,
        "the pass's skipped-session count"
    );
}

#[then(
    regex = r#"^the refresh reports ([0-9]+) new sessions?, ([0-9]+) updated, ([0-9]+) pruned, and ([0-9]+) skipped sessions?$"#
)]
fn refresh_reports_with_skipped(
    world: &mut BotSpyWorld,
    new: String,
    updated: String,
    pruned: String,
    skipped: String,
) {
    let totals = detailed_report(world).totals;
    assert_eq!(
        totals.new,
        new.parse::<usize>().expect("numeric count"),
        "new sessions"
    );
    assert_eq!(
        totals.updated,
        updated.parse::<usize>().expect("numeric count"),
        "updated sessions"
    );
    assert_eq!(
        totals.pruned,
        pruned.parse::<usize>().expect("numeric count"),
        "pruned sessions"
    );
    assert_eq!(
        totals.skipped,
        skipped.parse::<usize>().expect("numeric count"),
        "skipped sessions"
    );
}

#[then(regex = r#"^the (ingest|refresh) reports ([0-9]+) added sessions? for "([^"]+)"$"#)]
fn pass_reports_added_for(world: &mut BotSpyWorld, _pass: String, count: String, agent: String) {
    assert_eq!(
        adapter_row(world, &agent).added_sessions,
        count.parse::<usize>().expect("numeric count"),
        "added sessions for {agent}"
    );
}

#[then(regex = r#"^the (ingest|refresh) reports ([0-9]+) messages? added by "([^"]+)"$"#)]
fn pass_reports_messages_for(world: &mut BotSpyWorld, _pass: String, count: String, agent: String) {
    assert_eq!(
        adapter_row(world, &agent).added_messages,
        count.parse::<usize>().expect("numeric count"),
        "messages added by {agent}"
    );
}

#[then(regex = r#"^the (ingest|refresh) reports ([0-9]+) sessions? that failed to open$"#)]
fn pass_reports_open_errors(world: &mut BotSpyWorld, _pass: String, count: String) {
    let expected = count.parse::<usize>().expect("numeric count");
    let errors: usize = detailed_report(world)
        .adapters
        .iter()
        .map(|row| row.errors)
        .sum();
    assert_eq!(errors, expected, "sessions that failed to open");
}

// 02_filters.feature — engine-pushed-down filters (BOTSPY-72dc40).
// 03_laziness.feature's counter steps already landed green with
// BOTSPY-a927fc.

/// A part of the given kind, for specs that build one-message sessions.
fn part_of_kind(kind: &str) -> Part {
    Part::Known(match kind {
        "text" => KnownPart::Text {
            text: "a text part".to_string(),
            extra: None,
        },
        "thinking" => KnownPart::Thinking {
            text: Some("a thinking part".to_string()),
            signature: None,
            encrypted: None,
            extra: None,
        },
        "tool_call" => KnownPart::ToolCall {
            id: "call_1".to_string(),
            name: "read_file".to_string(),
            arguments: None,
            status: None,
            extra: None,
        },
        other => panic!("unsupported part kind in feature: {other}"),
    })
}

fn iterate_filtered_sessions(world: &mut BotSpyWorld, filter: &SessionFilter) {
    ensure_store_ingested(world);
    let store = world.local_store.as_ref().expect("no store is open");
    let query = store.query();
    world.local_sessions = query.sessions_with(filter).collect();
    world.local_query_counters = Some(query.counters());
}

#[when(regex = r#"^I iterate the sessions filtered by source "([^"]+)"$"#)]
fn iterate_sessions_filtered_by_source(world: &mut BotSpyWorld, agent: String) {
    let filter = SessionFilter {
        agent: Some(parse_agent(&agent)),
        ..SessionFilter::default()
    };
    iterate_filtered_sessions(world, &filter);
}

#[given(
    regex = r#"^fixture sessions "([^"]+)" from "([^"]+)" in project "([^"]+)" and "([^"]+)" from "([^"]+)" in project "([^"]+)" and "([^"]+)" from "([^"]+)" in project "([^"]+)"$"#
)]
#[allow(clippy::too_many_arguments)]
fn fixture_sessions_three_projects(
    world: &mut BotSpyWorld,
    id1: String,
    agent1: String,
    project1: String,
    id2: String,
    agent2: String,
    project2: String,
    id3: String,
    agent3: String,
    project3: String,
) {
    for (id, agent, project) in [
        (&id1, &agent1, &project1),
        (&id2, &agent2, &project2),
        (&id3, &agent3, &project3),
    ] {
        let adapter = adapter_for(world, agent);
        adapter.add_session(fixture_session(
            id,
            parse_agent(agent),
            project,
            "2026-10-01T09:00:00Z",
        ));
    }
}

#[when(regex = r#"^I iterate the sessions filtered by project "([^"]+)"$"#)]
fn iterate_sessions_filtered_by_project(world: &mut BotSpyWorld, project: String) {
    let filter = SessionFilter {
        project: Some(project),
        ..SessionFilter::default()
    };
    iterate_filtered_sessions(world, &filter);
}

#[given(
    regex = r#"^fixture sessions "([^"]+)" and "([^"]+)" from "([^"]+)" in project "([^"]+)"$"#
)]
fn fixture_sessions_two_one_agent(
    world: &mut BotSpyWorld,
    id1: String,
    id2: String,
    agent: String,
    project: String,
) {
    for id in [&id1, &id2] {
        let adapter = adapter_for(world, &agent);
        adapter.add_session(fixture_session(
            id,
            parse_agent(&agent),
            &project,
            "2026-10-01T09:00:00Z",
        ));
    }
}

#[given(regex = r#"^session "([^"]+)" has a message with a part of kind "([^"]+)"$"#)]
fn session_has_part_of_kind(world: &mut BotSpyWorld, id: String, kind: String) {
    let mut session = crate::steps::find_session(world, &id).expect("fixture session");
    session.messages.push(Message {
        role: Role::Assistant,
        parts: vec![part_of_kind(&kind)],
        timestamp: Some("2026-10-01T09:00:00Z".to_string()),
        ..Message::default()
    });
    crate::steps::save_session_by_id(world, session);
}

#[when(regex = r#"^I iterate the sessions filtered by part kind "([^"]+)"$"#)]
fn iterate_sessions_filtered_by_part_kind(world: &mut BotSpyWorld, kind: String) {
    let filter = SessionFilter {
        part_kind: Some(kind),
        ..SessionFilter::default()
    };
    iterate_filtered_sessions(world, &filter);
}

#[when(regex = r#"^I iterate the messages of session "([^"]+)" filtered by part kind "([^"]+)"$"#)]
fn iterate_messages_filtered_by_part_kind(world: &mut BotSpyWorld, id: String, kind: String) {
    ensure_store_ingested(world);
    let filter = MessageFilter {
        part_kind: Some(kind.clone()),
    };
    let store = world.local_store.as_ref().expect("no store is open");
    let query = store.query();
    world.local_parts.clear();
    world.local_messages = query.messages_with(&id, &filter).collect();
    world.local_message_filter = Some(kind);
    world.local_query_counters = Some(query.counters());
}

#[given(
    regex = r#"^fixture sessions "([^"]+)" last active at "([^"]*)" and "([^"]+)" last active at "([^"]*)" and "([^"]+)" last active at "([^"]*)"$"#
)]
#[allow(clippy::too_many_arguments)]
fn fixture_sessions_three_last_active(
    world: &mut BotSpyWorld,
    id1: String,
    at1: String,
    id2: String,
    at2: String,
    id3: String,
    at3: String,
) {
    for (id, at) in [(&id1, &at1), (&id2, &at2), (&id3, &at3)] {
        let adapter = adapter_for(world, "claude_code");
        adapter.add_session(fixture_session(id, parse_agent("claude_code"), "demo", at));
    }
}

#[when(regex = r#"^I iterate the sessions active after "([^"]+)" and before "([^"]+)"$"#)]
fn iterate_sessions_active_between(world: &mut BotSpyWorld, after: String, before: String) {
    let filter = SessionFilter {
        since: Some(after),
        until: Some(before),
        ..SessionFilter::default()
    };
    iterate_filtered_sessions(world, &filter);
}
// 07_text_search.feature — FTS5 text search (green with BOTSPY-e5be2f).
// 08_vector_search.feature reuses the same store/search steps for its
// semantic scenarios (BOTSPY-c8c597); the shared "the search yields"
// steps below serve both.

#[given(regex = r#"^session "([^"]+)" has messages (.+)$"#)]
fn session_by_id_has_messages(world: &mut BotSpyWorld, id: String, texts: String) {
    let mut session = crate::steps::find_session(world, &id).expect("fixture session");
    for text in quoted_list(&texts) {
        session.messages.push(Message {
            role: Role::User,
            parts: vec![Part::Known(KnownPart::Text { text, extra: None })],
            ..Message::default()
        });
    }
    crate::steps::save_session_by_id(world, session);
}

#[given(
    regex = r#"^fixture sessions "([^"]+)", "([^"]+)", and "([^"]+)" from "([^"]+)" in project "([^"]+)"$"#
)]
fn fixture_sessions_three_one_project(
    world: &mut BotSpyWorld,
    id1: String,
    id2: String,
    id3: String,
    agent: String,
    project: String,
) {
    for id in [&id1, &id2, &id3] {
        let adapter = adapter_for(world, &agent);
        adapter.add_session(fixture_session(
            id,
            parse_agent(&agent),
            &project,
            "2026-10-01T09:00:00Z",
        ));
    }
}

#[given(regex = r#"^fixture sessions "([^"]+)" and "([^"]+)" from "([^"]+)"$"#)]
fn fixture_sessions_two_plain(world: &mut BotSpyWorld, id1: String, id2: String, agent: String) {
    for id in [&id1, &id2] {
        let adapter = adapter_for(world, &agent);
        adapter.add_session(fixture_session(
            id,
            parse_agent(&agent),
            "",
            "2026-10-01T09:00:00Z",
        ));
    }
}

#[given(
    regex = r#"^fixture sessions "([^"]+)", "([^"]+)", "([^"]+)", "([^"]+)", and "([^"]+)" from "([^"]+)" in project "([^"]+)"$"#
)]
#[allow(clippy::too_many_arguments)]
fn fixture_sessions_five_one_project(
    world: &mut BotSpyWorld,
    id1: String,
    id2: String,
    id3: String,
    id4: String,
    id5: String,
    agent: String,
    project: String,
) {
    for id in [&id1, &id2, &id3, &id4, &id5] {
        let adapter = adapter_for(world, &agent);
        adapter.add_session(fixture_session(
            id,
            parse_agent(&agent),
            &project,
            "2026-10-01T09:00:00Z",
        ));
    }
}

#[given(
    regex = r#"^a fixture session "([^"]+)" from agent "([^"]+)" in project "([^"]+)" is registered$"#
)]
#[when(
    regex = r#"^a fixture session "([^"]+)" from agent "([^"]+)" in project "([^"]+)" is registered$"#
)]
fn fixture_session_registered_in_project(
    world: &mut BotSpyWorld,
    id: String,
    agent: String,
    project: String,
) {
    adapter_for(world, &agent).add_session(fixture_session(
        &id,
        parse_agent(&agent),
        &project,
        "2026-10-01T09:00:00Z",
    ));
}

#[given(regex = r#"^the store was opened with the "([^"]+)" embedder$"#)]
fn store_opened_with_embedder(world: &mut BotSpyWorld, model: String) {
    // The store itself opens (with the embedder) when the scenario first
    // needs it — the fixtures are registered after this step.
    world.local_embedder_model = Some(model);
}

#[when(regex = r#"^I search the store for "([^"]+)"$"#)]
fn search_store(world: &mut BotSpyWorld, query: String) {
    ensure_store_ingested(world);
    let store = world.local_store.as_ref().expect("no store is open");
    let query_surface = store.query();
    world.local_search_hits = query_surface.search(&query);
}

#[when(regex = r#"^I search the store for "([^"]+)" filtered by source "([^"]+)"$"#)]
fn search_store_filtered(world: &mut BotSpyWorld, query: String, agent: String) {
    ensure_store_ingested(world);
    let filter = SessionFilter {
        agent: Some(parse_agent(&agent)),
        ..SessionFilter::default()
    };
    let store = world.local_store.as_ref().expect("no store is open");
    let query_surface = store.query();
    world.local_search_hits = query_surface.search_with(&query, &filter);
}

/// Semantic search through the scenario's store (opened with the shipped
/// embedder when a prior step asked for it), `k` sessions at most.
fn search_semantically(world: &mut BotSpyWorld, query: &str, k: usize) {
    ensure_store_ingested(world);
    let store = world.local_store.as_ref().expect("no store is open");
    let query_surface = store.query();
    world.local_search_hits = query_surface.search_semantic(query, k);
}

/// Sessions per hybrid/semantic search without an explicit k.
const DEFAULT_SEARCH_K: usize = 10;

#[when(regex = r#"^I search the store semantically for "([^"]+)"$"#)]
fn search_store_semantically(world: &mut BotSpyWorld, query: String) {
    search_semantically(world, &query, DEFAULT_SEARCH_K);
}

#[when(regex = r#"^I search the store semantically for "([^"]+)" with k ([0-9]+)$"#)]
fn search_store_semantically_with_k(world: &mut BotSpyWorld, query: String, k: usize) {
    search_semantically(world, &query, k);
}

#[when(regex = r#"^I run the hybrid search for "([^"]+)"$"#)]
fn run_hybrid_search(world: &mut BotSpyWorld, query: String) {
    ensure_store_ingested(world);
    let store = world.local_store.as_ref().expect("no store is open");
    let query_surface = store.query();
    world.local_search_hits = query_surface.search_hybrid(&query);
}

/// An embedder claiming a different model than the store's: the specs
/// use it to pin that stores never mix embedding models. Its vectors are
/// never written — the model mismatch is refused before embedding.
#[derive(Debug)]
struct OtherModelEmbedder {
    model: &'static str,
}

impl Embedder for OtherModelEmbedder {
    fn model_id(&self) -> &'static str {
        self.model
    }

    fn dim(&self) -> usize {
        384
    }

    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, botspy::store::embed::EmbedError> {
        Ok(texts.iter().map(|_| vec![0.0; 384]).collect())
    }
}

#[when(regex = r#"^I ingest into the store with an embedder claiming model "([^"]+)"$"#)]
fn ingest_with_other_model(world: &mut BotSpyWorld, model: String) {
    ensure_store_ingested(world);
    let path = world
        .local_store
        .as_ref()
        .expect("no store is open")
        .path()
        .to_path_buf();
    // Box::leak pins the scenario-local model name for the trait's
    // &'static str; the leak is bounded by the test process.
    let model: &'static str = Box::leak(model.into_boxed_str());
    let store = Store::open_with_embedder(&path, Arc::new(OtherModelEmbedder { model }))
        .expect("a store with another embedder opens");
    let adapters = registered_adapters(world);
    world.local_ingest_error = store.ingest(&adapters).err();
}

/// The search's result sessions, best first (one entry per session).
fn search_result_sessions(hits: &[SearchHit]) -> Vec<String> {
    let mut sessions: Vec<String> = Vec::new();
    for hit in hits {
        if !sessions.contains(&hit.session_id) {
            sessions.push(hit.session_id.clone());
        }
    }
    sessions
}

/// The matched texts in hit order, one entry per matched message.
fn search_result_texts(hits: &[SearchHit]) -> Vec<String> {
    let mut texts: Vec<String> = Vec::new();
    let mut seen_messages: Vec<(String, usize)> = Vec::new();
    for hit in hits {
        let key = (hit.session_id.clone(), hit.message_ordinal);
        if !seen_messages.contains(&key) {
            seen_messages.push(key);
            texts.push(hit.text.clone());
        }
    }
    texts
}

#[then(regex = r#"^the search yields ([0-9]+) sessions?$"#)]
fn search_yields_count(world: &mut BotSpyWorld, count: usize) {
    assert_eq!(
        search_result_sessions(&world.local_search_hits).len(),
        count,
        "the search yielded the wrong number of sessions"
    );
}

#[then(regex = r#"^the search yields sessions (.+)(?: in that order)?$"#)]
fn search_yields_sessions(world: &mut BotSpyWorld, ids: String) {
    assert_eq!(
        search_result_sessions(&world.local_search_hits),
        quoted_list(&ids),
        "the search results are out of order"
    );
}

#[then(regex = r#"^the search yields no sessions$"#)]
fn search_yields_nothing(world: &mut BotSpyWorld) {
    assert!(
        world.local_search_hits.is_empty(),
        "the search should have matched nothing"
    );
}

#[then(regex = r#"^the search yields the matched texts (.+)$"#)]
fn search_yields_matched_texts(world: &mut BotSpyWorld, texts: String) {
    assert_eq!(
        search_result_texts(&world.local_search_hits),
        quoted_list(&texts),
        "the search matched different messages"
    );
}

#[then(regex = r#"^every search result carries a score$"#)]
fn search_results_carry_scores(world: &mut BotSpyWorld) {
    assert!(
        !world.local_search_hits.is_empty(),
        "no search results to check for scores"
    );
    for hit in &world.local_search_hits {
        assert!(
            hit.score.is_finite(),
            "search result score is not finite: {hit:?}"
        );
    }
}

#[then(regex = r#"^searching the store for "([^"]+)" still yields sessions (.+)$"#)]
fn search_still_yields(world: &mut BotSpyWorld, query: String, ids: String) {
    let store = world.local_store.as_ref().expect("no store is open");
    let query_surface = store.query();
    world.local_search_hits = query_surface.search(&query);
    search_yields_sessions(world, ids);
}

/// The embedding model recorded in the store's meta, read through the
/// public surface the specs pin (BOTSPY-7e1836).
fn recorded_embedding_model(world: &BotSpyWorld) -> Option<String> {
    world
        .local_store
        .as_ref()
        .expect("no store is open")
        .embedding_model()
        .expect("reading the recorded embedding model")
}

#[then(regex = r#"^the store's embedding model is "([^"]+)"$"#)]
fn store_embedding_model_is(world: &mut BotSpyWorld, model: String) {
    assert_eq!(
        recorded_embedding_model(world).as_deref(),
        Some(model.as_str()),
        "the store recorded a different embedding model"
    );
}

#[then(regex = r#"^the store records no embedding model$"#)]
fn store_records_no_embedding_model(world: &mut BotSpyWorld) {
    assert!(
        recorded_embedding_model(world).is_none(),
        "a store that never embedded must record no embedding model"
    );
}

#[then(regex = r#"^the ingest fails with a model mismatch error$"#)]
fn ingest_fails_model_mismatch(world: &mut BotSpyWorld) {
    let err = world
        .local_ingest_error
        .as_ref()
        .expect("the mismatched-model ingest should have failed");
    assert!(
        matches!(
            err,
            botspy::store::StoreError::EmbeddingModelMismatch { .. }
        ),
        "expected a model mismatch error, got {err:?}"
    );
}
