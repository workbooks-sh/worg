//! Wire-format types for the Workbooks Orchestrator Protocol's `.wb-orch/`
//! JSON files. These mirror `packages/orchestrator-core/src/types.rs`
//! byte-for-byte at the JSON level, but are independent at the Rust level
//! so WORG stays foundational (no internal Workbooks deps — see CLAUDE.md
//! "Dependency layers").
//!
//! If orchestrator-core's wire format changes, the `round_trip_agents` /
//! `round_trip_task` / `round_trip_run` tests below will fail when run
//! against fresh fixtures — that's the detection point for drift.
//!
//! Used by:
//!   - `worg orch export agents` (orgfile → AgentsFile JSON)
//!   - `worg orch export tasks` (orgfile → Task JSON files)
//!   - `worg orch import runs` (Run JSON files → orgfile LOGBOOK + state)

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::fmt;
use time::OffsetDateTime;

mod walker;
pub use walker::{
    agent_definition_by_id, agent_definitions, agents_file, board_snapshot,
    task_definition_by_id, task_definitions, AgentDefinition, BoardSnapshot, ExportOpts,
    TaskDefinition,
};

/// Protocol version this crate targets. Mirrors orchestrator-core's
/// `PROTOCOL_VERSION`. Bumping this is a deliberate, coordinated change.
pub const PROTOCOL_VERSION: u32 = 1;

/// Canonical board directory name. Mirrors orchestrator-core's `BOARD_DIR`.
pub const BOARD_DIR: &str = ".wb-orch";

// ─────────────────────────── id newtypes ─────────────────────────────────

macro_rules! string_newtype {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            /// Construct from a string. Caller is responsible for ensuring
            /// the id is filesystem-safe (`[A-Za-z0-9._-]{1,64}`).
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            /// Borrow the underlying string.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }
    };
}

string_newtype!(AgentId, "Agent identifier. Matches `agents[].id` in `.wb-orch/agents.json`.");
string_newtype!(TaskId, "Task identifier. Matches `tasks/{id}.json` on disk.");
string_newtype!(RunId, "Run identifier. Matches `runs/{id}.json`; convention `{task-id}-{attempt}`.");

/// Protocol version, written to `.wb-orch/version`. Transparent over `u32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProtocolVersion(pub u32);

impl Default for ProtocolVersion {
    fn default() -> Self {
        Self(PROTOCOL_VERSION)
    }
}

// ─────────────────────────────── Agent ───────────────────────────────────

/// Agent kind discriminator (AI vs human).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentType {
    /// AI agent (Pi, Claude Code, Codex, etc.)
    Ai,
    /// Human participant
    Human,
}

/// Whether an agent participates in scheduling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    /// Active and eligible to claim tasks
    Active,
    /// Temporarily paused (existing claims remain valid)
    Paused,
    /// Permanently retired
    Terminated,
}

/// An agent registered in `.wb-orch/agents.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    pub id: AgentId,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: AgentType,
    pub status: AgentStatus,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reports_to: Option<AgentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heartbeat_sec: Option<u32>,
}

/// The on-disk shape of `.wb-orch/agents.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentsFile {
    pub version: ProtocolVersion,
    pub agents: Vec<Agent>,
}

impl Default for AgentsFile {
    fn default() -> Self {
        Self {
            version: ProtocolVersion(PROTOCOL_VERSION),
            agents: Vec::new(),
        }
    }
}

// ─────────────────────────────── Task ────────────────────────────────────

/// Task lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// Created but not yet prioritized
    Backlog,
    /// Prioritized; awaiting a claim
    Ready,
    /// Actively being worked on; has one running Run
    InProgress,
    /// Paused waiting for input from a human or another agent
    InputRequired,
    /// Work complete; awaiting reviewer
    Review,
    /// Completed successfully (terminal)
    Done,
    /// Cannot proceed (with `blocked_reason`)
    Blocked,
    /// Will not be done (terminal)
    Cancelled,
}

impl TaskState {
    /// True if no transitions out of this state are allowed.
    pub fn is_terminal(self) -> bool {
        matches!(self, TaskState::Done | TaskState::Cancelled)
    }
}

/// Comment attached to a task. Application-layer; the protocol preserves
/// the array.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub by: AgentId,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub text: String,
}

/// A unit of work tracked by the orchestrator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub title: String,
    pub state: TaskState,
    pub created_by: AgentId,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assigned_to: Vec<AgentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<TaskId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub due: Option<OffsetDateTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<AgentId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_full: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_required_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<Comment>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub updated_at: Option<OffsetDateTime>,
}

// ──────────────────────────────── Run ────────────────────────────────────

/// Run lifecycle state. A run is one execution attempt at a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// Currently executing; holds the active claim
    Running,
    /// Finished successfully
    Completed,
    /// Finished with error
    Failed,
    /// Operator stopped
    Cancelled,
}

impl RunState {
    /// True if no transitions out of this state are allowed.
    pub fn is_terminal(self) -> bool {
        !matches!(self, RunState::Running)
    }
}

/// Token usage for an LLM-backed run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCounts {
    pub input: u64,
    pub output: u64,
}

/// One execution attempt against a task. Append-only: once terminal,
/// the file is immutable; subsequent attempts create new files with
/// incremented `attempt`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub id: RunId,
    pub task: TaskId,
    pub agent: AgentId,
    pub state: RunState,
    pub attempt: u32,
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,

    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub finished_at: Option<OffsetDateTime>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub lease_until: Option<OffsetDateTime>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub last_heartbeat: Option<OffsetDateTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<TokenCounts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_full: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commits: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<String>,
}

/// Compute the canonical run id for a task + attempt.
pub fn run_id(task: &TaskId, attempt: u32) -> RunId {
    RunId(format!("{}-{}", task.0, attempt))
}

// ────────────────────────────── Tests ────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    /// Minimal fixture: a single AI agent. Confirms field renames and
    /// snake_case enum serialization match the orchestrator-core wire
    /// format.
    #[test]
    fn round_trip_agents_file_minimal() {
        let src = r#"{
  "version": 1,
  "agents": [
    {
      "id": "workhorse",
      "name": "Workhorse",
      "type": "ai",
      "status": "active"
    }
  ]
}"#;
        let parsed: AgentsFile = serde_json::from_str(src).unwrap();
        assert_eq!(parsed.version, ProtocolVersion(1));
        assert_eq!(parsed.agents.len(), 1);
        let a = &parsed.agents[0];
        assert_eq!(a.id.as_str(), "workhorse");
        assert_eq!(a.kind, AgentType::Ai);
        assert_eq!(a.status, AgentStatus::Active);
        // Round-trip back to JSON; should match the input semantically.
        let emitted = serde_json::to_string_pretty(&parsed).unwrap();
        let reparsed: AgentsFile = serde_json::from_str(&emitted).unwrap();
        assert_eq!(parsed, reparsed);
    }

    #[test]
    fn round_trip_agents_file_full_optional_fields() {
        let src = r#"{
  "version": 1,
  "agents": [
    {
      "id": "workhorse",
      "name": "Workhorse",
      "type": "ai",
      "status": "active",
      "runtime": "claude-code",
      "role": "default-orchestrator",
      "capabilities": ["bash", "read", "write", "lua-eval"],
      "reports_to": "operator",
      "heartbeat_sec": 300
    },
    {
      "id": "shane",
      "name": "Shane",
      "type": "human",
      "status": "active"
    }
  ]
}"#;
        let parsed: AgentsFile = serde_json::from_str(src).unwrap();
        assert_eq!(parsed.agents.len(), 2);
        let w = &parsed.agents[0];
        assert_eq!(w.runtime.as_deref(), Some("claude-code"));
        assert_eq!(w.role.as_deref(), Some("default-orchestrator"));
        assert_eq!(w.capabilities, vec!["bash", "read", "write", "lua-eval"]);
        assert_eq!(w.reports_to.as_ref().map(|i| i.as_str()), Some("operator"));
        assert_eq!(w.heartbeat_sec, Some(300));
        assert_eq!(parsed.agents[1].kind, AgentType::Human);

        let emitted = serde_json::to_string(&parsed).unwrap();
        let reparsed: AgentsFile = serde_json::from_str(&emitted).unwrap();
        assert_eq!(parsed, reparsed);
    }

    #[test]
    fn round_trip_task_minimal() {
        let src = r#"{
  "id": "wb-nlln.4",
  "title": "Read orchestrator-core types",
  "state": "in_progress",
  "created_by": "shane",
  "created_at": "2026-05-23T20:00:00Z"
}"#;
        let parsed: Task = serde_json::from_str(src).unwrap();
        assert_eq!(parsed.id.as_str(), "wb-nlln.4");
        assert_eq!(parsed.state, TaskState::InProgress);
        assert!(!parsed.state.is_terminal());

        let emitted = serde_json::to_string(&parsed).unwrap();
        let reparsed: Task = serde_json::from_str(&emitted).unwrap();
        assert_eq!(parsed, reparsed);
    }

    #[test]
    fn round_trip_task_with_dag_edges_and_comments() {
        let src = r#"{
  "id": "wb-nlln.5",
  "title": "Implement org → AgentJson walker",
  "state": "ready",
  "created_by": "shane",
  "created_at": "2026-05-23T20:00:00Z",
  "description": "Walks an org Document and emits Agent records.",
  "assigned_to": ["workhorse"],
  "parent": "wb-nlln",
  "capabilities": ["rust", "worg"],
  "priority": 1,
  "due": "2026-06-01T00:00:00Z",
  "tags": ["worg", "phase-1"],
  "acceptance": "Round-trips against the workhorse fixture.",
  "comments": [
    { "by": "shane", "at": "2026-05-23T20:01:00Z", "text": "ship it" }
  ]
}"#;
        let parsed: Task = serde_json::from_str(src).unwrap();
        assert_eq!(parsed.state, TaskState::Ready);
        assert_eq!(parsed.parent.as_ref().map(|i| i.as_str()), Some("wb-nlln"));
        assert_eq!(parsed.assigned_to.len(), 1);
        assert_eq!(parsed.comments.len(), 1);
        assert_eq!(parsed.comments[0].text, "ship it");

        let emitted = serde_json::to_string(&parsed).unwrap();
        let reparsed: Task = serde_json::from_str(&emitted).unwrap();
        assert_eq!(parsed, reparsed);
    }

    #[test]
    fn round_trip_run_minimal() {
        let src = r#"{
  "id": "wb-nlln.4-1",
  "task": "wb-nlln.4",
  "agent": "workhorse",
  "state": "completed",
  "attempt": 1,
  "started_at": "2026-05-23T20:00:00Z",
  "finished_at": "2026-05-23T20:05:00Z",
  "result_summary": "types defined; 5 round-trip tests pass"
}"#;
        let parsed: Run = serde_json::from_str(src).unwrap();
        assert_eq!(parsed.state, RunState::Completed);
        assert!(parsed.state.is_terminal());
        assert_eq!(parsed.attempt, 1);

        let emitted = serde_json::to_string(&parsed).unwrap();
        let reparsed: Run = serde_json::from_str(&emitted).unwrap();
        assert_eq!(parsed, reparsed);
    }

    #[test]
    fn round_trip_run_with_tokens_and_artifacts() {
        let src = r#"{
  "id": "wb-nlln.5-1",
  "task": "wb-nlln.5",
  "agent": "workhorse",
  "state": "completed",
  "attempt": 1,
  "started_at": "2026-05-23T20:00:00Z",
  "finished_at": "2026-05-23T20:08:30Z",
  "tokens": { "input": 12400, "output": 1800 },
  "cost_usd": 0.043,
  "commits": ["68d050d08"],
  "artifacts": ["packages/worg/crates/worg-orch/src/lib.rs"]
}"#;
        let parsed: Run = serde_json::from_str(src).unwrap();
        assert_eq!(parsed.tokens.unwrap().input, 12400);
        assert_eq!(parsed.cost_usd, Some(0.043));
        assert_eq!(parsed.commits, vec!["68d050d08"]);

        let emitted = serde_json::to_string(&parsed).unwrap();
        let reparsed: Run = serde_json::from_str(&emitted).unwrap();
        assert_eq!(parsed, reparsed);
    }

    #[test]
    fn run_id_format_matches_protocol() {
        let id = run_id(&TaskId::new("wb-nlln.5"), 2);
        assert_eq!(id.as_str(), "wb-nlln.5-2");
    }

    #[test]
    fn terminal_states_classified_correctly() {
        assert!(TaskState::Done.is_terminal());
        assert!(TaskState::Cancelled.is_terminal());
        assert!(!TaskState::InProgress.is_terminal());
        assert!(!TaskState::Backlog.is_terminal());

        assert!(RunState::Completed.is_terminal());
        assert!(RunState::Failed.is_terminal());
        assert!(RunState::Cancelled.is_terminal());
        assert!(!RunState::Running.is_terminal());
    }

    #[test]
    fn agent_type_enum_serializes_as_snake_case() {
        let ai = AgentType::Ai;
        let json = serde_json::to_string(&ai).unwrap();
        assert_eq!(json, "\"ai\"");
        let human = AgentType::Human;
        let json = serde_json::to_string(&human).unwrap();
        assert_eq!(json, "\"human\"");
    }

    #[test]
    fn task_state_in_progress_serializes_with_underscore() {
        let s = TaskState::InProgress;
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, "\"in_progress\"");
        let back: TaskState = serde_json::from_str("\"in_progress\"").unwrap();
        assert_eq!(back, TaskState::InProgress);
    }

    #[test]
    fn empty_optional_fields_omitted_on_serialize() {
        let a = Agent {
            id: AgentId::new("workhorse"),
            name: "Workhorse".into(),
            kind: AgentType::Ai,
            status: AgentStatus::Active,
            runtime: None,
            role: None,
            capabilities: Vec::new(),
            reports_to: None,
            heartbeat_sec: None,
        };
        let json = serde_json::to_string(&a).unwrap();
        // None / empty fields must be absent (matches orchestrator-core
        // skip_serializing_if behavior — important for diff stability).
        assert!(!json.contains("runtime"));
        assert!(!json.contains("role"));
        assert!(!json.contains("capabilities"));
        assert!(!json.contains("reports_to"));
        assert!(!json.contains("heartbeat_sec"));
    }

    /// Anchor: the timestamp module compiles + parses RFC 3339.
    #[test]
    fn rfc3339_timestamp_anchor() {
        let t: OffsetDateTime = datetime!(2026-05-23 20:00 UTC);
        let s = t.format(&time::format_description::well_known::Rfc3339).unwrap();
        assert_eq!(s, "2026-05-23T20:00:00Z");
    }
}
