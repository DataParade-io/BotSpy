# BotSpy

Pop open the conversation history of your coding agents.

BotSpy reads the on-disk session histories of **Claude Code, Cursor,
Codex, Grok Bot, and Antigravity** — locally, and read-only. One
adapter per agent, one normalized schema, one local SQLite store with
full-text and semantic search. Everything stays on your machine: no
server or telemetry. BotSpy does not edit conversation records. SQLite
may create or update shared-memory sidecars while reading live databases.

BotSpy is an MIT-licensed [DataParade-io project](https://github.com/DataParade-io/BotSpy),
distributed as the `botspy` crate and command line tool on
[crates.io](https://crates.io/crates/botspy).

## The system in one picture

BotSpy is one local CLI and library on your machine. The coding agents
write their session data to their own local files; BotSpy reads those
files read-only and shows you the results.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/DataParade-io/BotSpy/develop/docs/diagrams/context.dark.png">
  <img src="https://raw.githubusercontent.com/DataParade-io/BotSpy/develop/docs/diagrams/context.light.png" alt="C4 context diagram: user, BotSpy, and the coding agents on the machine">
</picture>

## What BotSpy does

- **Reads source records.** Each agent's history is read where it lives
  on disk. Live SQLite stores are copied through a read-only connection
  using SQLite's online backup API. This includes committed WAL content;
  normal SQLite coordination and locking still apply.
- **One normalized schema.** Every record from every agent becomes a
  `Session`, `Message`, and `Part`, with provenance back to the exact
  source file, line, or row. Records an adapter does not know are
  skipped and counted, never fatal.
- **One local store.** One SQLite database at `~/.botspy/store.db`
  (WAL mode) holds sessions, messages, parts, a full-text index (FTS5),
  and a vector index (sqlite-vec, with embeddings from
  all-MiniLM-L6-v2 computed on your CPU).
- **Search three ways.** Full-text search, semantic search, or the
  hybrid fusion of both.
- **One command line over all agents.** `botspy sources`, `doctor`,
  `sessions`, `show`, `snapshot`, `scan`, `import`, `search`, and
  `stats` — a thin shell over the library; every verb is one library
  call plus rendering.
- **Sensitive-data visibility.** BotSpy never hides where session
  content lives. Every architecture diagram marks the sensitive places,
  and [`docs/sensitive-data.md`](https://github.com/DataParade-io/BotSpy/blob/develop/docs/sensitive-data.md)
  lists every path BotSpy reads, copies, or writes.

## How the data flows

Five agents' files feed five source adapters. The adapters convert
their records into the unified schema, ingest writes the store, and the
query surface answers over it — text, semantic, or hybrid. The diagram
shows the source and storage boundaries. The sensitive-data inventory
below is the reference for paths and cleanup behavior.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/DataParade-io/BotSpy/develop/docs/diagrams/dataflow.dark.png">
  <img src="https://raw.githubusercontent.com/DataParade-io/BotSpy/develop/docs/diagrams/dataflow.light.png" alt="Top-level data flow: five sources, snapshot copies, adapters, unified schema, ingest, store, query, output">
</picture>

## One machine, one store

On one machine there are four data places and one process: the agent
session files (read-only), temporary snapshot copies for live SQLite
files, the local store at `~/.botspy/store.db`, and a model cache that
holds model weights only, no session data. The BotSpy CLI is the one
process. Its persistent outputs normally live under `~/.botspy/`;
custom database paths, explicit snapshots, redirected output, backups,
and temporary copies left by older versions or interrupted processes
must be handled separately. See the
[sensitive-data inventory](https://github.com/DataParade-io/BotSpy/blob/develop/docs/sensitive-data.md).

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/DataParade-io/BotSpy/develop/docs/diagrams/containers.dark.png">
  <img src="https://raw.githubusercontent.com/DataParade-io/BotSpy/develop/docs/diagrams/containers.light.png" alt="C4 container diagram: CLI, agent files, snapshot copies, store, model cache">
</picture>

## Everything stays local

- Adapters are read-only by construction — the protocol has no surface
  for writing to a source.
- Live SQLite databases are read through temporary snapshot copies.
  Read-only connections do not edit conversation records, but SQLite
  can create or update WAL shared-memory sidecars.
- Normal CLI operations run locally without network calls, telemetry,
  or a server. Terminal output and the store contain sensitive data;
  piping or copying them to another service is your choice.
- Semantic search uses quantized all-MiniLM-L6-v2 through ONNX on your
  CPU. The explicit [model setup step](#optional-semantic-search-setup)
  downloads its weights and tokenizer from Hugging Face and verifies
  pinned digests. Query and session content are not sent to that service.

Both read-only behavior and WAL-safe snapshotting are pinned by
executed specifications:
[`features/03_importers/contract.feature`](https://github.com/DataParade-io/BotSpy/blob/develop/features/03_importers/contract.feature)
and
[`features/03_importers/wal_snapshot.feature`](https://github.com/DataParade-io/BotSpy/blob/develop/features/03_importers/wal_snapshot.feature).

## Supported agents and where their data lives

| Agent | Source name | Reads |
|---|---|---|
| Claude Code | `claude_code` | `~/.claude/projects/` — per-session JSONL transcripts |
| Cursor | `cursor` | `~/.cursor/state.vscdb` and CLI agent transcripts, read through a WAL-safe snapshot copy |
| Codex | `codex` | `~/.codex/sessions/` rollout JSONL plus the thread index at the root |
| Grok Bot | `grok_bot` | `~/.grok/sand-client-persistence/` entry logs, plus roster and cloud-agent files |
| Antigravity | `antigravity` | `~/.gemini/antigravity/` summaries DB, brain transcripts, and payload databases, read through snapshot copies |

Default roots sit under your home directory; `--root` and `--home`
override them. The full inventory of every exact path, in every
category, is in
[`docs/sensitive-data.md`](https://github.com/DataParade-io/BotSpy/blob/develop/docs/sensitive-data.md).

## Install

```bash
cargo install botspy --locked
```

Prerequisites:

- A current stable Rust toolchain — install or update it via
  [rustup](https://rustup.rs).
- A C compiler: SQLite is bundled and compiled at build time.
  - macOS: Xcode Command Line Tools (`xcode-select --install`)
  - Linux: `build-essential` (Debian/Ubuntu) or your distribution's
    `gcc`/`clang` packages
  - Windows: the Microsoft C++ Build Tools (MSVC)

BotSpy is built and tested on Linux (x86_64 and arm64, glibc and musl),
macOS (Apple Silicon and Intel), and Windows (MSVC).

Start with `botspy --help`, then `botspy sources` or `botspy doctor`.
For a text-only local index, run `botspy import --no-embed`, then
`botspy search 'your query'`. The index copies session content to your
local store; use a deliberate source selection and location.

### Optional semantic search setup

The installed CLI has no model-download command. Run the setup example
from a checkout of the release matching `botspy --version`:

```bash
git clone https://github.com/DataParade-io/BotSpy.git
cd BotSpy
git checkout v0.3.0 # replace with the installed release tag
cargo run --example setup_models
```

The cache is `BOTSPY_MODELS` when set, otherwise
`$HOME/.botspy/models`. `--home` and `BOTSPY_HOME` do not move this cache.
Run setup before the first import into a store intended for semantic
search, then omit `--no-embed` when importing. Setup alone does not add
vectors to an existing text-only store, and unchanged sessions are not
re-embedded by `--full`. To index existing source history with embeddings,
import into a new `--db` path after setup. See
[search setup](https://github.com/DataParade-io/BotSpy/blob/develop/docs/search.md).

## Usage

The examples below come from BotSpy's synthetic test corpus (`--home
/tmp/demo` points at fixture agent files). Paths, ids, and titles are
fixtures, not real sessions. Most verbs support `-o human` (default),
`-o json`, and `-o ndjson`; machine modes never truncate. `import`
currently prints a human report even when an output flag is supplied.
Add `--no-truncate` in human mode for full ids and paths.

### See what BotSpy can read

```console
$ botspy sources --home /tmp/demo
SOURCE        ROOT                                        SESSIONS  STATUS
claude_code   /tmp/demo/.claude/projects                  4         ok
cursor        /tmp/demo/.cursor/state.vscdb               2         ok
codex         /tmp/demo/.codex                            2         ok
grok_bot      …mp/demo/.grok/sand-client-persistence      3         ok
antigravity   /tmp/demo/.gemini/antigravity               2         ok
```

A source whose files are missing is listed as `missing`, never hidden.
Restrict to one source with `--source claude_code`; point at a different
home with `--home` (or the `BOTSPY_HOME` environment variable).

### Check the health of your sources

```console
$ botspy doctor --home /tmp/demo
claude_code   ok    3 project dirs, 4 transcripts, 0 issues
cursor        ok    2 composers, 4 bubbles, 0 issues
codex         ok    2 threads, 0 issues
grok_bot      ok    2 entry logs, 1 cloud agents, 0 issues
antigravity   ok    2 conversations, 2 transcripts, 0 issues
```

Doctor reports; it does not gate — skipped files and missing stores are
data about your machine, not failures. The exit code stays `0`. Narrow
it with `botspy doctor --source cursor`.

### List sessions across every agent

```console
$ botspy sessions --home /tmp/demo
SESSION   AGENT         PROJECT               MSGS  LAST ACTIVITY         TITLE
00000000  claude_code   …ynth-workspace-beta     1  2026-10-01T09:20:00Z  —
00000000  claude_code   …nth-workspace-alpha     1  2026-10-01T09:10:00Z  —
conv-syn  antigravity   corpus sweep             0  2026-10-01T09:05:00Z  —
00000000  claude_code   …nth-workspace-alpha     4  2026-10-01T09:00:15Z  Synthetic parser session
conv-syn  antigravity   parser work              0  2026-10-01T09:00:00Z  —
solo-abc  claude_code   extra-demo               2  2026-09-15T12:05:00Z  —
00000000  grok_bot                               0  —                     —
00000000  grok_bot                               0  —                     —
composer  cursor        parser work              0  —                     —
composer  cursor        corpus sweep             0  —                     —
peer-syn  grok_bot                               0  —                     —
t-synth-  codex         /workspace/alpha         0  —                     —
t-synth-  codex         /workspace/alpha         0  —                     —
13 sessions (use --no-truncate for full ids)
```

Sessions are ordered by most recent activity. Filters combine:

```console
$ botspy sessions --home /tmp/demo --since 2026-10-01T09:05:00Z
SESSION   AGENT         PROJECT               MSGS  LAST ACTIVITY         TITLE
00000000  claude_code   …ynth-workspace-beta     1  2026-10-01T09:20:00Z  —
00000000  claude_code   …nth-workspace-alpha     1  2026-10-01T09:10:00Z  —
conv-syn  antigravity   corpus sweep             0  2026-10-01T09:05:00Z  —
3 sessions (use --no-truncate for full ids)
```

- `--source <name>` (repeatable), `--project <id>`, `--limit <n>`
- `--since <timestamp>` / `--until <timestamp>` keep only sessions with
  activity in the window (RFC 3339 timestamps)
- `-o json` for the full summaries:

```console
$ botspy sessions --home /tmp/demo --source claude_code -o json
[
  {
    "id": "00000000-0000-4000-8000-000000000003",
    "agent": "claude_code",
    "project_id": "-Users-synth-workspace-beta",
    "started_at": "2026-10-01T09:20:00Z",
    "last_activity_at": "2026-10-01T09:20:00Z",
    "message_count": 1,
    "metadata": {}
  },
  …
]
```

### Open one session

```console
$ botspy show --home /tmp/demo solo-abc123
Session solo-abc (claude_code, extra-demo) — 2 messages, started 2026-09-15T12:00:00Z
#1 user 2026-09-15T12:00:00Z
  · text  hello from the fixture
#2 user 2026-09-15T12:05:00Z
  · text  second message
```

A session is addressed by the agent's own stable id, or any unambiguous
prefix. Drill into a single message by its 1-based ordinal:

```console
$ botspy show --home /tmp/demo solo-abc123 --message 2
Session solo-abc (claude_code, extra-demo) — 2 messages, started 2026-09-15T12:00:00Z
#2 user 2026-09-15T12:05:00Z
  · text  second message
```

`-o json` prints the whole session; `-o ndjson` streams one message per
line. An unknown id is an error, never an empty result; an ambiguous
prefix is an error listing the candidates, never a guess.

### WAL-safe snapshot of a SQLite source

```console
$ botspy snapshot --home /tmp/demo cursor
snapshot: /tmp/demo/.botspy/snapshots/cursor/snapshot.db
rows: 8
source unmutated (digest verified before/after)
```

The positional argument is a source name or a path to a SQLite database.
Use `--out <dir>` to override where the copy lands (default:
`<home>/.botspy/snapshots/<source>/snapshot.db`).

### Mine the adapters into the local store

`botspy scan` runs a CDC pass into `<home>/.botspy/store.db` (override
with `--store <path>`). Pruning is off — a source that reports nothing
never deletes stored history. For scripts and CI, run exactly one pass
with `--once`:

```console
$ botspy scan --home /tmp/demo --once --store /tmp/demo/.botspy/store.db
store  /tmp/demo/.botspy/store.db
SOURCE        ADDED  UPDATED  UNCHANGED  SKIPPED  ERRORS  MESSAGES
claude_code       4        0          0        0       0  +8
cursor            2        0          0        0       0  +4
codex             2        0          0        0       0  +7
grok_bot          2        0          0        0       1  +5
antigravity       2        0          0        0       0  +8
scanned 5 sources in 0.0s: 12 added, 0 updated, 0 unchanged, 0 skipped, 1 errors
```

Without `--once`, scan keeps running: it re-scans every
`--interval <SECONDS>` seconds (default `60`, measured from the end of
the previous pass so passes never overlap) until interrupted. SIGINT lets
the current pass finish and commit, then exits with code `130`.

```console
$ botspy scan --home /tmp/demo --interval 1
pass 1
store  /tmp/demo/.botspy/store.db
SOURCE        ADDED  UPDATED  UNCHANGED  SKIPPED  ERRORS  MESSAGES
claude_code       4        0          0        0       0  +8
…
scanned 5 sources in 0.0s: 12 added, 0 updated, 0 unchanged, 0 skipped, 1 errors
pass 2
store  /tmp/demo/.botspy/store.db
SOURCE        ADDED  UPDATED  UNCHANGED  SKIPPED  ERRORS  MESSAGES
claude_code       0        0          4        0       0  0
…
```

`--since` accepts an RFC 3339 timestamp, an ISO date (`YYYY-MM-DD`), or a
relative duration like `7d` / `24h`. `--dry-run` reports what would
change without writing.

### One-shot import into the local store

`botspy import` runs a single refresh pass with pruning on: sessions the
selected sources no longer report are removed. Sessions from unselected
agents remain in the store. A missing or unreadable selected source can
look empty to discovery; use `scan --once` when you need a pass without
pruning. The default store is
`<home>/.botspy/store.db` (override with `--db <path>`). Use `--full` to
re-extract every session from byte zero; `--dry-run` reports what would
land and writes nothing.

```console
$ botspy import --home /tmp/demo --no-embed --db /tmp/demo/.botspy/store.db
12 new sessions, 0 updated sessions, 0 unchanged sessions, 0 pruned sessions
store: /tmp/demo/.botspy/store.db
```

`scan` loops on an interval with pruning off; `import` is the one-shot,
prune-on shape for scripts and ad-hoc refreshes.

### Search the store

`botspy search` reads the local store, not the live sources. Text mode
(FTS5) is the default; `--mode semantic` and `--mode hybrid` need
both the model cache and stored embeddings. Complete
[model setup](#optional-semantic-search-setup) before importing into a
new store, then omit `--no-embed`. A model download alone does not
populate the vector index.

```console
$ botspy search parser --home /tmp/demo --no-truncate --db /tmp/demo/.botspy/store.db
SESSION                               MSG  SCORE  TEXT
00000000-0000-4000-8000-000000000001    0  0.016  kick off the parser work
00000000-0000-4000-8000-000000000001    3  0.016  the parser work is done
2 hits in 1 session
```

### Stats over the store

`botspy stats` buckets sessions by source (default), project (`--by
project`), or day of last activity (`--by day`).

```console
$ botspy stats --home /tmp/demo --db /tmp/demo/.botspy/store.db
SOURCE       SESSIONS
claude_code         4
antigravity         2
codex               2
cursor              2
grok_bot            2
total: 12 sessions
```

`watch` is specified on the board but not shipped yet. See
[`docs/cli.md`](https://github.com/DataParade-io/BotSpy/blob/develop/docs/cli.md)
for every flag.

### Pipe and compose

```bash
botspy sessions -o ndjson | jq -r .project_id | sort | uniq -c
botspy show solo-abc -o ndjson | jq -c 'select(.role=="assistant")'
botspy sources -o json | jq -r '.[] | select(.status != "ok") | .name'
```

## The library: one local store, searchable

As a library, BotSpy keeps everything it ingests in one local SQLite
store (WAL mode, with the sqlite-vec extension). `Store::open(path)`
uses an explicit path; `Store::open_default()` uses
`~/.botspy/store.db` (`BOTSPY_HOME` moves it). Add the library with
`cargo add botspy`, then register adapters, ingest their sessions, and
query the store:

```rust
use botspy::{adapter::Adapter, adapters::claude_code::ClaudeCodeSource, store::Store};
use std::sync::Arc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let store = Store::open("botspy-demo.db")?;
    let sources: Vec<Arc<dyn Adapter>> = vec![Arc::new(ClaudeCodeSource::new(
        "path/to/claude/projects",
    ))];
    let report = store.ingest(&sources)?;
    println!("{} new sessions", report.new);
    for session in store.query().sessions() {
        println!("{}: {} messages", session.id, session.message_count);
    }
    Ok(())
}
```

Replace the example source path with a directory you intend to read.
This example makes a plaintext local copy and does not enable embeddings.
The query interface supports:

- iteration over sessions, messages, and parts in order, with filters
  by source, project, part kind, and time window pushed down to SQLite;
- FTS5 text search, ranked and filter-composable;
- semantic search over all-MiniLM-L6-v2 embeddings (computed on your
  CPU, after explicit model setup and embedding-enabled ingest),
  with a relevance floor so noise never ranks;
- hybrid search that fuses the lexical and semantic halves.

Incremental refresh keeps the store in step with the sources without
re-mining everything. The behavior is pinned by the executable specs
under
[`features/02_querying/`](https://github.com/DataParade-io/BotSpy/blob/develop/features/02_querying/)
and the store chapter in
[`docs/store.md`](https://github.com/DataParade-io/BotSpy/blob/develop/docs/store.md).

## Parallel sessions timeline

`cargo run --example timeline` reads your sessions through the public API
and writes one self-contained HTML file (inline SVG and CSS, no scripts,
nothing loaded from elsewhere): sessions active at once on top, one
swimlane per session below, grouped by project. Bars are active stretches,
dotted lines are idle breaks (gaps that are outliers among that session's
own gaps, so there is no fixed cutoff), and sub-agents and forks nest under
the session that spawned them. Agents differ by pattern and legend label,
not colour alone, and the page follows light and dark mode.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/DataParade-io/BotSpy/develop/docs/images/timeline.dark.png">
  <img src="https://raw.githubusercontent.com/DataParade-io/BotSpy/develop/docs/images/timeline.light.png" alt="Timeline of a synthetic demo: a concurrency curve peaking at 5 sessions, and swimlanes for three projects with idle breaks, a fork and sub-agents">
</picture>

The picture above is rendered from the synthetic `--demo` corpus.

```bash
cargo run --example timeline -- --demo            # try it on made-up sessions
cargo run --example timeline -- --since 2026-10-01T00:00:00Z --project web \
  --source claude_code --source codex --out week.html [data-root]
```

Flags: `--since`, `--until`, `--project` (substring), `--source`
(repeatable), `--titles`, `--out PATH` (default `timeline.html`), `--demo`.
Session titles appear only with `--titles`; message text never appears.

## Learn more

The docs pages are the deep reference; this README stays the overview.

- [`docs/architecture.md`](https://github.com/DataParade-io/BotSpy/blob/develop/docs/architecture.md)
  — the C4 views and the end-to-end data flow
- [`docs/cli.md`](https://github.com/DataParade-io/BotSpy/blob/develop/docs/cli.md)
  — the `botspy` command: every verb, flag, output mode, and exit code
- [`docs/store.md`](https://github.com/DataParade-io/BotSpy/blob/develop/docs/store.md)
  — the local store: where it lives, its tables, ingest, and
  incremental refresh; how to use BotSpy as a library
- [`docs/search.md`](https://github.com/DataParade-io/BotSpy/blob/develop/docs/search.md)
  — text search, semantic search, hybrid search, and the one-time model
  download
- [`docs/sensitive-data.md`](https://github.com/DataParade-io/BotSpy/blob/develop/docs/sensitive-data.md)
  — every path BotSpy reads, copies, or writes
- [`docs/sources/`](https://github.com/DataParade-io/BotSpy/blob/develop/docs/sources/)
  — one document per source adapter, plus the shared adapter protocol
- [`docs/diagrams.md`](https://github.com/DataParade-io/BotSpy/blob/develop/docs/diagrams.md)
  — the diagram sources and how to re-render them
- [`docs/DESIGN.md`](https://github.com/DataParade-io/BotSpy/blob/develop/docs/DESIGN.md)
  — the original design narrative and detailed FAQ
- API documentation: [docs.rs/botspy](https://docs.rs/botspy)
- Executable specifications: the Gherkin behavior specs live under
  [`features/`](https://github.com/DataParade-io/BotSpy/blob/develop/features/)
  and run with `cargo test --test bdd`; the specs are the source of
  truth, and the implementation is generated from them
- Example consumers of the public API: [`examples/`](https://github.com/DataParade-io/BotSpy/blob/develop/examples/)
  — a session meter, local sentiment analysis, a metrics stub, and
  [privacy controls](https://github.com/DataParade-io/BotSpy/blob/develop/examples/privacy/main.rs)
  that find and redact secrets and personal data with LeftBlock
  (`pipx install leftblock`) before any analysis runs, and a
  [parallel sessions timeline](#parallel-sessions-timeline) that draws
  how many sessions ran at once as one local HTML page
- Repository: [github.com/DataParade-io/BotSpy](https://github.com/DataParade-io/BotSpy)

## Development

Use a current stable Rust toolchain and the C compiler listed under
Install. The tests use synthetic fixtures; no personal agent histories
are needed. From a repository checkout:

```bash
rustup update stable
export BOTSPY_MODELS="$PWD/target/botspy-models"
cargo +stable run --example setup_models
cargo +stable test
cargo +stable fmt --all --check
cargo +stable clippy --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo +stable doc --no-deps
```

The setup step downloads verified public model assets into the explicit
ignored build directory. `cargo test` includes the executable Gherkin
suite; `cargo test --test bdd` runs that suite on its own. See
[CONTRIBUTING_AGENT.md](https://github.com/DataParade-io/BotSpy/blob/develop/CONTRIBUTING_AGENT.md)
for the required Kanbus issue, specification-first workflow, receipt
hooks, and pull requests into `develop`.

## License

MIT — see the unchanged copyright notice and terms in [LICENSE](LICENSE).
