//! A timeline of parallel agent sessions: an example consumer of the BotSpy
//! public API that draws how many sessions ran at once, where they
//! overlapped, and how sessions fanned out into sub-agents. The output is
//! one self-contained figure: a single SVG holding the title, the peak
//! line, a short description, the legend, the concurrency chart, the
//! swimlanes and the axis. Its viewBox is the content plus an even 16 px
//! padding, so there are no blank margins. The HTML page embeds that same
//! figure (inline SVG and CSS, no scripts, nothing loaded from elsewhere),
//! and the same figure can be written as a standalone SVG or a PNG.
//!
//! ```text
//! cargo run --example timeline -- [--since T] [--until T] [--project P]
//!     [--source S]... [--titles] [--no-ids] [--out PATH] [--svg PATH] [--png PATH]
//!     [--scale N] [--theme light|dark] [--demo] [path-to-agent-data-root]
//! ```
//!
//! # Outputs
//!
//! - `--out PATH` writes the HTML page (default `timeline.html` when no
//!   other output is asked for). It follows `prefers-color-scheme`.
//! - `--svg PATH` writes the standalone figure SVG.
//! - `--png PATH` rasterizes the figure with the pure-Rust `resvg` crate
//!   (a dev-dependency, so the library's dependencies do not change) at
//!   `--scale N` times its size, 2 by default, using the system fonts.
//! - `--theme light|dark` picks the colours of the SVG and PNG (default
//!   light). The HTML page always follows the reader's colour scheme.
//!
//! The images in `docs/images/` are made from the synthetic corpus:
//!
//! ```text
//! cargo run --example timeline -- --demo --theme light --png docs/images/timeline.light.png
//! cargo run --example timeline -- --demo --theme dark  --png docs/images/timeline.dark.png
//! ```
//!
//! # What the figure shows
//!
//! - **Top:** the concurrency step-curve (agents working at once over
//!   time) and its peak.
//! - **Below:** one swimlane per session, grouped by project and ordered by
//!   start. Bars are turns (the agent working); dotted lines are waiting for you.
//!   Sub-agents and forks nest under the session that spawned them, joined
//!   by a connector at the moment of the spawn. A sub-agent lane is labelled
//!   with its kind and its own short id ("↳ Explore a3f9c2"). A child whose
//!   parent is not in the selection sits at top level with a note.
//! - Agents are told apart by a fill pattern and a legend label as well as
//!   colour. The page follows `prefers-color-scheme`, and hovering a bar
//!   shows times and message counts.
//!
//! # Turns and waiting
//!
//! Activity is defined by what the agent was doing, with no thresholds. A
//! bar is a **turn**: the agent working, from a human message to the last
//! non-human message (assistant, tool, system, or tool results) before the
//! next human message. The time from there to the next human message is
//! **waiting for you**, drawn as a dotted line and not counted as working.
//! Consecutive human messages keep the turn's first start; a turn never has
//! zero length, so a trailing human message with no reply is dropped. A
//! session that starts with agent messages (a sub-agent) starts its first
//! turn at its first message. Messages without timestamps are skipped, and
//! a session with no message timestamps at all is one started..last-activity
//! span marked "approximate".
//!
//! # Privacy
//!
//! The figure holds session timing, agent and project names, and short
//! session ids. `--no-ids` leaves the ids out: labels name the agent or
//! sub-agent kind alone, and every id in the SVG becomes `session-N`.
//! Session titles appear only with `--titles`. Message text is never read
//! into it.
//!
//! `--demo` draws a generated, synthetic parallel-sessions corpus instead
//! of reading a data root. Exit status: 0 on success, 2 on any error.
//!
//! Like every example, this imports only `botspy` and the standard library
//! (plus `resvg` for the PNG, in `raster.rs`).

pub mod activity;
#[path = "../common/mod.rs"]
pub mod common;
pub mod demo;
pub mod raster;
pub mod render;

use self::activity::{parse_time, Ms};
use self::render::{Figure, Scheme, Theme};
use botspy::{Agent, Session};
use common::{collect_sessions, default_root};
use std::io::Write;
use std::path::PathBuf;

const USAGE: &str = "usage: timeline [--since T] [--until T] [--project P] [--source S]... [--titles] [--no-ids] [--out PATH] [--svg PATH] [--png PATH] [--scale N] [--theme light|dark] [--demo] [path-to-agent-data-root]";

/// What the command line asked for.
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub since: Option<Ms>,
    pub until: Option<Ms>,
    pub project: Option<String>,
    pub sources: Vec<Agent>,
    pub titles: bool,
    /// Leave session ids out: labels name the agent or sub-agent kind
    /// alone, and every id in the figure is replaced by a pseudonym.
    pub no_ids: bool,
    /// The HTML page; `timeline.html` when no output at all was asked for.
    pub out: Option<PathBuf>,
    pub svg: Option<PathBuf>,
    pub png: Option<PathBuf>,
    /// PNG scale (device pixels per figure pixel).
    pub scale: f32,
    pub theme: Theme,
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
        scale: 2.0,
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
            "--out" => options.out = Some(PathBuf::from(value(arg)?)),
            "--svg" => options.svg = Some(PathBuf::from(value(arg)?)),
            "--png" => options.png = Some(PathBuf::from(value(arg)?)),
            "--scale" => {
                let text = value(arg)?;
                options.scale = text
                    .parse::<f32>()
                    .ok()
                    .filter(|scale| scale.is_finite() && *scale > 0.0)
                    .ok_or_else(|| format!("--scale: {text:?} is not a positive number"))?;
            }
            "--theme" => {
                options.theme = match value(arg)?.as_str() {
                    "light" => Theme::Light,
                    "dark" => Theme::Dark,
                    other => return Err(format!("--theme: {other:?} is not light or dark")),
                };
            }
            "--titles" => options.titles = true,
            "--no-ids" => options.no_ids = true,
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

fn sessions_for(options: &Options, given: Option<Vec<Session>>) -> Vec<Session> {
    let sessions = match given {
        Some(sessions) => sessions,
        None if options.demo => demo::sessions(),
        None => collect_sessions(options.root.as_deref().unwrap_or(&default_root())),
    };
    if options.no_ids {
        pseudonymise(sessions)
    } else {
        sessions
    }
}

/// Replace every session id, and every parent pointer, with `session-N`,
/// numbered in id order. The graph is kept; the identifiers are not.
fn pseudonymise(mut sessions: Vec<Session>) -> Vec<Session> {
    let mut ids: Vec<String> = sessions
        .iter()
        .flat_map(|session| std::iter::once(session.id.clone()).chain(session.parent_id.clone()))
        .collect();
    ids.sort();
    ids.dedup();
    let pseudonym = |id: &str| {
        let n = ids
            .binary_search_by(|probe| probe.as_str().cmp(id))
            .unwrap_or(0)
            + 1;
        format!("session-{n}")
    };
    for session in &mut sessions {
        session.id = pseudonym(&session.id);
        session.parent_id = session.parent_id.as_deref().map(pseudonym);
    }
    sessions
}

/// Render the page for `args`. With `given` sessions they are used as they
/// are; otherwise the demo corpus or the data root is read.
pub fn page_for(args: &[String], given: Option<Vec<Session>>) -> Result<String, String> {
    let options = parse_args(args)?;
    Ok(render::render(&sessions_for(&options, given), &options))
}

/// The standalone figure for `args`, in the requested `--theme`.
pub fn figure_for(args: &[String], given: Option<Vec<Session>>) -> Result<Figure, String> {
    let options = parse_args(args)?;
    Ok(render::figure(
        &sessions_for(&options, given),
        &options,
        Scheme::Fixed(options.theme),
        true,
    ))
}

fn write(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes)
        .map_err(|error| format!("cannot write {}: {}", path.display(), error.kind()))
}

/// Run the example: write each requested output and print its path.
/// Returns the process exit status: 0 on success, 2 on any error.
pub fn run(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let result = parse_args(args).and_then(|options| {
        let sessions = sessions_for(&options, None);
        let mut written: Vec<PathBuf> = Vec::new();
        let page_path = options.out.clone().or_else(|| {
            (options.svg.is_none() && options.png.is_none()).then(|| PathBuf::from("timeline.html"))
        });
        if let Some(path) = page_path {
            write(&path, render::render(&sessions, &options).as_bytes())?;
            written.push(path);
        }
        if options.svg.is_some() || options.png.is_some() {
            let figure = render::figure(&sessions, &options, Scheme::Fixed(options.theme), true);
            if let Some(path) = &options.svg {
                write(path, figure.svg.as_bytes())?;
                written.push(path.clone());
            }
            if let Some(path) = &options.png {
                write(path, &raster::png(&figure.svg, options.scale)?)?;
                written.push(path.clone());
            }
        }
        Ok(written)
    });
    match result {
        Ok(paths) => {
            for path in paths {
                let _ = writeln!(out, "{}", path.display());
            }
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
