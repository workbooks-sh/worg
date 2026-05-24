//! The agent loop. Sibling to Elixir's `WorgAgent.Loop`.
//!
//! ## Flow
//!
//! 1. Build the conversation: system prompt + accumulated history +
//!    new user message.
//! 2. Call the LLM with the per-agent tool catalog (intersection of
//!    [`AgentSpec::tools`] and the registry).
//! 3. If the response carries `tool_calls`, dispatch each through
//!    the registry, append the tool results, persist the round, and
//!    loop with one less round budget.
//! 4. If the response is plain text, persist + return.
//!
//! ## Per-round persistence
//!
//! Mirrors wb-nljb.4 in the Elixir runtime: every round writes the
//! updated conversation + cost to the transcript so external observers
//! (eval runners, dashboards) can stream progress without polling. In
//! the Rust runtime "the transcript" is a JSONL file appended to on
//! every persist call — `Studio.Repo` + `Phoenix.Sync` are not on this
//! side of the contract.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tracing::{debug, info, instrument, warn};

use crate::llm::{ChatRequest, LlmClient, LlmError, LlmResponse, Message, Usage};
use crate::tool_registry::{DispatchError, ToolRegistry};
use crate::types::{AgentSpec, ToolCtx, ToolOutput};

/// Caps the follow-up LLM calls after a tool round. Matches the
/// Elixir default. The hard ceiling clamps runaway agents regardless
/// of per-call overrides.
pub const DEFAULT_MAX_TOOL_ROUNDS: u32 = 10;
pub const HARD_MAX_TOOL_ROUNDS: u32 = 50;

/// Per-turn configuration. The caller constructs one of these per
/// `execute_turn` call; it carries everything the loop needs that
/// isn't baked into the agent spec.
#[derive(Debug, Clone)]
pub struct TurnConfig {
    pub max_tool_rounds: u32,
    /// Where tools that touch the filesystem should resolve relative
    /// paths against.
    pub working_dir: PathBuf,
    /// Optional path to a transcript file. Each round appends one
    /// JSON line; if `None`, persistence is in-memory only and the
    /// caller reads the final messages off the `TurnOutcome`.
    pub transcript_path: Option<PathBuf>,
    /// Task id stamped on telemetry events + tool ctx. `None` for
    /// free-form chat sessions.
    pub task_id: Option<String>,
    /// Per-stage model override (wb-ki6b.7 / Phase 5). When set,
    /// this overrides the agent's `:MODEL:` for this turn. Used by
    /// the scheduler dispatcher to escalate specific stages to
    /// specialist models (e.g. orchestrator runs Qwen3-VL but the
    /// final brand-voice gate runs Opus 4.7).
    pub stage_model: Option<String>,
}

impl TurnConfig {
    /// Common-case constructor: pick sane defaults given a working dir.
    pub fn new(working_dir: impl Into<PathBuf>) -> Self {
        Self {
            max_tool_rounds: DEFAULT_MAX_TOOL_ROUNDS,
            working_dir: working_dir.into(),
            transcript_path: None,
            task_id: None,
            stage_model: None,
        }
    }

    /// Builder: tighten the tool-round cap. Values above the hard
    /// ceiling are silently clamped.
    pub fn with_max_tool_rounds(mut self, n: u32) -> Self {
        self.max_tool_rounds = n.min(HARD_MAX_TOOL_ROUNDS);
        self
    }

    /// Builder: enable JSONL transcript persistence.
    pub fn with_transcript(mut self, path: impl Into<PathBuf>) -> Self {
        self.transcript_path = Some(path.into());
        self
    }

    /// Builder: stamp telemetry + tool ctx with a task id.
    pub fn with_task_id(mut self, id: impl Into<String>) -> Self {
        self.task_id = Some(id.into());
        self
    }

    /// Builder: override the agent's model for just this turn.
    pub fn with_stage_model(mut self, model: impl Into<String>) -> Self {
        self.stage_model = Some(model.into());
        self
    }
}

/// What `execute_turn` returns when the loop terminates normally.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnOutcome {
    /// Full conversation including system, the original user message,
    /// every assistant + tool round, and the final assistant reply.
    pub messages: Vec<Message>,
    /// Aggregated token usage across every LLM round in this turn.
    pub usage: Usage,
    /// Number of LLM rounds executed. 1 = no tools were called; >1 =
    /// at least one tool round followed by a final text reply.
    pub rounds: u32,
}

#[derive(Debug, Error)]
pub enum LoopError {
    #[error(transparent)]
    Llm(#[from] LlmError),
    #[error(transparent)]
    Dispatch(#[from] DispatchError),
    /// The loop ran out of rounds before the LLM produced a terminal
    /// response. Surfaces the partial conversation so the caller can
    /// decide whether to persist or discard it.
    #[error("max tool rounds ({rounds_budget}) exhausted before the LLM produced a terminal response")]
    RoundsExhausted {
        rounds_budget: u32,
        partial_messages: Vec<Message>,
    },
    #[error("transcript write failed: {0}")]
    Transcript(#[from] std::io::Error),
    #[error("transcript serialization failed: {0}")]
    TranscriptSerde(#[from] serde_json::Error),
}

/// One agent turn. Takes the existing conversation + a new user
/// message, returns the conversation after the loop terminates.
///
/// `existing_history` is the conversation up to but not including the
/// new user turn. Pass an empty slice for the first message of a
/// session. The loop prepends `agent.system_prompt` automatically.
#[instrument(skip(client, registry, existing_history, user_message), fields(agent = %agent.id, model = %agent.model))]
pub async fn execute_turn(
    agent: &AgentSpec,
    client: &dyn LlmClient,
    registry: &ToolRegistry,
    existing_history: &[Message],
    user_message: &str,
    config: &TurnConfig,
) -> Result<TurnOutcome, LoopError> {
    let mut conversation: Vec<Message> = Vec::with_capacity(existing_history.len() + 4);
    if let Some(sp) = &agent.system_prompt {
        conversation.push(Message::system(sp.clone()));
    }
    conversation.extend_from_slice(existing_history);
    conversation.push(Message::user(user_message));

    let tool_catalog = build_tool_catalog(agent, registry);
    let ctx = ToolCtx {
        working_dir: config.working_dir.clone(),
        trust_level: Default::default(),
        task_id: config.task_id.clone(),
        capabilities: agent.capabilities.clone(),
    };

    // wb-ki6b.7 — stage_model override wins over agent.model when set.
    let effective_model = config.stage_model.as_deref().unwrap_or(&agent.model);

    let mut total_usage = Usage::default();
    let mut rounds = 0u32;

    loop {
        if rounds >= config.max_tool_rounds {
            // We hit the cap. Surface the partial conversation so the
            // caller can inspect it; the cap is a safety net, not a
            // success path.
            return Err(LoopError::RoundsExhausted {
                rounds_budget: config.max_tool_rounds,
                partial_messages: conversation,
            });
        }

        let request = ChatRequest {
            model: effective_model,
            messages: &conversation,
            tools: &tool_catalog,
        };

        let LlmResponse {
            message,
            usage,
            finish_reason,
        } = client.chat(request).await?;
        rounds += 1;
        debug!(
            ?finish_reason,
            prompt_tokens = usage.prompt_tokens,
            completion_tokens = usage.completion_tokens,
            "LLM round complete"
        );
        accumulate_usage(&mut total_usage, &usage);

        let tool_calls = message.tool_calls.clone();
        conversation.push(message);
        persist_round(&conversation, &total_usage, config)?;

        match tool_calls {
            Some(calls) if !calls.is_empty() => {
                for call in calls {
                    let args = call
                        .function
                        .parsed_arguments()
                        .map_err(|e| LlmError::Malformed(format!("tool args: {e}")))?;
                    let dispatch_result =
                        registry.dispatch(&call.function.name, args, &ctx).await;

                    let content_value = match dispatch_result {
                        Ok(output) => tool_output_to_value(output),
                        Err(err) => {
                            warn!(
                                tool = %call.function.name,
                                error = %err,
                                "tool dispatch failed — surfacing as content to the LLM"
                            );
                            Value::String(format!("ERROR: {err}"))
                        }
                    };

                    conversation.push(Message::tool_result(
                        call.id.clone(),
                        call.function.name.clone(),
                        content_value,
                    ));
                }
                persist_round(&conversation, &total_usage, config)?;
                // Re-enter loop: send the augmented conversation back
                // to the LLM so it can react to the tool results.
                continue;
            }
            _ => {
                info!(rounds, "loop terminated with text response");
                return Ok(TurnOutcome {
                    messages: conversation,
                    usage: total_usage,
                    rounds,
                });
            }
        }
    }
}

fn build_tool_catalog(agent: &AgentSpec, registry: &ToolRegistry) -> Vec<Value> {
    if agent.tools.is_empty() {
        return Vec::new();
    }
    let full = registry.catalog();
    full.into_iter()
        .filter(|entry| {
            entry
                .get("name")
                .and_then(|v| v.as_str())
                .map(|name| agent.tools.iter().any(|t| t == name))
                .unwrap_or(false)
        })
        .collect()
}

fn accumulate_usage(total: &mut Usage, round: &Usage) {
    total.prompt_tokens = total.prompt_tokens.saturating_add(round.prompt_tokens);
    total.completion_tokens = total.completion_tokens.saturating_add(round.completion_tokens);
    total.cost = match (total.cost, round.cost) {
        (Some(a), Some(b)) => Some(a + b),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };
}

fn tool_output_to_value(output: ToolOutput) -> Value {
    match output {
        ToolOutput::Text(s) => Value::String(s),
        ToolOutput::Blocks(b) => serde_json::to_value(b).unwrap_or(Value::Null),
    }
}

fn persist_round(messages: &[Message], usage: &Usage, config: &TurnConfig) -> Result<(), LoopError> {
    let Some(path) = &config.transcript_path else {
        return Ok(());
    };
    let entry = serde_json::json!({
        "kind": "round",
        "message_count": messages.len(),
        "usage": usage,
        "tail": messages.last(),
    });
    let mut line = serde_json::to_string(&entry)?;
    line.push('\n');
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{LlmResponse, ToolCall, ToolCallFunction};
    use async_trait::async_trait;
    use std::sync::Mutex;

    /// Queue-backed stub: pops one canned response per call. Lets a
    /// single test drive a deterministic multi-round turn.
    struct QueueLlm {
        responses: Mutex<Vec<LlmResponse>>,
    }

    impl QueueLlm {
        fn new(responses: Vec<LlmResponse>) -> Self {
            // Use a queue: pop_front semantics via reverse + pop_back.
            let mut v = responses;
            v.reverse();
            Self {
                responses: Mutex::new(v),
            }
        }
    }

    #[async_trait]
    impl LlmClient for QueueLlm {
        async fn chat(&self, _req: ChatRequest<'_>) -> Result<LlmResponse, LlmError> {
            self.responses
                .lock()
                .unwrap()
                .pop()
                .ok_or_else(|| LlmError::Malformed("queue exhausted".into()))
        }
    }

    fn text_response(content: &str) -> LlmResponse {
        LlmResponse {
            message: Message {
                role: "assistant".into(),
                content: Some(Value::String(content.into())),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            usage: Usage {
                prompt_tokens: 50,
                completion_tokens: 10,
                cost: None,
            },
            finish_reason: Some("stop".into()),
        }
    }

    fn tool_call_response(name: &str, args_json: &str) -> LlmResponse {
        LlmResponse {
            message: Message {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![ToolCall {
                    id: format!("call_{name}_1"),
                    kind: "function".into(),
                    function: ToolCallFunction {
                        name: name.into(),
                        arguments: args_json.into(),
                    },
                }]),
                tool_call_id: None,
                name: None,
            },
            usage: Usage {
                prompt_tokens: 50,
                completion_tokens: 10,
                cost: None,
            },
            finish_reason: Some("tool_calls".into()),
        }
    }

    fn test_agent() -> AgentSpec {
        AgentSpec {
            id: "tester".into(),
            title: "Tester".into(),
            model: "test/model".into(),
            system_prompt: Some("You are a test agent.".into()),
            capabilities: vec!["bash".into()],
            tools: vec!["echo".into()],
            extra_properties: Default::default(),
        }
    }

    struct EchoTool;
    #[async_trait]
    impl crate::tool::Tool for EchoTool {
        fn name(&self) -> &'static str {
            "echo"
        }
        fn description(&self) -> &'static str {
            "Echo back the `text` arg."
        }
        fn input_schema(&self) -> Value {
            serde_json::json!({
                "type": "object",
                "properties": {"text": {"type": "string"}},
                "required": ["text"]
            })
        }
        async fn execute(
            &self,
            args: Value,
            _ctx: &ToolCtx,
        ) -> Result<ToolOutput, crate::tool::ToolError> {
            Ok(args["text"].as_str().unwrap_or("").into())
        }
    }

    #[tokio::test]
    async fn single_round_text_response_terminates() {
        let agent = test_agent();
        let llm = QueueLlm::new(vec![text_response("hello back")]);
        let mut registry = ToolRegistry::new();
        registry.register(EchoTool);
        let cfg = TurnConfig::new("/tmp");

        let outcome = execute_turn(&agent, &llm, &registry, &[], "hi", &cfg)
            .await
            .unwrap();

        assert_eq!(outcome.rounds, 1);
        // system + user + assistant
        assert_eq!(outcome.messages.len(), 3);
        assert_eq!(outcome.messages[0].role, "system");
        assert_eq!(outcome.messages[1].role, "user");
        assert_eq!(outcome.messages[2].role, "assistant");
    }

    #[tokio::test]
    async fn tool_round_then_text_response_completes() {
        let agent = test_agent();
        let llm = QueueLlm::new(vec![
            tool_call_response("echo", r#"{"text":"pong"}"#),
            text_response("done"),
        ]);
        let mut registry = ToolRegistry::new();
        registry.register(EchoTool);
        let cfg = TurnConfig::new("/tmp");

        let outcome = execute_turn(&agent, &llm, &registry, &[], "say pong", &cfg)
            .await
            .unwrap();

        // 2 LLM rounds (tool_calls, then final text)
        assert_eq!(outcome.rounds, 2);
        // system + user + assistant(tool_calls) + tool + assistant(text)
        assert_eq!(outcome.messages.len(), 5);
        assert_eq!(outcome.messages[3].role, "tool");
        assert_eq!(outcome.messages[3].tool_call_id.as_deref(), Some("call_echo_1"));
        assert_eq!(outcome.messages[4].role, "assistant");
    }

    #[tokio::test]
    async fn rounds_exhausted_when_llm_keeps_calling_tools() {
        let agent = test_agent();
        // Three tool_calls in a row, no text — will exhaust budget of 2.
        let llm = QueueLlm::new(vec![
            tool_call_response("echo", r#"{"text":"a"}"#),
            tool_call_response("echo", r#"{"text":"b"}"#),
            tool_call_response("echo", r#"{"text":"c"}"#),
        ]);
        let mut registry = ToolRegistry::new();
        registry.register(EchoTool);
        let cfg = TurnConfig::new("/tmp").with_max_tool_rounds(2);

        let err = execute_turn(&agent, &llm, &registry, &[], "loop forever", &cfg)
            .await
            .unwrap_err();

        match err {
            LoopError::RoundsExhausted {
                rounds_budget,
                partial_messages,
            } => {
                assert_eq!(rounds_budget, 2);
                assert!(partial_messages.len() >= 4); // sys + user + at least one round
            }
            other => panic!("expected RoundsExhausted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unknown_tool_failure_is_surfaced_as_content_not_loop_error() {
        let agent = test_agent();
        // The LLM calls a tool the registry doesn't know about. The
        // loop should forward the error to the model as a tool result
        // (matching the Elixir runtime's recovery semantics) — and
        // since the next response is text, the loop completes.
        let llm = QueueLlm::new(vec![
            tool_call_response("missing_tool", "{}"),
            text_response("oh well"),
        ]);
        let registry = ToolRegistry::new(); // intentionally empty
        let cfg = TurnConfig::new("/tmp").with_max_tool_rounds(3);

        let outcome = execute_turn(&agent, &llm, &registry, &[], "use a missing tool", &cfg)
            .await
            .unwrap();
        assert_eq!(outcome.rounds, 2);
        let tool_msg = outcome
            .messages
            .iter()
            .find(|m| m.role == "tool")
            .unwrap();
        match &tool_msg.content {
            Some(Value::String(s)) => assert!(s.starts_with("ERROR: unknown tool")),
            other => panic!("expected error string content, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn transcript_file_gets_one_line_per_round() {
        let agent = test_agent();
        let llm = QueueLlm::new(vec![
            tool_call_response("echo", r#"{"text":"x"}"#),
            text_response("done"),
        ]);
        let mut registry = ToolRegistry::new();
        registry.register(EchoTool);

        let tmp = tempfile::NamedTempFile::new().unwrap();
        let cfg = TurnConfig::new("/tmp").with_transcript(tmp.path());

        execute_turn(&agent, &llm, &registry, &[], "test", &cfg)
            .await
            .unwrap();

        let contents = std::fs::read_to_string(tmp.path()).unwrap();
        // Each persist_round call appends one line. We get:
        //  round 1 assistant (tool_call) + round 1 tool result +
        //  round 2 assistant (text) = 3 lines.
        let line_count = contents.lines().count();
        assert_eq!(line_count, 3, "transcript: {contents}");
    }
}
