//! # Chapter 4 — Examples (consumers of the public API)
//!
//! *[index](crate) ·
//! [previous: importers / CDC](crate::spec::ch03_importers)*
//!
//! Examples are consumers of the BotSpy library, never core work and
//! never core dependencies. They live in `examples/` (or an optional
//! `botspy[metrics]` extra) and may only use the public API surface —
//! the same unified history and schema every other caller sees.
//!
//! The two flagship examples:
//!
//! - **The thanks-vs-F-bombs meter** — a coding-session civility meter:
//!   counts "thanks" versus F-bombs across a session (or the whole
//!   unified history) and reports the ratio.
//! - **Local sentiment analysis** — VADER-style scoring over message
//!   text, with an optional GoEmotions transformer behind a feature
//!   flag; everything runs on the local machine, nothing phones home.
//!
//! - **Privacy controls** — LeftBlock scans transcript text for secrets
//!   and personal data; the example audits (counts and provenance, never
//!   values) or redacts, and the other examples then score only the
//!   redacted text.
//!
//! ## Status: specified
//!
//! The scenarios live in
//! [`consumers.feature`](../../features/04_examples/consumers.feature)
//! and are embedded below, tagged `@wip` until the examples land.
//!
#![doc = concat!(
    "## Behavior specification\n\n",
    "### consumers.feature\n\n```gherkin\n",
    include_str!("../../features/04_examples/consumers.feature"),
    "\n```\n\n",
    "### privacy.feature\n\n```gherkin\n",
    include_str!("../../features/04_examples/privacy.feature"),
    "\n```\n"
)]
