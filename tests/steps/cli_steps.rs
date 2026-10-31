//! Steps for the CLI specifications (features/cli/).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use botspy::cli;
use cucumber::{given, then, when};
use serde_json::Value;

use crate::BotSpyWorld;

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn claude_projects_root() -> PathBuf {
    corpus_root().join("claude_code/projects")
}

fn cursor_store_path() -> PathBuf {
    corpus_root().join("cursor/store/state.vscdb")
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("botspy-cli-spec").join(format!(
        "{}-{}-{tag}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// Copy a file or a whole directory tree into the fixture home. Copies
/// (not symlinks) keep the fixture home portable and guarantee the
/// committed corpus itself is never written to.
fn copy_entry(target: &Path, link_path: &Path) {
    if target.is_dir() {
        std::fs::create_dir_all(link_path).expect("create copied dir");
        for entry in std::fs::read_dir(target).expect("read copied dir") {
            let entry = entry.expect("copied dir entry").path();
            copy_entry(&entry, &link_path.join(file_name(&entry)));
        }
    } else {
        std::fs::copy(target, link_path).unwrap_or_else(|err| {
            panic!(
                "copy {} -> {}: {err}",
                target.display(),
                link_path.display()
            )
        });
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string()
}

/// A deterministic Claude Code session with known ids, timestamps, and
/// text, so scenarios can assert on its content.
fn write_solo_session(home: &Path) {
    let dir = home.join(".claude/projects/extra-demo");
    std::fs::create_dir_all(&dir).expect("create extra project dir");
    std::fs::write(
        dir.join("solo-abc123.jsonl"),
        concat!(
            r#"{"type":"user","uuid":"u1","timestamp":"2026-09-15T12:00:00Z","message":{"role":"user","content":"hello from the fixture"}}"#,
            "\n",
            r#"{"type":"user","uuid":"u2","timestamp":"2026-09-15T12:05:00Z","message":{"role":"user","content":"second message"}}"#,
            "\n",
        ),
    )
    .expect("write solo transcript");
}

/// Lay out the fixture home the way the sources' real homes look,
/// pointing the per-agent locations at the synthetic corpus. Only the
/// listed sources get locations; the rest are missing.
fn build_home(home: &Path, sources: &[&str]) {
    for source in sources {
        match *source {
            "claude_code" => {
                let projects = home.join(".claude/projects");
                std::fs::create_dir_all(&projects).expect("create projects dir");
                copy_entry(&claude_projects_root(), &projects);
                write_solo_session(home);
            }
            "cursor" => {
                std::fs::create_dir_all(home.join(".cursor")).expect("create .cursor");
                copy_entry(&cursor_store_path(), &home.join(".cursor/state.vscdb"));
            }
            "codex" => copy_entry(&corpus_root().join("codex"), &home.join(".codex")),
            "grok_bot" => {
                std::fs::create_dir_all(home.join(".grok")).expect("create .grok");
                copy_entry(
                    &corpus_root().join("grok_bot/sand-client-persistence"),
                    &home.join(".grok/sand-client-persistence"),
                );
            }
            "antigravity" => {
                std::fs::create_dir_all(home.join(".gemini")).expect("create .gemini");
                copy_entry(
                    &corpus_root().join("antigravity"),
                    &home.join(".gemini/antigravity"),
                );
            }
            other => panic!("unknown fixture source {other}"),
        }
    }
}

#[given(expr = "a fixture home built from the synthetic corpus")]
fn fixture_home(world: &mut BotSpyWorld) {
    let home = scratch("home");
    build_home(
        &home,
        &["claude_code", "cursor", "codex", "grok_bot", "antigravity"],
    );
    world.cli_home = Some(home);
}

#[given(regex = r#"^a fixture home with only the sources "(.*)"$"#)]
fn fixture_home_subset(world: &mut BotSpyWorld, list: String) {
    let sources: Vec<&str> = list.split(',').map(str::trim).collect();
    let home = scratch("home-subset");
    build_home(&home, &sources);
    world.cli_home = Some(home);
}

/// A synthetic duplicate: the same session id recorded under two
/// different project directories, the real-data collision shape that
/// dedupe by id alone silently collapsed.
#[given(expr = "a fixture home with the same session id in two projects")]
fn fixture_home_duplicate_id(world: &mut BotSpyWorld) {
    let home = scratch("home-duplicate");
    build_home(&home, &["claude_code"]);
    let dupe = home.join(".claude/projects/dupe-demo");
    std::fs::create_dir_all(&dupe).expect("create duplicate project dir");
    std::fs::write(
        dupe.join("solo-abc123.jsonl"),
        concat!(
            r#"{"type":"user","uuid":"u1","timestamp":"2026-09-15T13:00:00Z","message":{"role":"user","content":"hello from the duplicate project"}}"#,
            "\n",
        ),
    )
    .expect("write duplicate transcript");
    world.cli_home = Some(home);
}

fn run_cli(world: &mut BotSpyWorld, args: &str, with_home: bool) {
    let mut argv: Vec<String> = vec!["botspy".to_string()];
    argv.extend(
        expand_with(world, args)
            .split_whitespace()
            .map(str::to_string),
    );
    if with_home {
        argv.push("--home".to_string());
        argv.push(
            world
                .cli_home
                .as_ref()
                .expect("fixture home")
                .display()
                .to_string(),
        );
    }
    world.cli_run = Some(cli::run_from(argv));
}

/// The scenario's scratch output directory. Stable across the steps of
/// one scenario — a scan writes its store in one step and later steps
/// assert on that store — and fresh for every scenario.
fn out_dir(world: &mut BotSpyWorld) -> PathBuf {
    if let Some(dir) = &world.cli_out_dir {
        return dir.clone();
    }
    let dir = scratch("out");
    world.cli_out_dir = Some(dir.clone());
    dir
}

/// Replace the spec placeholders with concrete fixture paths, using the
/// scenario's stable output directory.
fn expand_with(world: &mut BotSpyWorld, text: &str) -> String {
    let mut text = text.replace("{out_dir}", &out_dir(world).to_string_lossy());
    if let Some(home) = world.cli_home.clone() {
        text = text.replace("{home}", &home.display().to_string());
    }
    text.replace(
        "{claude_projects_root}",
        &claude_projects_root().to_string_lossy(),
    )
    .replace(
        "{cursor_store_path}",
        &cursor_store_path().to_string_lossy(),
    )
}

#[when(regex = r#"^I run "botspy (.*?)" against the fixture home$"#)]
fn run_against_home(world: &mut BotSpyWorld, args: String) {
    run_cli(world, &args, true);
}

#[when(regex = r#"^I run "botspy (.*?)" against the fixture corpus$"#)]
fn run_against_corpus(world: &mut BotSpyWorld, args: String) {
    run_cli(world, &args, false);
}

#[when(regex = r#"^I run "botspy (.*?)" with BOTSPY_HOME at the fixture home$"#)]
fn run_with_botspy_home(world: &mut BotSpyWorld, args: String) {
    let home = world.cli_home.clone().expect("fixture home");
    std::env::set_var("BOTSPY_HOME", &home);
    run_cli(world, &args, false);
    std::env::remove_var("BOTSPY_HOME");
}

#[when(regex = r#"^I run "botspy ([^"]*)"$"#)]
fn run_plain(world: &mut BotSpyWorld, args: String) {
    run_cli(world, &args, false);
}

fn run(world: &BotSpyWorld) -> &cli::RunOutcome {
    world.cli_run.as_ref().expect("no CLI run recorded")
}

fn json(world: &BotSpyWorld) -> Value {
    serde_json::from_str(&run(world).stdout).expect("stdout is valid JSON")
}

/// Every quoted expectation in the (placeholder-expanded) text must
/// appear in the haystack.
fn assert_all_in(world: &mut BotSpyWorld, label: &str, haystack: &str, text: &str) {
    let needle = expand_with(world, text);
    let parts: Vec<String> = needle
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect();
    assert!(
        !parts.is_empty(),
        "no quoted expectations in {label}: {needle}"
    );
    for part in parts {
        assert!(
            haystack.contains(&part),
            "{label} does not contain {part:?}; actual: {haystack:?}"
        );
    }
}

fn assert_none_in(label: &str, haystack: &str, text: &str) {
    for part in text.split('"').skip(1).step_by(2) {
        assert!(
            !haystack.contains(part),
            "{label} must not contain {part:?}; actual: {haystack:?}"
        );
    }
}

fn full_claude_projects_path(world: &BotSpyWorld) -> String {
    world
        .cli_home
        .as_ref()
        .expect("fixture home")
        .join(".claude/projects")
        .display()
        .to_string()
}

#[then(regex = r#"^the exit code is ([0-9]+)$"#)]
fn exit_code(world: &mut BotSpyWorld, code: String) {
    let actual = run(world).code;
    assert_eq!(
        actual,
        code.parse::<i32>().expect("numeric exit code"),
        "unexpected exit code; stdout: {:?}, stderr: {:?}",
        run(world).stdout,
        run(world).stderr
    );
}

#[then("stdout reports the current crate version")]
fn stdout_reports_crate_version(world: &mut BotSpyWorld) {
    assert_eq!(
        run(world).stdout,
        format!("botspy {}\n", env!("CARGO_PKG_VERSION")),
        "the CLI version must match the compiled crate"
    );
}

#[then(regex = r#"^stdout contains (".*")$"#)]
fn stdout_contains(world: &mut BotSpyWorld, text: String) {
    let stdout = run(world).stdout.clone();
    assert_all_in(world, "stdout", &stdout, &text);
}

#[then(regex = r#"^stdout does not contain (".*")$"#)]
fn stdout_not_contains(world: &mut BotSpyWorld, text: String) {
    assert_none_in("stdout", &run(world).stdout, &text);
}

#[then(regex = r#"^stderr contains (".*")$"#)]
fn stderr_contains(world: &mut BotSpyWorld, text: String) {
    let stderr = run(world).stderr.clone();
    assert_all_in(world, "stderr", &stderr, &text);
}

#[then(regex = r#"^stderr mentions (".*")$"#)]
fn stderr_mentions(world: &mut BotSpyWorld, text: String) {
    let stderr = run(world).stderr.clone();
    assert_all_in(world, "stderr", &stderr, &text);
}

#[then(regex = r#"^stdout reports "([^"]+)"$"#)]
fn stdout_reports(world: &mut BotSpyWorld, expected: String) {
    let stdout = &run(world).stdout;
    assert!(
        stdout.contains(expected.as_str()),
        "stdout does not report {expected:?}; actual: {stdout:?}"
    );
}

#[then(regex = r#"^stdout parses as a JSON array of ([0-9]+) entr(?:y|ies)$"#)]
fn stdout_json_array(world: &mut BotSpyWorld, count: String) {
    let value = json(world);
    let count = count.parse::<usize>().expect("numeric count");
    assert!(value.is_array(), "stdout is not a JSON array: {value:?}");
    let entries = value.as_array().expect("array");
    assert_eq!(
        entries.len(),
        count,
        "unexpected JSON array length; entries: {entries:?}"
    );
}

#[then(regex = r#"^stdout parses as a JSON object$"#)]
fn stdout_json_object(world: &mut BotSpyWorld) {
    let value = json(world);
    assert!(value.is_object(), "stdout is not a JSON object: {value:?}");
}

#[then(regex = r#"^stdout parses as a JSON object with "([^"]+)" "(.*)"$"#)]
#[then(regex = r#"^the JSON "([^"]+)" is "(.*)"$"#)]
fn stdout_json_object_with(world: &mut BotSpyWorld, key: String, expected: String) {
    let value = json(world);
    let object = value.as_object().expect("object");
    let actual = object
        .get(&key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("missing or non-string {key}: {value:?}"));
    assert_eq!(actual, expand_with(world, &expected), "unexpected {key}");
}

#[then(regex = r#"^stdout parses as NDJSON with ([0-9]+) lines$"#)]
fn stdout_ndjson(world: &mut BotSpyWorld, count: String) {
    let stdout = &run(world).stdout;
    let lines: Vec<&str> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let count = count.parse::<usize>().expect("numeric count");
    assert_eq!(
        lines.len(),
        count,
        "unexpected NDJSON line count; stdout: {stdout:?}"
    );
    for line in lines {
        let value: Value = serde_json::from_str(line)
            .unwrap_or_else(|err| panic!("NDJSON line is not JSON ({err}): {line:?}"));
        assert!(value.is_object(), "NDJSON line is not an object: {line:?}");
    }
}

fn json_entries(world: &mut BotSpyWorld) -> Vec<Value> {
    let value = json(world);
    value
        .as_array()
        .unwrap_or_else(|| panic!("stdout is not a JSON array: {value:?}"))
        .clone()
}

fn entry_named<'a>(entries: &'a [Value], name: &str) -> &'a Value {
    entries
        .iter()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(name))
        .unwrap_or_else(|| panic!("no JSON entry named {name}: {entries:?}"))
}

#[then(regex = r#"^the JSON entry "([^"]+)" has "([^"]+)" "(.*)"$"#)]
fn json_entry_string(world: &mut BotSpyWorld, name: String, key: String, expected: String) {
    let entries = json_entries(world);
    let entry = entry_named(&entries, &name);
    let actual = entry
        .get(&key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("missing or non-string {key} in {name}: {entry:?}"));
    assert_eq!(
        actual,
        expand_with(world, &expected),
        "unexpected {key} of {name}"
    );
}

#[then(regex = r#"^the JSON entry "([^"]+)" has "([^"]+)" (-?[0-9]+)$"#)]
fn json_entry_number(world: &mut BotSpyWorld, name: String, key: String, expected: String) {
    let entries = json_entries(world);
    let entry = entry_named(&entries, &name);
    let expected: i64 = expected.parse().expect("numeric expectation");
    let actual = entry
        .get(&key)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("missing or non-numeric {key} in {name}: {entry:?}"));
    assert_eq!(actual, expected, "unexpected {key} of {name}");
}

fn session_entry<'a>(entries: &'a [Value], id_prefix: &str) -> &'a Value {
    entries
        .iter()
        .find(|entry| {
            entry
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| id.starts_with(id_prefix))
        })
        .unwrap_or_else(|| panic!("no JSON session with id prefix {id_prefix}: {entries:?}"))
}

#[then(regex = r#"^the JSON session "([^"]+)" has "([^"]+)" "(.*)"$"#)]
fn json_session_string(world: &mut BotSpyWorld, id: String, key: String, expected: String) {
    let entries = json_entries(world);
    let entry = session_entry(&entries, &id);
    let actual = entry
        .get(&key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("missing or non-string {key} in session {id}: {entry:?}"));
    assert_eq!(
        actual,
        expand_with(world, &expected),
        "unexpected {key} of session {id}"
    );
}

#[then(regex = r#"^the JSON session "([^"]+)" has "([^"]+)" (-?[0-9]+)$"#)]
fn json_session_number(world: &mut BotSpyWorld, id: String, key: String, expected: String) {
    let entries = json_entries(world);
    let entry = session_entry(&entries, &id);
    let expected: i64 = expected.parse().expect("numeric expectation");
    let actual = entry
        .get(&key)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("missing or non-numeric {key} in session {id}: {entry:?}"));
    assert_eq!(actual, expected, "unexpected {key} of session {id}");
}

#[then(regex = r#"^the JSON sessions are ordered "(.*)"$"#)]
fn json_sessions_ordered(world: &mut BotSpyWorld, expected: String) {
    let entries = json_entries(world);
    let ids: Vec<&str> = entries
        .iter()
        .filter_map(|entry| entry.get("id").and_then(Value::as_str))
        .collect();
    let expected: Vec<&str> = expected.split(',').map(str::trim).collect();
    assert_eq!(
        ids.len(),
        expected.len(),
        "unexpected session count; ids: {ids:?}, expected: {expected:?}"
    );
    for (actual, prefix) in ids.iter().zip(expected.iter()) {
        assert!(
            actual.starts_with(prefix),
            "order mismatch: expected {prefix}… got {actual}; ids: {ids:?}"
        );
    }
}

#[then(expr = "the sessions table aligns the LAST ACTIVITY column")]
fn sessions_table_aligned(world: &mut BotSpyWorld) {
    let stdout = &run(world).stdout;
    let mut lines: Vec<&str> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert!(
        lines.len() >= 2,
        "table lacks header and footer: {stdout:?}"
    );
    let footer = lines.pop().expect("footer line");
    assert!(
        footer.contains("session"),
        "last line is not the count footer: {footer:?}"
    );
    let header = lines[0];
    let start = header
        .find("LAST ACTIVITY")
        .unwrap_or_else(|| panic!("no LAST ACTIVITY header: {header:?}"));
    for line in &lines[1..] {
        let chars: Vec<char> = line.chars().collect();
        assert!(
            chars.len() > start,
            "row ends before the activity column ({start}): {line:?}"
        );
        assert!(
            chars[start - 1] == ' ' && chars[start - 2] == ' ',
            "activity column gutter missing at {start}: {line:?}"
        );
        assert!(
            chars[start] != ' ',
            "activity column does not start at {start}: {line:?}"
        );
    }
    assert!(
        lines.len() > 1,
        "no table rows under the header: {stdout:?}"
    );
}

#[then(regex = r#"^every JSON entry has a non-empty "([^"]+)"$"#)]
fn every_entry_non_empty(world: &mut BotSpyWorld, key: String) {
    for entry in json_entries(world) {
        let value = entry
            .get(&key)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("missing or non-string {key}: {entry:?}"));
        assert!(!value.is_empty(), "empty {key}: {entry:?}");
    }
}

#[then(regex = r#"^every JSON entry has "([^"]+)" "([^"]+)"$"#)]
fn every_entry_equals(world: &mut BotSpyWorld, key: String, expected: String) {
    for entry in json_entries(world) {
        let actual = entry
            .get(&key)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("missing or non-string {key}: {entry:?}"));
        assert_eq!(actual, expected, "unexpected {key}: {entry:?}");
    }
}

#[then(regex = r#"^every JSON entry has "([^"]+)", "([^"]+)" and "([^"]+)"$"#)]
fn every_entry_has_keys(world: &mut BotSpyWorld, first: String, second: String, third: String) {
    for entry in json_entries(world) {
        for key in [&first, &second, &third] {
            assert!(entry.get(key).is_some(), "missing {key}: {entry:?}");
        }
    }
}

#[then(regex = r#"^every JSON entry has an "([^"]+)" list$"#)]
fn every_entry_has_list(world: &mut BotSpyWorld, key: String) {
    for entry in json_entries(world) {
        assert!(
            entry.get(&key).is_some_and(Value::is_array),
            "missing {key} list: {entry:?}"
        );
    }
}

#[then(expr = "no JSON entry has any issues")]
fn no_entry_has_issues(world: &mut BotSpyWorld) {
    for entry in json_entries(world) {
        let issues = entry
            .get("issues")
            .and_then(Value::as_array)
            .expect("issues list");
        assert!(
            issues.is_empty(),
            "unexpected issues: {issues:?} in {entry:?}"
        );
    }
}

#[then(regex = r#"^the JSON "([^"]+)" list has ([0-9]+) entries$"#)]
fn json_list_length(world: &mut BotSpyWorld, key: String, count: String) {
    let value = json(world);
    let list = value
        .get(&key)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("missing {key} list: {value:?}"));
    assert_eq!(list.len(), count.parse::<usize>().expect("numeric count"));
}

#[then(regex = r#"^the JSON "([^"]+)" is (true|false)$"#)]
fn json_bool(world: &mut BotSpyWorld, key: String, expected: String) {
    let value = json(world);
    let actual = value
        .get(&key)
        .and_then(Value::as_bool)
        .unwrap_or_else(|| panic!("missing or non-bool {key}: {value:?}"));
    assert_eq!(actual, expected == "true", "unexpected {key}");
}

#[then(regex = r#"^the JSON "([^"]+)" is more than ([0-9]+)$"#)]
fn json_number_above(world: &mut BotSpyWorld, key: String, bound: String) {
    let value = json(world);
    let actual = value
        .get(&key)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("missing or non-numeric {key}: {value:?}"));
    assert!(
        actual > bound.parse::<i64>().expect("numeric bound"),
        "{key} is not more than {bound}: {actual}"
    );
}

fn line_after(world: &BotSpyWorld, label: &str) -> String {
    let prefix = format!("{label} ");
    run(world)
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .unwrap_or_else(|| panic!("no line after {label:?} in {:?}", run(world).stdout))
        .trim()
        .to_string()
}

#[then(regex = r#"^the path after "([^"]+)" exists$"#)]
fn path_after_exists(world: &mut BotSpyWorld, label: String) {
    let path = line_after(world, &label);
    assert!(
        Path::new(&path).exists(),
        "path after {label:?} does not exist: {path}"
    );
}

#[then(regex = r#"^the number after "([^"]+)" is more than ([0-9]+)$"#)]
fn number_after_above(world: &mut BotSpyWorld, label: String, bound: String) {
    let text = line_after(world, &label);
    let actual: i64 = text
        .split_whitespace()
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no number after {label:?}: {text:?}"));
    assert!(
        actual > bound.parse::<i64>().expect("numeric bound"),
        "number after {label:?} is not more than {bound}: {actual}"
    );
}

#[then(expr = "stdout contains the full claude projects path")]
fn stdout_has_full_claude_path(world: &mut BotSpyWorld) {
    let full = full_claude_projects_path(world);
    assert!(
        run(world).stdout.contains(&full),
        "stdout does not contain {full:?}; actual: {:?}",
        run(world).stdout
    );
}

#[then(expr = "stdout does not contain the full claude projects path")]
fn stdout_lacks_full_claude_path(world: &mut BotSpyWorld) {
    let full = full_claude_projects_path(world);
    assert!(
        !run(world).stdout.contains(&full),
        "stdout unexpectedly contains {full:?}"
    );
}

#[when(regex = r#"^I run "botspy (.*?)" against an empty home$"#)]
fn run_against_empty_home(world: &mut BotSpyWorld, args: String) {
    world.cli_home = Some(scratch("empty-home"));
    run_cli(world, &args, true);
}

/// Import the fixture home into a store the scenario's steps share: the
/// store lives under the scenario's `{out_dir}`, and the search/stats
/// steps address it through `{out_dir}` too. `--no-embed` keeps the
/// default background fast and model-independent; the embedded variant
/// exercises the shipped MiniLM embedder.
#[given(regex = r#"^the fixture home is imported into the text-only store$"#)]
fn background_import_text_only(world: &mut BotSpyWorld) {
    background_import(world, false, "store.db");
}

#[given(regex = r#"^the fixture home is imported into the store with embeddings$"#)]
fn background_import_embedded(world: &mut BotSpyWorld) {
    background_import(world, true, "embedded.db");
}

fn background_import(world: &mut BotSpyWorld, embed: bool, name: &str) {
    let home = world.cli_home.clone().expect("fixture home");
    let db = out_dir(world).join(name);
    let mut argv: Vec<String> = vec![
        "botspy".to_string(),
        "import".to_string(),
        "--db".to_string(),
        db.display().to_string(),
    ];
    if !embed {
        argv.push("--no-embed".to_string());
    }
    argv.push("--home".to_string());
    argv.push(home.display().to_string());
    let outcome = cli::run_from(argv);
    assert_eq!(
        outcome.code, 0,
        "background import failed: {:?}",
        outcome.stderr
    );
    world.cli_store = Some(db);
}

#[when(regex = r#"^I search the imported store for "(.*)"$"#)]
fn run_search_text(world: &mut BotSpyWorld, query: String) {
    run_search(world, &query, &[]);
}

#[when(regex = r#"^I search the imported store for "(.*)" with JSON output$"#)]
fn run_search_text_json(world: &mut BotSpyWorld, query: String) {
    run_search(world, &query, &["-o", "json"]);
}

#[when(regex = r#"^I search the imported store semantically for "(.*)"$"#)]
fn run_search_semantic(world: &mut BotSpyWorld, query: String) {
    run_search(world, &query, &["--mode", "semantic"]);
}

#[when(regex = r#"^I search the imported store in hybrid mode for "(.*)"$"#)]
fn run_search_hybrid(world: &mut BotSpyWorld, query: String) {
    run_search(world, &query, &["--mode", "hybrid"]);
}

/// The background-imported store backs every search step; `--no-truncate`
/// keeps session ids assertable in human output.
fn run_search(world: &mut BotSpyWorld, query: &str, flags: &[&str]) {
    let db = world.cli_store.clone().expect("imported store");
    let mut argv: Vec<String> = vec![
        "botspy".to_string(),
        "search".to_string(),
        query.to_string(),
        "--db".to_string(),
        db.display().to_string(),
        "--no-truncate".to_string(),
    ];
    argv.extend(flags.iter().map(|flag| flag.to_string()));
    if let Some(home) = world.cli_home.clone() {
        argv.push("--home".to_string());
        argv.push(home.display().to_string());
    }
    world.cli_run = Some(cli::run_from(argv));
}

#[when(expr = "the fixture home's solo transcript gains a message")]
fn solo_transcript_gains_message(world: &mut BotSpyWorld) {
    let path = solo_transcript_path(world);
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("open solo transcript for append");
    writeln!(
        file,
        r#"{{"type":"user","uuid":"u3","timestamp":"2026-09-15T12:10:00Z","message":{{"role":"user","content":"one more thing"}}}}"#
    )
    .expect("append to solo transcript");
}

#[when(expr = "the fixture home's solo transcript is deleted")]
fn solo_transcript_deleted(world: &mut BotSpyWorld) {
    std::fs::remove_file(solo_transcript_path(world)).expect("delete solo transcript");
}

fn solo_transcript_path(world: &BotSpyWorld) -> PathBuf {
    world
        .cli_home
        .as_ref()
        .expect("fixture home")
        .join(".claude/projects/extra-demo/solo-abc123.jsonl")
}

#[then(regex = r#"^the file at "(.+)" exists$"#)]
fn file_at_exists(world: &mut BotSpyWorld, path: String) {
    let path = expand_with(world, &path);
    assert!(Path::new(&path).is_file(), "no file at {path}");
}

#[then(regex = r#"^the file at "(.+)" does not exist$"#)]
fn file_at_missing(world: &mut BotSpyWorld, path: String) {
    let path = expand_with(world, &path);
    assert!(!Path::new(&path).exists(), "unexpected file at {path}");
}

#[then(regex = r#"^the first JSON hit is for session "([^"]+)"$"#)]
fn first_json_hit_session(world: &mut BotSpyWorld, prefix: String) {
    let entries = json_entries(world);
    let first = entries.first().expect("at least one hit");
    let id = first
        .get("session_id")
        .and_then(Value::as_str)
        .expect("hit carries session_id");
    assert!(
        id.starts_with(&prefix),
        "first hit {id} does not start with {prefix}"
    );
}

// 60_scan.feature — the scan verb (BOTSPY-8ef5a5): the store it wrote
// and the per-source report it rendered.

#[given(regex = r#"^a non-store file at "([^"]+)"$"#)]
fn non_store_file(world: &mut BotSpyWorld, path: String) {
    let path = expand_with(world, &path);
    if let Some(parent) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(parent).expect("create parent directory");
    }
    std::fs::write(path, "this is not a sqlite database").expect("write the non-store file");
}

fn scan_entry<'a>(value: &'a Value, source: &str) -> &'a Value {
    value
        .get("adapters")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no adapters list: {value:?}"))
        .iter()
        .find(|entry| entry.get("source").and_then(Value::as_str) == Some(source))
        .unwrap_or_else(|| panic!("no scan source {source}: {value:?}"))
}

#[then(regex = r#"^the JSON scan source "([^"]+)" has "([^"]+)" (-?[0-9]+)$"#)]
fn scan_source_number(world: &mut BotSpyWorld, source: String, key: String, expected: String) {
    let value = json(world);
    let entry = scan_entry(&value, &source);
    let actual = entry
        .get(&key)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("missing or non-numeric {key} of {source}: {entry:?}"));
    assert_eq!(
        actual,
        expected.parse::<i64>().expect("numeric expectation")
    );
}

#[then(regex = r#"^the JSON scan source "([^"]+)" has "([^"]+)" ([0-9]+) and "([^"]+)" ([0-9]+)$"#)]
fn scan_source_two_numbers(
    world: &mut BotSpyWorld,
    source: String,
    first_key: String,
    first_expected: String,
    second_key: String,
    second_expected: String,
) {
    scan_source_number(world, source.clone(), first_key, first_expected);
    scan_source_number(world, source, second_key, second_expected);
}

#[then(regex = r#"^the JSON totals report "([^"]+)" (-?[0-9]+)$"#)]
fn scan_totals_number(world: &mut BotSpyWorld, key: String, expected: String) {
    let value = json(world);
    let totals = value
        .get("totals")
        .unwrap_or_else(|| panic!("no totals in the scan report: {value:?}"));
    let actual = totals
        .get(&key)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("missing or non-numeric totals {key}: {totals:?}"));
    assert_eq!(
        actual,
        expected.parse::<i64>().expect("numeric expectation")
    );
}

fn sessions_in_store(path: &str) -> usize {
    let store = botspy::store::Store::open(path)
        .unwrap_or_else(|err| panic!("the store at {path} opens: {err}"));
    store.sessions().count()
}

#[then(regex = r#"^the store at "([^"]+)" holds ([0-9]+) sessions$"#)]
fn store_at_holds(world: &mut BotSpyWorld, path: String, count: String) {
    let path = expand_with(world, &path);
    assert_eq!(
        sessions_in_store(&path),
        count.parse::<usize>().expect("numeric count"),
        "the store at {path} holds a different number of sessions"
    );
}

#[then(regex = r#"^the fixture store holds ([0-9]+) sessions$"#)]
fn fixture_store_holds(world: &mut BotSpyWorld, count: String) {
    let home = world
        .cli_home
        .as_ref()
        .expect("fixture home")
        .join(".botspy/store.db");
    assert_eq!(
        sessions_in_store(&home.display().to_string()),
        count.parse::<usize>().expect("numeric count"),
        "the fixture store holds a different number of sessions"
    );
}

// The scan loop: one pass to start, then re-scans on the interval until
// an interrupt. Tests drive the loop through an injected scripted
// ticker — virtual time, bounded passes, no real sleeps.

struct ScriptedTicker {
    now_ms: u64,
    script: std::collections::VecDeque<cli::scan::Wake>,
}

impl cli::scan::ScanTicker for ScriptedTicker {
    fn now_ms(&self) -> u64 {
        self.now_ms
    }

    fn wait_until(&mut self, deadline_ms: u64) -> cli::scan::Wake {
        self.now_ms = deadline_ms;
        self.script
            .pop_front()
            .unwrap_or(cli::scan::Wake::Completed)
    }

    fn stop_requested(&self) -> bool {
        false
    }
}

fn run_scan_loop(world: &mut BotSpyWorld, args: &str, ticks: usize, interrupt_after: bool) {
    let mut argv: Vec<String> = vec!["botspy".to_string(), "scan".to_string()];
    argv.extend(
        expand_with(world, args)
            .split_whitespace()
            .map(str::to_string),
    );
    let mut script: std::collections::VecDeque<cli::scan::Wake> =
        (0..ticks).map(|_| cli::scan::Wake::Tick).collect();
    if interrupt_after {
        script.push_back(cli::scan::Wake::Interrupted);
    }
    let mut ticker = ScriptedTicker { now_ms: 0, script };
    let home = world.cli_home.clone().expect("fixture home");
    std::env::set_var("BOTSPY_HOME", &home);
    world.cli_run = Some(cli::scan::scan_with_ticker(argv, &mut ticker));
    std::env::remove_var("BOTSPY_HOME");
}

#[when(
    regex = r#"^I scan "botspy scan (.*)" in a loop for ([0-9]+) ticks? with BOTSPY_HOME at the fixture home$"#
)]
fn scan_loop_ticks(world: &mut BotSpyWorld, args: String, ticks: usize) {
    run_scan_loop(world, &args, ticks, false);
}

#[when(
    regex = r#"^I scan "botspy scan (.*)" in a loop for ([0-9]+) ticks? and then the interrupt lands with BOTSPY_HOME at the fixture home$"#
)]
fn scan_loop_ticks_interrupt(world: &mut BotSpyWorld, args: String, ticks: usize) {
    run_scan_loop(world, &args, ticks, true);
}

#[when(
    regex = r#"^I scan "botspy scan (.*)" and the interrupt lands during the first pass with BOTSPY_HOME at the fixture home$"#
)]
fn scan_loop_interrupt_first(world: &mut BotSpyWorld, args: String) {
    run_scan_loop(world, &args, 0, true);
}

#[then(regex = r#"^stdout reports the passes (.*) in order$"#)]
fn passes_in_order(world: &mut BotSpyWorld, expected: String) {
    let wanted: Vec<u64> = expected
        .replace(" and ", ", ")
        .split(',')
        .map(|part| part.trim().parse().expect("pass number"))
        .collect();
    let reported: Vec<u64> = run(world)
        .stdout
        .lines()
        .filter_map(|line| line.strip_prefix("pass "))
        .filter_map(|rest| rest.parse::<u64>().ok())
        .collect();
    assert_eq!(
        reported, wanted,
        "the loop reported different passes: {reported:?}"
    );
}

#[then(regex = r#"^stdout parses as ([0-9]+) JSON documents$"#)]
fn json_documents(world: &mut BotSpyWorld, count: usize) {
    let stdout = &run(world).stdout;
    let docs: Vec<Value> = serde_json::Deserializer::from_str(stdout)
        .into_iter()
        .collect::<Result<_, _>>()
        .expect("stdout parses as concatenated JSON documents");
    assert_eq!(
        docs.len(),
        count,
        "stdout holds different documents: {stdout:?}"
    );
}

#[then(regex = r#"^JSON document ([0-9]+) has "([^"]+)" (-?[0-9]+)$"#)]
fn json_document_number(world: &mut BotSpyWorld, index: usize, key: String, expected: String) {
    let stdout = &run(world).stdout;
    let docs: Vec<Value> = serde_json::Deserializer::from_str(stdout)
        .into_iter()
        .collect::<Result<_, _>>()
        .expect("stdout parses as concatenated JSON documents");
    let document = docs
        .get(index - 1)
        .unwrap_or_else(|| panic!("no document {index}: {stdout:?}"));
    let actual = document
        .get(&key)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("missing or non-numeric {key}: {document:?}"));
    assert_eq!(
        actual,
        expected.parse::<i64>().expect("numeric expectation")
    );
}
