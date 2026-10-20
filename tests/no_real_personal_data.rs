//! Guard: the committed tree must not contain real personal data.
//!
//! This repo once carried a maintainer's real identifiers (name, emails,
//! machine paths, hardware model, real agent session IDs, a private repo
//! path) in board JSON, docs, and the LLM-usage log. Everything was
//! replaced with synthetic equivalents before the org move; this test
//! fails loudly if any of it ever creeps back (via a careless paste, an
//! importer fixture, or a restored backup).
//!
//! Known intentional exceptions:
//! - `LICENSE` carries the original copyright line and stays as is.
//! - `CHANGELOG.md` and `Cargo.toml` still point release-history links and
//!   the `repository` metadata at the old org; those are updated at move
//!   time and are exempt until then.
//! - `README.md` is also the crates.io readme. crates.io renders it with
//!   no base URL, so the embedded architecture diagrams must use absolute
//!   raw URLs at the repository; the README therefore names the org the
//!   same way `Cargo.toml` does.
//!
//! Patterns are assembled from concatenated fragments at runtime so this
//! source file itself never contains a real identifier.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

/// A forbidden literal `needle` plus a human-readable label for failures.
struct Needle {
    needle: String,
    label: &'static str,
    mode: Mode,
}

enum Mode {
    /// Byte-exact substring.
    Sensitive,
    /// Case-insensitive substring.
    Insensitive,
    /// Case-insensitive with non-alphanumeric neighbours required.
    Word,
}

/// Every forbidden literal as a (fragment_a, fragment_b) pair; the real
/// literal is `a + b` and must not appear in any scanned file or path.
fn forbidden_pairs() -> Vec<Needle> {
    let substring = |a: &str, b: &str| Needle {
        needle: a.to_string() + b,
        label: "",
        mode: Mode::Sensitive,
    };
    let mut needles = vec![
        substring("endy", "mion"),
        substring("cursor@anth", "us.ai"),
        Needle {
            needle: "anth".to_string() + "us",
            label: "",
            mode: Mode::Insensitive,
        },
        substring("/Users/", "home"),
        substring("Users-", "home"),
        substring("Black", "bookM1"),
        substring("Ry", "an Alyn Porter"),
        substring("Ry", "an Porter"),
        substring("\"actor_id\": \"", "home\""),
        substring("scanner-", "sentiment"),
        // Real agent session IDs (and distinctive fragments of them).
        substring("c58e", "bed2"),
        substring("cecba6", "ba"),
        substring("00fc", "dc4d"),
        substring("01547c", "91"),
        substring("0423e3", "fd"),
        substring("2da02", "ae2"),
        substring("67537d", "30"),
        substring("a3f9c1", "e2"),
        substring("b77d31", "aa"),
        substring("e1047f", "0c"),
        substring("f02a", "b"),
    ];
    // The bare maintainer first name, matched case-insensitively on word
    // boundaries so "bryan" or "maryan" do not trip it.
    needles.push(Needle {
        needle: "ry".to_string() + "an",
        label: "bare maintainer first name",
        mode: Mode::Word,
    });
    let labels = [
        "personal email domain",
        "agent email address",
        "old org name",
        "real home path",
        "encoded home path",
        "real hardware model",
        "full maintainer name (middle)",
        "full maintainer name",
        "real actor_id",
        "private repo path",
        "real session id 1",
        "real session id 2",
        "real session id 3",
        "real session id 4",
        "real session id 5",
        "real session id 6",
        "real session id 7",
        "real session id 8",
        "real session id 9",
        "real session id 10",
        "real session id fragment",
    ];
    for (n, label) in needles.iter_mut().zip(labels) {
        n.label = label;
    }
    needles
}

/// Files allowed to carry otherwise-forbidden literals, with the needle
/// fragments they are allowed to contain.
const EXEMPTED: &[(&str, &[&str])] = &[
    ("LICENSE", &["ry", "an", "porter"]),
    ("CHANGELOG.md", &["anth", "us"]),
    ("Cargo.toml", &["anth", "us"]),
    ("README.md", &["anth", "us"]),
];

fn is_exempt(path: &str, needle: &str) -> bool {
    let needle_lower = needle.to_lowercase();
    // The project owner explicitly requested the named logo assignment.
    // Permit only that first name in that issue and its creation event;
    // all other identifiers and all other tracker records remain guarded.
    if needle_lower == "ry".to_string() + "an" {
        const LOGO_ISSUE: &str = "BOTSPY-eb7f2c31-d954-4efb-a214-d63608c81d16";
        if path == format!("project/issues/{LOGO_ISSUE}.json") {
            return true;
        }
        if path.starts_with("project/events/") {
            let event = std::fs::read(path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
            if event.is_some_and(|event| {
                event["issue_id"] == LOGO_ISSUE && event["event_type"] == "issue_created"
            }) {
                return true;
            }
        }
    }
    for (exempt_path, fragments) in EXEMPTED {
        if (path == *exempt_path || path.ends_with(exempt_path))
            && fragments
                .iter()
                .any(|f| needle_lower.contains(&f.to_lowercase()))
        {
            return true;
        }
    }
    false
}

fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|&b| b == 0)
}

fn tracked_files() -> Vec<PathBuf> {
    let output = Command::new("git")
        .args(["ls-files", "-z"])
        .output()
        .expect("git ls-files must run");
    if output.status.success() {
        let paths: Vec<PathBuf> = String::from_utf8_lossy(&output.stdout)
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect();
        if !paths.is_empty() {
            return paths;
        }
    }
    // Fallback (e.g. running from a tarball without .git): walk the tree,
    // skipping the usual non-source directories.
    let mut files = Vec::new();
    let mut work = vec![PathBuf::from(".")];
    while let Some(dir) = work.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if path.is_dir() {
                if name != ".git" && name != "target" && name != "node_modules" {
                    work.push(path);
                }
            } else {
                files.push(path);
            }
        }
    }
    files
}

/// Case-insensitive match on word boundaries: neighbours must not be
/// alphanumeric, so embedded occurrences do not trip it.
fn contains_word(haystack_lower: &str, pattern: &str) -> bool {
    let bytes = haystack_lower.as_bytes();
    let mut start = 0;
    while let Some(pos) = haystack_lower[start..].find(pattern) {
        let idx = start + pos;
        let before_ok = idx == 0 || !bytes[idx - 1].is_ascii_alphanumeric();
        let after_idx = idx + pattern.len();
        let after_ok = after_idx == bytes.len() || !bytes[after_idx].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        start = idx + 1;
    }
    false
}

#[test]
fn logo_owner_exception_does_not_allow_other_identifiers_or_documents() {
    let owner = "ry".to_string() + "an";
    let logo_issue = "project/issues/BOTSPY-eb7f2c31-d954-4efb-a214-d63608c81d16.json";
    assert!(is_exempt(logo_issue, &owner));
    assert!(!is_exempt("README.md", &owner));
    assert!(!is_exempt("project/issues/example.json", &owner));
    assert!(!is_exempt(logo_issue, &(owner + " Porter")));
    assert!(!is_exempt(logo_issue, "/Users/example"));
}

#[test]
fn committed_tree_contains_no_real_personal_data() {
    let needles = forbidden_pairs();
    let files = tracked_files();
    assert!(
        !files.is_empty(),
        "no files discovered to scan; guard cannot run meaningfully"
    );

    let mut violations: BTreeMap<String, Vec<&'static str>> = BTreeMap::new();

    for path in &files {
        let path_str = path.to_string_lossy().into_owned();
        let path_lower = path_str.to_lowercase();

        for n in &needles {
            let path_hit = match n.mode {
                Mode::Sensitive => path_str.contains(&n.needle),
                Mode::Insensitive => path_lower.contains(&n.needle),
                Mode::Word => contains_word(&path_lower, &n.needle),
            };
            if path_hit && !is_exempt(&path_str, &n.needle) {
                violations
                    .entry(path_str.clone())
                    .or_default()
                    .push(n.label);
            }
        }

        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        if looks_binary(&bytes) {
            continue;
        }
        let content = String::from_utf8_lossy(&bytes);
        let content_lower = content.to_lowercase();

        for n in &needles {
            let hit = match n.mode {
                Mode::Sensitive => content.contains(&n.needle),
                Mode::Insensitive => content_lower.contains(&n.needle),
                Mode::Word => contains_word(&content_lower, &n.needle),
            };
            if hit && !is_exempt(&path_str, &n.needle) {
                violations
                    .entry(path_str.clone())
                    .or_default()
                    .push(n.label);
            }
        }
    }

    assert!(
        violations.is_empty(),
        "real personal data found in the committed tree:\n{}",
        violations
            .iter()
            .map(|(file, labels)| format!("  {}: {}", file, labels.join(", ")))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
