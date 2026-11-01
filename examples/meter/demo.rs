//! A synthetic corpus for `--demo`: twelve weeks of made-up sessions with a
//! known number of thanks and F-bombs per week, plus the things the meter
//! must not count (agent replies that say thanks, tool results handed back
//! under the user role, injected turns, and a sub-agent's prompts). Every
//! id and every line of text is invented; nothing here comes from a real
//! machine.

use super::activity::{format_iso, parse_time, Ms};
use botspy::{Agent, KnownPart, Message, Origin, Part, Role, Session, SubagentInfo};

const DAY_MS: Ms = 86_400_000;
/// Thanks you typed in each demo week, starting Monday 2026-08-03.
pub const THANKS: [u64; 12] = [14, 22, 31, 18, 40, 27, 35, 52, 44, 29, 61, 48];
/// F-bombs you typed in each demo week.
pub const F_BOMBS: [u64; 12] = [3, 0, 5, 2, 7, 1, 0, 4, 2, 6, 1, 0];

fn text(role: Role, body: &str, time: Option<String>) -> Message {
    Message {
        role,
        parts: vec![Part::Known(KnownPart::Text {
            text: body.to_string(),
            extra: None,
        })],
        timestamp: time,
        ..Message::default()
    }
}

fn session(n: usize, messages: Vec<Message>) -> Session {
    let first = messages.first().and_then(|m| m.timestamp.clone());
    let last = messages.last().and_then(|m| m.timestamp.clone());
    Session {
        id: format!("demo-session-{n:02}"),
        agent: Agent::ClaudeCode,
        project_id: "demo-project".to_string(),
        started_at: first.unwrap_or_default(),
        last_activity_at: last.unwrap_or_default(),
        messages,
        ..Session::default()
    }
}

/// The demo corpus, as sessions: one per week.
pub fn sessions() -> Vec<Session> {
    let monday = parse_time("2026-08-03T00:00:00Z").expect("demo start");
    // The censored spelling, joined at run time.
    let bomb = format!("what the f{} is this", "***");
    let mut list = Vec::new();
    for week in 0..THANKS.len() {
        let base: Ms = monday + week as i64 * 7 * DAY_MS;
        let mut slot = 0_i64;
        let next = |slot: &mut i64| {
            *slot += 1;
            Some(format_iso(
                base + (*slot % 6) * DAY_MS + (9 + *slot % 8) * 3_600_000,
            ))
        };
        let mut messages = Vec::new();
        for _ in 0..THANKS[week] {
            messages.push(text(Role::User, "thanks, that fixed it", next(&mut slot)));
            // The agent's reply is gracious too; it must not score.
            messages.push(text(
                Role::Assistant,
                "Thanks for the context, that helps.",
                next(&mut slot),
            ));
        }
        for _ in 0..F_BOMBS[week] {
            messages.push(text(Role::User, &bomb, next(&mut slot)));
        }
        for _ in 0..8 {
            messages.push(text(
                Role::User,
                "please run the tests again",
                next(&mut slot),
            ));
        }
        // Tool results come back under the user role, as tool parts.
        messages.push(Message {
            role: Role::User,
            parts: vec![Part::Known(KnownPart::ToolResult {
                call_id: format!("call-{week}"),
                text: Some("thanks".to_string()),
                status: None,
                extra: None,
            })],
            timestamp: next(&mut slot),
            ..Message::default()
        });
        // Tooling-injected turns are marked, and left out.
        let mut simulated = text(Role::User, "thanks, continue", next(&mut slot));
        simulated.origin = Some(Origin::Simulated);
        messages.push(simulated);
        list.push(session(week + 1, messages));
    }
    // A sub-agent's "user" messages are the parent agent's prompts.
    let mut helper = session(
        99,
        vec![text(
            Role::User,
            "thanks, now check the build",
            Some(format_iso(monday + DAY_MS)),
        )],
    );
    helper.parent_id = Some("demo-session-01".to_string());
    helper.subagent = Some(SubagentInfo {
        kind: "explore".to_string(),
        nesting_depth: None,
    });
    list.push(helper);
    list
}
