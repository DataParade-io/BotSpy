//! A synthetic parallel-sessions corpus for `--demo`: several projects,
//! overlapping sessions, idle breaks, a fork, spawned sub-agents, an orphan
//! whose parent is missing and a session with no message timestamps. Every
//! id, project and title is invented; nothing here comes from a real
//! machine.

use super::activity::{format_iso, parse_time, Ms};
use botspy::{Agent, ForkPoint, KnownPart, Message, Part, Role, Session, SubagentInfo};

const PREFIXES: [&str; 12] = [
    "3f9a1c07", "b82e5d44", "7c10e9a3", "e4d6208b", "19ab73cf", "a05f8e12", "6d3b94e0", "c7218fa5",
    "52e0b6d9", "f18c4a73", "0e47bd91", "9c3a60f8",
];

fn id(n: usize) -> String {
    format!("{}-0000-4000-8000-0000000000{n:02x}", PREFIXES[n % 12])
}

/// Message minutes: a run every `step` minutes with a little deterministic
/// jitter, from minute `from` to minute `to` after the demo's 09:00 start.
fn run(from: i64, to: i64, step: i64) -> Vec<i64> {
    let jitter = [0, 1, -1, 2, 0, 1];
    let mut minutes = vec![from];
    let mut at = from;
    let mut i = 0;
    while at < to {
        at = (at + (step + jitter[i % 6]).max(1)).min(to);
        minutes.push(at);
        i += 1;
    }
    minutes
}

struct Spec {
    n: usize,
    agent: Agent,
    project: &'static str,
    title: &'static str,
    minutes: Vec<i64>,
    parent: Option<String>,
    subagent: Option<&'static str>,
    fork: bool,
}

fn build(base: Ms, spec: Spec) -> Session {
    let at = |minute: i64| format_iso(base + minute * 60_000);
    let messages = spec
        .minutes
        .iter()
        .enumerate()
        .map(|(i, minute)| Message {
            role: if i % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            },
            parts: vec![Part::Known(KnownPart::Text {
                text: "synthetic message".to_string(),
                extra: None,
            })],
            timestamp: Some(at(*minute)),
            ..Message::default()
        })
        .collect();
    let mut session = Session {
        id: id(spec.n),
        agent: spec.agent,
        project_id: spec.project.to_string(),
        started_at: at(spec.minutes[0]),
        last_activity_at: at(*spec.minutes.last().expect("minutes")),
        messages,
        parent_id: spec.parent,
        subagent: spec.subagent.map(|kind| SubagentInfo {
            kind: kind.to_string(),
            nesting_depth: None,
        }),
        fork: spec.fork.then_some(ForkPoint { ordinal: 12 }),
        ..Session::default()
    };
    session.metadata.title = Some(spec.title.to_string());
    session
}

/// The demo corpus, as sessions.
pub fn sessions() -> Vec<Session> {
    let base = parse_time("2026-10-01T09:00:00Z").expect("demo base time");
    let spec = |n, agent, project, title, minutes| Spec {
        n,
        agent,
        project,
        title,
        minutes,
        parent: None,
        subagent: None,
        fork: false,
    };
    let join = |mut first: Vec<i64>, second: Vec<i64>| {
        first.extend(second);
        first
    };
    let mut list = vec![
        spec(
            1,
            Agent::ClaudeCode,
            "checkout-web",
            "Rework the checkout form",
            join(run(0, 60, 4), run(105, 150, 4)),
        ),
        spec(
            2,
            Agent::Cursor,
            "checkout-web",
            "Fix the cart badge",
            run(20, 95, 5),
        ),
        spec(
            3,
            Agent::Codex,
            "billing-api",
            "Add invoice export",
            join(run(10, 70, 4), run(120, 180, 5)),
        ),
        spec(
            4,
            Agent::ClaudeCode,
            "billing-api",
            "Tax rounding bug",
            run(120, 210, 5),
        ),
        spec(
            5,
            Agent::Antigravity,
            "data-pipeline",
            "Backfill the events table",
            run(30, 70, 3),
        ),
        spec(
            6,
            Agent::GrokBot,
            "data-pipeline",
            "Profile the nightly job",
            run(55, 130, 6),
        ),
    ];
    let child = |n, agent, project, title, minutes, parent: usize, kind, fork| Spec {
        parent: Some(id(parent)),
        subagent: kind,
        fork,
        ..Spec {
            n,
            agent,
            project,
            title,
            minutes,
            parent: None,
            subagent: None,
            fork: false,
        }
    };
    list.push(child(
        7,
        Agent::ClaudeCode,
        "checkout-web",
        "Survey form libraries",
        run(35, 55, 3),
        1,
        Some("explore"),
        false,
    ));
    list.push(child(
        8,
        Agent::ClaudeCode,
        "checkout-web",
        "Write form tests",
        run(112, 130, 3),
        1,
        Some("general"),
        false,
    ));
    list.push(child(
        9,
        Agent::Codex,
        "billing-api",
        "Try a different export format",
        run(85, 115, 4),
        3,
        None,
        true,
    ));
    // Its parent is not part of the corpus: shown at top level with a note.
    let mut orphan = child(
        10,
        Agent::ClaudeCode,
        "billing-api",
        "Check rounding rules",
        run(140, 160, 4),
        4,
        Some("explore"),
        false,
    );
    orphan.parent = Some(format!("0f0f0f0f-0000-4000-8000-{:012x}", 0));
    list.push(orphan);

    let mut sessions: Vec<Session> = list.into_iter().map(|spec| build(base, spec)).collect();
    // A session whose agent kept no message timestamps: one approximate span.
    let at = |minute: i64| format_iso(base + minute * 60_000);
    sessions.push(Session {
        id: id(11),
        agent: Agent::Cursor,
        project_id: "data-pipeline".to_string(),
        started_at: at(150),
        last_activity_at: at(195),
        messages: (0..6)
            .map(|_| Message {
                parts: vec![Part::Known(KnownPart::Text {
                    text: "synthetic message".to_string(),
                    extra: None,
                })],
                ..Message::default()
            })
            .collect(),
        ..Session::default()
    });
    sessions
}
