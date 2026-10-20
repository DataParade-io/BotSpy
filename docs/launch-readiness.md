# BotSpy launch preparation

Status: draft for owner review. This document is preparation, not a
published promotional post or a claim that the next release is available.

## Verified foundation

- Project: [DataParade-io/BotSpy](https://github.com/DataParade-io/BotSpy),
  a public repository.
- Existing license: MIT; the copyright notice and terms remain in
  [LICENSE](../LICENSE). Repository ownership and copyright attribution
  serve different purposes; release preparation does not change either.
- Distribution: the `botspy` Rust crate on
  [crates.io](https://crates.io/crates/botspy), including the `botspy`
  command-line executable. API documentation is hosted on
  [docs.rs](https://docs.rs/botspy).
- On 2026-10-20 the official Cargo index listed 0.3.0 as the newest
  published version, not yanked. Its archive checksum was
  `1755fa093e4e8b328c2b34f574788c34acf48119161cc7802c71d1878ca5088d`.
  The [v0.3.0 release](https://github.com/DataParade-io/BotSpy/releases/tag/v0.3.0)
  and its release workflow had completed successfully.

## Proposed introductory copy

BotSpy brings local coding-agent conversations into one searchable view.
Its Rust library and command-line tool read session histories from Claude
Code, Cursor, Codex, Grok Bot, and Antigravity, normalize their records,
and retain source provenance.

Use it to list sessions, inspect messages, and build a local SQLite
index with text search. Optional semantic and hybrid search use an
embedding model on your CPU after an explicit model-asset download.
BotSpy has no telemetry service. Reading and indexing session content
happens locally.

Install the published command with `cargo install botspy --locked`.
Start with the [README](../README.md), the [CLI reference](cli.md), and
the [sensitive-data inventory](sensitive-data.md).

Coding-agent histories can contain prompts, source code, tool output,
and secrets. BotSpy's index and snapshots are additional copies of that
content. It does not encrypt or redact those copies, and local operation
does not make them safe to share. Agent formats also change; the
supported adapters and executable synthetic fixtures define the tested
scope.

## Review and launch checklist

| Requirement | Owner | Evidence or dependency |
| --- | --- | --- |
| Source-safety and import-pruning regressions pass | Release maintainer | BOTSPY-9f2210 and BOTSPY-4dc1e3 |
| Accurate purpose, installation, model setup, library and CLI docs | Release maintainer | BOTSPY-d24517 |
| Tested package, installed command, registry checksum, GitHub release | Release maintainer | BOTSPY-5a5e60; record the actual version after publication |
| Hook rollout and synthetic verification | LeftBlock owner | BOTSPY-25e2c4; keep the existing receipt gate |
| Approved logo and usage assets | Logo owner | BOTSPY-eb7f2c |
| Review wording, screenshots, version and destination | Project owner | BOTSPY-5bbc31 |
| Publish an approved announcement on a Papyrus-powered site | Site/content owner | Later handoff; proposed Papyrus-powered site, destination pending |

Publicity is a separate deliverable from a code pull request. After the
package and branding are ready, prepare a concrete site draft with
synthetic examples, installation links, and a precise release version.
Obtain review of that draft and its destination before publication.

Do not claim universal agent compatibility, complete recovery of every
record, encrypted storage, secret removal, guaranteed absence of SQLite
locking, or that model setup alone indexes previously imported text.
Do not use live session screenshots or private history in launch assets.
