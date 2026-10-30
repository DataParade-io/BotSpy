//! Steps for the privacy example (spec 04_examples/privacy.feature).
//!
//! The example's own source is compiled into this test crate via #[path],
//! so the scenarios run its real `run` entry point in-process. LeftBlock is
//! replaced by the stub in `leftblock_stub` (the test executable itself,
//! hard-linked under another name), except in the `@real-leftblock`
//! scenario, which uses whatever `leftblock` is on PATH.

use crate::steps::{leftblock_stub, BotSpyWorld};
use cucumber::{given, then, when};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[path = "../../examples/privacy/main.rs"]
#[allow(dead_code)]
pub mod privacy_example;

use privacy_example::{common, meter_scoring, planted, scan, sentiment_scoring};

static UNIQUE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Default)]
pub struct PrivacyState {
    root: Option<PathBuf>,
    transcript: Option<PathBuf>,
    /// The scanner to run: `None` means `leftblock` from PATH.
    command: Option<OsString>,
    stub: Option<PathBuf>,
    values: Vec<String>,
    scratch: Vec<PathBuf>,
    exit: Option<i32>,
    elapsed: Duration,
    stdout: String,
    stderr: String,
}

impl Drop for PrivacyState {
    fn drop(&mut self) {
        for dir in &self.scratch {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// Whether a `leftblock` on PATH speaks `scan --jsonl` (older releases have
/// no `scan` command and exit 2 on it).
pub fn real_leftblock_available() -> bool {
    std::process::Command::new("leftblock")
        .args(["scan", "--help"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn scratch_dir(state: &mut PrivacyState) -> PathBuf {
    let dir = planted::private_temp_dir("privacy-test").expect("scratch dir");
    state.scratch.push(dir.clone());
    dir
}

fn seed() -> u64 {
    UNIQUE
        .fetch_add(1, Ordering::SeqCst)
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ u64::from(std::process::id())
}

/// Write a Claude Code-style transcript holding `texts` (one message each)
/// under a fresh root and make it the scenario's corpus.
fn write_corpus(state: &mut PrivacyState, texts: &[String]) {
    let root = scratch_dir(state);
    let project = root.join("claude_code/projects/-privacy-test");
    std::fs::create_dir_all(&project).expect("project dir");
    let transcript = project.join(format!("{}.jsonl", planted::SESSION_ID));
    let mut body = String::new();
    for (index, text) in texts.iter().enumerate() {
        let record = serde_json::json!({
            "type": "user",
            "uuid": format!("m{index}"),
            "parentUuid": "",
            "timestamp": format!("2026-10-01T09:{:02}:{:02}Z", (index / 60) % 60, index % 60),
            "message": {"role": "user", "content": text},
        });
        body.push_str(&format!("{record}\n"));
    }
    std::fs::write(&transcript, body).expect("write transcript");
    state.root = Some(root);
    state.transcript = Some(transcript);
}

fn expand(state: &mut PrivacyState, text: &str) -> String {
    let mut text = text.to_string();
    while let Some(at) = text.find("{aws-key}") {
        let value = planted::fake_aws_key(seed());
        text.replace_range(at..at + "{aws-key}".len(), &value);
        state.values.push(value);
    }
    text
}

fn expect(state: &PrivacyState, text: &str) -> String {
    match &state.transcript {
        Some(transcript) => text.replace("{transcript}", &transcript.display().to_string()),
        None => text.to_string(),
    }
}

#[given(regex = r#"^a planted corpus of fake secrets and personal data$"#)]
fn planted_corpus(world: &mut BotSpyWorld) {
    let state = &mut world.privacy;
    let planted = planted::plant().expect("plant");
    state.scratch.push(planted.root.clone());
    state.root = Some(planted.root);
    state.values = planted.values;
    state.transcript = Some(planted.transcript);
}

#[given(regex = r#"^a corpus with the message "(.*)"$"#)]
fn corpus_one(world: &mut BotSpyWorld, text: String) {
    let text = expand(&mut world.privacy, &text);
    write_corpus(&mut world.privacy, &[text]);
}

#[given(regex = r#"^a corpus with the messages "(.*)" and "(.*)"$"#)]
fn corpus_two(world: &mut BotSpyWorld, first: String, second: String) {
    let first = expand(&mut world.privacy, &first);
    let second = expand(&mut world.privacy, &second);
    write_corpus(&mut world.privacy, &[first, second]);
}

#[given(
    regex = r#"^a corpus of ([0-9]+) messages of ([0-9]+) bytes each carrying one planted key$"#
)]
fn corpus_many(world: &mut BotSpyWorld, count: usize, size: usize) {
    let texts: Vec<String> = (0..count)
        .map(|_| {
            let key = planted::fake_aws_key(seed());
            let filler = "x".repeat(size.saturating_sub(key.len() + 1));
            format!("{filler} {key}")
        })
        .collect();
    write_corpus(&mut world.privacy, &texts);
}

fn install_stub(state: &mut PrivacyState, mode: &str) {
    let dir = scratch_dir(state);
    let stub = leftblock_stub::install(&dir, mode);
    state.command = Some(stub.clone().into_os_string());
    state.stub = Some(stub);
}

#[given(regex = r#"^a stub LeftBlock scanner$"#)]
fn stub_ok(world: &mut BotSpyWorld) {
    install_stub(&mut world.privacy, "ok");
}

#[given(regex = r#"^a stub LeftBlock scanner that fails$"#)]
fn stub_fails(world: &mut BotSpyWorld) {
    install_stub(&mut world.privacy, "fail");
}

#[given(regex = r#"^a stub LeftBlock scanner that reports a range past the end$"#)]
fn stub_bad_range(world: &mut BotSpyWorld) {
    install_stub(&mut world.privacy, "badrange");
}

#[given(regex = r#"^no LeftBlock executable$"#)]
fn no_leftblock(world: &mut BotSpyWorld) {
    let dir = scratch_dir(&mut world.privacy);
    world.privacy.command = Some(dir.join("no-such-leftblock").into_os_string());
}

fn run_example(state: &mut PrivacyState, args: &str, command: Option<OsString>) {
    let mut argv: Vec<String> = args.split_whitespace().map(str::to_string).collect();
    if !argv.iter().any(|arg| arg == "--plant") {
        if let Some(root) = &state.root {
            argv.push(root.display().to_string());
        }
    }
    let (sender, receiver) = mpsc::channel();
    let started = Instant::now();
    std::thread::spawn(move || {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let status = privacy_example::run(&argv, command, &mut out, &mut err);
        let _ = sender.send((
            status,
            String::from_utf8_lossy(&out).into_owned(),
            String::from_utf8_lossy(&err).into_owned(),
        ));
    });
    // A client that deadlocks against the streaming scanner hangs here, so
    // the wait is bounded.
    match receiver.recv_timeout(Duration::from_secs(120)) {
        Ok((status, out, err)) => {
            state.exit = Some(status);
            state.stdout = out;
            state.stderr = err;
        }
        Err(_) => {
            state.exit = None;
            state.stdout.clear();
            state.stderr = "timed out".to_string();
        }
    }
    state.elapsed = started.elapsed();
}

#[when(regex = r#"^I run the privacy example with "(.*)"$"#)]
fn run_privacy(world: &mut BotSpyWorld, args: String) {
    let command = world.privacy.command.clone();
    run_example(&mut world.privacy, &args, command);
}

#[when(regex = r#"^I run the privacy example with "(.*)" against the real LeftBlock$"#)]
fn run_privacy_real(world: &mut BotSpyWorld, args: String) {
    run_example(&mut world.privacy, &args, None);
}

#[then(regex = r#"^it exits with status ([0-9]+)$"#)]
fn exits_with(world: &mut BotSpyWorld, status: i32) {
    let state = &world.privacy;
    assert_eq!(
        state.exit,
        Some(status),
        "stdout: {}\nstderr: {}",
        state.stdout,
        state.stderr
    );
}

#[then(regex = r#"^it finishes within ([0-9]+) seconds with status ([0-9]+)$"#)]
fn finishes_within(world: &mut BotSpyWorld, seconds: u64, status: i32) {
    assert!(
        world.privacy.elapsed < Duration::from_secs(seconds),
        "took {:?}",
        world.privacy.elapsed
    );
    exits_with(world, status);
}

#[then(regex = r#"^output contains "(.*)"$"#)]
fn stdout_contains(world: &mut BotSpyWorld, needle: String) {
    let needle = expect(&world.privacy, &needle);
    assert!(
        world.privacy.stdout.contains(&needle),
        "stdout lacks {needle:?}:\n{}",
        world.privacy.stdout
    );
}

#[then(regex = r#"^output does not contain "(.*)"$"#)]
fn stdout_lacks(world: &mut BotSpyWorld, needle: String) {
    assert!(
        !world.privacy.stdout.contains(&needle),
        "stdout has {needle:?}:\n{}",
        world.privacy.stdout
    );
}

#[then(regex = r#"^output never contains a planted value$"#)]
fn no_planted_values(world: &mut BotSpyWorld) {
    let state = &world.privacy;
    assert!(!state.values.is_empty(), "no planted values to check");
    for value in &state.values {
        assert!(
            !state.stdout.contains(value.as_str()) && !state.stderr.contains(value.as_str()),
            "a planted value leaked into the output"
        );
    }
}

#[then(regex = r#"^output is empty$"#)]
fn stdout_empty(world: &mut BotSpyWorld) {
    assert_eq!(world.privacy.stdout, "");
}

#[then(regex = r#"^error output is empty$"#)]
fn stderr_empty(world: &mut BotSpyWorld) {
    assert_eq!(world.privacy.stderr, "");
}

#[then(regex = r#"^error output is one line mentioning "(.*)"$"#)]
fn stderr_one_line(world: &mut BotSpyWorld, needle: String) {
    let stderr = world.privacy.stderr.trim_end();
    assert!(!stderr.contains('\n'), "more than one line: {stderr:?}");
    assert!(
        stderr.contains(&needle),
        "stderr lacks {needle:?}: {stderr:?}"
    );
}

fn stub_file(state: &PrivacyState, extension: &str) -> String {
    let stub = state.stub.as_ref().expect("no stub scanner");
    std::fs::read_to_string(leftblock_stub::sidecar(stub, extension)).expect("stub sidecar")
}

#[then(regex = r#"^the scanner was invoked with "(.*)"$"#)]
fn invoked_with(world: &mut BotSpyWorld, args: String) {
    assert_eq!(stub_file(&world.privacy, "args"), args);
}

#[then(regex = r#"^the scanner received only record ids shaped like s0/m0/p0$"#)]
fn opaque_ids(world: &mut BotSpyWorld) {
    let ids = stub_file(&world.privacy, "ids");
    assert!(ids.lines().count() >= 5, "too few records: {ids:?}");
    for id in ids.lines() {
        let slot = scan::Slot::parse(id).unwrap_or_else(|| panic!("id {id:?} is not an index"));
        assert_eq!(slot.id(), id);
    }
}

#[then(
    regex = r#"^scoring the unredacted corpus would have counted ([0-9]+) F-bomb and ([0-9]+) negative message$"#
)]
fn unredacted_control(world: &mut BotSpyWorld, f_bombs: u64, negative: u64) {
    let root = world.privacy.root.clone().expect("corpus root");
    let sessions = common::collect_sessions(&root);
    assert_eq!(meter_scoring::score_history(&sessions).f_bombs, f_bombs);
    assert_eq!(
        sentiment_scoring::score_history(&sessions).negative,
        negative
    );
}

#[then(regex = r#"^the planted demo corpus is gone$"#)]
fn demo_corpus_gone(world: &mut BotSpyWorld) {
    let line = world
        .privacy
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("Planted a demo corpus under "))
        .expect("the run announced its demo corpus");
    assert!(!Path::new(line).exists(), "{line} was left behind");
}

#[then(regex = r#"^output reports at least ([0-9]+) findings?$"#)]
fn at_least_findings(world: &mut BotSpyWorld, minimum: usize) {
    let total: usize = world
        .privacy
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("Totals: "))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|count| count.parse().ok())
        .unwrap_or(0);
    assert!(total >= minimum, "stdout:\n{}", world.privacy.stdout);
}
