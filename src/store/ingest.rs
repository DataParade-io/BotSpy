//! Adapter-driven ingestion: mine every registered adapter into the store.
//!
//! Ingestion drives the adapter contract as-is — `discover()` to find
//! sessions, `open()` to pull each one — and writes normalized rows in one
//! transaction per pass. Sources stay read-only: adapters have no write
//! surface, and the store never touches them.

use crate::adapter::Adapter;
use crate::schema::Agent;
use crate::session::{Session, UnknownSession};
use crate::store::{
    agent_code, delete_session_vectors, session_content_hash, sqlite_error, upsert_session, Store,
    StoreError,
};
use rusqlite::{params, OptionalExtension};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// What one ingest or refresh pass reports, per outcome.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct IngestReport {
    /// Sessions the store had not seen before.
    pub new: usize,
    /// Sessions whose stored rows were replaced because they changed.
    pub updated: usize,
    /// Sessions already in the store and unchanged — never re-opened from
    /// their adapter.
    pub unchanged: usize,
    /// Sessions pruned because their source no longer reports them.
    pub pruned: usize,
    /// Sessions not picked up because they fall before the pass's cutoff.
    pub skipped: usize,
}

/// Options limiting what an ingest or refresh pass picks up.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IngestOptions {
    /// Only sessions whose `last_activity_at` is at or after this
    /// timestamp are picked up (timestamps compare the way the library
    /// records them: UTC strings, lexicographically). Sessions with no
    /// recorded activity are never cut off — they cannot be judged. A
    /// cutoff never deletes: sessions already in the store stay there.
    pub since: Option<String>,
    /// Compute the pass's report but roll the transaction back, writing
    /// nothing.
    pub dry_run: bool,
    /// Refresh passes only: prune sessions belonging to the supplied
    /// adapters' agents that those sources no longer report. Sessions from
    /// unselected agents are retained. Ingest passes never prune.
    pub prune: bool,
    /// Refresh passes only: ignore the staleness shortcut and re-extract
    /// every reported session from byte zero, so fixes to an adapter's
    /// parsing reach the store even when the discover summary (id,
    /// last_activity_at, message_count) looks unchanged.
    pub force_full: bool,
}

/// What one adapter contributed to an ingest or refresh pass. `discovered`
/// counts every summary the adapter's discovery reported — including ids
/// another adapter reported first and sessions before the cutoff — so it
/// can exceed its outcome counts summed. An adapter that reports nothing
/// shows a zero row, never a silent absence.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct AdapterIngestReport {
    /// The agent the adapter taps.
    pub agent: Agent,
    /// Summaries the adapter's discovery reported.
    pub discovered: usize,
    /// Sessions stored for the first time, and the messages they brought.
    pub added_sessions: usize,
    pub added_messages: usize,
    /// Sessions replaced because they changed, and the net message delta
    /// (negative when the replacement holds fewer messages).
    pub updated_sessions: usize,
    pub updated_message_delta: i64,
    /// Sessions already stored and unchanged, with their stored messages.
    pub unchanged_sessions: usize,
    pub unchanged_messages: usize,
    /// Sessions not picked up because they fall before the cutoff.
    pub skipped_sessions: usize,
    /// Sessions the adapter reported but could not open.
    pub errors: usize,
}

/// The full report of one ingest or refresh pass: the store-level totals
/// plus what each adapter contributed.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct DetailedIngestReport {
    /// The store-level totals. Under the first-report-wins rule an id
    /// reported by two adapters is counted once, so the totals are not
    /// the sum of the adapter rows.
    pub totals: IngestReport,
    /// One row per adapter, in the order the pass ran them.
    pub adapters: Vec<AdapterIngestReport>,
}

/// Which staleness rule the pass applies to sessions already stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PassMode {
    /// New sessions only: a stored id counts unchanged and is never
    /// re-opened from its adapter.
    Ingest,
    /// Staleness is judged from the discover summaries (id,
    /// last_activity_at, message_count); changed sessions are opened and
    /// re-ingested only when the content hash actually moved.
    Refresh,
}

/// One stored session row, as the staleness rules see it.
struct StoredRow {
    agent: String,
    last_activity_at: String,
    message_count: i64,
    content_hash: String,
}

impl Store {
    /// Mine every registered adapter into the store. A session reported by
    /// two adapters is stored once — the first adapter to report it wins
    /// (the chapter-1 rule) — and sessions already in the store are not
    /// re-opened from their adapters (a refresh pass handles changes).
    /// The store-level totals of [`Store::ingest_with`] with default
    /// options.
    pub fn ingest(&self, adapters: &[Arc<dyn Adapter>]) -> Result<IngestReport, StoreError> {
        self.ingest_with(adapters, &IngestOptions::default())
            .map(|detailed| detailed.totals)
    }

    /// [`Store::ingest`] with options: a `since` cutoff limits what is
    /// picked up (without deleting anything older), `dry_run` reports
    /// what would change without writing. The report carries what each
    /// adapter contributed.
    pub fn ingest_with(
        &self,
        adapters: &[Arc<dyn Adapter>],
        options: &IngestOptions,
    ) -> Result<DetailedIngestReport, StoreError> {
        self.pass(adapters, options, PassMode::Ingest)
    }

    /// [`Store::refresh_with`] with pruning enabled: sessions belonging to
    /// the supplied adapters' agents that those sources no longer report
    /// are removed. The store-level totals only.
    pub fn refresh(&self, adapters: &[Arc<dyn Adapter>]) -> Result<IngestReport, StoreError> {
        self.refresh_with(
            adapters,
            &IngestOptions {
                prune: true,
                ..IngestOptions::default()
            },
        )
        .map(|detailed| detailed.totals)
    }

    /// Keep the store in step with its sources without re-mining
    /// everything: staleness is judged from the discover summaries
    /// (id, last_activity_at, message_count), so unchanged sessions are
    /// never opened from their adapters; changed sessions are opened and
    /// re-ingested only when the content hash actually moved; with
    /// `prune`, missing sessions belonging to the supplied adapters' agents
    /// are removed while unselected agents' sessions are retained; with
    /// `force_full`, every reported session is re-extracted regardless of
    /// what its discover summary says. An empty adapter list prunes nothing.
    /// One transaction per pass; `dry_run` rolls it back, reporting what
    /// would have changed.
    pub fn refresh_with(
        &self,
        adapters: &[Arc<dyn Adapter>],
        options: &IngestOptions,
    ) -> Result<DetailedIngestReport, StoreError> {
        self.pass(adapters, options, PassMode::Refresh)
    }

    /// The one pass both entry points share: discover, judge staleness by
    /// the mode, write in one transaction, then commit (or roll the
    /// transaction back for a dry run).
    fn pass(
        &self,
        adapters: &[Arc<dyn Adapter>],
        options: &IngestOptions,
        mode: PassMode,
    ) -> Result<DetailedIngestReport, StoreError> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(sqlite_error("begin ingest transaction"))?;
        if let Some(embedder) = self.embedder.as_deref() {
            crate::store::check_embedding_model(&tx, embedder)?;
        }
        let stored: BTreeMap<String, StoredRow> = {
            let mut map = BTreeMap::new();
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT id, last_activity_at, message_count, content_hash, agent FROM sessions",
                )
                .map_err(sqlite_error("list stored sessions"))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                })
                .map_err(sqlite_error("list stored sessions"))?;
            for row in rows {
                let (id, last_activity_at, message_count, content_hash, agent) =
                    row.map_err(sqlite_error("list stored sessions"))?;
                map.insert(
                    id,
                    StoredRow {
                        agent,
                        last_activity_at,
                        message_count,
                        content_hash,
                    },
                );
            }
            map
        };
        let mut totals = IngestReport::default();
        let mut adapter_rows: Vec<AdapterIngestReport> = Vec::new();
        // First report wins within the pass: the first adapter to name an
        // id owns it for this transaction.
        let mut reported: BTreeSet<String> = BTreeSet::new();
        let mut selected_agents = BTreeSet::new();
        for adapter in adapters {
            let mut row = AdapterIngestReport {
                agent: adapter.agent(),
                ..AdapterIngestReport::default()
            };
            selected_agents.insert(agent_code(&row.agent));
            for summary in adapter.discover() {
                row.discovered += 1;
                if !reported.insert(summary.id.clone()) {
                    continue;
                }
                if let Some(cutoff) = &options.since {
                    if !summary.last_activity_at.is_empty()
                        && summary.last_activity_at.as_str() < cutoff.as_str()
                    {
                        row.skipped_sessions += 1;
                        totals.skipped += 1;
                        continue;
                    }
                }
                match stored.get(&summary.id) {
                    None => {
                        let Some(session) = adapter.open(&summary.id) else {
                            row.errors += 1;
                            continue;
                        };
                        row.added_messages += session.messages.len();
                        upsert_session(&tx, &session, self.embedder.as_deref())?;
                        row.added_sessions += 1;
                        totals.new += 1;
                    }
                    Some(stored_row) => match mode {
                        PassMode::Ingest => {
                            row.unchanged_sessions += 1;
                            row.unchanged_messages += stored_row.message_count as usize;
                            totals.unchanged += 1;
                        }
                        PassMode::Refresh => {
                            let stale = options.force_full
                                || stored_row.last_activity_at != summary.last_activity_at
                                || stored_row.message_count != summary.message_count as i64;
                            if !stale {
                                row.unchanged_sessions += 1;
                                row.unchanged_messages += stored_row.message_count as usize;
                                totals.unchanged += 1;
                                continue;
                            }
                            let Some(session) = adapter.open(&summary.id) else {
                                row.errors += 1;
                                continue;
                            };
                            let hash = session_content_hash(&session)?;
                            if hash == stored_row.content_hash {
                                row.unchanged_sessions += 1;
                                row.unchanged_messages += stored_row.message_count as usize;
                                totals.unchanged += 1;
                                continue;
                            }
                            row.updated_message_delta +=
                                session.messages.len() as i64 - stored_row.message_count;
                            upsert_session(&tx, &session, self.embedder.as_deref())?;
                            row.updated_sessions += 1;
                            totals.updated += 1;
                        }
                    },
                }
            }
            adapter_rows.push(row);
        }
        if mode == PassMode::Refresh && options.prune {
            for (id, stored_row) in &stored {
                if !selected_agents.contains(stored_row.agent.as_str()) || reported.contains(id) {
                    continue;
                }
                tx.execute("DELETE FROM message_fts WHERE session_id = ?1", params![id])
                    .map_err(sqlite_error("prune session"))?;
                delete_session_vectors(&tx, id)?;
                tx.execute("DELETE FROM sessions WHERE id = ?1", params![id])
                    .map_err(sqlite_error("prune session"))?;
                totals.pruned += 1;
            }
        }
        if options.dry_run {
            tx.rollback()
                .map_err(sqlite_error("roll back the dry-run pass"))?;
        } else {
            tx.commit().map_err(sqlite_error("commit ingest"))?;
        }
        Ok(DetailedIngestReport {
            totals,
            adapters: adapter_rows,
        })
    }

    /// Materialize one session out of the store: the serde JSON columns
    /// rebuild the exact session the adapter produced. An unknown id is a
    /// typed error, never a silent empty result.
    pub fn open_session(&self, id: &str) -> Result<Session, StoreError> {
        let session_json: Option<String> = self
            .conn
            .query_row(
                "SELECT session_json FROM sessions WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()
            .map_err(sqlite_error("read session row"))?;
        let Some(session_json) = session_json else {
            return Err(StoreError::UnknownSession { id: id.to_string() });
        };
        let mut session: Session =
            serde_json::from_str(&session_json).map_err(|err| StoreError::Sqlite {
                context: format!("deserialize session {id}"),
                source: rusqlite::Error::ToSqlConversionFailure(Box::new(err)),
            })?;
        let message_jsons: Vec<String> = self
            .conn
            .prepare("SELECT message_json FROM messages WHERE session_id = ?1 ORDER BY ordinal")
            .map_err(sqlite_error("read messages"))?
            .query_map(params![id], |row| row.get(0))
            .map_err(sqlite_error("read messages"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_error("read messages"))?;
        for message_json in message_jsons {
            let message: crate::schema::Message =
                serde_json::from_str(&message_json).map_err(|err| StoreError::Sqlite {
                    context: format!("deserialize message of session {id}"),
                    source: rusqlite::Error::ToSqlConversionFailure(Box::new(err)),
                })?;
            session.messages.push(message);
        }
        Ok(session)
    }
}

impl From<UnknownSession> for StoreError {
    fn from(unknown: UnknownSession) -> Self {
        StoreError::UnknownSession { id: unknown.id }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::fixture::FixtureAdapter;

    /// A fixture adapter with one single-message session last active at
    /// the given timestamp.
    fn one_session_adapter(agent: Agent, id: &str, at: &str) -> FixtureAdapter {
        let adapter = FixtureAdapter::new(agent);
        adapter.add_session(Session {
            id: id.to_string(),
            agent,
            started_at: at.to_string(),
            last_activity_at: at.to_string(),
            messages: vec![crate::schema::Message {
                role: crate::schema::Role::User,
                parts: vec![crate::schema::Part::Known(crate::schema::KnownPart::Text {
                    text: format!("hello from {id}"),
                    extra: None,
                })],
                timestamp: Some(at.to_string()),
                ..crate::schema::Message::default()
            }],
            ..Session::default()
        });
        adapter
    }

    fn temp_store(tag: &str) -> Store {
        let dir =
            std::env::temp_dir().join(format!("botspy-ingest-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Store::open(dir.join("store.db")).expect("store opens")
    }

    #[test]
    fn the_cutoff_skips_only_sessions_before_it() {
        let store = temp_store("cutoff");
        let adapters: Vec<Arc<dyn Adapter>> = vec![
            Arc::new(one_session_adapter(
                Agent::ClaudeCode,
                "kept",
                "2026-10-01T09:00:00Z",
            )),
            Arc::new(one_session_adapter(
                Agent::Cursor,
                "cut",
                "2026-09-01T09:00:00Z",
            )),
        ];
        let report = store
            .ingest_with(
                &adapters,
                &IngestOptions {
                    since: Some("2026-10-01T00:00:00Z".to_string()),
                    ..IngestOptions::default()
                },
            )
            .expect("ingest with cutoff");
        assert_eq!(report.totals.new, 1, "only the newer session stored");
        assert_eq!(report.totals.skipped, 1, "the older session skipped");
        assert_eq!(report.adapters.len(), 2, "one row per adapter");
        assert_eq!(
            report.adapters[0].added_messages, 1,
            "the session's message"
        );
        let ids: Vec<String> = store.sessions().map(|summary| summary.id).collect();
        assert_eq!(ids, vec!["kept"], "the cut-off session stayed out");
    }

    #[test]
    fn a_dry_run_reports_but_writes_nothing() {
        let store = temp_store("dryrun");
        let adapters: Vec<Arc<dyn Adapter>> = vec![Arc::new(one_session_adapter(
            Agent::ClaudeCode,
            "d1",
            "2026-10-01T09:00:00Z",
        ))];
        let report = store
            .ingest_with(
                &adapters,
                &IngestOptions {
                    dry_run: true,
                    ..IngestOptions::default()
                },
            )
            .expect("dry run");
        assert_eq!(report.totals.new, 1, "the dry run reports the session");
        assert_eq!(store.sessions().count(), 0, "the dry run wrote nothing");
        let report = store
            .ingest(&adapters)
            .expect("the real ingest after the dry run");
        assert_eq!(report.new, 1, "the session was still new");
    }

    #[test]
    fn an_unopenable_session_is_counted_not_silently_dropped() {
        let store = temp_store("unopenable");
        let adapter = FixtureAdapter::new(Agent::ClaudeCode);
        adapter.add_session(Session {
            id: "ghost".to_string(),
            agent: Agent::ClaudeCode,
            last_activity_at: "2026-10-01T09:00:00Z".to_string(),
            ..Session::default()
        });
        adapter.block_open("ghost");
        let adapters: Vec<Arc<dyn Adapter>> = vec![Arc::new(adapter)];
        let report = store
            .ingest_with(&adapters, &IngestOptions::default())
            .expect("ingest");
        assert_eq!(report.totals.new, 0, "nothing stored");
        assert_eq!(report.totals.pruned, 0, "nothing pruned");
        assert_eq!(report.adapters[0].errors, 1, "the failed open is counted");
        assert_eq!(report.adapters[0].discovered, 1, "the ghost was discovered");
    }

    #[test]
    fn an_adapter_reporting_nothing_shows_a_zero_row() {
        let store = temp_store("zero-row");
        let adapters: Vec<Arc<dyn Adapter>> = vec![
            Arc::new(one_session_adapter(
                Agent::ClaudeCode,
                "z1",
                "2026-10-01T09:00:00Z",
            )),
            Arc::new(FixtureAdapter::new(Agent::Codex)),
        ];
        let report = store.ingest(&adapters).expect("ingest");
        assert_eq!(report.new, 1, "the first pass stores the session");
        let detailed = store
            .ingest_with(&adapters, &IngestOptions::default())
            .expect("detailed ingest");
        let codex = &detailed.adapters[1];
        assert_eq!(codex.agent, Agent::Codex);
        assert_eq!(codex.discovered, 0, "zero sessions, visible as a zero row");
        assert_eq!(
            codex.added_sessions, 0,
            "zero sessions, visible as a zero row"
        );
        assert_eq!(
            detailed
                .adapters
                .iter()
                .map(|row| row.added_sessions)
                .sum::<usize>(),
            detailed.totals.new,
            "the totals agree with the per-adapter rows"
        );
    }

    #[test]
    fn a_refresh_prunes_only_when_asked() {
        let store = temp_store("noprune");
        let adapter = Arc::new(FixtureAdapter::new(Agent::ClaudeCode));
        adapter.add_session(Session {
            id: "gone".to_string(),
            agent: Agent::ClaudeCode,
            last_activity_at: "2026-10-01T09:00:00Z".to_string(),
            ..Session::default()
        });
        let adapters: Vec<Arc<dyn Adapter>> = vec![adapter.clone()];
        store.ingest(&adapters).expect("ingest");
        assert!(adapter.remove_session("gone"), "the session is gone");
        let report = store
            .refresh_with(&adapters, &IngestOptions::default())
            .expect("refresh without pruning");
        assert_eq!(report.totals.pruned, 0, "nothing pruned");
        assert_eq!(
            store
                .sessions()
                .map(|summary| summary.id)
                .collect::<Vec<_>>(),
            vec!["gone"],
            "the gone session stays without pruning"
        );
        let report = store.refresh(&adapters).expect("refresh with pruning");
        assert_eq!(report.pruned, 1, "the plain refresh still prunes");
        assert_eq!(store.sessions().count(), 0, "the gone session was pruned");
    }

    #[test]
    fn a_full_refresh_reopens_even_unchanged_sessions() {
        let store = temp_store("full-refresh");
        let adapter = Arc::new(one_session_adapter(
            Agent::ClaudeCode,
            "f1",
            "2026-10-01T09:00:00Z",
        ));
        let adapters: Vec<Arc<dyn Adapter>> = vec![adapter.clone()];
        store.ingest(&adapters).expect("ingest");
        assert_eq!(adapter.open_count(), 1, "the ingest opened the session");
        let report = store
            .refresh_with(&adapters, &IngestOptions::default())
            .expect("plain refresh");
        assert_eq!(report.totals.unchanged, 1, "the session is unchanged");
        assert_eq!(
            adapter.open_count(),
            1,
            "the plain refresh never re-opened it"
        );
        let report = store
            .refresh_with(
                &adapters,
                &IngestOptions {
                    force_full: true,
                    prune: true,
                    ..IngestOptions::default()
                },
            )
            .expect("full refresh");
        assert_eq!(
            report.totals.unchanged, 1,
            "identical content still counts unchanged"
        );
        assert_eq!(adapter.open_count(), 2, "the full refresh re-extracted it");
    }
}
