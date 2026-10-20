//! The `botspy` command line: a thin shell over the library.
//!
//! Every verb is one library call plus rendering — no business logic
//! lives here. The CLI owns flags, exit codes, and formatting only:
//!
//! - `0` success (including empty results and skipped records)
//! - `1` target not found (unknown session id or unknown source)
//! - `2` usage error (bad flags, misuse of `--root`)
//! - `3` source failure (unreadable root/store, snapshot error)
//!
//! The store builder wires the real adapters by source name, deriving
//! each adapter's root from `--home` (or `BOTSPY_HOME`, or `$HOME`) the
//! way each agent really lays its files out — which for Codex and Cursor
//! differs from the reference registry's `DEFAULT_SUBDIRS` (Codex keeps
//! its `state_5.sqlite` threads index at `~/.codex`, not in `sessions/`;
//! Cursor's adapter wants the KV store file, not the directory).

pub mod render;
pub mod scan;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Parser, Subcommand, ValueEnum};
use serde_json::Value;

use crate::adapter::Adapter;
use crate::adapters::antigravity::AntigravitySource;
use crate::adapters::claude_code::ClaudeCodeSource;
use crate::adapters::codex::CodexSource;
use crate::adapters::cursor::CursorSource;
use crate::adapters::grok_bot::GrokBotSource;
use crate::importer::SourceOptions;
use crate::importer::UnknownSource;
use crate::schema::{Agent, KnownPart, Message, Part, Role};
use crate::session::{Session, SessionStore, SessionSummary, UnknownSession};
use crate::snapshot::{count_rows, digest_files, snapshot_sqlite};
use crate::store::embed::EmbedError;
use crate::store::embed_minilm::MiniLMEmbedder;
use crate::store::ingest::IngestOptions;
use crate::store::ingest::IngestReport;
use crate::store::query::SessionFilter;
use crate::store::text::SearchHit;
use crate::store::{Store, StoreError};

/// Every source the registry knows, in registry order.
pub const ALL_SOURCES: [&str; 5] = ["claude_code", "cursor", "codex", "grok_bot", "antigravity"];

/// Parse and run one `botspy` invocation. The first argument is the
/// program name, as in `std::env::args_os`.
pub fn run_from<I, T>(argv: I) -> RunOutcome
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    run_parsed(Cli::try_parse_from(argv), None)
}

/// Like [`run_from`], but a scan loop streams each pass's report
/// through `scan_sink` as soon as the pass finishes, instead of holding
/// everything until exit. Other verbs ignore the sink.
pub fn run_from_with_sink<I, T>(argv: I, scan_sink: &mut dyn FnMut(&str)) -> RunOutcome
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    run_parsed(Cli::try_parse_from(argv), Some(scan_sink))
}

fn run_parsed(
    parsed: clap::error::Result<Cli>,
    scan_sink: Option<&mut dyn FnMut(&str)>,
) -> RunOutcome {
    match parsed {
        Ok(cli) => execute(cli, scan_sink),
        Err(err) => {
            let rendered = err.render().to_string();
            let (stdout, stderr) = if err.use_stderr() {
                (String::new(), rendered)
            } else {
                (rendered, String::new())
            };
            RunOutcome {
                code: err.exit_code(),
                stdout,
                stderr,
            }
        }
    }
}

/// The outcome of one CLI run: exit code plus what was written where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl RunOutcome {
    fn ok(stdout: String) -> Self {
        Self {
            code: 0,
            stdout,
            stderr: String::new(),
        }
    }

    fn fail(code: i32, stderr: String) -> Self {
        Self {
            code,
            stdout: String::new(),
            stderr,
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "botspy",
    version,
    about = "Pop open the conversation history of any coding agent.",
    after_help = "Read-only, always: botspy never writes to the sources it taps."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Inspect the source registry: roots, session counts, status.
    Sources(SourcesArgs),
    /// List sessions across every source, most recent activity first.
    Sessions(SessionsArgs),
    /// Open one session; walk its messages.
    Show(ShowArgs),
    /// Per-source diagnostics: counts, issues, and what was skipped.
    Doctor(DoctorArgs),
    /// Take a WAL-safe snapshot of a SQLite source and inspect it.
    Snapshot(SnapshotArgs),
    /// Mine the adapters into the local store.
    Scan(scan::ScanArgs),
    /// Mine every registered adapter into the local store (one-shot CDC).
    Import(ImportArgs),
    /// Search the store's messages: text, semantic, or hybrid.
    Search(SearchArgs),
    /// Bucket the store's sessions by source, project, or day.
    Stats(StatsArgs),
}

/// Flags shared by every verb.
#[derive(Debug, clap::Args)]
pub struct GlobalArgs {
    /// Restrict to these source names (repeatable).
    #[arg(short = 's', long = "source", value_name = "SOURCE")]
    pub source: Vec<String>,

    /// Override the root of the single selected source.
    #[arg(long, requires = "source", value_name = "PATH")]
    pub root: Option<PathBuf>,

    /// Override the home directory the default roots derive from.
    #[arg(long, value_name = "PATH")]
    pub home: Option<PathBuf>,

    /// Output mode: human (truncated), json, or ndjson (one record per
    /// line). Machine modes never truncate.
    #[arg(short = 'o', long = "output", value_enum, default_value = "human")]
    pub output: OutputMode,

    /// Show full ids and paths in human output.
    #[arg(long)]
    pub no_truncate: bool,
}

impl GlobalArgs {
    fn options(&self) -> SourceOptions {
        SourceOptions {
            root: self.root.clone(),
            home: self.home.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputMode {
    Human,
    Json,
    Ndjson,
}

#[derive(Debug, clap::Args)]
pub struct SourcesArgs {
    #[command(flatten)]
    pub global: GlobalArgs,
}

#[derive(Debug, clap::Args)]
pub struct SessionsArgs {
    #[command(flatten)]
    pub global: GlobalArgs,

    /// Filter by project id.
    #[arg(long, value_name = "ID")]
    pub project: Option<String>,

    /// Only sessions active at or after this timestamp.
    #[arg(long, value_name = "TS")]
    pub since: Option<String>,

    /// Only sessions active at or before this timestamp.
    #[arg(long, value_name = "TS")]
    pub until: Option<String>,

    /// Cap the number of listed sessions.
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,

    /// List from the local store at this path instead of the live
    /// sources.
    #[arg(long, value_name = "PATH")]
    pub db: Option<PathBuf>,
}

#[derive(Debug, clap::Args)]
pub struct ShowArgs {
    #[command(flatten)]
    pub global: GlobalArgs,

    /// Session id, or any unambiguous prefix of one.
    #[arg(value_name = "SESSION")]
    pub session: String,

    /// Show one message by its 1-based ordinal.
    #[arg(long, value_name = "N")]
    pub message: Option<usize>,
}

#[derive(Debug, clap::Args)]
pub struct DoctorArgs {
    #[command(flatten)]
    pub global: GlobalArgs,
}

#[derive(Debug, clap::Args)]
pub struct SnapshotArgs {
    #[command(flatten)]
    pub global: GlobalArgs,

    /// Source name, or a path to a SQLite database.
    #[arg(value_name = "SOURCE|PATH")]
    pub target: String,

    /// Directory the snapshot copy lands in.
    #[arg(long, value_name = "DIR")]
    pub out: Option<PathBuf>,
}

#[derive(Debug, clap::Args)]
pub struct ImportArgs {
    #[command(flatten)]
    pub global: GlobalArgs,

    /// Store path (default: the `.botspy/store.db` under the home).
    #[arg(long, value_name = "PATH")]
    pub db: Option<PathBuf>,

    /// Re-extract every reported session from byte zero, ignoring the
    /// staleness shortcut.
    #[arg(long)]
    pub full: bool,

    /// Report what would land and write nothing.
    #[arg(long)]
    pub dry_run: bool,

    /// Skip embeddings: the store stays text-only (text search is
    /// unaffected).
    #[arg(long)]
    pub no_embed: bool,
}

#[derive(Debug, clap::Args)]
pub struct SearchArgs {
    #[command(flatten)]
    pub global: GlobalArgs,

    /// The words to search for.
    #[arg(value_name = "QUERY")]
    pub query: String,

    /// Ranking mode: text (FTS5), semantic (embeddings), or hybrid
    /// (both fused).
    #[arg(long, value_enum, default_value = "text")]
    pub mode: SearchMode,

    /// Store path (default: the `.botspy/store.db` under the home).
    #[arg(long, value_name = "PATH")]
    pub db: Option<PathBuf>,

    /// Only sessions in this project.
    #[arg(long, value_name = "ID")]
    pub project: Option<String>,

    /// Cap the number of hits.
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SearchMode {
    Text,
    Semantic,
    Hybrid,
}

#[derive(Debug, clap::Args)]
pub struct StatsArgs {
    #[command(flatten)]
    pub global: GlobalArgs,

    /// Bucket sessions by source, project, or day of last activity.
    #[arg(long, value_enum, default_value = "source")]
    pub by: StatsBy,

    /// Store path (default: the `.botspy/store.db` under the home).
    #[arg(long, value_name = "PATH")]
    pub db: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum StatsBy {
    Source,
    Project,
    Day,
}

fn execute(cli: Cli, scan_sink: Option<&mut dyn FnMut(&str)>) -> RunOutcome {
    match cli.command {
        Command::Sources(args) => sources(args),
        Command::Sessions(args) => sessions(args),
        Command::Show(args) => show(args),
        Command::Doctor(args) => doctor(args),
        Command::Snapshot(args) => snapshot(args),
        Command::Scan(args) => scan::scan(args, scan_sink),
        Command::Import(args) => import(args),
        Command::Search(args) => search(args),
        Command::Stats(args) => stats(args),
    }
}

fn sources(args: SourcesArgs) -> RunOutcome {
    if let Some(out) = check_root(&args.global) {
        return out;
    }
    let names = match selected_sources(&args.global.source) {
        Ok(names) => names,
        Err(err) => return err.into_outcome(),
    };
    let options = args.global.options();
    let facts = match source_facts(&names, &options) {
        Ok(facts) => facts,
        Err(err) => return err.into_outcome(),
    };
    match args.global.output {
        OutputMode::Json => {
            let entries: Vec<serde_json::Value> = facts.iter().map(source_json).collect();
            RunOutcome::ok(format!(
                "{}\n",
                serde_json::to_string_pretty(&entries).expect("sources serialize")
            ))
        }
        OutputMode::Ndjson => {
            let lines: Vec<String> = facts
                .iter()
                .map(|fact| serde_json::to_string(&source_json(fact)).expect("sources serialize"))
                .collect();
            RunOutcome::ok(format!("{}\n", lines.join("\n")))
        }
        OutputMode::Human => RunOutcome::ok(sources_human(&facts, args.global.no_truncate)),
    }
}

/// The registry entry behind one source name: resolved root, session
/// count from discovery, and whether the location exists at all.
struct SourceFacts {
    name: String,
    agent: Agent,
    root: PathBuf,
    sessions: usize,
    status: &'static str,
}

/// The selected source names, or every known source when none given.
/// All names are validated before anything is resolved.
pub(crate) fn selected_sources(names: &[String]) -> Result<Vec<String>, UnknownSource> {
    if names.is_empty() {
        return Ok(ALL_SOURCES.iter().map(|name| name.to_string()).collect());
    }
    for name in names {
        if !ALL_SOURCES.contains(&name.as_str()) {
            return Err(UnknownSource { name: name.clone() });
        }
    }
    Ok(names.to_vec())
}

fn source_facts(
    names: &[String],
    options: &SourceOptions,
) -> Result<Vec<SourceFacts>, UnknownSource> {
    let mut facts = Vec::new();
    for name in names {
        let root = resolved_root(name, options);
        let sessions = real_source(name, options)?.discover().len();
        facts.push(SourceFacts {
            name: name.clone(),
            agent: agent_of(name).expect("validated source"),
            sessions,
            status: if root.exists() { "ok" } else { "missing" },
            root,
        });
    }
    Ok(facts)
}

fn source_json(fact: &SourceFacts) -> serde_json::Value {
    serde_json::json!({
        "name": fact.name,
        "agent": agent_name(fact.agent),
        "root": fact.root.display().to_string(),
        "sessions": fact.sessions,
        "status": fact.status,
    })
}

fn sources_human(facts: &[SourceFacts], no_truncate: bool) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{}  {}  {}  {}\n",
        render::column("SOURCE", 12),
        render::column("ROOT", render::TRUNCATE_WIDTH),
        render::number_str("SESSIONS", 8),
        "STATUS"
    ));
    for fact in facts {
        out.push_str(&format!(
            "{}  {}  {}  {}\n",
            render::column(&fact.name, 12),
            render::column(
                &render::truncate(&fact.root.display().to_string(), no_truncate),
                render::TRUNCATE_WIDTH
            ),
            render::number(fact.sessions, 8),
            fact.status
        ));
    }
    out
}

fn sessions(args: SessionsArgs) -> RunOutcome {
    if let Some(out) = check_root(&args.global) {
        return out;
    }
    if let Some(value) = &args.since {
        if !valid_rfc3339(value) {
            return RunOutcome::fail(
                2,
                format!(
                    "error: invalid value for --since: {value:?} is not an RFC 3339 timestamp\n"
                ),
            );
        }
    }
    if let Some(value) = &args.until {
        if !valid_rfc3339(value) {
            return RunOutcome::fail(
                2,
                format!(
                    "error: invalid value for --until: {value:?} is not an RFC 3339 timestamp\n"
                ),
            );
        }
    }
    let options = args.global.options();
    let summaries = match &args.db {
        Some(db) => match open_store(db, false) {
            Ok((store, _)) => store.query().sessions().collect(),
            Err(err) => return RunOutcome::fail(3, format!("error: {err}\n")),
        },
        None => {
            let store = match build_store(&args.global.source, &options) {
                Ok(store) => store,
                Err(err) => return err.into_outcome(),
            };
            store.list_sessions()
        }
    };
    let mut summaries = summaries;
    if !args.global.source.is_empty() {
        let agents: Vec<Agent> = args
            .global
            .source
            .iter()
            .filter_map(|name| agent_of(name))
            .collect();
        summaries.retain(|summary| agents.contains(&summary.agent));
    }
    if let Some(project) = &args.project {
        summaries.retain(|summary| &summary.project_id == project);
    }
    if let Some(since) = &args.since {
        summaries.retain(|summary| {
            !summary.last_activity_at.is_empty()
                && summary.last_activity_at.as_str() >= since.as_str()
        });
    }
    if let Some(until) = &args.until {
        summaries.retain(|summary| {
            summary.last_activity_at.is_empty()
                || summary.last_activity_at.as_str() <= until.as_str()
        });
    }
    if let Some(limit) = args.limit {
        summaries.truncate(limit);
    }
    match args.global.output {
        OutputMode::Json => RunOutcome::ok(format!(
            "{}\n",
            serde_json::to_string_pretty(&summaries).expect("summaries serialize")
        )),
        OutputMode::Ndjson => {
            let lines: Vec<String> = summaries
                .iter()
                .map(|summary| serde_json::to_string(summary).expect("summary serializes"))
                .collect();
            RunOutcome::ok(format!("{}\n", lines.join("\n")))
        }
        OutputMode::Human => RunOutcome::ok(sessions_human(&summaries, args.global.no_truncate)),
    }
}

fn sessions_human(summaries: &[SessionSummary], no_truncate: bool) -> String {
    // Rows are built as logical cells first, then padded to the widest
    // cell per column: in both modes every data row stays aligned with
    // the header, no matter how long ids, projects, or titles are.
    let mut cells: Vec<[String; 6]> = vec![[
        "SESSION".to_string(),
        "AGENT".to_string(),
        "PROJECT".to_string(),
        "MSGS".to_string(),
        "LAST ACTIVITY".to_string(),
        "TITLE".to_string(),
    ]];
    let mut truncated = false;
    for summary in summaries {
        let id = if no_truncate || summary.id.chars().count() <= 8 {
            summary.id.clone()
        } else {
            truncated = true;
            summary.id.chars().take(8).collect()
        };
        let activity = if summary.last_activity_at.is_empty() {
            render::DASH.to_string()
        } else {
            summary.last_activity_at.clone()
        };
        let title = summary
            .metadata
            .title
            .clone()
            .unwrap_or_else(|| render::DASH.to_string());
        cells.push([
            id,
            agent_name(summary.agent).to_string(),
            render::truncate_within(&summary.project_id, no_truncate, 20),
            summary.message_count.to_string(),
            activity,
            render::truncate(&title, no_truncate),
        ]);
    }
    let width = |column: usize| {
        render::column_width(
            &cells
                .iter()
                .map(|row| row[column].as_str())
                .collect::<Vec<_>>(),
        )
    };
    let (id_w, project_w, msgs_w, activity_w) = (width(0), width(2), width(3), width(4));
    let mut out = String::new();
    for cell in &cells {
        out.push_str(&format!(
            "{}  {}  {}  {}  {}  {}\n",
            render::column(&cell[0], id_w),
            render::column(&cell[1], 12),
            render::column(&cell[2], project_w),
            render::number_str(&cell[3], msgs_w),
            render::column(&cell[4], activity_w),
            cell[5]
        ));
    }
    let mut footer = render::count_line(summaries.len());
    if truncated {
        footer.push_str(" (use --no-truncate for full ids)");
    }
    out.push_str(&footer);
    out.push('\n');
    out
}

/// Whether a value is an RFC 3339 timestamp: `YYYY-MM-DDTHH:MM:SS`, an
/// optional fractional second, and a `Z` or `±HH:MM` offset. The
/// activity-window filters compare the library's recorded timestamps
/// lexicographically, so the argument must share that shape — an
/// off-shape value would silently match nothing.
pub(crate) fn valid_rfc3339(value: &str) -> bool {
    if !value.is_ascii() {
        return false;
    }
    let b = value.as_bytes();
    let digits = |start: usize, len: usize| b[start..start + len].iter().all(u8::is_ascii_digit);
    let two = |start: usize| (b[start] - b'0') as usize * 10 + (b[start + 1] - b'0') as usize;
    if b.len() < 20
        || !digits(0, 4)
        || b[4] != b'-'
        || !digits(5, 2)
        || b[7] != b'-'
        || !digits(8, 2)
        || !matches!(b[10], b'T' | b't')
        || !digits(11, 2)
        || b[13] != b':'
        || !digits(14, 2)
        || b[16] != b':'
        || !digits(17, 2)
    {
        return false;
    }
    let month = two(5);
    let day = two(8);
    let hour = two(11);
    let minute = two(14);
    let second = two(17);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return false;
    }
    let mut rest = &b[19..];
    if rest.first() == Some(&b'.') {
        let fraction = rest[1..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if fraction == 0 {
            return false;
        }
        rest = &rest[1 + fraction..];
    }
    match rest {
        [b'Z'] | [b'z'] => true,
        [sign, first, second, b':', third, fourth] => {
            (*sign == b'+' || *sign == b'-')
                && [*first, *second, *third, *fourth]
                    .iter()
                    .all(|byte| byte.is_ascii_digit())
        }
        _ => false,
    }
}

fn show(args: ShowArgs) -> RunOutcome {
    if let Some(out) = check_root(&args.global) {
        return out;
    }
    let options = args.global.options();
    let store = match build_store(&args.global.source, &options) {
        Ok(store) => store,
        Err(err) => return err.into_outcome(),
    };
    let id = match resolve_session_id(&store, &args.session) {
        Ok(id) => id,
        Err(outcome) => return outcome,
    };
    let session = match store.open(&id) {
        Ok(session) => session,
        Err(err) => return err.into_outcome(),
    };
    let selected: Option<usize> = match args.message {
        Some(ordinal) => match session.messages.get(ordinal - 1) {
            Some(_) => Some(ordinal),
            None => {
                return RunOutcome::fail(
                    1,
                    format!(
                        "error: message {ordinal} not found in session {} ({} messages)\n",
                        id,
                        session.messages.len()
                    ),
                )
            }
        },
        None => None,
    };
    match args.global.output {
        OutputMode::Json => RunOutcome::ok(format!(
            "{}\n",
            serde_json::to_string_pretty(&session).expect("session serializes")
        )),
        OutputMode::Ndjson => {
            let messages: Vec<&Message> = match selected {
                Some(ordinal) => vec![&session.messages[ordinal - 1]],
                None => session.messages.iter().collect(),
            };
            let lines: Vec<String> = messages
                .iter()
                .map(|message| serde_json::to_string(message).expect("message serializes"))
                .collect();
            RunOutcome::ok(format!("{}\n", lines.join("\n")))
        }
        OutputMode::Human => {
            RunOutcome::ok(show_human(&session, selected, args.global.no_truncate))
        }
    }
}

/// Resolve a session id or unambiguous prefix against the unified
/// listing. An exact id wins; an ambiguous prefix is an error listing
/// the candidates, never a guess.
fn resolve_session_id(store: &SessionStore, input: &str) -> Result<String, RunOutcome> {
    let ids: Vec<String> = store
        .list_sessions()
        .into_iter()
        .map(|summary| summary.id)
        .collect();
    if ids.iter().any(|id| id == input) {
        return Ok(input.to_string());
    }
    let candidates: Vec<&String> = ids.iter().filter(|id| id.starts_with(input)).collect();
    match candidates.len() {
        1 => Ok(candidates[0].clone()),
        0 => Err(UnknownSession {
            id: input.to_string(),
        }
        .into_outcome()),
        _ => Err(RunOutcome::fail(
            1,
            format!(
                "error: ambiguous session id {:?}, candidates: {}\n",
                input,
                candidates
                    .iter()
                    .map(|id| id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

fn role_name(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
        Role::Tool => "tool",
    }
}

/// One line per part: its kind and a short human summary.
fn part_line(part: &Part, no_truncate: bool) -> String {
    let kind = part.kind_name().unwrap_or("part").to_string();
    let summary: Option<String> = match part {
        Part::Known(KnownPart::Text { text, .. }) => Some(text.clone()),
        Part::Known(KnownPart::Thinking { text, .. }) => {
            Some(text.clone().unwrap_or_else(|| "(none)".to_string()))
        }
        Part::Known(KnownPart::ToolCall { name, .. }) => Some(name.clone()),
        Part::Known(KnownPart::ToolResult { text, .. }) => {
            Some(text.clone().unwrap_or_else(|| "(none)".to_string()))
        }
        Part::Known(KnownPart::System { text, .. }) => Some(text.clone()),
        _ => None,
    };
    let summary = summary
        .as_deref()
        .and_then(|text| text.lines().next())
        .unwrap_or_default();
    if summary.is_empty() {
        format!("· {kind}")
    } else {
        format!("· {kind}  {}", render::truncate(summary, no_truncate))
    }
}

fn show_human(session: &Session, selected: Option<usize>, no_truncate: bool) -> String {
    let id = if no_truncate || session.id.chars().count() <= 8 {
        session.id.clone()
    } else {
        session.id.chars().take(8).collect()
    };
    let mut out = format!(
        "Session {} ({}, {}) — {} messages, started {}\n",
        id,
        agent_name(session.agent),
        session.project_id,
        session.messages.len(),
        if session.started_at.is_empty() {
            render::DASH
        } else {
            &session.started_at
        }
    );
    let messages: Vec<(usize, &Message)> = match selected {
        Some(ordinal) => session
            .messages
            .get(ordinal - 1)
            .map(|message| vec![(ordinal, message)])
            .unwrap_or_default(),
        None => session
            .messages
            .iter()
            .enumerate()
            .map(|(index, message)| (index + 1, message))
            .collect(),
    };
    for (ordinal, message) in messages {
        let timestamp = message
            .timestamp
            .as_deref()
            .filter(|timestamp| !timestamp.is_empty())
            .unwrap_or(render::DASH);
        out.push_str(&format!(
            "#{} {} {}\n",
            ordinal,
            role_name(message.role),
            timestamp
        ));
        for part in &message.parts {
            out.push_str(&format!("  {}\n", part_line(part, no_truncate)));
        }
    }
    out
}

fn doctor(args: DoctorArgs) -> RunOutcome {
    if let Some(out) = check_root(&args.global) {
        return out;
    }
    let names = match selected_sources(&args.global.source) {
        Ok(names) => names,
        Err(err) => return err.into_outcome(),
    };
    let options = args.global.options();
    let facts = doctor_facts(&names, &options);
    match args.global.output {
        OutputMode::Json => {
            let entries: Vec<serde_json::Value> =
                facts.iter().map(|fact| fact.report.clone()).collect();
            RunOutcome::ok(format!(
                "{}\n",
                serde_json::to_string_pretty(&entries).expect("doctor reports serialize")
            ))
        }
        OutputMode::Ndjson => {
            let lines: Vec<String> = facts
                .iter()
                .map(|fact| serde_json::to_string(&fact.report).expect("doctor report serializes"))
                .collect();
            RunOutcome::ok(format!("{}\n", lines.join("\n")))
        }
        OutputMode::Human => RunOutcome::ok(doctor_human(&facts)),
    }
}

/// One source's doctor facts: its report as JSON (with the source name
/// inserted), plus the pieces the human renderer needs. Doctor reports;
/// it does not gate, so issues never change the exit code.
struct DoctorFacts {
    name: String,
    status: &'static str,
    counts: String,
    issues: Vec<String>,
    report: serde_json::Value,
}

fn doctor_facts(names: &[String], options: &SourceOptions) -> Vec<DoctorFacts> {
    names
        .iter()
        .map(|name| {
            let root = resolved_root(name, options);
            let (counts, report) = match name.as_str() {
                "claude_code" => {
                    let report = ClaudeCodeSource::new(root).doctor();
                    let counts = format!(
                        "{} project dirs, {} transcripts",
                        report.project_dirs, report.transcripts
                    );
                    (
                        counts,
                        serde_json::to_value(&report).expect("report serializes"),
                    )
                }
                "cursor" => {
                    let report = CursorSource::new(root).doctor();
                    let counts =
                        format!("{} composers, {} bubbles", report.composers, report.bubbles);
                    (
                        counts,
                        serde_json::to_value(&report).expect("report serializes"),
                    )
                }
                "codex" => {
                    let report = CodexSource::new(root).doctor();
                    let counts = format!("{} threads", report.threads);
                    (
                        counts,
                        serde_json::to_value(&report).expect("report serializes"),
                    )
                }
                "grok_bot" => {
                    let report = GrokBotSource::new(root).doctor();
                    let counts = format!(
                        "{} entry logs, {} cloud agents",
                        report.entry_logs, report.cloud_agents
                    );
                    (
                        counts,
                        serde_json::to_value(&report).expect("report serializes"),
                    )
                }
                _ => {
                    let report = AntigravitySource::new(root).doctor();
                    let counts = format!(
                        "{} conversations, {} transcripts",
                        report.conversations, report.transcripts
                    );
                    (
                        counts,
                        serde_json::to_value(&report).expect("report serializes"),
                    )
                }
            };
            let mut report = report;
            report["source"] = serde_json::Value::String(name.clone());
            let issues = report["issues"]
                .as_array()
                .expect("issues list")
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            let status = if report["issues"]
                .as_array()
                .is_some_and(|issues| issues.is_empty())
            {
                "ok"
            } else {
                "warn"
            };
            DoctorFacts {
                name: name.clone(),
                status,
                counts,
                issues,
                report,
            }
        })
        .collect()
}

fn doctor_human(facts: &[DoctorFacts]) -> String {
    let mut out = String::new();
    for fact in facts {
        out.push_str(&format!(
            "{}  {}  {}, {} {}\n",
            render::column(&fact.name, 12),
            render::column(fact.status, 4),
            fact.counts,
            fact.issues.len(),
            if fact.issues.len() == 1 {
                "issue"
            } else {
                "issues"
            }
        ));
        for issue in &fact.issues {
            out.push_str(&format!("  {issue}\n"));
        }
    }
    out
}

fn snapshot(args: SnapshotArgs) -> RunOutcome {
    if let Some(out) = check_root(&args.global) {
        return out;
    }
    let options = args.global.options();
    let (name, db) = match snapshot_target(&args.target, &options) {
        Ok(resolved) => resolved,
        Err(outcome) => return outcome,
    };
    let out_dir = match &args.out {
        Some(dir) => dir.clone(),
        None => home_of(&options).join(".botspy/snapshots").join(&name),
    };
    let wal = sidecar(&db, "-wal");
    let shm = sidecar(&db, "-shm");
    // Digest the source files as they existed before the read. A
    // read-only open of a WAL-mode database creates empty -wal/-shm
    // sidecars; that is SQLite's read machinery, not a mutation, so
    // sidecars created by the read are not held against it.
    let watched: Vec<PathBuf> = [Some(db.clone()), Some(wal), Some(shm)]
        .into_iter()
        .flatten()
        .filter(|path| path.is_file())
        .collect();
    let refs: Vec<&Path> = watched.iter().map(|path| path.as_path()).collect();
    let before = digest_files(&refs);
    let snapshot_path = match snapshot_sqlite(&db, &out_dir) {
        Ok(path) => path,
        Err(err) => {
            return RunOutcome::fail(
                3,
                format!("error: snapshot of {} failed: {err}\n", db.display()),
            )
        }
    };
    let after = digest_files(&refs);
    let unmutated = before == after;
    let rows = match count_rows(&snapshot_path) {
        Ok(rows) => rows,
        Err(err) => {
            return RunOutcome::fail(
                3,
                format!(
                    "error: counting rows in {} failed: {err}\n",
                    snapshot_path.display()
                ),
            )
        }
    };
    let snapshot = snapshot_path.display().to_string();
    match args.global.output {
        OutputMode::Json => RunOutcome::ok(format!(
            "{}\n",
            serde_json::json!({
                "snapshot": snapshot,
                "rows": rows,
                "unmutated": unmutated,
            })
        )),
        OutputMode::Ndjson => RunOutcome::ok(format!(
            "{}\n",
            serde_json::json!({
                "snapshot": snapshot,
                "rows": rows,
                "unmutated": unmutated,
            })
        )),
        OutputMode::Human => {
            let mut out = format!("snapshot: {snapshot}\nrows: {rows}\n");
            if unmutated {
                out.push_str("source unmutated (digest verified before/after)\n");
            } else {
                out.push_str("source mutated during the read (digest changed)\n");
            }
            RunOutcome::ok(out)
        }
    }
}

/// One count with correct pluralization, for report lines ("1 hit",
/// "2 hits").
fn plural(count: usize, noun: &str) -> String {
    match count {
        1 => format!("1 {noun}"),
        n => format!("{n} {noun}s"),
    }
}

/// Open the store at `path`, attaching the shipped MiniLM embedder when
/// `embed` is wanted and the model cache is populated. A missing cache
/// degrades to a text-only store — reported by the caller, never a
/// failed verb and never a silent download (the runtime never touches
/// the network).
fn open_store(path: &Path, embed: bool) -> Result<(Store, bool), StoreError> {
    if !embed {
        return Ok((Store::open(path)?, false));
    }
    match MiniLMEmbedder::open() {
        Ok(embedder) => Ok((Store::open_with_embedder(path, Arc::new(embedder))?, true)),
        Err(EmbedError::ModelMissing { .. }) => Ok((Store::open(path)?, false)),
        Err(source) => Err(StoreError::Embed { source }),
    }
}

/// The default store path under the resolved home.
fn store_path(options: &SourceOptions) -> PathBuf {
    home_of(options).join(".botspy").join("store.db")
}

fn import(args: ImportArgs) -> RunOutcome {
    if let Some(out) = check_root(&args.global) {
        return out;
    }
    let names = match selected_sources(&args.global.source) {
        Ok(names) => names,
        Err(err) => return err.into_outcome(),
    };
    let options = args.global.options();
    let db = args.db.clone().unwrap_or_else(|| store_path(&options));
    let mut adapters: Vec<Arc<dyn Adapter>> = Vec::new();
    for name in &names {
        match real_source(name, &options) {
            Ok(source) => adapters.push(Arc::from(source)),
            Err(err) => return err.into_outcome(),
        }
    }
    if args.dry_run {
        return import_dry_run(&db, &adapters);
    }
    let (store, embedded) = match open_store(&db, !args.no_embed) {
        Ok(opened) => opened,
        Err(err) => return RunOutcome::fail(3, format!("error: {err}\n")),
    };
    let pass_options = IngestOptions {
        prune: true,
        force_full: args.full,
        ..IngestOptions::default()
    };
    match store.refresh_with(&adapters, &pass_options) {
        Ok(detailed) => {
            let report = detailed.totals;
            let mut out = format!(
                "{}, {}, {}, {}\nstore: {}\n",
                plural(report.new, "new session"),
                plural(report.updated, "updated session"),
                plural(report.unchanged, "unchanged session"),
                plural(report.pruned, "pruned session"),
                db.display(),
            );
            if !args.no_embed && !embedded {
                out.push_str(
                    "embeddings: skipped (model assets missing — run `cargo run --example setup_models`)\n",
                );
            }
            RunOutcome::ok(out)
        }
        Err(err) => RunOutcome::fail(3, format!("error: {err}\n")),
    }
}

/// What one import pass would land, computed the way the real pass
/// judges sessions — from the adapters' own discovery, opening the
/// sessions that would be re-extracted — with no writes, and a missing
/// store file treated as an empty store rather than created.
fn import_dry_run(db: &Path, adapters: &[Arc<dyn Adapter>]) -> RunOutcome {
    let stored: Vec<SessionSummary> = if db.exists() {
        match Store::open(db) {
            Ok(store) => store.query().sessions().collect(),
            Err(err) => return RunOutcome::fail(3, format!("error: {err}\n")),
        }
    } else {
        Vec::new()
    };
    let mut report = IngestReport::default();
    let mut reported: std::collections::BTreeSet<String> = Default::default();
    let selected_agents: std::collections::BTreeSet<Agent> =
        adapters.iter().map(|adapter| adapter.agent()).collect();
    for adapter in adapters {
        for summary in adapter.discover() {
            if !reported.insert(summary.id.clone()) {
                continue;
            }
            let existing = stored.iter().find(|stored| stored.id == summary.id);
            let stale = match existing {
                None => true,
                Some(existing) => {
                    existing.last_activity_at != summary.last_activity_at
                        || existing.message_count != summary.message_count
                }
            };
            if !stale {
                report.unchanged += 1;
                continue;
            }
            // The real pass opens every stale session; a session that
            // cannot be opened lands nowhere, so the preview counts it
            // the same way.
            let Some(session) = adapter.open(&summary.id) else {
                continue;
            };
            let _ = session;
            match existing {
                None => report.new += 1,
                Some(_) => report.updated += 1,
            }
        }
    }
    report.pruned = stored
        .iter()
        .filter(|stored| selected_agents.contains(&stored.agent) && !reported.contains(&stored.id))
        .count();
    RunOutcome::ok(format!(
        "{}, {}, {}, {} (dry run — nothing written)\n",
        plural(report.new, "new session"),
        plural(report.updated, "updated session"),
        plural(report.unchanged, "unchanged session"),
        plural(report.pruned, "pruned session"),
    ))
}

fn search(args: SearchArgs) -> RunOutcome {
    if let Some(out) = check_root(&args.global) {
        return out;
    }
    if args.global.source.len() > 1 {
        return RunOutcome::fail(
            2,
            "error: search filters by at most one --source\n".to_string(),
        );
    }
    for name in &args.global.source {
        if agent_of(name).is_none() {
            return UnknownSource { name: name.clone() }.into_outcome();
        }
    }
    let options = args.global.options();
    let db = args.db.clone().unwrap_or_else(|| store_path(&options));
    let store = match open_store(
        &db,
        matches!(args.mode, SearchMode::Semantic | SearchMode::Hybrid),
    ) {
        Ok((store, _)) => store,
        Err(err) => return RunOutcome::fail(3, format!("error: {err}\n")),
    };
    let filter = SessionFilter {
        agent: args.global.source.first().and_then(|name| agent_of(name)),
        project: args.project.clone(),
        ..Default::default()
    };
    let query = store.query();
    let mut hits = match args.mode {
        SearchMode::Text => query.search_with(&args.query, &filter),
        SearchMode::Semantic => {
            query.search_semantic_with(&args.query, args.limit.unwrap_or(10), &filter)
        }
        SearchMode::Hybrid => query.search_hybrid_with(&args.query, &filter),
    };
    if let Some(limit) = args.limit {
        hits.truncate(limit);
    }
    match args.global.output {
        OutputMode::Json => RunOutcome::ok(format!(
            "{}\n",
            serde_json::to_string_pretty(&search_json(&hits)).expect("hits serialize")
        )),
        OutputMode::Ndjson => {
            let lines: Vec<String> = search_json(&hits)
                .iter()
                .map(|hit| serde_json::to_string(hit).expect("hit serializes"))
                .collect();
            RunOutcome::ok(format!("{}\n", lines.join("\n")))
        }
        OutputMode::Human => RunOutcome::ok(search_human(&hits, args.global.no_truncate)),
    }
}

fn search_json(hits: &[SearchHit]) -> Vec<serde_json::Value> {
    hits.iter()
        .map(|hit| {
            serde_json::json!({
                "session_id": hit.session_id,
                "message_ordinal": hit.message_ordinal,
                "part_ordinal": hit.part_ordinal,
                "text": hit.text,
                "score": hit.score,
            })
        })
        .collect()
}

fn search_human(hits: &[SearchHit], no_truncate: bool) -> String {
    let mut cells: Vec<[String; 4]> = vec![[
        "SESSION".to_string(),
        "MSG".to_string(),
        "SCORE".to_string(),
        "TEXT".to_string(),
    ]];
    for hit in hits {
        let id = if no_truncate || hit.session_id.chars().count() <= 8 {
            hit.session_id.clone()
        } else {
            hit.session_id.chars().take(8).collect()
        };
        let text = hit.text.lines().next().unwrap_or_default();
        cells.push([
            id,
            hit.message_ordinal.to_string(),
            format!("{:.3}", hit.score),
            render::truncate(text, no_truncate),
        ]);
    }
    let width = |column: usize| {
        render::column_width(
            &cells
                .iter()
                .map(|row| row[column].as_str())
                .collect::<Vec<_>>(),
        )
    };
    let (id_w, msg_w, score_w) = (width(0), width(1), width(2));
    let mut out = String::new();
    for cell in &cells {
        out.push_str(&format!(
            "{}  {}  {}  {}\n",
            render::column(&cell[0], id_w),
            render::number_str(&cell[1], msg_w),
            render::number_str(&cell[2], score_w),
            cell[3]
        ));
    }
    let sessions: std::collections::BTreeSet<&str> =
        hits.iter().map(|hit| hit.session_id.as_str()).collect();
    out.push_str(&format!(
        "{} in {}\n",
        plural(hits.len(), "hit"),
        plural(sessions.len(), "session")
    ));
    out
}

fn stats(args: StatsArgs) -> RunOutcome {
    if let Some(out) = check_root(&args.global) {
        return out;
    }
    let options = args.global.options();
    let db = args.db.clone().unwrap_or_else(|| store_path(&options));
    let store = match open_store(&db, false) {
        Ok((store, _)) => store,
        Err(err) => return RunOutcome::fail(3, format!("error: {err}\n")),
    };
    let mut buckets: std::collections::BTreeMap<String, usize> = Default::default();
    for summary in store.query().sessions() {
        let key = match args.by {
            StatsBy::Source => agent_name(summary.agent).to_string(),
            StatsBy::Project => {
                if summary.project_id.is_empty() {
                    "no project".to_string()
                } else {
                    summary.project_id.clone()
                }
            }
            StatsBy::Day => {
                if summary.last_activity_at.is_empty() {
                    "no activity".to_string()
                } else {
                    summary.last_activity_at.chars().take(10).collect()
                }
            }
        };
        *buckets.entry(key).or_default() += 1;
    }
    // Biggest bucket first; ties in key order so the output is stable.
    let mut rows: Vec<(String, usize)> = buckets.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let total: usize = rows.iter().map(|(_, count)| count).sum();
    match args.global.output {
        OutputMode::Json => RunOutcome::ok(format!(
            "{}\n",
            serde_json::to_string_pretty(&stats_json(&rows)).expect("stats serialize")
        )),
        OutputMode::Ndjson => {
            let lines: Vec<String> = stats_json(&rows)
                .iter()
                .map(|row| serde_json::to_string(row).expect("stat row serializes"))
                .collect();
            RunOutcome::ok(format!("{}\n", lines.join("\n")))
        }
        OutputMode::Human => RunOutcome::ok(stats_human(&rows, total, args.by)),
    }
}

fn stats_json(rows: &[(String, usize)]) -> Vec<serde_json::Value> {
    rows.iter()
        .map(|(key, count)| {
            serde_json::json!({
                "key": key,
                "sessions": count,
            })
        })
        .collect()
}

fn stats_human(rows: &[(String, usize)], total: usize, by: StatsBy) -> String {
    let label = match by {
        StatsBy::Source => "SOURCE",
        StatsBy::Project => "PROJECT",
        StatsBy::Day => "DAY",
    };
    let mut cells: Vec<[String; 2]> = vec![[label.to_string(), "SESSIONS".to_string()]];
    for (key, count) in rows {
        cells.push([key.clone(), count.to_string()]);
    }
    let key_w = render::column_width(&cells.iter().map(|row| row[0].as_str()).collect::<Vec<_>>());
    let count_w =
        render::column_width(&cells.iter().map(|row| row[1].as_str()).collect::<Vec<_>>());
    let mut out = String::new();
    for cell in &cells {
        out.push_str(&format!(
            "{}  {}\n",
            render::column(&cell[0], key_w),
            render::number_str(&cell[1], count_w)
        ));
    }
    out.push_str(&format!("total: {}\n", plural(total, "session")));
    out
}

/// Resolve the snapshot target: a source name resolved through the
/// per-agent roots, or a path to a SQLite file. A known source whose
/// location is not a SQLite file is a clean failure (3), not a guess.
fn snapshot_target(target: &str, options: &SourceOptions) -> Result<(String, PathBuf), RunOutcome> {
    if ALL_SOURCES.contains(&target) {
        let root = resolved_root(target, options);
        if root.is_file() {
            return Ok((target.to_string(), root));
        }
        return Err(RunOutcome::fail(
            3,
            format!(
                "error: no SQLite store for {target} at {} ({target} keeps its history outside SQLite)\n",
                root.display()
            ),
        ));
    }
    let path = PathBuf::from(target);
    if path.is_file() {
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("snapshot")
            .to_string();
        return Ok((stem, path));
    }
    let path_like = target.contains('/') || target.contains('\\');
    if path_like {
        return Err(RunOutcome::fail(
            3,
            format!("error: snapshot source {target} is not a readable file\n"),
        ));
    }
    Err(UnknownSource {
        name: target.to_string(),
    }
    .into_outcome())
}

/// The WAL/SHM sidecar path next to a database file, as SQLite names it.
fn sidecar(db: &Path, suffix: &str) -> PathBuf {
    let mut name = db.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// Build a store of the real adapters for `sources` (every source when
/// empty), honoring `options`. All names are validated before anything
/// is resolved, so an unknown name fails without touching the
/// filesystem.
pub fn build_store(
    sources: &[String],
    options: &SourceOptions,
) -> Result<SessionStore, UnknownSource> {
    let names: Vec<&str> = if sources.is_empty() {
        ALL_SOURCES.to_vec()
    } else {
        sources.iter().map(String::as_str).collect()
    };
    for name in &names {
        if !ALL_SOURCES.contains(name) {
            return Err(UnknownSource {
                name: (*name).to_string(),
            });
        }
    }
    let mut store = SessionStore::new();
    for name in names {
        store.register(Arc::from(real_source(name, options)?));
    }
    Ok(store)
}

/// The adapter behind a source name, rooted at its resolved location.
pub(crate) fn real_source(
    name: &str,
    options: &SourceOptions,
) -> Result<Box<dyn Adapter>, UnknownSource> {
    if !ALL_SOURCES.contains(&name) {
        return Err(UnknownSource {
            name: name.to_string(),
        });
    }
    let root = resolved_root(name, options);
    Ok(match name {
        "claude_code" => Box::new(ClaudeCodeSource::new(root)),
        "cursor" => Box::new(CursorSource::new(root)),
        "codex" => Box::new(CodexSource::new(root)),
        "grok_bot" => Box::new(GrokBotSource::new(root)),
        _ => Box::new(AntigravitySource::new(root)),
    })
}

/// The root a source resolves to: the `--root` override, or the
/// location derived from the home directory.
pub fn resolved_root(name: &str, options: &SourceOptions) -> PathBuf {
    if let Some(root) = &options.root {
        return root.clone();
    }
    let home = home_of(options);
    match name {
        "claude_code" => home.join(".claude/projects"),
        "cursor" => home.join(".cursor/state.vscdb"),
        "codex" => home.join(".codex"),
        "grok_bot" => home.join(".grok/sand-client-persistence"),
        _ => home.join(".gemini/antigravity"),
    }
}

/// The home the default roots derive from: `--home`, then `BOTSPY_HOME`,
/// then `$HOME`.
pub(crate) fn home_of(options: &SourceOptions) -> PathBuf {
    options
        .home
        .clone()
        .or_else(|| std::env::var_os("BOTSPY_HOME").map(PathBuf::from))
        .unwrap_or_else(default_home)
}

fn default_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// The registry name of an agent.
pub fn agent_name(agent: Agent) -> &'static str {
    match agent {
        Agent::ClaudeCode => "claude_code",
        Agent::Cursor => "cursor",
        Agent::Codex => "codex",
        Agent::GrokBot => "grok_bot",
        Agent::Antigravity => "antigravity",
    }
}

/// The agent behind a registry name.
pub(crate) fn agent_of(name: &str) -> Option<Agent> {
    match name {
        "claude_code" => Some(Agent::ClaudeCode),
        "cursor" => Some(Agent::Cursor),
        "codex" => Some(Agent::Codex),
        "grok_bot" => Some(Agent::GrokBot),
        "antigravity" => Some(Agent::Antigravity),
        _ => None,
    }
}

/// `--root` pairs with exactly one source; anything else is a usage
/// error.
pub(crate) fn check_root(args: &GlobalArgs) -> Option<RunOutcome> {
    if args.root.is_some() && args.source.len() != 1 {
        return Some(RunOutcome::fail(
            2,
            "error: --root needs exactly one --source\n".to_string(),
        ));
    }
    None
}

/// Map a library error onto the CLI's exit codes.
trait IntoOutcome {
    fn into_outcome(self) -> RunOutcome;
}

impl IntoOutcome for UnknownSource {
    fn into_outcome(self) -> RunOutcome {
        RunOutcome::fail(1, format!("error: {self}\n"))
    }
}

impl IntoOutcome for UnknownSession {
    fn into_outcome(self) -> RunOutcome {
        RunOutcome::fail(1, format!("error: {self}\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::valid_rfc3339;

    #[test]
    fn accepts_recorded_timestamp_shapes() {
        for value in [
            "2026-10-01T09:00:00Z",
            "2026-10-01T09:00:00.032Z",
            "2026-10-01t09:00:00z",
            "2026-10-01T09:00:00-04:00",
            "2026-10-01T09:00:60Z",
        ] {
            assert!(valid_rfc3339(value), "must accept {value}");
        }
    }

    #[test]
    fn rejects_off_shape_values() {
        for value in [
            "bogus",
            "",
            "2026-10-01",
            "2026-13-01T09:00:00Z",
            "2026-10-32T09:00:00Z",
            "2026-10-01T24:00:00Z",
            "2026-10-01T09:00Z",
            "2026-10-01T09:00:00",
            "2026-10-01T09:00:00+04",
            "2026-10-01T09:00:00.βZ",
        ] {
            assert!(!valid_rfc3339(value), "must reject {value}");
        }
    }
}
