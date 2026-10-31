//! # Claude Code importer
//!
//! *[Chapter 3: Importers / CDC](crate::spec::ch03_importers) ·
//! [index](crate) · mirrors `features/03_importers/claude_code/`*
//!
//! The Claude Code importer is the per-agent translation of the CDC
//! pipeline for Claude Code's storage: append-only JSONL transcripts under
//! `~/.claude/projects/<encoded-cwd>/`, one per session, plus sibling
//! `<session-id>/subagents/agent-*.jsonl` files for sub-agent (sidechain) transcripts, imported as sessions spawned by the parent
//! and auxiliary records (`cost-state`, `pr-link`, `custom-title`,
//! `file-history-*`) inside the session files.
//!
//! ## Pipeline translation
//!
//! - **Detection**: follow each session JSONL by byte offset; notice new
//!   files in project directories; treat mtime changes as rescan hints.
//! - **Normalization handoff**: map `user`/`assistant`/`system` records
//!   into chapter-1 messages (tool results arrive as `tool_result`
//!   content in user records); keep `uuid`/`parentUuid` as provenance
//!   (see the pending `provenance.feature` scenarios); park `cost-state`,
//!   `pr-link`, and friends in session metadata pending
//!   `11_session_metadata.feature`.
//! - **Retries**: the transcript file is being appended live; partial
//!   trailing lines are retried on the next read, malformed records are
//!   skipped and counted.
//!
//! ## Status: specified
//!
//! The scenarios live in [`adapter.feature`](../../../features/03_importers/claude_code/adapter.feature)
//! and are embedded below, tagged `@wip` until this importer's
//! implementation lands.
//!
#![doc = concat!(
    "## Behavior specification\n\n",
    "### claude_code/adapter.feature\n\n```gherkin\n",
    include_str!("../../../features/03_importers/claude_code/adapter.feature"),
    "\n```\n"
)]
//!
//! No scenarios exist yet; `features/03_importers/claude_code/` is a
//! placeholder and this page is the outline its specs will be written
//! against.
