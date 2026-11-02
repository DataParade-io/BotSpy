//! Gratitude-versus-profanity scoring of your own messages, for the
//! thanks-vs-F-bombs meter example. Shared by the example binary and the BDD steps, and written
//! against the public `botspy` API only — compiling anywhere proves the
//! example-consumer constraint.

use botspy::{KnownPart, Message, Part, Role, Session};

/// Aggregate meter counts across a unified history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeterReport {
    /// Gratitude hits across all scored sessions.
    pub thanks: u64,
    /// F-bomb hits across all scored sessions.
    pub f_bombs: u64,
    /// Sessions that went into the report.
    pub sessions: u64,
}

/// Whether a message is you speaking: a user-role message that is not
/// text injected by tooling, and that sits in a session you opened rather
/// than one an agent spawned.
///
/// - Tool results that adapters store under the user role are tool parts,
///   not text, so they never reach the classifiers (see [`typed_text`]).
/// - Adapters mark injected text with an origin (Cursor's simulated user
///   turns, Codex's inter-agent mail) or as a compaction summary; those
///   are left out.
/// - A sub-agent's "user" messages are the parent agent's prompts, so a
///   sub-agent session contributes nothing.
pub fn is_yours(session: &Session, message: &Message) -> bool {
    session.subagent.is_none()
        && message.role == Role::User
        && message.origin.is_none()
        && !message.is_compaction_summary
}

/// What you typed in one message: its text parts, joined. `None` when the
/// message has no text (for example, only tool results).
pub fn typed_text(message: &Message) -> Option<String> {
    let texts: Vec<&str> = message
        .parts
        .iter()
        .filter_map(|part| match part {
            Part::Known(KnownPart::Text { text, .. }) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    (!texts.is_empty()).then(|| texts.join("\n"))
}

/// The text of every message of yours in a session, one entry per message.
/// Agent replies, thinking, and tool parts are not you, so they never
/// score.
pub fn message_texts(session: &Session) -> Vec<String> {
    session
        .messages
        .iter()
        .filter(|message| is_yours(session, message))
        .filter_map(typed_text)
        .collect()
}

/// Whether one message text counts as gratitude.
pub fn is_thanks(text: &str) -> bool {
    let text = text.to_lowercase();
    text.contains("thanks") || text.contains("thank you")
}

/// Whether one message text counts as an F-bomb, censored or not.
pub fn is_f_bomb(text: &str) -> bool {
    let text = text.to_lowercase();
    text.contains("f***") || text.contains("f**k") || text.contains("fuck")
}

/// Score one session: `(thanks, f_bombs)`.
pub fn score_session(session: &Session) -> (u64, u64) {
    let mut thanks = 0;
    let mut f_bombs = 0;
    for text in message_texts(session) {
        if is_thanks(&text) {
            thanks += 1;
        }
        if is_f_bomb(&text) {
            f_bombs += 1;
        }
    }
    (thanks, f_bombs)
}

/// Score a whole unified history in one report.
pub fn score_history(sessions: &[Session]) -> MeterReport {
    let mut report = MeterReport {
        sessions: sessions.len() as u64,
        thanks: 0,
        f_bombs: 0,
    };
    for session in sessions {
        let (session_thanks, session_f_bombs) = score_session(session);
        report.thanks += session_thanks;
        report.f_bombs += session_f_bombs;
    }
    report
}

/// Whether the conversation leaned grateful rather than foul-mouthed.
pub fn favors_thanks(report: &MeterReport) -> bool {
    report.thanks > report.f_bombs
}
