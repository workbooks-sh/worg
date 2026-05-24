//! The [`Tool`] trait. Sibling to Elixir's `WorgAgent.Tool` behaviour
//! at `packages/worg/elixir/worg-agent/lib/worg_agent/tool.ex` — same
//! contract, different host.

use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;

use crate::types::{ToolCtx, ToolOutput};

/// Errors a tool may return. Tools are expected to surface specific
/// failure modes through the `kind` field so the loop can decide
/// whether to retry, escalate, or surface to the user verbatim.
#[derive(Debug, Error)]
#[error("{kind}: {message}")]
pub struct ToolError {
    pub kind: ToolErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolErrorKind {
    /// Argument shape didn't match `input_schema` (missing field,
    /// wrong type). Non-retryable — the LLM should re-emit with a
    /// corrected payload.
    BadArgs,
    /// The tool refused to run because the granted capabilities
    /// don't include what it needs. Non-retryable in the same turn.
    CapabilityDenied,
    /// The tool ran but the underlying operation failed (CLI exited
    /// non-zero, file not found, HTTP 5xx). Retry semantics depend on
    /// the tool — the loop forwards the error string to the LLM and
    /// lets it decide.
    Execution,
    /// The tool would have touched host state above its trust level.
    /// Non-retryable.
    TrustDenied,
}

impl std::fmt::Display for ToolErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolErrorKind::BadArgs => f.write_str("bad_args"),
            ToolErrorKind::CapabilityDenied => f.write_str("capability_denied"),
            ToolErrorKind::Execution => f.write_str("execution"),
            ToolErrorKind::TrustDenied => f.write_str("trust_denied"),
        }
    }
}

impl ToolError {
    pub fn bad_args(msg: impl Into<String>) -> Self {
        Self {
            kind: ToolErrorKind::BadArgs,
            message: msg.into(),
        }
    }

    pub fn execution(msg: impl Into<String>) -> Self {
        Self {
            kind: ToolErrorKind::Execution,
            message: msg.into(),
        }
    }

    pub fn capability_denied(msg: impl Into<String>) -> Self {
        Self {
            kind: ToolErrorKind::CapabilityDenied,
            message: msg.into(),
        }
    }

    pub fn trust_denied(msg: impl Into<String>) -> Self {
        Self {
            kind: ToolErrorKind::TrustDenied,
            message: msg.into(),
        }
    }
}

/// Tools an agent can invoke. The LLM emits tool-use calls naming a
/// tool by [`Tool::name`]; the loop dispatches via [`crate::tool_registry::ToolRegistry`]
/// to that tool's `execute`.
///
/// ## Implementation notes
///
/// - Implementations should be cheap to construct (no I/O in
///   `new`/`Default`) — the registry instantiates each tool once and
///   re-uses it across turns.
/// - `name`, `description`, and `input_schema` are called during
///   catalog construction and should be pure / `const`-ish.
/// - `execute` may do real work (HTTP, fs, subprocess). Errors should
///   be returned as [`ToolError`] not panics; a panic poisons the
///   loop.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Canonical name. Matches the value the LLM uses in tool-use
    /// calls. Conventionally lowercase, underscore-separated.
    fn name(&self) -> &'static str;

    /// One-paragraph description fed to the LLM. Tell the model when
    /// to use this tool, what the inputs are, and what comes back.
    fn description(&self) -> &'static str;

    /// JSON Schema for the tool's input parameters. The LLM client
    /// converts this to provider-specific tool-use schema format
    /// (OpenAI `tools.function`, Anthropic bare-tools).
    fn input_schema(&self) -> Value;

    /// Execute the tool. `args` is the JSON object the LLM emitted,
    /// already parsed.
    async fn execute(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &'static str {
            "echo"
        }
        fn description(&self) -> &'static str {
            "Echo the `text` arg back unchanged."
        }
        fn input_schema(&self) -> Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" }
                },
                "required": ["text"]
            })
        }
        async fn execute(&self, args: Value, _ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
            args.get("text")
                .and_then(|v| v.as_str())
                .map(|s| ToolOutput::from(s))
                .ok_or_else(|| ToolError::bad_args("missing string field `text`"))
        }
    }

    #[tokio::test]
    async fn tool_round_trip_through_trait_object() {
        let tool: Box<dyn Tool> = Box::new(EchoTool);
        let ctx = ToolCtx::sandboxed("/tmp");
        let out = tool
            .execute(serde_json::json!({"text": "hi"}), &ctx)
            .await
            .unwrap();
        assert!(matches!(out, ToolOutput::Text(ref t) if t == "hi"));
    }

    #[tokio::test]
    async fn missing_required_arg_returns_bad_args() {
        let tool = EchoTool;
        let ctx = ToolCtx::sandboxed("/tmp");
        let err = tool
            .execute(serde_json::json!({}), &ctx)
            .await
            .unwrap_err();
        assert_eq!(err.kind, ToolErrorKind::BadArgs);
    }
}
