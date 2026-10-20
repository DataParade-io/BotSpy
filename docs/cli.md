# The BotSpy CLI

The `botspy` command reads coding-agent histories and maintains a local
SQLite index. It ships nine commands: `sources`, `sessions`, `show`,
`doctor`, `snapshot`, `scan`, `import`, `search`, and `stats`.
`watch` is not implemented. Use `botspy <command> --help` for the
installed version's accepted flags.

## Common flags and paths

Flags go after the command, for example `botspy sessions -o json`.

| Flag | Effect |
| --- | --- |
| `-s, --source <name>` | Select sources for discovery/import/scan; repeat to select several. `search` supports at most one source filter. `stats` currently ignores this flag. |
| `--root <path>` | Override the live source root; requires exactly one `--source`. It does not choose a store database. |
| `--home <path>` | Derive source roots and the default store path from this home. |
| `-o, --output <mode>` | `human` (default), `json`, or `ndjson`. `import` currently emits a human report regardless of this flag. |
| `--no-truncate` | Show full ids and paths in human output. Machine output is not truncated. |

Source names are exact: `claude_code`, `cursor`, `codex`, `grok_bot`,
and `antigravity`. Home resolution is `--home`, then `BOTSPY_HOME`,
then `$HOME`. The default store is `<home>/.botspy/store.db`.
`scan --store` chooses another database; `import`, `search`, `stats`,
and store-backed `sessions` use `--db`.

The embedding-model cache is separate: `BOTSPY_MODELS`, otherwise
`$HOME/.botspy/models`. Neither `--home` nor `BOTSPY_HOME` moves it.
The installed CLI does not download models; see [search.md](search.md)
for explicit setup from a source checkout.

`botspy --version` prints the version. It is a top-level flag.

## Output and exit codes

JSON/NDJSON represent the command's records without human truncation.
`show -o ndjson` emits messages; `sessions -o ndjson` emits session
summaries. A continuous scan emits a report after each pass, so its
JSON output is a sequence of documents, not one JSON array. `import`
currently supports only the human summary in practice.

| Code | Meaning |
| --- | --- |
| 0 | Success, including empty results and reported skips. |
| 1 | Target not found: an unknown session or source. |
| 2 | Usage error, such as invalid flags or ambiguous source selection. |
| 3 | Source/store/snapshot failure. |
| 130 | Scan stopped after an interrupt. |

Doctor and scan report many per-source problems as data; exit 0 does
not prove that every source was read successfully. Inspect their reports.
Output can contain prompts, source code, tool output, and sensitive paths.

## Commands

### `sources`

Discover the selected sources and report their resolved roots, session
counts, and `ok` or `missing` status. No source selection means all five.
Discovery runs each time; the command does not create the local index.

```bash
botspy sources --source claude_code -o json
```

### `doctor`

Report source paths, discovered counts, skipped files, and adapter
issues. Missing or malformed source content can appear in the report
without a failing exit code. `--parity` is not implemented.

```bash
botspy doctor --source cursor -o json
```

### `sessions`

List session summaries from the live adapters, ordered by recent
activity. `--db <path>` instead lists the local store. Filters:
`--source`, `--project <id>`, `--since <timestamp>`, `--until <timestamp>`,
and `--limit N`. The date bounds are RFC 3339 timestamps. `--tree` and
`--kind` are not implemented.

```bash
botspy sessions --source codex --limit 10 -o ndjson
botspy sessions --db ./history.db --project example-app -o json
```

### `show <session-id>`

Open a live-source session by its native id or unambiguous prefix.
An unknown id exits 1; an ambiguous prefix reports the candidates.
`--message N` selects a 1-based message ordinal. There is no `--db`
option for this command. `--turn`, `--thinking`, `--raw`, and `--graph`
are not implemented.

```bash
botspy show synth-session --source claude_code --message 1 -o json
```

### `snapshot <source-or-path>`

Copy a SQLite source with the online backup API over a read-only
connection. The output is `<home>/.botspy/snapshots/<source>/snapshot.db`
by default, or `<out>/snapshot.db` with `--out <dir>`. A destination
that aliases the source is rejected. Existing `snapshot.db-wal`,
`snapshot.db-shm`, or `snapshot.db-journal` files also cause refusal,
because they may belong to a live or recoverable destination database.
Close or recover that database through its owning application before
retrying; do not delete sidecars blindly. An explicit snapshot persists;
you manage its lifetime.

The command reports a row count for known `events` or `cursorDiskKV`
tables. Arbitrary SQLite files can be copied, but row reporting fails
if neither table exists. A source name currently resolves directly to
its root, so `cursor` is supported; pass the database file path for a
source whose root is a directory, such as Codex or Antigravity.

Before/after digests compare source files that existed before the read.
They are a diagnostic, not proof that BotSpy caused a change: a live
writer can change the files concurrently. SQLite can create or update
shared-memory sidecars even for read-only source connections.

```bash
botspy snapshot cursor --out ./snapshots/cursor -o json
```

### `scan`

Discover and refresh the local store with pruning disabled. A missing
source never removes stored sessions. By default it repeats every
`--interval <seconds>` (60 by default, at least 1), measured from the
end of the previous pass. `--once` runs one pass and exits.

Use `--source`, `--store <path>`, and `--dry-run` as needed. `--since`
accepts an RFC 3339 timestamp, an ISO date (`YYYY-MM-DD`), or a relative
duration such as `7d` or `24h`. It limits what is read; older stored
sessions remain. An interrupt lets the current pass finish, then exits
130. Scan is a foreground process, not a system service.

```bash
botspy scan --source claude_code --once --store ./history.db
botspy scan --since 7d --interval 60 -o ndjson
```

### `import`

Run one refresh pass with pruning enabled. Sessions no longer reported
by the selected agents are removed; unselected agents' sessions remain.
A selected root that is missing or unreadable can look empty to
discovery. Use `scan --once` if you want to retain all existing sessions.

Flags: `--source`, `--db <path>`, `--full`, `--dry-run`, `--no-embed`.
`--full` re-extracts reported sessions even when their discovery summary
is unchanged. `--dry-run` previews counts. `--no-embed` skips embeddings;
if the model cache is absent, ordinary import also falls back to text-only
and reports this. Model setup alone does not backfill old vectors; see
[search.md](search.md). Output is currently a human report for all
`--output` values.

```bash
botspy import --source claude_code --no-embed --db ./history.db
botspy import --source claude_code --dry-run --db ./history.db
```

### `search <query>`

Search the local store, not the live adapters. Flags: `--db <path>`,
`--mode text|semantic|hybrid` (default `text`), one `--source`,
`--project <id>`, and `--limit N`. Text mode uses FTS5; semantic mode
uses local embeddings; hybrid combines both. `--kind`, `--role`,
`--context`, and time filters are not implemented. See
[search.md](search.md) for ranking and model setup.

```bash
botspy search 'parser error' --db ./history.db -o json
```

### `stats`

Count stored sessions by `--by source|project|day` (default `source`).
`--db <path>` selects the store. Rows are sorted by descending count,
then key. This command does not aggregate token usage or costs.
`--source` is accepted as a common flag but currently does not filter
the results; project and time filters are not implemented.

```bash
botspy stats --db ./history.db --by project -o json
```

## Composition

```bash
botspy sessions -o ndjson | jq -r .project_id | sort | uniq -c
botspy show synth-session -o ndjson | jq -c 'select(.role=="assistant")'
botspy sources -o json | jq -r '.[] | select(.status != "ok") | .name'
```
