//! # Cursor importer
//!
//! *[Chapter 3: Importers / CDC](crate::spec::ch03_importers) ·
//! [index](crate) · mirrors `features/03_importers/cursor/`*
//!
//! The Cursor importer is the per-agent translation of the CDC pipeline
//! for Cursor's two separate sources: the IDE's SQLite KV store
//! (`state.vscdb`: `composerHeaders`, `composerData:<id>`,
//! `bubbleId:<composerId>:<bubbleId>` values, plus the content-addressed
//! protobuf `agentKv:blob:<sha256>` entries) and the CLI agent transcripts
//! under `~/.cursor/projects/<encoded-cwd>/agent-transcripts/`.
//!
//! ## Pipeline translation
//!
//! - **Detection**: watch `composerHeaders` for new/updated composers and
//!   `lastUpdatedAt` for changed ones; watch the CLI transcript directory
//!   for new session files. SQLite is copied or opened `immutable=1` —
//!   never written.
//! - **Normalization handoff**: bubbles become messages (user/assistant,
//!   thinking, tool calls from `toolFormerData`); CLI transcript records
//!   map the same way minus results and usage; tool results that only
//!   exist as protobuf blobs stay as blob references pending
//!   `12_raw_blob_escape_hatch.feature`.
//! - **Retries**: the 6 GB vscdb has a hot WAL; reads are retried against
//!   a snapshot copy, taken once and reused while the store is unchanged
//!   (see `snapshot.feature`). Bubble keys are unordered — order comes from
//!   `composerData.fullConversationHeadersOnly`.
//!
//! ## Status: specified
//!
//! The scenarios live in [`adapter.feature`](../../../features/03_importers/cursor/adapter.feature)
//! and are embedded below, tagged `@wip` until this importer's
//! implementation lands.
//!
#![doc = concat!(
    "## Behavior specification\n\n",
    "### cursor/adapter.feature\n\n```gherkin\n",
    include_str!("../../../features/03_importers/cursor/adapter.feature"),
    "\n```\n\n",
    "### cursor/snapshot.feature\n\n```gherkin\n",
    include_str!("../../../features/03_importers/cursor/snapshot.feature"),
    "\n```\n\n",
    "### cursor/workspace.feature\n\n```gherkin\n",
    include_str!("../../../features/03_importers/cursor/workspace.feature"),
    "\n```\n\n",
    "### cursor/message_origin.feature\n\n```gherkin\n",
    include_str!("../../../features/03_importers/cursor/message_origin.feature"),
    "\n```\n"
)]
//!
//! No scenarios exist yet; `features/03_importers/cursor/` is a
//! placeholder and this page is the outline its specs will be written
//! against.
