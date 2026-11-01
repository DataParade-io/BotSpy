//! Steps for the example consumers (spec 04_examples/consumers.feature).

use crate::steps::BotSpyWorld;
use botspy::{KnownPart, Message, Part, Role, Session};
use cucumber::{given, then, when};

// The meter scoring module is the example's own source, compiled into this
// test crate via #[path]: the scenarios below therefore exercise the real
// example logic, and the compile itself proves it only touches the public
// botspy API.
#[path = "../../examples/meter/scoring.rs"]
mod meter_scoring;

#[path = "../../examples/sentiment/scoring.rs"]
mod sentiment_scoring;

pub use meter_scoring::MeterReport;
pub use sentiment_scoring::SentimentReport;

fn text_session(id: &str, texts: &[&str]) -> Session {
    Session {
        id: id.to_string(),
        agent: botspy::Agent::ClaudeCode,
        project_id: "examples".to_string(),
        started_at: "2026-10-01T09:00:00Z".to_string(),
        last_activity_at: "2026-10-01T09:00:00Z".to_string(),
        messages: texts
            .iter()
            .map(|text| Message {
                role: Role::User,
                parts: vec![Part::Known(KnownPart::Text {
                    text: (*text).to_string(),
                    extra: None,
                })],
                ..Message::default()
            })
            .collect(),
        ..Session::default()
    }
}

#[given(regex = r#"^a session with messages "([^"]+)", "([^"]+)", and "([^"]+)"$"#)]
fn session_with_three_messages(
    world: &mut BotSpyWorld,
    first: String,
    second: String,
    third: String,
) {
    world.example_sessions = vec![text_session("example-1", &[&first, &second, &third])];
}

#[given(regex = r#"^sessions "([^"]+)" with ([0-9]+) thanks and "([^"]+)" with ([0-9]+) F-bombs$"#)]
fn sessions_with_counts(
    world: &mut BotSpyWorld,
    first: String,
    thanks: usize,
    second: String,
    f_bombs: usize,
) {
    let thanks_texts: Vec<String> = (0..thanks)
        .map(|_| "thanks, that helped".to_string())
        .collect();
    let bomb_texts: Vec<String> = (0..f_bombs)
        .map(|_| "what the f*** is this".to_string())
        .collect();
    let thanks_refs: Vec<&str> = thanks_texts.iter().map(String::as_str).collect();
    let bomb_refs: Vec<&str> = bomb_texts.iter().map(String::as_str).collect();
    world.example_sessions = vec![
        text_session(&first, &thanks_refs),
        text_session(&second, &bomb_refs),
    ];
}

/// One-session fixture the "only you" scenarios fill in.
fn one_session(world: &mut BotSpyWorld) -> &mut Session {
    if world.example_sessions.is_empty() {
        world.example_sessions = vec![text_session("example-1", &[])];
    }
    world.example_sessions.first_mut().expect("a session")
}

fn message_as(role: Role, text: &str) -> Message {
    Message {
        role,
        parts: vec![Part::Known(KnownPart::Text {
            text: text.to_string(),
            extra: None,
        })],
        ..Message::default()
    }
}

#[given(regex = r#"^a session where I typed "([^"]+)" and the agent replied "([^"]+)"$"#)]
fn typed_and_replied(world: &mut BotSpyWorld, typed: String, replied: String) {
    let session = one_session(world);
    session.messages.push(message_as(Role::User, &typed));
    session.messages.push(message_as(Role::Assistant, &replied));
}

#[given(regex = r#"^a session where a tool result "([^"]+)" came back under the user role$"#)]
fn tool_result_under_user(world: &mut BotSpyWorld, text: String) {
    one_session(world).messages.push(Message {
        role: Role::User,
        parts: vec![Part::Known(KnownPart::ToolResult {
            call_id: "call-1".to_string(),
            text: Some(text),
            status: None,
            extra: None,
        })],
        ..Message::default()
    });
}

#[given(regex = r#"^a session where tooling injected the user-role text "([^"]+)"$"#)]
fn tooling_injected(world: &mut BotSpyWorld, text: String) {
    let mut message = message_as(Role::User, &text);
    message.origin = Some(botspy::Origin::Simulated);
    one_session(world).messages.push(message);
}

#[given(
    regex = r#"^the same session holds the compaction summary "([^"]+)" stored under the user role$"#
)]
fn compaction_under_user(world: &mut BotSpyWorld, text: String) {
    let mut message = message_as(Role::User, &text);
    message.is_compaction_summary = true;
    one_session(world).messages.push(message);
}

#[given(regex = r#"^a sub-agent session where the parent agent prompted "([^"]+)"$"#)]
fn subagent_prompted(world: &mut BotSpyWorld, text: String) {
    let session = one_session(world);
    session.parent_id = Some("parent-1".to_string());
    session.subagent = Some(botspy::SubagentInfo {
        kind: "explore".to_string(),
        nesting_depth: None,
    });
    session.messages.push(message_as(Role::User, &text));
}

#[given(regex = r#"^a message of mine with the text parts "([^"]+)" and "([^"]+)"$"#)]
fn message_with_two_parts(world: &mut BotSpyWorld, first: String, second: String) {
    let mut message = message_as(Role::User, &first);
    message.parts.push(Part::Known(KnownPart::Text {
        text: second,
        extra: None,
    }));
    one_session(world).messages.push(message);
}

#[given(regex = r#"^the unified history with ([0-9]+) fixture sessions$"#)]
fn unified_history_with_sessions(world: &mut BotSpyWorld, count: usize) {
    world.example_sessions = (0..count)
        .map(|index| {
            let texts = ["thanks, that unblocked me", "the corpus sweep is done"];
            text_session(&format!("example-{index}"), &texts)
        })
        .collect();
}

#[when(regex = r#"^the meter scores the session$"#)]
fn meter_scores_session(world: &mut BotSpyWorld) {
    let session = world.example_sessions.first().expect("no session to score");
    let (thanks, f_bombs) = meter_scoring::score_session(session);
    world.meter_report = Some(MeterReport {
        thanks,
        f_bombs,
        sessions: 1,
    });
}

#[when(regex = r#"^the meter scores the unified history$"#)]
fn meter_scores_history(world: &mut BotSpyWorld) {
    world.meter_report = Some(meter_scoring::score_history(&world.example_sessions));
}

#[when(regex = r#"^an example consumer is built against the published crate$"#)]
fn example_consumer_built(world: &mut BotSpyWorld) {
    // Read every example source: all of them must honor the consumer
    // constraints, not just one.
    let examples = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let mut files: Vec<std::path::PathBuf> = walk_sources(&examples);
    // raster.rs is the one file allowed an outside crate (resvg, a
    // dev-dependency, rasterizes the timeline PNG); timeline_steps.rs
    // checks it on its own.
    files.retain(|file| file.file_name().is_none_or(|name| name != "raster.rs"));
    files.sort();
    assert!(!files.is_empty(), "no example sources found");
    let mut sources = String::new();
    for file in files {
        sources.push_str(&std::fs::read_to_string(&file).expect("read example source"));
        sources.push('\n');
    }
    world.example_sources = Some(sources);
}

fn walk_sources(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(walk_sources(&path));
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            files.push(path);
        }
    }
    files
}

#[then(regex = r#"^it reports ([0-9]+) thanks and ([0-9]+) F-bombs?$"#)]
fn meter_reports_counts(world: &mut BotSpyWorld, thanks: u64, f_bombs: u64) {
    let report = world.meter_report.as_ref().expect("no meter report");
    assert_eq!(
        report.thanks, thanks,
        "the meter counted the wrong number of thanks"
    );
    assert_eq!(
        report.f_bombs, f_bombs,
        "the meter counted the wrong number of F-bombs"
    );
}

#[then(regex = r#"^it reports ([0-9]+) thanks and ([0-9]+) F-bombs? across ([0-9]+) sessions$"#)]
fn meter_reports_totals(world: &mut BotSpyWorld, thanks: u64, f_bombs: u64, sessions: u64) {
    meter_reports_counts(world, thanks, f_bombs);
    let report = world.meter_report.as_ref().expect("no meter report");
    assert_eq!(
        report.sessions, sessions,
        "the meter aggregated the wrong number of sessions"
    );
}

#[then(regex = r#"^the ratio favors thanks$"#)]
fn ratio_favors_thanks(world: &mut BotSpyWorld) {
    let report = world.meter_report.as_ref().expect("no meter report");
    assert!(
        meter_scoring::favors_thanks(report),
        "the ratio did not favor thanks: {report:?}"
    );
}

/// Import roots an example consumer may touch: the crate's public surface —
/// public modules plus crate-root type re-exports (`botspy::Session` and
/// friends, which always start with an uppercase letter).
fn is_public_root(import: &str) -> bool {
    let Some(rest) = import.strip_prefix("botspy::") else {
        return false;
    };
    rest.starts_with('{')
        || PUBLIC_MODULES.iter().any(|module| rest.starts_with(module))
        || rest
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_uppercase())
}

const PUBLIC_MODULES: &[&str] = &[
    "adapters", "adapter", "session", "schema", "importer", "snapshot",
];

const DEPENDENCY_FREE_ROOTS: &[&str] = &[
    "std::",
    "botspy",
    "crate::",
    "super::",
    "self::",
    "scoring::",
    "common::",
];

/// Substrings that would mean the examples reach for the network; sentiment
/// must score entirely in-process.
const NETWORK_MARKERS: &[&str] = &[
    "http",
    "reqwest",
    "ureq",
    "hyper",
    "curl",
    "socket",
    "tcpstream",
    "tokio",
    "async-std",
    "upload",
];

#[then(regex = r#"^it uses only the public API surface$"#)]
fn uses_only_public_api(world: &mut BotSpyWorld) {
    // The meter_scoring module above is compiled from the example's own
    // source, so the build itself is the proof; this check keeps botspy
    // import paths on the documented public roots as well.
    let sources = world.example_sources.as_ref().expect("no example sources");
    let imports = sources
        .lines()
        .map(str::trim_start)
        .filter_map(|line| line.strip_prefix("use ").map(str::trim_start));
    for import in imports.clone() {
        if import.starts_with("botspy::") {
            assert!(
                is_public_root(import),
                "example import {import:?} is not on the public API surface"
            );
        }
    }
    for import in imports {
        assert!(
            DEPENDENCY_FREE_ROOTS
                .iter()
                .any(|root| import.starts_with(root)),
            "example import {import:?} reaches outside botspy and the standard library"
        );
    }
}

#[then(regex = r#"^it adds no dependency to the core library$"#)]
fn adds_no_dependency(world: &mut BotSpyWorld) {
    let sources = world.example_sources.as_ref().expect("no example sources");
    for line in sources
        .lines()
        .map(str::trim_start)
        .filter_map(|line| line.strip_prefix("use ").map(str::trim_start))
    {
        assert!(
            DEPENDENCY_FREE_ROOTS
                .iter()
                .any(|root| line.starts_with(root)),
            "example import {line:?} reaches outside botspy and the standard library"
        );
    }
    let cargo_toml = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
    )
    .expect("read Cargo.toml");
    let dependencies = cargo_toml
        .split("[dependencies]")
        .nth(1)
        .and_then(|rest| rest.split('[').next())
        .unwrap_or_default();
    for extra in [
        "vader",
        "goemotions",
        "transformers",
        "candle",
        "tract",
        "ort",
    ] {
        assert!(
            !dependencies.contains(extra),
            "the core library picked up the example-only dependency {extra:?}"
        );
    }
}

#[when(regex = r#"^the sentiment example scores the session$"#)]
fn sentiment_scores_session(world: &mut BotSpyWorld) {
    let session = world.example_sessions.first().expect("no session to score");
    world.sentiment_report = Some(sentiment_scoring::score_session(session));
}

#[then(
    regex = r#"^it reports ([0-9]+) positive, ([0-9]+) negative, and ([0-9]+) neutral messages?$"#
)]
fn sentiment_reports_counts(world: &mut BotSpyWorld, positive: u64, negative: u64, neutral: u64) {
    let report = world
        .sentiment_report
        .as_ref()
        .expect("no sentiment report");
    assert_eq!(
        report.positive, positive,
        "the sentiment example counted the wrong positives"
    );
    assert_eq!(
        report.negative, negative,
        "the sentiment example counted the wrong negatives"
    );
    assert_eq!(
        report.neutral, neutral,
        "the sentiment example counted the wrong neutrals"
    );
}

#[then(regex = r#"^no message text leaves the machine$"#)]
fn no_message_text_leaves(world: &mut BotSpyWorld) {
    // Scoring is a pure in-process function over &str; the proof here is that
    // the example sources import nothing network-capable at all.
    if world.example_sources.is_none() {
        example_consumer_built(world);
    }
    let sources = world.example_sources.as_ref().expect("no example sources");
    let lowered = sources.to_lowercase();
    for marker in NETWORK_MARKERS {
        assert!(
            !lowered.contains(marker),
            "example sources look network-capable: {marker:?}"
        );
    }
}
