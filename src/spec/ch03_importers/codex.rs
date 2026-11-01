//! # Codex importer
//!
//! *[Chapter 3: Importers / CDC](crate::spec::ch03_importers) ·
//! [index](crate) · mirrors `features/03_importers/codex/`*
//!
//! The Codex importer is the per-agent translation of the CDC pipeline for
//! Codex's storage: rollout JSONL event logs under
//! `~/.codex/sessions/YYYY/MM/DD/`, indexed by `state_5.sqlite` — the
//! `threads` table (with the thread metadata, and no parent column) plus
//! the `thread_spawn_edges` edge table — and `thread_history_1.sqlite`
//! (turn and item projections with rollout byte offsets). The index is a
//! live database: it is read through a safe snapshot copy, never in
//! place.
//!
//! ## Pipeline translation
//!
//! - **Detection**: the threads index is snapshotted and read on the
//!   copy; each thread names its rollout (absolute or relative to the
//!   home root); the spawn edges come from `thread_spawn_edges`; a query
//!   failure is a visible issue, never a silent empty result.
//! - **Metadata**: the thread's title, model, git branch and origin, cwd,
//!   and created/updated instants map into `SessionMetadata` (R11) and
//!   the summary instants.
//! - **Normalization handoff**: every record carries an explicit
//!   `ordinal` — the authoritative order; `response_item` records become
//!   messages (roles user/assistant/developer, reasoning summaries, tool
//!   calls and outputs); `token_usage_record` and `compacted` map to the
//!   pending usage and compaction specs.
//! - **Retries**: rollouts are paginated/compacted and appended live;
//!   partial trailing lines are retried, malformed records skipped and
//!   counted.
//!
//! ## Status: specified
//!
//! The scenarios live in [`adapter.feature`](../../../features/03_importers/codex/adapter.feature)
//! and are embedded below; every scenario is implemented by
//! `adapters::codex::CodexSource`.
//!
#![doc = concat!(
    "## Behavior specification\n\n",
    "### codex/adapter.feature\n\n```gherkin\n",
    include_str!("../../../features/03_importers/codex/adapter.feature"),
    "\n```\n\n",
    "### codex/message_origin.feature\n\n```gherkin\n",
    include_str!("../../../features/03_importers/codex/message_origin.feature"),
    "\n```\n"
)]
