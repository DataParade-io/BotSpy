//! Shared world and helpers for the behavior specifications.

pub mod antigravity_steps;
pub mod arguments_steps;
pub mod blob_steps;
pub mod claude_code_steps;
pub mod cli_steps;
pub mod codex_steps;
pub mod compaction_steps;
pub mod cursor_steps;
pub mod examples_steps;
pub mod graph_steps;
pub mod grok_steps;
pub mod importer_steps;
pub mod inline_steps;
pub mod leftblock_stub;
pub mod metadata_steps;
pub mod privacy_steps;
pub mod role_steps;
pub mod schema_steps;
pub mod session_steps;
pub mod sqlite_steps;
pub mod status_steps;
pub mod store_steps;
pub mod timestamp_steps;
pub mod turn_steps;
pub mod unified_steps;
pub mod usage_steps;

use botspy::{
    Adapter, Agent, FixtureAdapter, Message, Session, SessionStore, SessionSummary, UnknownSession,
};
use cucumber::World;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug, Default, World)]
pub struct BotSpyWorld {
    pub store: SessionStore,
    /// Fixture adapters keyed by agent name as written in the features.
    pub adapters: BTreeMap<String, Arc<FixtureAdapter>>,
    pub listed: Vec<SessionSummary>,
    pub opened: Option<Result<Session, UnknownSession>>,
    pub walked: Option<Result<Vec<Message>, UnknownSession>>,
    /// Session id the schema steps record into.
    pub current_session: Option<String>,
    /// Turn id of the most recently recorded turn, when a step made one.
    pub last_turn: Option<String>,
    /// Fixture source root under construction (importer contract steps).
    pub source_root: Option<std::path::PathBuf>,
    /// The reference in-memory source for the current root.
    pub memory_source: Option<botspy::importer::MemorySource>,
    /// Sessions from the most recent discovery.
    pub discovered: Vec<botspy::SessionSummary>,
    /// New ids reported by each discovery round, in order.
    pub discovery_rounds: Vec<Vec<String>>,
    /// Records pulled by the most recent extraction.
    pub extracted: Vec<botspy::importer::RawRecord>,
    /// Skips reported by the most recent extraction.
    pub skipped: botspy::importer::SkipCounter,
    /// Peak in-memory buffering of the most recent extraction.
    pub peak_buffered: usize,
    /// Digest of the source tree before extraction.
    pub source_digest_before: Option<String>,
    /// Digest of the source tree after extraction.
    pub source_digest_after: Option<String>,
    /// Name of the source the reference in-memory source was built for.
    pub source_name: Option<String>,
    /// Fixture SQLite source database (WAL snapshot steps).
    pub sqlite_db: Option<std::path::PathBuf>,
    /// The live writer connection holding the fixture SQLite source.
    pub sqlite_writer: Option<rusqlite::Connection>,
    /// Rows present in the source at snapshot time.
    pub sqlite_rows_at_snapshot: u64,
    /// Digest of the source db + WAL before the snapshot copy.
    pub sqlite_digest_before: Option<String>,
    /// Digest of the source db + WAL after the snapshot copy.
    pub sqlite_digest_after: Option<String>,
    /// Rows the snapshot copy reports.
    pub snapshot_rows: Option<u64>,
    /// Claude Code source root under construction (Claude Code steps).
    pub cc_root: Option<std::path::PathBuf>,
    /// The Claude Code source watching that root.
    pub cc_source: Option<botspy::adapters::claude_code::ClaudeCodeSource>,
    /// Transcript file name the current Claude Code fixture uses.
    pub cc_transcript: Option<String>,
    /// Result of the most recent Claude Code extraction pass.
    pub cc_extraction: Option<botspy::adapters::claude_code::ClaudeExtraction>,
    /// Codex home root under construction (Codex steps).
    pub cx_root: Option<std::path::PathBuf>,
    /// The Codex source watching that root.
    pub cx_source: Option<botspy::adapters::codex::CodexSource>,
    /// Thread id the current Codex fixture extracts.
    pub cx_thread: Option<String>,
    /// Result of the most recent Codex discovery.
    pub cx_discovery: Option<botspy::adapters::codex::CodexDiscovery>,
    /// Result of the most recent Codex extraction pass.
    pub cx_extraction: Option<botspy::adapters::codex::CodexExtraction>,
    /// Result of the most recent Codex doctor report.
    pub cx_report: Option<botspy::adapters::codex::CodexReport>,
    /// Digest of the threads index + WAL before the snapshot read.
    pub cx_digest_before: Option<String>,
    /// Digest of the threads index + WAL after the snapshot read.
    pub cx_digest_after: Option<String>,
    /// Cursor KV store file under construction (Cursor steps).
    pub cur_store: Option<std::path::PathBuf>,
    /// The Cursor KV source watching that store.
    pub cur_source: Option<botspy::adapters::cursor::CursorSource>,
    /// Composer id the current Cursor fixture extracts.
    pub cur_composer: Option<String>,
    /// Result of the most recent Cursor KV discovery.
    pub cur_discovery: Option<botspy::adapters::cursor::CursorDiscovery>,
    /// Result of the most recent Cursor composer extraction.
    pub cur_extraction: Option<botspy::adapters::cursor::CursorExtraction>,
    /// Cursor CLI projects root under construction.
    pub cur_cli_root: Option<std::path::PathBuf>,
    /// The Cursor CLI source watching that root.
    pub cur_cli_source: Option<botspy::adapters::cursor::CursorCliSource>,
    /// Result of the most recent Cursor CLI discovery.
    pub cur_cli_discovery: Option<botspy::adapters::cursor::CursorCliDiscovery>,
    /// The live app writer holding the fixture Cursor KV store.
    pub cur_writer: Option<rusqlite::Connection>,
    /// Digest of the store + WAL before the snapshot read.
    pub cur_digest_before: Option<String>,
    /// Digest of the store + WAL after the snapshot read.
    pub cur_digest_after: Option<String>,
    /// Grok Bot persistence dir under construction (Grok Bot steps).
    pub gb_root: Option<std::path::PathBuf>,
    /// The Grok Bot source watching that dir.
    pub gb_source: Option<botspy::adapters::grok_bot::GrokBotSource>,
    /// Blob name the current Grok Bot fixture extracts.
    pub gb_blob: Option<String>,
    /// Result of the most recent Grok Bot discovery.
    pub gb_discovery: Option<botspy::adapters::grok_bot::GrokDiscovery>,
    /// Result of the most recent Grok Bot extraction pass.
    pub gb_extraction: Option<botspy::adapters::grok_bot::GrokExtraction>,
    /// Antigravity data root under construction (Antigravity steps).
    pub ag_root: Option<std::path::PathBuf>,
    /// The Antigravity source watching that root.
    pub ag_source: Option<botspy::adapters::antigravity::AntigravitySource>,
    /// Conversation id the current Antigravity fixture extracts.
    pub ag_conversation: Option<String>,
    /// Result of the most recent Antigravity discovery.
    pub ag_discovery: Option<botspy::adapters::antigravity::AntigravityDiscovery>,
    /// Result of the most recent Antigravity extraction pass.
    pub ag_extraction: Option<botspy::adapters::antigravity::AntigravityExtraction>,
    /// The live app writer holding the fixture Antigravity payload DB.
    pub ag_writer: Option<rusqlite::Connection>,
    /// Digest of the payload DB + WAL before the snapshot read.
    pub ag_digest_before: Option<String>,
    /// Digest of the payload DB + WAL after the snapshot read.
    pub ag_digest_after: Option<String>,
    /// Sessions the example-consumer steps build and score.
    pub example_sessions: Vec<botspy::Session>,
    /// The most recent meter report.
    pub meter_report: Option<examples_steps::MeterReport>,
    /// The most recent sentiment report.
    pub sentiment_report: Option<examples_steps::SentimentReport>,
    /// State of the privacy-example scenarios.
    pub privacy: privacy_steps::PrivacyState,
    /// Sources of the example consumers, for the public-API checks.
    pub example_sources: Option<String>,
    /// Fixture home the CLI steps laid out for the current scenario.
    pub cli_home: Option<std::path::PathBuf>,
    /// The scenario's scratch output directory: stable across steps.
    pub cli_out_dir: Option<std::path::PathBuf>,
    /// Store the background import steps filled for search/stats steps.
    pub cli_store: Option<std::path::PathBuf>,
    /// The outcome of the most recent CLI run.
    pub cli_run: Option<botspy::cli::RunOutcome>,
    /// The local store under test (store steps).
    pub local_store: Option<botspy::store::Store>,
    /// A second reader connection open on the same store file.
    pub local_reader: Option<botspy::store::Store>,
    /// The reader's mid-iteration cursor.
    pub local_reader_iter: Option<botspy::store::SessionIter>,
    /// Whether the reader's iteration ran to completion.
    pub local_reader_finished: bool,
    /// Session summaries from the most recent local-store iteration.
    pub local_sessions: Vec<botspy::SessionSummary>,
    /// The error from the most recent store-open attempt, when it failed.
    pub local_open_error: Option<botspy::store::StoreError>,
    /// The session materialized out of the store by the most recent open.
    pub local_opened: Option<Result<botspy::Session, botspy::store::StoreError>>,
    /// The id of the session the query surface last materialized.
    pub local_opened_id: Option<String>,
    /// Messages streamed out of the store by the most recent iteration.
    pub local_messages: Vec<botspy::Message>,
    /// Parts streamed out of the store by the most recent iteration.
    pub local_parts: Vec<botspy::Part>,
    /// The query counters of the most recent query surface use.
    pub local_query_counters: Option<botspy::QueryCounters>,
    /// The report of the most recent ingest or refresh pass.
    pub ingest_report: Option<botspy::IngestReport>,
    /// The detailed report (per adapter) of the most recent option-carrying
    /// ingest or refresh pass.
    pub detailed_ingest_report: Option<botspy::DetailedIngestReport>,
    /// Digest of the fixture adapters' sessions before an ingest pass.
    pub local_digest_before: Option<String>,
    /// Digest of the fixture adapters' sessions after an ingest pass.
    pub local_digest_after: Option<String>,
    /// The part-kind filter of the most recent filtered message iteration.
    pub local_message_filter: Option<String>,
    /// The hits of the most recent store search (text or semantic).
    pub local_search_hits: Vec<botspy::SearchHit>,
    /// The embedder a store-open step requested, applied when the store
    /// is next opened for ingest.
    pub local_embedder_model: Option<String>,
    /// The error from the most recent ingest attempt, when it failed.
    pub local_ingest_error: Option<botspy::store::StoreError>,
    /// The fixture adapters' open() call count before a refresh pass.
    pub local_open_count_before: Option<usize>,
    /// The fixture adapters' open() call count after a refresh pass.
    pub local_open_count_after: Option<usize>,
    /// The BOTSPY_HOME value before the store steps overrode it.
    pub saved_botspy_home: Option<std::ffi::OsString>,
}

pub fn parse_agent(name: &str) -> Agent {
    match name {
        "claude_code" | "claude-code" => Agent::ClaudeCode,
        "cursor" => Agent::Cursor,
        "codex" => Agent::Codex,
        "grok_bot" | "grok-bot" => Agent::GrokBot,
        "antigravity" => Agent::Antigravity,
        other => panic!("unknown agent in feature: {other}"),
    }
}

/// Get or lazily create (and register) the fixture adapter for an agent.
pub fn adapter_for(world: &mut BotSpyWorld, agent: &str) -> Arc<FixtureAdapter> {
    if let Some(existing) = world.adapters.get(agent) {
        return existing.clone();
    }
    let adapter = Arc::new(FixtureAdapter::new(parse_agent(agent)));
    world.store.register(adapter.clone());
    world.adapters.insert(agent.to_string(), adapter.clone());
    adapter
}

/// Table rows below the header row.
pub fn data_rows(step: &cucumber::gherkin::Step) -> impl Iterator<Item = &[String]> {
    let rows: &[Vec<String>] = step
        .table
        .as_ref()
        .map(|table| table.rows.as_slice())
        .unwrap_or_default();
    rows.iter().skip(1).map(Vec::as_slice)
}

pub fn header(step: &cucumber::gherkin::Step) -> Vec<String> {
    step.table
        .as_ref()
        .map(|table| table.rows[0].clone())
        .unwrap_or_default()
}

/// Locate the adapter that owns a session id.
pub fn adapter_owning(world: &BotSpyWorld, id: &str) -> Option<Arc<FixtureAdapter>> {
    world
        .adapters
        .values()
        .find(|adapter| adapter.open(id).is_some())
        .cloned()
}

/// Load the current fixture session (by id) through its owning adapter.
pub fn current_session(world: &BotSpyWorld) -> Session {
    let id = world
        .current_session
        .as_deref()
        .expect("no current fixture session");
    find_session(world, id).expect("current session not found")
}

/// Find any fixture session by id across all fixture adapters.
pub fn find_session(world: &BotSpyWorld, id: &str) -> Option<Session> {
    world.adapters.values().find_map(|a| a.open(id))
}

/// Persist changes to the current fixture session.
pub fn save_session(world: &mut BotSpyWorld, session: Session) {
    let id = session.id.clone();
    let adapter = adapter_owning(world, &id).expect("current session not found");
    adapter.add_session(session);
}

/// Persist changes to a session looked up by id (any fixture adapter).
pub fn save_session_by_id(world: &mut BotSpyWorld, session: Session) {
    let id = session.id.clone();
    let adapter = adapter_owning(world, &id)
        .unwrap_or_else(|| panic!("no fixture adapter owns session {id}"));
    adapter.add_session(session);
}

/// The most recently recorded message of the current fixture session.
pub fn last_message(world: &BotSpyWorld) -> Message {
    current_session(world)
        .messages
        .last()
        .cloned()
        .expect("no messages recorded")
}

/// Record one more message on the current fixture session, refreshing
/// `last_activity_at` from the message timestamp when it has one.
pub fn record_message_on_current(world: &mut BotSpyWorld, message: Message) {
    let mut session = current_session(world);
    if let Some(ts) = &message.timestamp {
        session.last_activity_at = ts.clone();
    }
    session.messages.push(message);
    save_session(world, session);
}
