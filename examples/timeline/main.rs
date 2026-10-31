//! A timeline of parallel agent sessions: an example consumer of the BotSpy
//! public API that draws how many sessions ran at once, where they
//! overlapped, and how sessions fanned out into sub-agents. The output is
//! one self-contained local HTML file (inline SVG and CSS, no scripts and
//! nothing loaded from elsewhere).
//!
//! ```text
//! cargo run --example timeline -- [--since T] [--until T] [--project P]
//!     [--source S]... [--titles] [--out PATH] [--demo] [path-to-agent-data-root]
//! ```
//!
//! # What the page shows
//!
//! - **Top:** the concurrency step-curve (sessions active at once over
//!   time) and its peak.
//! - **Below:** one swimlane per session, grouped by project and ordered by
//!   start. Bars are active stretches; dotted lines are idle breaks.
//!   Sub-agents and forks nest under the session that spawned them, joined
//!   by a connector at the moment of the spawn. A child whose parent is not
//!   in the selection sits at top level with a note.
//! - Agents are told apart by a fill pattern and a legend label as well as
//!   colour. The page follows `prefers-color-scheme`, and hovering a bar
//!   shows times and message counts.
//!
//! # Idle detection
//!
//! There is no fixed cutoff. A gap between consecutive messages is idle
//! when it is an outlier among that session's own gaps: above Tukey's upper
//! fence, Q3 + 1.5 IQR, taken on ln(gap) because gaps are heavy-tailed
//! (seconds between tool calls, minutes of thinking, hours away). Records
//! that share a timestamp are not pauses: zero gaps never break a stretch
//! and stay out of the estimate. A session with fewer than 4 non-zero gaps
//! is one stretch, and a session with no message
//! timestamps is one started..last-activity span marked "approximate".
//!
//! # Privacy
//!
//! The page holds session timing, agent and project names, and short
//! session ids. Session titles appear only with `--titles`. Message text is
//! never read into the page.
//!
//! `--demo` draws a generated, synthetic parallel-sessions corpus instead
//! of reading a data root. `--out` defaults to `timeline.html`. Exit
//! status: 0 on success, 2 on any error.
//!
//! Like every example, this imports only `botspy` and the standard library.

pub mod activity;
#[path = "../common/mod.rs"]
pub mod common;
pub mod demo;
pub mod render;

use self::activity::{parse_time, Ms};
use botspy::{Agent, Session};
use common::{collect_sessions, default_root};
use std::io::Write;
use std::path::PathBuf;

const USAGE: &str = "usage: timeline [--since T] [--until T] [--project P] [--source S]... [--titles] [--out PATH] [--demo] [path-to-agent-data-root]";

/// What the command line asked for.
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub since: Option<Ms>,
    pub until: Option<Ms>,
    pub project: Option<String>,
    pub sources: Vec<Agent>,
    pub titles: bool,
    pub out: PathBuf,
    pub demo: bool,
    pub root: Option<PathBuf>,
}

fn parse_source(name: &str) -> Option<Agent> {
    [
        Agent::ClaudeCode,
        Agent::Cursor,
        Agent::Codex,
        Agent::GrokBot,
        Agent::Antigravity,
    ]
    .into_iter()
    .find(|agent| render::agent_name(*agent) == name.replace('-', "_"))
}

pub fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut options = Options {
        out: PathBuf::from("timeline.html"),
        ..Options::default()
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = |flag: &str| {
            args.next()
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match arg.as_str() {
            "--since" | "--until" => {
                let text = value(arg)?;
                let time =
                    parse_time(&text).ok_or_else(|| format!("{arg}: cannot read time {text:?}"))?;
                if arg == "--since" {
                    options.since = Some(time);
                } else {
                    options.until = Some(time);
                }
            }
            "--project" => options.project = Some(value(arg)?),
            "--source" => {
                let name = value(arg)?;
                options.sources.push(parse_source(&name).ok_or_else(|| {
                    format!("--source: unknown agent {name:?} (claude_code, cursor, codex, grok_bot, antigravity)")
                })?);
            }
            "--out" => options.out = PathBuf::from(value(arg)?),
            "--titles" => options.titles = true,
            "--demo" => options.demo = true,
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            path if options.root.is_none() => options.root = Some(PathBuf::from(path)),
            _ => return Err("too many arguments".to_string()),
        }
    }
    if options.demo && options.root.is_some() {
        return Err("--demo generates its own corpus; pass no data root".to_string());
    }
    Ok(options)
}

/// Render the page for `args`. With `given` sessions they are used as they
/// are; otherwise the demo corpus or the data root is read.
pub fn page_for(args: &[String], given: Option<Vec<Session>>) -> Result<String, String> {
    let options = parse_args(args)?;
    let sessions = match given {
        Some(sessions) => sessions,
        None if options.demo => demo::sessions(),
        None => collect_sessions(options.root.as_deref().unwrap_or(&default_root())),
    };
    Ok(render::render(&sessions, &options))
}

/// Run the example: write the page and print its path. Returns the process
/// exit status: 0 on success, 2 on any error.
pub fn run(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let result = parse_args(args).and_then(|options| {
        let page = page_for(args, None)?;
        std::fs::write(&options.out, page)
            .map_err(|error| format!("cannot write {}: {}", options.out.display(), error.kind()))?;
        Ok(options.out)
    });
    match result {
        Ok(path) => {
            let _ = writeln!(out, "{}", path.display());
            0
        }
        Err(message) => {
            let _ = writeln!(err, "timeline: {message}\n{USAGE}");
            2
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    std::process::exit(run(&args, &mut stdout.lock(), &mut stderr.lock()));
}
