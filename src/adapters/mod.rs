//! Source adapters. The per-agent adapters (Claude Code, Cursor, Codex,
//! Grok Bot, Antigravity) land here, specified under features/03_importers/.

pub mod antigravity;
pub mod claude_code;
pub mod codex;
pub mod cursor;
pub mod fixture;
pub mod grok_bot;

use crate::adapter::Adapter;
use crate::schema::Agent;
use crate::session::{Session, SessionSummary};

/// The real sources are adapters too: discovery yields their session
/// summaries, and opening a session extracts it. This is what lets the
/// unified history (and the CLI's store builder) treat every agent the
/// same way.
impl Adapter for claude_code::ClaudeCodeSource {
    fn agent(&self) -> Agent {
        Agent::ClaudeCode
    }

    fn discover(&self) -> Vec<SessionSummary> {
        let (summaries, _) = claude_code::ClaudeCodeSource::discover(self);
        summaries
    }

    fn open(&self, id: &str) -> Option<Session> {
        self.extract(id).map(|extraction| extraction.session)
    }
}

impl Adapter for cursor::CursorSource {
    fn agent(&self) -> Agent {
        Agent::Cursor
    }

    fn discover(&self) -> Vec<SessionSummary> {
        let (summaries, _) = cursor::CursorSource::discover(self);
        summaries
    }

    fn open(&self, id: &str) -> Option<Session> {
        self.extract(id).map(|extraction| extraction.session)
    }
}

impl Adapter for cursor::CursorCliSource {
    fn agent(&self) -> Agent {
        Agent::Cursor
    }

    fn discover(&self) -> Vec<SessionSummary> {
        let (summaries, _) = cursor::CursorCliSource::discover(self);
        summaries
    }

    fn open(&self, id: &str) -> Option<Session> {
        self.extract(id).map(|extraction| extraction.session)
    }
}

impl Adapter for codex::CodexSource {
    fn agent(&self) -> Agent {
        Agent::Codex
    }

    fn discover(&self) -> Vec<SessionSummary> {
        let (summaries, _) = codex::CodexSource::discover(self);
        summaries
    }

    fn open(&self, id: &str) -> Option<Session> {
        self.extract(id).map(|extraction| extraction.session)
    }
}

impl Adapter for grok_bot::GrokBotSource {
    fn agent(&self) -> Agent {
        Agent::GrokBot
    }

    fn discover(&self) -> Vec<SessionSummary> {
        let (summaries, _) = grok_bot::GrokBotSource::discover(self);
        summaries
    }

    fn open(&self, id: &str) -> Option<Session> {
        self.extract(id).map(|extraction| extraction.session)
    }
}

impl Adapter for antigravity::AntigravitySource {
    fn agent(&self) -> Agent {
        Agent::Antigravity
    }

    fn discover(&self) -> Vec<SessionSummary> {
        let (summaries, _) = antigravity::AntigravitySource::discover(self);
        summaries
    }

    fn open(&self, id: &str) -> Option<Session> {
        self.extract(id).map(|extraction| extraction.session)
    }
}
