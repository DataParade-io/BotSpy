//! # BotSpy — one library for every coding agent's conversation history
//!
//! BotSpy pops open the conversation history of any coding agent and
//! normalizes it into **one unified agent-session history**. One adapter per
//! agent — Claude Code, Cursor, Codex, Grok Bot, Antigravity, more later —
//! taps that agent's on-disk chat histories; the normalized schema makes
//! user messages, agent messages, thinking traces, tool calls and their
//! results, attachments, and per-record provenance look the same no matter
//! which agent produced them.
//!
//! ## What BotSpy is not
//!
//! BotSpy is a **library crate plus its `botspy` CLI** — a thin,
//! read-only shell over the library (phase 1: `sources`, `sessions`,
//! `show`, `doctor`, `snapshot`; store-backed verbs: `import`, `search`,
//! `stats`, and `sessions --db`). There is no server and no C
//! interface/FFI. Metrics such as the coding-session thanks-vs-F-bombs
//! meter or local sentiment analysis are **example consumers** of the
//! library, not part of it.
//!
//! ## Specs are the source of truth
//!
//! BotSpy is a behavior-driven specification project. The Gherkin
//! specifications under `features/` are the backbone and the true source of
//! the project; implementation code is considered generated from the specs.
//! These docs are therefore organized around the specifications, not around
//! code modules: each chapter below is one spec layer and mirrors one folder
//! under `features/`, and every chapter embeds the actual `.feature` files
//! it is backed by.
//!
//! ## How these docs are organized
//!
//! The chapters follow the record's journey — structure first, then
//! querying, then how data arrives:
//!
//! 1. [Structure](crate::spec::ch01_structure) —
//!    the basic structure of records: the session object, ordered
//!    messages, walking through them, and the anatomy of a message
//!    (text, thinking, tool calls and results, attachments, provenance,
//!    the `extra` escape hatch, and the pending schema extensions).
//!    Mirrors `features/01_structure/`.
//! 2. [Querying](crate::spec::ch02_querying) —
//!    one query interface backed by SQLite + sqlite-vec, completely
//!    transparent to callers; implemented by the Local Store Initiative
//!    (BOTSPY-c7795b). Mirrors `features/02_querying/` (implemented and
//!    green — see the chapter's status section).
//! 3. [Importers / CDC](crate::spec::ch03_importers) —
//!    the change-data-capture pipeline (detect new or changed transcripts,
//!    hand off to normalization, retry), with per-agent importer
//!    sub-chapters: [Claude Code](crate::spec::ch03_importers::claude_code),
//!    [Cursor](crate::spec::ch03_importers::cursor),
//!    [Codex](crate::spec::ch03_importers::codex),
//!    [Grok Bot](crate::spec::ch03_importers::grok_bot), and
//!    [Antigravity](crate::spec::ch03_importers::antigravity).
//!    Mirrors `features/03_importers/` (specified next).
//!
//! ## A 60-second drill-down
//!
//! Build a store, register an adapter with in-memory fixture data (real
//! adapters read agent directories instead), list sessions, open one, and
//! walk its messages:
//!
//! ```
//! use botspy::{Agent, FixtureAdapter, KnownPart, Message, Part, Role, Session, SessionStore};
//! use std::sync::Arc;
//!
//! let mut store = SessionStore::new();
//! let adapter = Arc::new(FixtureAdapter::new(Agent::ClaudeCode));
//! adapter.add_session(Session {
//!     id: "c1".into(),
//!     agent: Agent::ClaudeCode,
//!     project_id: "demo".into(),
//!     started_at: "2026-10-01T09:00:00Z".into(),
//!     last_activity_at: "2026-10-01T10:00:00Z".into(),
//!     messages: vec![Message {
//!         role: Role::User,
//!         parts: vec![Part::Known(KnownPart::Text {
//!             text: "please fix the bug".into(),
//!             extra: None,
//!         })],
//!         timestamp: Some("2026-10-01T09:01:00Z".into()),
//!         ..Message::default()
//!     }],
//!     ..Session::default()
//! });
//! store.register(adapter);
//!
//! // Sessions from every registered agent, most recent activity first.
//! let summaries = store.list_sessions();
//! assert_eq!(summaries[0].id, "c1");
//!
//! // Open one and walk its messages in order.
//! let session = store.open("c1")?;
//! assert_eq!(session.messages.len(), 1);
//! assert!(matches!(&session.messages[0].parts[0], Part::Known(KnownPart::Text { .. })));
//!
//! // An id no adapter knows is an error, never a silent empty result.
//! assert!(store.open("nope").is_err());
//! # Ok::<(), botspy::UnknownSession>(())
//! ```
//!
//! Continue with [chapter 1: structure](crate::spec::ch01_structure).

pub mod adapter;
pub mod adapters;
pub mod cli;
pub mod importer;
pub mod privacy;
pub mod schema;
pub mod session;
pub mod snapshot;
pub mod spec;
pub mod store;

pub use adapter::Adapter;
pub use adapters::fixture::FixtureAdapter;
pub use importer::{
    create_source, JsonlStream, MemorySource, RawRecord, RecordStream, SkipCounter, SourceOptions,
    UnknownSource,
};
pub use schema::{
    content_hash, Agent, BattleLink, CompactionEvent, CompactionWindow, ForkPoint, KnownPart,
    Message, ModelCost, Origin, Part, PartStatus, PartialHistory, PartialReason, Peer, Provenance,
    RateLimitState, Role, SessionCost, SessionMetadata, SubagentInfo, Timestamp, ToolArguments,
    Turn, TurnPosition, TurnStatus, Usage, SCHEMA_VERSION,
};
pub use session::{Session, SessionStore, SessionSummary, UnknownSession};
pub use snapshot::{count_events, count_rows, snapshot_sqlite, SnapshotError};
pub use store::{
    default_store_path,
    ingest::{AdapterIngestReport, DetailedIngestReport, IngestOptions, IngestReport},
    query::{MessageFilter, Query, QueryCounters, SessionFilter},
    text::SearchHit,
    SessionIter, Store, StoreError,
};
