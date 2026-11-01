//! The normalized schema: messages made of typed parts.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Version of the normalized schema.
pub const SCHEMA_VERSION: u32 = 1;

/// RFC 3339 UTC timestamp.
pub type Timestamp = String;

/// Who produced a message.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    #[default]
    User,
    Assistant,
    System,
    Tool,
}

/// A coding-agent source.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Agent {
    #[default]
    ClaudeCode,
    Cursor,
    Codex,
    GrokBot,
    Antigravity,
}

/// Outcome carried by tool results, tool calls, and turns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartStatus {
    Ok,
    Error,
    Interrupted,
    Loading,
    Failed,
    Success,
}

impl PartStatus {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "ok" => Some(Self::Ok),
            "error" => Some(Self::Error),
            "interrupted" => Some(Self::Interrupted),
            "loading" => Some(Self::Loading),
            "failed" => Some(Self::Failed),
            "success" => Some(Self::Success),
            _ => None,
        }
    }
}

/// Tool-call arguments in whatever form the agent wrote them. The raw
/// form is always kept; JSON is parsed only once, never double-decoded.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolArguments {
    /// Structured arguments the agent passed as a JSON object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    /// The raw string form (JS source, a JSON-encoded string parameter).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    /// The first parse of `raw`, present when the raw string is valid JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parsed: Option<Value>,
}

impl ToolArguments {
    /// Arguments from a raw string: kept verbatim, parsed when valid JSON.
    pub fn from_raw(raw: impl Into<String>) -> Self {
        let raw = raw.into();
        let parsed = serde_json::from_str(&raw).ok();
        Self {
            value: None,
            raw: Some(raw),
            parsed,
        }
    }

    /// Arguments from a JSON object the agent passed as structured data.
    pub fn from_value(value: Value) -> Self {
        Self {
            value: Some(value),
            raw: None,
            parsed: None,
        }
    }
}

/// The content hash of inline bytes: SHA-256 over the base64 payload as
/// recorded, hex-encoded. Cursor content-addresses blobs this way; Claude
/// Code duplicates the same bytes across two fields of one record.
pub fn content_hash(base64_data: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(base64_data.as_bytes());
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// A part with one of the fixed kinds.
///
/// `extra` carries agent-specific fields the schema does not model yet, so
/// nothing is lost on normalization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum KnownPart {
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        extra: Option<Value>,
    },
    Thinking {
        /// The plain reasoning text, when the agent stored it unencrypted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        /// Signature blob for signed reasoning blocks (Claude thinking
        /// signature).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
        /// Encrypted reasoning content, kept verbatim (Codex
        /// reasoning.encrypted_content).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        encrypted: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        extra: Option<Value>,
    },
    ToolCall {
        id: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        arguments: Option<ToolArguments>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<PartStatus>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        extra: Option<Value>,
    },
    ToolResult {
        call_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<PartStatus>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        extra: Option<Value>,
    },
    Attachment {
        path: String,
        mime: String,
        size: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        extra: Option<Value>,
    },
    InlineData {
        mime: String,
        /// The inline bytes, base64-encoded, when they travel in the record.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<String>,
        /// Content-addressed reference: a content hash for inline bytes or
        /// a blob reference (e.g. Cursor `agentKv:blob:<sha256>`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data_ref: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        extra: Option<Value>,
    },
    Blob {
        /// Hash of the undecodable payload, content-addressed.
        blob_hash: String,
        /// The container the blob lives in (e.g. Cursor `agentKv:blob`).
        container: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        extra: Option<Value>,
    },
    System {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        extra: Option<Value>,
    },
}

/// A typed content part.
///
/// Anything outside the fixed kinds falls through to [`Part::Extra`], the raw
/// passthrough: the original JSON object (including its `kind`) is preserved
/// verbatim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Part {
    Known(KnownPart),
    Extra(Value),
}

impl Part {
    /// The kind name, for known kinds and for raw parts that carry a `kind`.
    pub fn kind_name(&self) -> Option<&str> {
        match self {
            Part::Known(known) => Some(match known {
                KnownPart::Text { .. } => "text",
                KnownPart::Thinking { .. } => "thinking",
                KnownPart::ToolCall { .. } => "tool_call",
                KnownPart::ToolResult { .. } => "tool_result",
                KnownPart::Attachment { .. } => "attachment",
                KnownPart::InlineData { .. } => "inline_data",
                KnownPart::Blob { .. } => "blob",
                KnownPart::System { .. } => "system",
            }),
            Part::Extra(raw) => raw.get("kind").and_then(Value::as_str),
        }
    }

    /// The raw passthrough payload when this part is not one of the fixed kinds.
    pub fn as_extra(&self) -> Option<&Value> {
        match self {
            Part::Extra(raw) => Some(raw),
            _ => None,
        }
    }
}

/// Token usage an agent persisted for one message, one turn, or a whole
/// thread. Counters an agent does not record stay `None`, never zero.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl Usage {
    /// Accumulate `other` into `self`: counters add up when both sides have
    /// them, otherwise the present side wins. The model is kept from either.
    pub fn add(&mut self, other: &Usage) {
        for (mine, theirs) in [
            (&mut self.input_tokens, &other.input_tokens),
            (&mut self.output_tokens, &other.output_tokens),
            (&mut self.cache_read_tokens, &other.cache_read_tokens),
            (&mut self.cache_write_tokens, &other.cache_write_tokens),
            (&mut self.cached_input_tokens, &other.cached_input_tokens),
            (&mut self.reasoning_tokens, &other.reasoning_tokens),
        ] {
            *mine = match (*mine, *theirs) {
                (Some(a), Some(b)) => Some(a + b),
                (None, Some(b)) => Some(b),
                (a, None) => a,
            };
        }
        self.model = self.model.take().or_else(|| other.model.clone());
    }
}

/// The cost state an agent persisted for a session.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionCost {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_api_duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tool_duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_lines_added: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_lines_removed: Option<u64>,
    /// Cost attributed per model, for agents that record it that way.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub per_model: BTreeMap<String, ModelCost>,
}

/// Cost attributed to one model within a session.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelCost {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

/// Rate-limit and plan state an agent persisted for a session.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RateLimitState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_percent: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_minutes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<Timestamp>,
}

/// A session's place in the session graph: what spawned it and where.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SubagentInfo {
    /// The agent's own sub-agent kind or name (Claude "general"/"explore",
    /// Cursor subagentTypeName, Antigravity "subagent").
    pub kind: String,
    /// Nesting depth for nested conversations (Antigravity nesting_depth).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nesting_depth: Option<u64>,
}

/// Where a forked session split from its parent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ForkPoint {
    /// The fork is the history strictly before this ordinal
    /// (Codex forked_from_ordinal_exclusive).
    pub ordinal: u64,
}

/// A best-of-N battle link between competing sessions.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BattleLink {
    pub battle_id: String,
    /// The winning conversation, recorded on the winner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub winning_conversation_id: Option<String>,
}

/// A peer cloud agent of this session (Grok Bot cloudAgentPeerIds): the
/// peer has no local transcript here.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Peer {
    pub peer_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_local_transcript: bool,
}

/// Optional session metadata agents record about a session, named so
/// listing across agents can use them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_origin_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    /// Models used by the session, in use order (mid-session changes visible).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
}

/// Why a session's local history is only partial.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartialReason {
    /// The local store is a cache of cloud-side history
    /// (Cursor conversation-search.db source "cloud-cache").
    CloudCache,
    /// The local log is truncated by a cap (Grok Bot entry logs capped at
    /// 200 entries; full history is server-side).
    LocalCap,
}

/// A session whose local records cover only part of the real history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartialHistory {
    pub reason: PartialReason,
    /// Free-form detail the agent recorded about the partial view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// One compaction boundary: the agent summarized its own context.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CompactionEvent {
    /// Token count in context before compaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_tokens: Option<u64>,
    /// Token count in context after compaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_tokens: Option<u64>,
    /// The message the boundary logically follows (Claude Code
    /// compactMetadata.logicalParentUuid).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logical_parent_message_id: Option<String>,
}

/// One link in a compaction window chain (Codex compacted records): the
/// window replaced the history after its previous window and records what
/// it retained.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CompactionWindow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_window: Option<String>,
    /// What this window retained, as the agent recorded it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retained_context: Option<String>,
}

/// A native message shade mapped by convention onto the four roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// Codex role `developer`: system-level instructions from the developer.
    Developer,
    /// Codex `agent_message`: inter-agent mail between agent sessions.
    AgentMessage,
    /// Cursor `isSimulatedMsg`: user content injected by tooling, not typed
    /// by a human.
    Simulated,
    /// Text the agent harness injected under the user role: skill bodies,
    /// injected context, notifications, hook output, summaries of an earlier
    /// conversation, interruption markers. The human did not type it.
    Harness,
}

impl Origin {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "developer" => Some(Self::Developer),
            "agent_message" => Some(Self::AgentMessage),
            "simulated" => Some(Self::Simulated),
            "harness" => Some(Self::Harness),
            _ => None,
        }
    }
}

/// Life-cycle state of a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Completed,
    Failed,
    Aborted,
}

/// One turn of a session: a user request plus everything the agent did to
/// answer it. Turn-level facts live here, not on the messages.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TurnStatus>,
    /// Why the turn was aborted (Codex turn_aborted.reason and friends).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aborted_reason: Option<String>,
    /// The error that failed the turn (Codex usageLimitExceeded and friends).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// How long the whole turn took (Codex task_complete duration_ms,
    /// Cursor turnDurationMs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Latency to the first response token (Codex time_to_first_token_ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_to_first_token_ms: Option<u64>,
}

/// Claude's turnPosition: which prompt the turn answers, and which turn of
/// that prompt it is.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TurnPosition {
    pub prompt_index: u64,
    pub turn_index: u64,
}

/// Where a record came from on disk.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    pub source_file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row: Option<u64>,
    /// The agent's own record id (Claude uuid, Codex record id, Cursor row id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record_id: Option<String>,
    /// The agent's own record type (Claude type field and friends).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record_type: Option<String>,
    /// The agent's ordinal/sort key, when it numbers records (Codex).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ordinal: Option<u64>,
    /// The native parent pointer (Claude parentUuid and friends).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_record: Option<String>,
}

/// A message inside a session: ordered typed parts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// The agent's own record id for this message (Claude uuid and friends).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub role: Role,
    pub parts: Vec<Part>,
    /// The native timestamp, when the agent records one. Absent — never
    /// fabricated — for transcripts that carry no timestamps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    /// Claude's turnPosition {promptIndex, turnIndex}.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_position: Option<TurnPosition>,
    /// Cursor's stepDurationMs — one response bubble's own duration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// The message records an error event (Antigravity error steps and friends).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
    /// The message carries an agent-written compaction summary.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_compaction_summary: bool,
    /// The native shade this message came from, when it maps by convention
    /// (developer, inter-agent mail, simulated user).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,
    /// The persona or agent session that authored the message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// The recipient of inter-agent mail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipient: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Session;

    /// Every fixed part kind survives a JSON round trip with its `kind`
    /// tag intact.
    #[test]
    fn part_kinds_round_trip_through_json() {
        let parts = vec![
            Part::Known(KnownPart::Text {
                text: "hello".into(),
                extra: None,
            }),
            Part::Known(KnownPart::Thinking {
                text: Some("why".into()),
                signature: Some("sig".into()),
                encrypted: None,
                extra: None,
            }),
            Part::Known(KnownPart::ToolCall {
                id: "call_1".into(),
                name: "read_file".into(),
                arguments: Some(ToolArguments::from_value(serde_json::json!({"path": "x"}))),
                status: Some(PartStatus::Loading),
                extra: None,
            }),
            Part::Known(KnownPart::ToolResult {
                call_id: "call_1".into(),
                text: Some("contents".into()),
                status: Some(PartStatus::Error),
                extra: None,
            }),
            Part::Known(KnownPart::Attachment {
                path: "docs/spec.md".into(),
                mime: "text/markdown".into(),
                size: 4123,
                extra: None,
            }),
            Part::Known(KnownPart::InlineData {
                mime: "image/png".into(),
                data: Some("aGk=".into()),
                data_ref: Some(content_hash("aGk=")),
                extra: None,
            }),
            Part::Known(KnownPart::Blob {
                blob_hash: "9d2c".into(),
                container: "agentKv:blob".into(),
                extra: None,
            }),
            Part::Known(KnownPart::System {
                text: "notice".into(),
                extra: None,
            }),
        ];
        for part in parts {
            let json = serde_json::to_value(&part).expect("part serializes");
            assert!(
                json.get("kind").and_then(Value::as_str).is_some(),
                "part lost its kind tag: {json}"
            );
            let round_tripped: Part = serde_json::from_value(json).expect("part deserializes");
            assert_eq!(round_tripped, part, "part did not survive the round trip");
        }
    }

    /// Agent-specific part kinds outside the fixed set degrade to the raw
    /// passthrough, preserving the payload verbatim — including its `kind`.
    #[test]
    fn unknown_kinds_degrade_to_extra_verbatim() {
        let raw = serde_json::json!({"kind": "quantum_flux", "payload": [1, 2, 3]});
        let part: Part = serde_json::from_value(raw.clone()).expect("raw part parses");
        assert_eq!(part.kind_name(), Some("quantum_flux"));
        assert_eq!(part.as_extra(), Some(&raw));
        let round_tripped = serde_json::to_value(&part).expect("raw part serializes");
        assert_eq!(round_tripped, raw, "raw passthrough was not verbatim");
    }

    /// A fully-populated session — turns, usage, cost, metadata, partial
    /// flags, provenance — round-trips through JSON unchanged.
    #[test]
    fn populated_session_round_trips_through_json() {
        let mut session = Session {
            id: "s1".into(),
            agent: Agent::Cursor,
            project_id: "demo".into(),
            started_at: "2026-10-01T09:00:00Z".into(),
            last_activity_at: "2026-10-01T10:00:00Z".into(),
            parent_id: Some("s0".into()),
            root_id: Some("s0".into()),
            residual_context_tokens: Some(512),
            ..Session::default()
        };
        session.messages.push(Message {
            id: Some("m1".into()),
            role: Role::Assistant,
            parts: vec![Part::Known(KnownPart::Text {
                text: "hi".into(),
                extra: None,
            })],
            timestamp: Some("2026-10-01T09:30:00Z".into()),
            provenance: Some(Provenance {
                source_file: "store.db".into(),
                row: Some(17),
                record_id: Some("row-17".into()),
                ordinal: Some(3),
                ..Provenance::default()
            }),
            turn_id: Some("turn-1".into()),
            turn_position: Some(TurnPosition {
                prompt_index: 0,
                turn_index: 1,
            }),
            step_duration_ms: Some(420),
            ..Message::default()
        });
        session.turns.insert(
            "turn-1".into(),
            Turn {
                id: "turn-1".into(),
                status: Some(TurnStatus::Completed),
                duration_ms: Some(14300),
                time_to_first_token_ms: Some(850),
                ..Turn::default()
            },
        );
        session.partial = Some(PartialHistory {
            reason: PartialReason::CloudCache,
            detail: Some("cloud-cache".into()),
        });
        let json = serde_json::to_value(&session).expect("session serializes");
        let round_tripped: Session = serde_json::from_value(json).expect("session deserializes");
        assert_eq!(round_tripped, session);
    }

    /// The schema version constant is importable and stable.
    #[test]
    fn schema_version_is_stable() {
        assert_eq!(SCHEMA_VERSION, 1);
    }
}
