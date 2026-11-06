use super::{agent_name, doctor_facts, real_source, resolved_root, selected_sources, RunOutcome};
use crate::{importer::SourceOptions, Session};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::ffi::OsString;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, clap::Args)]
pub struct PrivacyAuditArgs {
    /// Project directory. Defaults to the working directory.
    #[arg(long, conflicts_with_all = ["all", "session"])]
    project: Option<PathBuf>,
    /// Include sessions across all projects.
    #[arg(long, conflicts_with = "session")]
    all: bool,
    /// Audit one exact session ID without a project or date filter.
    #[arg(long, conflicts_with = "since")]
    session: Option<String>,
    /// Only sessions active since this date, timestamp, or duration (default 7d).
    #[arg(long)]
    since: Option<String>,
    /// Restrict the audit to these sources (repeatable).
    #[arg(long)]
    source: Vec<String>,
    /// Override the home directory used for source discovery.
    #[arg(long)]
    home: Option<PathBuf>,
    /// Override the root of the single selected source.
    #[arg(long, requires = "source")]
    root: Option<PathBuf>,
    /// Use a specific LeftBlock executable.
    #[arg(long)]
    leftblock: Option<OsString>,
    /// Try the local Privacy Filter model, or disable it.
    #[arg(long, value_parser = ["auto", "off"], default_value = "auto")]
    privacy_filter: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PassStatus {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    finding_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Deserialize)]
struct LeftBlockFinding {
    id: String,
    start: usize,
    end: usize,
    line: usize,
    rule: String,
    kind: String,
    finding_id: String,
    score: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LeftBlockReport {
    schema_version: u32,
    rules: PassStatus,
    privacy_filter: PassStatus,
    findings: Vec<LeftBlockFinding>,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceStatus {
    source: String,
    status: &'static str,
    sessions_scanned: usize,
    sessions_without_project: usize,
    read_errors: usize,
}

fn project_match(root: &Path, cwd: Option<&str>, project_id: &str) -> Option<bool> {
    let directory = Path::new(cwd.filter(|p| !p.is_empty()).unwrap_or(project_id));
    if !directory.is_absolute() {
        return None;
    }
    let canonical = directory.canonicalize().ok()?;
    Some(canonical.starts_with(root))
}

pub(super) fn run(args: PrivacyAuditArgs) -> RunOutcome {
    let mut names = match selected_sources(&args.source) {
        Ok(names) => names,
        Err(_) => return RunOutcome::fail(2, "Unknown session source.\n".into()),
    };
    let mut seen = HashSet::new();
    names.retain(|name| seen.insert(name.clone()));
    if args.root.is_some() && names.len() != 1 {
        return RunOutcome::fail(2, "Select one source with --root.\n".into());
    }
    let project = if args.all || args.session.is_some() {
        None
    } else {
        let input = args.project.as_deref().unwrap_or(Path::new("."));
        match input.canonicalize() {
            Ok(path) if path.is_dir() => Some(path),
            _ => return RunOutcome::fail(2, "Cannot open the project directory.\n".into()),
        }
    };
    let since = if args.session.is_some() {
        None
    } else {
        match super::scan::parse_cutoff(args.since.as_deref().unwrap_or("7d")) {
            Ok(cutoff) => Some(cutoff),
            Err(_) => return RunOutcome::fail(2, "Invalid --since date or duration.\n".into()),
        }
    };
    let options = SourceOptions {
        root: args.root,
        home: args.home,
    };
    let mut sources = Vec::new();
    let mut sessions = Vec::new();
    for name in names {
        let mut status = SourceStatus {
            source: name.clone(),
            status: "completed",
            ..Default::default()
        };
        let root = resolved_root(&name, &options);
        match root.metadata() {
            Ok(meta)
                if if name == "cursor" {
                    meta.is_file()
                } else {
                    meta.is_dir()
                } => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound && args.source.is_empty() => {
                status.status = "missing";
                sources.push(status);
                continue;
            }
            _ => {
                status.status = "error";
                status.read_errors = 1;
                sources.push(status);
                continue;
            }
        }
        let adapter = real_source(&name, &options).expect("validated source");
        status.read_errors = doctor_facts(std::slice::from_ref(&name), &options)[0]
            .issues
            .len();
        for summary in adapter.discover() {
            if let Some(id) = &args.session {
                if &summary.id != id {
                    continue;
                }
            } else if since
                .as_ref()
                .is_some_and(|ts| summary.last_activity_at < *ts)
            {
                continue;
            }
            if project.as_ref().is_some_and(|root| {
                project_match(root, summary.metadata.cwd.as_deref(), &summary.project_id)
                    == Some(false)
            }) {
                continue;
            }
            let Some(session) = adapter.open(&summary.id) else {
                status.read_errors += 1;
                continue;
            };
            if let Some(root) = &project {
                match project_match(
                    root,
                    session
                        .metadata
                        .cwd
                        .as_deref()
                        .or(summary.metadata.cwd.as_deref()),
                    &summary.project_id,
                ) {
                    Some(true) => (),
                    Some(false) => continue,
                    None => {
                        status.sessions_without_project += 1;
                        continue;
                    }
                }
            }
            status.sessions_scanned += 1;
            sessions.push(session);
        }
        if status.read_errors > 0 {
            status.status = if status.sessions_scanned == 0 {
                "error"
            } else {
                "partial"
            };
        }
        sources.push(status);
    }
    let source_error = sources
        .iter()
        .any(|source| matches!(source.status, "error" | "partial"))
        || sources.iter().all(|source| source.status == "missing");
    if args.session.is_some() {
        if sessions.len() > 1 {
            return RunOutcome::fail(2, "Session ID is ambiguous. Select one source.\n".into());
        }
        if sessions.is_empty() && !source_error {
            return RunOutcome::fail(1, "Session ID not found.\n".into());
        }
    }
    if source_error && sessions.is_empty() {
        return outcome(
            3,
            &sources,
            &[],
            None,
            "Could not read the selected session sources.\n",
        );
    }
    let executable = args
        .leftblock
        .or_else(|| std::env::var_os("BOTSPY_LEFTBLOCK"))
        .unwrap_or_else(|| "leftblock".into());
    match scan_sessions(&executable, &args.privacy_filter, &sessions) {
        Ok((report, findings)) => outcome(
            if source_error { 3 } else { 0 },
            &sources,
            &findings,
            Some(&report),
            if source_error {
                "Some session sources could not be read.\n"
            } else {
                ""
            },
        ),
        Err(()) => outcome(
            3,
            &sources,
            &[],
            None,
            "LeftBlock could not complete the session audit.\n",
        ),
    }
}

fn outcome(
    code: i32,
    sources: &[SourceStatus],
    findings: &[Value],
    report: Option<&LeftBlockReport>,
    stderr: &str,
) -> RunOutcome {
    let payload = json!({
        "schemaVersion": 1,
        "sources": sources,
        "findings": findings,
        "rules": report.map_or_else(|| json!({"status": "error"}), |r| json!(r.rules)),
        "privacyFilter": report.map_or_else(|| json!({"status": "not-run"}), |r| json!(r.privacy_filter)),
    });
    RunOutcome {
        code,
        stdout: format!("{payload}\n"),
        stderr: stderr.into(),
    }
}

fn scan_sessions(
    executable: &OsString,
    model_mode: &str,
    sessions: &[Session],
) -> Result<(LeftBlockReport, Vec<Value>), ()> {
    let mut child = Command::new(executable)
        .args([
            "scan",
            "--jsonl",
            "--format",
            "report",
            "--privacy-filter",
            model_mode,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| ())?;
    let input = child.stdin.take().ok_or(())?;
    let (output, locations) = std::thread::scope(|scope| {
        let writer = scope.spawn(move || {
            let mut input = BufWriter::new(input);
            let mut locations = Vec::new();
            for (s, session) in sessions.iter().enumerate() {
                for (m, message) in session.messages.iter().enumerate() {
                    for (p, part) in message.parts.iter().enumerate() {
                        if let Some(text) = crate::privacy::part_text(part) {
                            let record = json!({"id": locations.len().to_string(), "text": text});
                            writeln!(input, "{record}").map_err(|_| ())?;
                            locations.push((s, m, p, text.len()));
                        }
                    }
                }
            }
            input.flush().map_err(|_| ())?;
            Ok(locations)
        });
        (
            child.wait_with_output().map_err(|_| ()),
            writer.join().map_err(|_| ()).and_then(|r| r),
        )
    });
    let output = output?;
    let locations = locations?;
    if !matches!(output.status.code(), Some(0 | 1)) {
        return Err(());
    }
    let report: LeftBlockReport = serde_json::from_slice(&output.stdout).map_err(|_| ())?;
    if report.schema_version != 1 || report.rules.status != "completed" {
        return Err(());
    }
    let findings = report.findings.iter().map(|finding| {
        let index = finding.id.parse::<usize>().map_err(|_| ())?;
        let &(s, m, p, length) = locations.get(index).ok_or(())?;
        if finding.start >= finding.end || finding.end > length || finding.line == 0 { return Err(()); }
        let session = &sessions[s];
        let source = session.messages[m].provenance.as_ref().map(|prov| json!({
            "file": prov.source_file, "line": prov.line, "row": prov.row,
        }));
        Ok(json!({
            "agent": agent_name(session.agent), "sessionId": session.id, "message": m + 1, "part": p + 1,
            "source": source, "rule": finding.rule, "kind": finding.kind, "finding_id": finding.finding_id,
            "start": finding.start, "end": finding.end, "line": finding.line, "score": finding.score,
        }))
    }).collect::<Result<Vec<_>, ()>>()?;
    Ok((report, findings))
}
