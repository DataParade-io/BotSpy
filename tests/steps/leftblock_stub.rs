//! A stand-in for `leftblock scan --jsonl`, used by the privacy scenarios
//! because CI has no LeftBlock. It is not a separate binary: the bdd test
//! executable is hard-linked as `leftblock-stub` and, when started under
//! that name, behaves as the scanner instead of running cucumber (see
//! `maybe_run_as_stub`, called first thing in `main`).
//!
//! The stub speaks the scan contract: JSONL records in on stdin, one JSONL
//! finding per line out on stdout (UTF-8 byte offsets, never the value),
//! exit 1 on findings, 0 on none, 2 on error. It streams: each record's
//! findings are written and flushed before the next record is read, so a
//! client that fails to drain stdout while writing stdin deadlocks it.
//!
//! What it recognises, by shape:
//! - `AKIA` + 16 upper-case alphanumerics: rule `aws-access-token`, kind `secret`
//! - `ghp_` + 36 alphanumerics: rule `github-pat`, kind `secret`
//! - `postgres://` up to whitespace: rule `database-url`, kind `secret`
//! - `[[rule|anything]]`: that rule, kind `secret`
//! - `{{rule|anything}}`: an overlapping pair: the whole marker plus a
//!   second finding that starts 4 bytes in and runs 1 byte past the end
//! - with `--deep` only: an email address, rule `privacy-filter-private_email`,
//!   kind `deep`, with a score
//!
//! Behaviour is configured by files next to the executable (never by
//! environment, so parallel scenarios cannot interfere): `<exe>.mode`
//! holds `ok` (default), `fail` (exit 2 with a stderr line) or `badrange`
//! (every record gets a finding that runs past the end of its text); the stub records its arguments in
//! `<exe>.args` and the record ids it saw in `<exe>.ids`.

use serde_json::json;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

const STUB_NAME: &str = "leftblock-stub";

/// If this process was started as the stub, act as the scanner and exit.
pub fn maybe_run_as_stub() {
    let Some(argv0) = std::env::args_os().next() else {
        return;
    };
    let stem = Path::new(&argv0)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_string);
    if stem.as_deref() == Some(STUB_NAME) {
        std::process::exit(run(Path::new(&argv0)));
    }
}

/// Make a stub in `dir` and return its path. `mode` is `ok`, `fail` or `badrange`.
pub fn install(dir: &Path, mode: &str) -> PathBuf {
    let name = if cfg!(windows) {
        format!("{STUB_NAME}.exe")
    } else {
        STUB_NAME.to_string()
    };
    let path = dir.join(name);
    let me = std::env::current_exe().expect("current exe");
    if std::fs::hard_link(&me, &path).is_err() {
        std::fs::copy(&me, &path).expect("copy test executable as stub");
    }
    std::fs::write(sidecar(&path, "mode"), mode).expect("write stub mode");
    path
}

pub fn sidecar(exe: &Path, extension: &str) -> PathBuf {
    let mut name = exe.as_os_str().to_owned();
    name.push(format!(".{extension}"));
    PathBuf::from(name)
}

fn run(exe: &Path) -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let _ = std::fs::write(sidecar(exe, "args"), args.join(" "));
    let mode = std::fs::read_to_string(sidecar(exe, "mode")).unwrap_or_default();
    if mode.trim() == "fail" {
        eprintln!("leftblock: simulated scanner failure");
        return 2;
    }
    let bad_range = mode.trim() == "badrange";
    if args.first().map(String::as_str) != Some("scan") || !args.iter().any(|a| a == "--jsonl") {
        eprintln!("leftblock: stub only implements `scan --jsonl`");
        return 2;
    }
    let deep = args.iter().any(|arg| arg == "--deep");
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut ids = std::fs::File::create(sidecar(exe, "ids")).ok();
    let mut any = false;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { return 2 };
        let Ok(record) = serde_json::from_str::<serde_json::Value>(&line) else {
            eprintln!("leftblock: bad record");
            return 2;
        };
        let id = record["id"].as_str().unwrap_or_default().to_string();
        let text = record["text"].as_str().unwrap_or_default();
        if let Some(file) = ids.as_mut() {
            let _ = writeln!(file, "{id}");
        }
        let hits = if bad_range {
            vec![(0, text.len() + 5, "oops".to_string(), "secret")]
        } else {
            find(text, deep)
        };
        for (start, end, rule, kind) in hits {
            any = true;
            let mut finding = json!({
                "id": id, "start": start, "end": end, "rule": rule, "kind": kind,
                "finding_id": format!("{:016x}", (start as u64) << 32 | end as u64),
            });
            if kind == "deep" {
                finding["score"] = json!(0.97);
            }
            if writeln!(out, "{finding}").is_err() || out.flush().is_err() {
                return 2;
            }
        }
    }
    i32::from(any)
}

type Hit = (usize, usize, String, &'static str);

fn run_of(text: &str, from: usize, ok: impl Fn(u8) -> bool, max: usize) -> usize {
    text.as_bytes()[from..]
        .iter()
        .take(max)
        .take_while(|byte| ok(**byte))
        .count()
}

fn find(text: &str, deep: bool) -> Vec<Hit> {
    let mut hits: Vec<Hit> = Vec::new();
    for (at, _) in text.match_indices("AKIA") {
        let n = run_of(
            text,
            at + 4,
            |b| b.is_ascii_uppercase() || b.is_ascii_digit(),
            16,
        );
        if n == 16 {
            hits.push((at, at + 20, "aws-access-token".into(), "secret"));
        }
    }
    for (at, _) in text.match_indices("ghp_") {
        let n = run_of(text, at + 4, |b| b.is_ascii_alphanumeric(), 36);
        if n == 36 {
            hits.push((at, at + 40, "github-pat".into(), "secret"));
        }
    }
    for (at, _) in text.match_indices("postgres://") {
        let n = run_of(
            text,
            at,
            |b| !b.is_ascii_whitespace() && b != b'"',
            usize::MAX,
        );
        hits.push((at, at + n, "database-url".into(), "secret"));
    }
    for (open, close, overlap) in [("[[", "]]", false), ("{{", "}}", true)] {
        for (at, _) in text.match_indices(open) {
            let Some(len) = text[at..].find(close) else {
                continue;
            };
            let end = at + len + close.len();
            let inner = &text[at + open.len()..at + len];
            let Some((rule, _)) = inner.split_once('|') else {
                continue;
            };
            hits.push((at, end, rule.to_string(), "secret"));
            if overlap {
                let tail = if text.as_bytes().get(end).is_some_and(u8::is_ascii) {
                    end + 1
                } else {
                    end
                };
                hits.push((at + 4, tail, rule.to_string(), "secret"));
            }
        }
    }
    if deep {
        for (at, _) in text.match_indices('@') {
            let word = |b: u8| b.is_ascii_alphanumeric() || b"._+-".contains(&b);
            let bytes = text.as_bytes();
            let mut start = at;
            while start > 0 && word(bytes[start - 1]) {
                start -= 1;
            }
            let end = at + 1 + run_of(text, at + 1, word, usize::MAX);
            let inside_hit = hits.iter().any(|(s, e, ..)| *s <= at && at < *e);
            if start < at && end > at + 1 && text[at..end].contains('.') && !inside_hit {
                hits.push((start, end, "privacy-filter-private_email".into(), "deep"));
            }
        }
    }
    hits.sort();
    hits
}
