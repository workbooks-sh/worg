//! `worg-agent` — Rust runtime for WORG-defined agents.
//!
//! Sibling to `packages/worg/elixir/worg-agent/`. The two runtimes
//! satisfy the same WORG contract:
//!
//! - Load an agent definition from a `.org` file (via [`worg-parse`]
//!   and [`worg-query`]).
//! - Loop: prompt an LLM, dispatch tool calls, fold tool results back
//!   into the conversation, persist each round.
//! - Emit a [`worg_orch::Run`] when the loop terminates.
//!
//! The two runtimes do NOT share code — they share *contract*. This
//! crate exists because wavelet (the GPU-native renderer) and worg's
//! parser/query crates are both Rust; a Rust runtime keeps the local
//! eval stack single-language. Studio keeps the Elixir runtime for
//! server-side concurrency + supervision + Phoenix.Sync.
//!
//! ## Crate layout
//!
//! - [`tool`]          — the `Tool` trait every tool implements
//! - [`tool_registry`] — name → tool dispatch
//! - [`llm`]           — `LlmClient` trait + OpenRouter implementation
//! - [`types`]         — agent spec, turn context, message shapes
//! - [`loader`]        — `.org` → [`types::AgentSpec`] adapter
//! - [`loop_`]         — the agent loop (renamed because `loop` is a keyword)
//! - [`tools`]         — bundled tool implementations (bash/read/write +
//!                       Phase 3 wavelet/brandwork wrappers)
//! - [`wire`]          — re-export of `worg_orch` wire types

pub mod llm;
pub mod loader;
#[path = "loop_.rs"]
pub mod loop_;
pub mod scheduler;
pub mod tool;
pub mod tool_registry;
pub mod tools;
pub mod types;

/// Re-export of the orchestrator-protocol wire types we share with
/// the Elixir runtime + the `worg orch` CLI. Use these when reading
/// or writing on-disk JSON; use [`types::AgentSpec`] for the in-memory
/// agent shape that the loop actually executes against (it carries
/// runtime fields like the resolved model and tool list).
pub mod wire {
    pub use worg_orch::{
        Agent, AgentStatus, AgentType, AgentsFile, ProtocolVersion, Run, RunState, Task,
        TaskState, TokenCounts,
    };
}
