//! Privacy controls for session transcripts: an example consumer of the
//! BotSpy public API that finds and redacts sensitive material before any
//! analysis sees it. Agent histories routinely hold pasted keys, `.env`
//! contents, database URLs and personal data; this is the control that
//! keeps them out of downstream scoring.
//!
//! ```text
//! cargo run --example privacy -- [audit|redact] [--deep] [--plant] [path-to-agent-data-root]
//! ```
//!
//! # The pipeline
//!
//! 1. **BotSpy reads.** `common::collect_sessions` extracts every session
//!    under the data root (default: the committed synthetic corpus, which
//!    by design contains nothing sensitive) into the unified schema.
//! 2. **LeftBlock scans.** Each text-bearing part (text, thinking, tool
//!    results, system text, tool-call arguments) is streamed as one JSONL
//!    record, `{"id":"s<session>/m<message>/p<part>","text":...}`, to the
//!    stdin of a local `leftblock scan --jsonl` process. Findings come back
//!    on its stdout as byte ranges plus rule and kind, never the matched
//!    value. Exit 1 means findings, 2 means error. Transcript text is never
//!    written to disk by this example.
//! 3. **The example redacts.** `redact` replaces each finding's UTF-8 byte
//!    range with `[REDACTED:<rule>]`, merging overlaps, and consumes the
//!    original sessions so nothing downstream can see them.
//! 4. **Analysis runs on redacted text.** The meter and sentiment scoring
//!    (shared with their examples via `#[path]`) run on the redacted
//!    sessions only.
//!
//! `audit` (the default) prints counts by rule and kind per session with
//! provenance back to the source (agent, session id, message index, source
//! file and line or row) and totals. It never prints matched values.
//!
//! `--deep` passes through to LeftBlock's OpenAI Privacy Filter scan, which
//! suits conversational prose (names, emails, addresses). `--plant` swaps
//! the corpus for a generated demo session full of fake secrets in a
//! private temp root that is removed afterwards.
//!
//! LeftBlock is found as `leftblock` on PATH, or at `BOTSPY_LEFTBLOCK`.
//! Install it with `pipx install leftblock`. Only text-bearing parts are
//! scanned and redacted; titles, file paths and raw passthrough parts are
//! out of scope for this example. Exit status: 0 on success, 2 on any error
//! (including LeftBlock missing or failing).
//!
//! Like every example, this imports only `botspy` and the standard library.

#[path = "../common/mod.rs"]
pub mod common;
#[path = "../meter/scoring.rs"]
#[allow(dead_code)]
pub mod meter_scoring;
pub mod planted;
pub mod redact;
pub mod scan;
#[path = "../sentiment/scoring.rs"]
#[allow(dead_code)]
pub mod sentiment_scoring;

use botspy::{KnownPart, Part, Session, ToolArguments};
use common::{collect_sessions, default_root};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;

const USAGE: &str = "usage: privacy [audit|redact] [--deep] [--plant] [path-to-agent-data-root]";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Audit,
    Redact,
}

struct Options {
    mode: Mode,
    deep: bool,
    plant: bool,
    root: Option<PathBuf>,
}

fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut options = Options {
        mode: Mode::Audit,
        deep: false,
        plant: false,
        root: None,
    };
    let mut mode_seen = false;
    for arg in args {
        match arg.as_str() {
            "audit" | "redact" if !mode_seen && options.root.is_none() => {
                mode_seen = true;
                options.mode = if arg == "audit" {
                    Mode::Audit
                } else {
                    Mode::Redact
                };
            }
            "--deep" => options.deep = true,
            "--plant" => options.plant = true,
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            path if options.root.is_none() => options.root = Some(PathBuf::from(path)),
            _ => return Err("too many arguments".to_string()),
        }
    }
    if options.plant && options.root.is_some() {
        return Err("--plant generates its own corpus; pass no data root".to_string());
    }
    Ok(options)
}

/// Run the example. `leftblock` is the `BOTSPY_LEFTBLOCK` override, if any.
/// Returns the process exit status: 0 on success, 2 on any error.
pub fn run(
    args: &[String],
    leftblock: Option<OsString>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let options = match parse_args(args) {
        Ok(options) => options,
        Err(message) => {
            let _ = writeln!(err, "privacy: {message}\n{USAGE}");
            return 2;
        }
    };
    let command = leftblock.unwrap_or_else(|| OsString::from("leftblock"));
    let planted = if options.plant {
        match planted::plant() {
            Ok(planted) => Some(planted),
            Err(error) => {
                let _ = writeln!(
                    err,
                    "privacy: cannot plant the demo corpus: {}",
                    error.kind()
                );
                return 2;
            }
        }
    } else {
        None
    };
    let root = match (&planted, &options.root) {
        (Some(planted), _) => planted.root.clone(),
        (None, Some(root)) => root.clone(),
        (None, None) => default_root(),
    };
    if planted.is_some() {
        let _ = writeln!(out, "Planted a demo corpus under {}", root.display());
    }
    let status = match execute(&options, &root, &command, out) {
        Ok(()) => 0,
        Err(message) => {
            let _ = writeln!(err, "privacy: {message}");
            2
        }
    };
    if let Some(planted) = planted {
        planted.remove();
    }
    status
}

fn execute(
    options: &Options,
    root: &std::path::Path,
    command: &OsString,
    out: &mut dyn Write,
) -> Result<(), String> {
    let sessions = collect_sessions(root);
    let findings = scan::scan(&sessions, command, options.deep).map_err(|error| error.0)?;
    let mut by_slot: BTreeMap<scan::Slot, Vec<scan::Finding>> = BTreeMap::new();
    let known: std::collections::BTreeSet<scan::Slot> =
        scan::slots(&sessions).map(|(slot, _)| slot).collect();
    for finding in findings {
        let slot = scan::Slot::parse(&finding.id)
            .filter(|slot| known.contains(slot))
            .ok_or("leftblock reported a finding for a record that was not sent")?;
        by_slot.entry(slot).or_default().push(finding);
    }
    let result = match options.mode {
        Mode::Audit => audit(&sessions, &by_slot, root, options.deep, out),
        Mode::Redact => {
            let redacted = redact_sessions(sessions, &by_slot)?;
            report_redacted(&redacted, &by_slot, root, out)
        }
    };
    result.map_err(|error| format!("cannot write output: {}", error.kind()))
}

fn audit(
    sessions: &[Session],
    by_slot: &BTreeMap<scan::Slot, Vec<scan::Finding>>,
    root: &std::path::Path,
    deep: bool,
    out: &mut dyn Write,
) -> std::io::Result<()> {
    writeln!(
        out,
        "Privacy audit of {} sessions under {} (leftblock scan --jsonl{}):",
        sessions.len(),
        root.display(),
        if deep { " --deep" } else { "" }
    )?;
    let mut total = 0usize;
    let mut with_findings = 0usize;
    let mut total_rules: BTreeMap<&str, usize> = BTreeMap::new();
    let mut total_kinds: BTreeMap<&str, usize> = BTreeMap::new();
    for (index, session) in sessions.iter().enumerate() {
        let mine: Vec<(&scan::Slot, &scan::Finding)> = by_slot
            .iter()
            .filter(|(slot, _)| slot.session == index)
            .flat_map(|(slot, list)| list.iter().map(move |finding| (slot, finding)))
            .collect();
        if mine.is_empty() {
            continue;
        }
        with_findings += 1;
        total += mine.len();
        let mut rules: BTreeMap<&str, usize> = BTreeMap::new();
        let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
        for (_, finding) in &mine {
            *rules.entry(&finding.rule).or_default() += 1;
            *kinds.entry(&finding.kind).or_default() += 1;
            *total_rules.entry(&finding.rule).or_default() += 1;
            *total_kinds.entry(&finding.kind).or_default() += 1;
        }
        writeln!(
            out,
            "{:?} {}: {} findings",
            session.agent,
            session.id,
            mine.len()
        )?;
        writeln!(out, "  by rule: {}", counts(&rules))?;
        writeln!(out, "  by kind: {}", counts(&kinds))?;
        for (slot, finding) in mine {
            writeln!(
                out,
                "  - {} [{}] message {} part {}, {}",
                finding.rule,
                finding.kind,
                slot.message,
                slot.part,
                source_of(session, slot.message)
            )?;
        }
    }
    writeln!(
        out,
        "Totals: {total} findings in {with_findings} of {} sessions",
        sessions.len()
    )?;
    if total > 0 {
        writeln!(out, "  by rule: {}", counts(&total_rules))?;
        writeln!(out, "  by kind: {}", counts(&total_kinds))?;
    }
    writeln!(out, "Matched values are never printed.")
}

fn counts(map: &BTreeMap<&str, usize>) -> String {
    map.iter()
        .map(|(name, count)| format!("{name} x{count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Where a message came from on disk, per BotSpy's provenance.
fn source_of(session: &Session, message: usize) -> String {
    let Some(provenance) = session
        .messages
        .get(message)
        .and_then(|message| message.provenance.as_ref())
    else {
        return "source unknown".to_string();
    };
    match (provenance.line, provenance.row) {
        (Some(line), _) => format!("source {}:{line}", provenance.source_file),
        (None, Some(row)) => format!("source {} row {row}", provenance.source_file),
        (None, None) => format!("source {}", provenance.source_file),
    }
}

/// Replace every finding in the sessions. Consumes the originals: from here
/// on only redacted text exists.
fn redact_sessions(
    mut sessions: Vec<Session>,
    by_slot: &BTreeMap<scan::Slot, Vec<scan::Finding>>,
) -> Result<Vec<Session>, String> {
    for (slot, findings) in by_slot {
        let ranges: Vec<redact::Range> = findings
            .iter()
            .map(|finding| redact::Range {
                start: finding.start,
                end: finding.end,
                rule: finding.rule.clone(),
            })
            .collect();
        let part = &mut sessions[slot.session].messages[slot.message].parts[slot.part];
        let original = scan::part_text(part).ok_or("a finding points at a part with no text")?;
        let replaced = redact::redact_text(&original, &ranges)?;
        drop(original);
        match part {
            Part::Known(
                KnownPart::Text { text, .. }
                | KnownPart::System { text, .. }
                | KnownPart::Thinking {
                    text: Some(text), ..
                }
                | KnownPart::ToolResult {
                    text: Some(text), ..
                },
            ) => *text = replaced,
            Part::Known(KnownPart::ToolCall { arguments, .. }) => {
                *arguments = Some(ToolArguments::from_raw(replaced));
            }
            _ => return Err("a finding points at a part with no text".to_string()),
        }
    }
    Ok(sessions)
}

fn report_redacted(
    sessions: &[Session],
    by_slot: &BTreeMap<scan::Slot, Vec<scan::Finding>>,
    root: &std::path::Path,
    out: &mut dyn Write,
) -> std::io::Result<()> {
    let replaced: usize = by_slot.values().map(Vec::len).sum();
    writeln!(
        out,
        "Redacted {} sessions under {} ({replaced} findings replaced):",
        sessions.len(),
        root.display()
    )?;
    for session in sessions {
        writeln!(out, "{:?} {}", session.agent, session.id)?;
        for (index, message) in session.messages.iter().enumerate() {
            for (part_index, part) in message.parts.iter().enumerate() {
                if let Some(text) = scan::part_text(part) {
                    writeln!(
                        out,
                        "  m{index}.p{part_index} {:?}/{}: {}",
                        message.role,
                        part.kind_name().unwrap_or("part"),
                        text.escape_debug()
                    )?;
                }
            }
        }
    }
    // The control working: analysis only ever sees the redacted sessions.
    let meter = meter_scoring::score_history(sessions);
    let sentiment = sentiment_scoring::score_history(sessions);
    writeln!(out, "Analysis on redacted text only:")?;
    writeln!(
        out,
        "  Meter: {} thanks vs {} F-bombs across {} sessions",
        meter.thanks, meter.f_bombs, meter.sessions
    )?;
    writeln!(
        out,
        "  Sentiment: {} positive, {} negative, {} neutral messages",
        sentiment.positive, sentiment.negative, sentiment.neutral
    )
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    let status = run(
        &args,
        std::env::var_os("BOTSPY_LEFTBLOCK"),
        &mut stdout.lock(),
        &mut stderr.lock(),
    );
    std::process::exit(status);
}
