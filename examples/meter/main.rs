//! The coding-session thanks-vs-F-bombs meter: an example consumer of the
//! BotSpy public API. It walks a fixture root (default: the committed
//! synthetic corpus under `tests/fixtures/`), extracts every session it can
//! reach through the per-agent sources, and scores your tone: only the
//! messages you typed count, never the agent's replies, tool results, or
//! text injected by tooling.
//!
//! ```text
//! cargo run --example meter [path-to-agent-data-root]
//! cargo run --example meter -- [--since T] [--until T] [--svg PATH]
//!     [--png PATH] [--scale N] [--theme light|dark] [--by week|day|hour]
//!     [--utc-offset HOURS] [--demo]
//!     [path-to-agent-data-root]
//! ```
//!
//! # Outputs
//!
//! - With no output flag it prints the text report: thanks and F-bombs per
//!   session and in total.
//! - `--svg PATH` writes a standalone figure: your messages per week, as a
//!   diverging bar chart. Thanks bars rise from a zero axis as solid fills;
//!   F-bomb bars hang below it as diagonal hatching, so the two differ by
//!   pattern as well as colour. Weeks start on Monday, UTC. Each bar is
//!   labelled with its count where there is room.
//! - `--png PATH` rasterizes the same figure with the pure-Rust `resvg`
//!   crate (a dev-dependency) at `--scale N` times its size, 2 by default,
//!   using the system fonts.
//! - `--theme light|dark` picks the colours of the SVG and PNG (default
//!   light).
//! - `--by week|day|hour` sets what one bar covers (default week). Days show
//!   which ones were hard over a long stretch; hours show one day up close.
//! - `--utc-offset HOURS` places days and hours in your local time, such as
//!   `-4` (default 0, UTC). A fixed offset, so it does not follow daylight
//!   saving across a change.
//! - `--since T` / `--until T` keep only messages inside the window (an
//!   RFC 3339 time or a bare date) and stretch the axis to cover it.
//!
//! The images in `docs/images/` are made from the synthetic corpus:
//!
//! ```text
//! cargo run --example meter -- --demo --theme light --png docs/images/meter.light.png
//! cargo run --example meter -- --demo --theme dark  --png docs/images/meter.dark.png
//! ```
//!
//! # Privacy
//!
//! The figure holds counts, week dates and fixed wording. Message text,
//! session titles and session ids are never read into it. Messages without
//! a timestamp cannot be placed on a week and are left out of the figure
//! (the text report still counts them).
//!
//! `--demo` draws a generated, synthetic corpus instead of reading a data
//! root. Exit status: 0 on success, 2 on any error.
//!
//! The figure shares its look with the timeline example: the rasterizer
//! (`raster.rs`) and the time helpers (`activity.rs`) are the timeline's own
//! files, compiled in through `#[path]`; the fonts, palette, padding and
//! title block are copied into `chart.rs`.
//!
//! This example lives outside the core dependency graph: it imports only
//! `botspy` and the standard library (plus `resvg` for the PNG, in
//! `raster.rs`).

#[path = "../timeline/activity.rs"]
#[allow(dead_code)]
pub mod activity;
pub mod chart;
#[path = "../common/mod.rs"]
pub mod common;
pub mod demo;
#[path = "../timeline/raster.rs"]
pub mod raster;
pub mod scoring;

use self::activity::{parse_time, Ms};
use self::chart::{Figure, Period, Theme};
use botspy::Session;
use common::{collect_sessions, default_root};
use scoring::{favors_thanks, score_history, score_session};
use std::io::Write;
use std::path::PathBuf;

const USAGE: &str = "usage: meter [--since T] [--until T] [--svg PATH] [--png PATH] [--scale N] [--theme light|dark] [--by week|day|hour] [--utc-offset HOURS] [--demo] [path-to-agent-data-root]";

/// What the command line asked for.
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub since: Option<Ms>,
    pub until: Option<Ms>,
    pub svg: Option<PathBuf>,
    pub png: Option<PathBuf>,
    /// PNG scale (device pixels per figure pixel).
    pub scale: f32,
    pub theme: Theme,
    pub by: Period,
    pub utc_offset: i64,
    pub demo: bool,
    pub root: Option<PathBuf>,
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
            "--by" => {
                options.by = match value(arg)?.as_str() {
                    "week" => Period::Week,
                    "day" => Period::Day,
                    "hour" => Period::Hour,
                    other => return Err(format!("--by: {other:?} is not week, day or hour")),
                };
            }
            "--utc-offset" => {
                let text = value(arg)?;
                options.utc_offset = text
                    .parse::<i64>()
                    .ok()
                    .filter(|hours| hours.abs() <= 14)
                    .ok_or_else(|| {
                        format!("--utc-offset: {text:?} is not a whole number of hours")
                    })?;
            }
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
    match given {
        Some(sessions) => sessions,
        None if options.demo => demo::sessions(),
        None => collect_sessions(options.root.as_deref().unwrap_or(&default_root())),
    }
}

/// The standalone figure for `args`, in the requested `--theme`. With
/// `given` sessions they are used as they are; otherwise the demo corpus or
/// the data root is read.
pub fn figure_for(args: &[String], given: Option<Vec<Session>>) -> Result<Figure, String> {
    let options = parse_args(args)?;
    let bars = chart::periods(
        &sessions_for(&options, given),
        options.since,
        options.until,
        options.by,
        options.utc_offset,
    );
    Ok(chart::figure_by(
        &bars,
        options.theme,
        options.by,
        options.utc_offset,
    ))
}

/// The text report: thanks and F-bombs in your messages, per session and
/// in total.
fn report(sessions: &[Session], root: &str, out: &mut dyn Write) {
    let total = score_history(sessions);
    let _ = writeln!(out, "Scoring {} sessions under {root}:", sessions.len());
    for session in sessions {
        let (thanks, f_bombs) = score_session(session);
        let _ = writeln!(
            out,
            "  {:?} {}: {thanks} thanks, {f_bombs} F-bombs in your messages",
            session.agent, session.id
        );
    }
    let _ = writeln!(
        out,
        "Meter: {} thanks vs {} F-bombs in your messages across {} sessions — the ratio {} thanks.",
        total.thanks,
        total.f_bombs,
        total.sessions,
        if favors_thanks(&total) {
            "favors"
        } else {
            "does not favor"
        }
    );
}

fn write(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes)
        .map_err(|error| format!("cannot write {}: {}", path.display(), error.kind()))
}

/// Run the example: print the text report, or write each requested figure
/// and print its path. Returns the process exit status: 0 on success, 2 on
/// any error.
pub fn run(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let result = parse_args(args).and_then(|options| {
        let sessions = sessions_for(&options, None);
        if options.svg.is_none() && options.png.is_none() {
            let root = if options.demo {
                "the synthetic demo corpus".to_string()
            } else {
                options
                    .root
                    .clone()
                    .unwrap_or_else(default_root)
                    .display()
                    .to_string()
            };
            report(&sessions, &root, out);
            return Ok(Vec::new());
        }
        let bars = chart::periods(
            &sessions,
            options.since,
            options.until,
            options.by,
            options.utc_offset,
        );
        let figure = chart::figure_by(&bars, options.theme, options.by, options.utc_offset);
        let mut written = Vec::new();
        if let Some(path) = &options.svg {
            write(path, figure.svg.as_bytes())?;
            written.push(path.clone());
        }
        if let Some(path) = &options.png {
            write(path, &raster::png(&figure.svg, options.scale)?)?;
            written.push(path.clone());
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
            let _ = writeln!(err, "meter: {message}\n{USAGE}");
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
