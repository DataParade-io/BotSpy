//! A synthetic parallel-sessions corpus for `--demo`: several projects,
//! overlapping sessions, turns with waiting between them, a fork, spawned sub-agents, an orphan
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

/// One message: minutes after the demo's 09:00 start, and whether the human
/// wrote it.
type Stamp = (i64, bool);

/// The conversation of turns `(from, to)`: the human speaks at `from`, then
/// the agent replies every few minutes until `to`.
fn talk(turns: &[(i64, i64)]) -> Vec<Stamp> {
    let mut stamps = Vec::new();
    for (from, to) in turns {
        stamps.push((*from, true));
        let mut at = *from + 3;
        while at < *to {
            stamps.push((at, false));
            at += 3;
        }
        stamps.push((*to, false));
    }
    stamps
}

struct Spec {
    n: usize,
    agent: Agent,
    project: &'static str,
    title: &'static str,
    stamps: Vec<Stamp>,
    parent: Option<String>,
    subagent: Option<&'static str>,
    fork: bool,
}

fn build(base: Ms, spec: Spec) -> Session {
    let at = |minute: i64| format_iso(base + minute * 60_000);
    let messages = spec
        .stamps
        .iter()
        .map(|(minute, human)| Message {
            role: if *human { Role::User } else { Role::Assistant },
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
        started_at: at(spec.stamps[0].0),
        last_activity_at: at(spec.stamps.last().expect("stamps").0),
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
        stamps: minutes,
        parent: None,
        subagent: None,
        fork: false,
    };
    let mut list = vec![
        spec(
            1,
            Agent::ClaudeCode,
            "checkout-web",
            "Rework the checkout form",
            talk(&[(0, 18), (26, 45), (52, 60), (105, 128), (135, 150)]),
        ),
        spec(
            2,
            Agent::Cursor,
            "checkout-web",
            "Fix the cart badge",
            talk(&[(20, 38), (44, 70), (78, 95)]),
        ),
        spec(
            3,
            Agent::Codex,
            "billing-api",
            "Add invoice export",
            talk(&[(10, 30), (36, 52), (58, 70), (120, 145), (150, 180)]),
        ),
        spec(
            4,
            Agent::ClaudeCode,
            "billing-api",
            "Tax rounding bug",
            talk(&[(120, 140), (148, 175), (185, 210)]),
        ),
        spec(
            5,
            Agent::Antigravity,
            "data-pipeline",
            "Backfill the events table",
            talk(&[(30, 50), (56, 70)]),
        ),
        spec(
            6,
            Agent::GrokBot,
            "data-pipeline",
            "Profile the nightly job",
            talk(&[(55, 75), (82, 100), (110, 130)]),
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
            stamps: minutes,
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
        talk(&[(30, 44)]),
        1,
        Some("explore"),
        false,
    ));
    list.push(child(
        8,
        Agent::ClaudeCode,
        "checkout-web",
        "Write form tests",
        talk(&[(112, 126)]),
        1,
        Some("general"),
        false,
    ));
    list.push(child(
        9,
        Agent::Codex,
        "billing-api",
        "Try a different export format",
        talk(&[(85, 100), (104, 115)]),
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
        talk(&[(140, 158)]),
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
