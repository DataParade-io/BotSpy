# Sensitive data

Your coding-agent sessions hold sensitive content: prompts, source
code, file contents, tool output, thinking traces, and sometimes
credentials and secrets that you or a tool put into a prompt.

Sensitive-data visibility is a central purpose of BotSpy, not a
side note. BotSpy reads this content, copies it, and indexes it so you
can search it. That only helps you if you can also see exactly where
the copies live. This page is that map.

## The marker

Every diagram in this documentation marks a sensitive place with the
same marker: an amber border, 3 px wide, plus the explicit text label
"Sensitive data". The
marker means: **this place reads, copies, or stores session content.**
It is not a claim that the data is protected, which is why no lock or
shield icon appears anywhere; the shared theme file
`diagrams/_shared.d2` defines the marker once for all diagrams.

Places that only move or transform data — adapters, the CLI process,
the model cache — keep the theme's default look.

## The inventory

The paths BotSpy reads, copies, or writes. `<home>` is the
`--home` flag, else `BOTSPY_HOME`, else `$HOME`. `~` in the source
inventory means that resolved home; `--root` can replace one source
root. Model cache resolution is different, as described below.

### 1. Read (the agents' own conversation records)

| Source | Exact paths | Content |
| --- | --- | --- |
| Claude Code | `~/.claude/projects/<encoded-cwd>/<session>.jsonl`; `~/.claude/projects/<encoded-cwd>/subagents/agent-*.jsonl` (skipped, counted) | Prompts, replies, thinking, tool calls and results, costs, titles |
| Cursor | `~/.cursor/state.vscdb` (plus `-wal`, `-shm`); `~/.cursor/projects/<encoded-cwd>/agent-transcripts/*.jsonl` | Composer conversations, bubbles, tool blobs |
| Codex | `~/.codex/state_5.sqlite`; `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` | Threads, rollouts, messages, reasoning, tool calls, token usage |
| Grok Bot | `~/.grok/sand-client-persistence/*.json` — entry logs, plus the reserved `roster.json` and `cloud-agents.json` | Messages, voice calls (raw passthrough), agent names, agent status |
| Antigravity | `~/.gemini/antigravity/conversation_summaries.db`; `~/.gemini/antigravity/brain/<id>/payload.db`; `~/.gemini/antigravity/brain/<id>/.system_generated/logs/transcript.jsonl` | Conversations, planner responses, checkpoints, tool calls and results |

### 2. Copied (BotSpy-created snapshot copies — temporary or cached)

| Copy | Exact path | When |
| --- | --- | --- |
| Cursor, Codex, and Antigravity snapshots | `<temp>/botspy-snapshot-<random>/snapshot.db` (one per opened database) | Automatic: the live databases are read through these copies |
| Explicit snapshot | `<home>/.botspy/snapshots/<source>/snapshot.db`, or `<out>/snapshot.db` with `--out` | Written by the `snapshot` verb, on demand; persists until removed |

`<temp>` is the platform temporary directory (`$TMPDIR` on Unix when
set). Automatic copies use random private directories. On Unix the
directory mode is `0700` and snapshot file mode is `0600`. The directory
is removed when its owning SQLite connection is dropped, including
normal error returns. A crash, forced kill, or filesystem cleanup
failure can leave a copy behind. Earlier versions also used
`botspy-cursor-snaps`, `botspy-ag-snaps`, and `botspy-codex-snaps`
directories with persistent copies; upgrading does not remove them.

The copies contain sensitive source data. The snapshot command compares
before/after digests of source files that existed before the read. A
changed digest can reflect a concurrent writer, and new SQLite sidecars
are not included in that comparison. The diagnostic is not a guarantee
that source sidecars are untouched or that a live writer never waits.

Explicit snapshot destinations must be idle and under your control.
Destination validation is not a concurrency lock and does not protect
against another process replacing a parent directory during the copy.
Keep independent source backups; BotSpy's snapshots and index are not
a backup policy.

### 3. Stored (BotSpy's own store — the long-term copy)

| Place | Exact path | Content |
| --- | --- | --- |
| Store database | `<home>/.botspy/store.db` | Full session JSON (`sessions`, `messages`), plaintext part text (`parts`) |
| WAL sidecars | `<home>/.botspy/store.db-wal`, `<home>/.botspy/store.db-shm` | Recent page writes — same content |
| FTS5 index | the `message_fts` virtual table inside `store.db` | Plaintext index over `parts.text` |
| Vector index | the `message_vec_rows` + `message_vec` tables inside `store.db` | Embeddings — derived data, not plaintext |

Provenance on every stored part carries the absolute source path, so
the store always knows where each piece of content came from.

### 4. Output (leaves BotSpy through your terminal)

| Place | Content |
| --- | --- |
| stdout / JSON / NDJSON of `sessions`, `show`, `search`, `stats` | Session summaries, full messages, part text, search hits — whatever you asked to see |

Search query text also transits the local embedder at search time
(semantic and hybrid modes). The embedder runs on your CPU and sends
nothing over the network.

### 5. Not sensitive

| Place | Exact path | Why |
| --- | --- | --- |
| Model cache | `$BOTSPY_MODELS/all-MiniLM-L6-v2/`, otherwise `$HOME/.botspy/models/all-MiniLM-L6-v2/` (`model_quantized.onnx`, `tokenizer.json`) | Model files only. No session data. Explicitly downloaded, digest-verified. |

`--home` and `BOTSPY_HOME` do not move the model cache. Setup downloads
public model assets from Hugging Face; it does not send session data.

## Source access and local copies

- Adapters do not edit conversation records. Source SQLite connections
  use read-only flags; Cursor, Codex, and Antigravity databases are
  copied with SQLite's online backup API before querying the copy.
- SQLite may create or update WAL shared-memory sidecars as part of
  reading. Normal SQLite locking and coordination still apply.
- Claude Code, Grok Bot, Codex rollouts, and other transcript files are
  read as files. BotSpy's store, explicit snapshots, and automatic
  snapshots are separate copies of sensitive content.
- The store and text index are plaintext, not encrypted or redacted.
  Embeddings are sensitive derived data, not an anonymization guarantee.
  Protect the storage paths and any terminal logs or redirected output.
- Normal CLI operations make no network calls. Explicit model setup is
  the download step; both model files have pinned digests.

## Removing the data

Stop BotSpy processes before removing their files. Deleting
`<home>/.botspy/` removes the default store, its sidecars, and explicit
snapshots under that directory. It does not remove:

- stores selected with `--db` or `--store` elsewhere;
- snapshots selected with `--out` elsewhere;
- automatic temporary copies still in use or left by older versions,
  a crash, forced kill, or cleanup error;
- redirected output, terminal logs, backups, or copies made by other tools;
- a model cache at a different `BOTSPY_MODELS` or `$HOME` location.

Inspect the appropriate temporary directory for leftover BotSpy snapshot
directories after processes stop. Removing a file is not a guarantee of
secure erasure from the underlying storage or its backups. The agents'
own histories are separate and remain in their original locations.
