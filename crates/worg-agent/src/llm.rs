//! LLM client abstraction. The runtime depends on the trait; the
//! OpenRouter implementation is the default impl.
//!
//! Mirrors Elixir's `WorgAgent.Llm`. Provider abstraction is
//! deliberately minimal in Phase 1: one trait method, one OpenAI-
//! compatible JSON shape on the wire. Streaming + multi-provider
//! support are deferred to Phase 5 (per the `wb-ki6b` epic).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// An OpenAI chat-completions-style message. `role` is one of
/// `system`/`user`/`assistant`/`tool`. We use string-typed fields
/// rather than enums so providers can extend with custom roles
/// without a breaking change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    /// Plain string OR an array of content blocks (Anthropic-style).
    /// `None` when the LLM emits a tool_calls-only assistant message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,
    /// Set on `assistant` messages that triggered tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Set on `tool` messages — references the originating call's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Set on `tool` messages — the tool's canonical name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: Some(Value::String(content.into())),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: Some(Value::String(content.into())),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn tool_result(call_id: impl Into<String>, name: impl Into<String>, content: Value) -> Self {
        Self {
            role: "tool".into(),
            content: Some(content),
            tool_calls: None,
            tool_call_id: Some(call_id.into()),
            name: Some(name.into()),
        }
    }
}

/// A tool call the LLM emitted on an assistant turn. The `function`
/// shape matches OpenAI's `tools[].function` schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "tool_call_type_default")]
    pub kind: String,
    pub function: ToolCallFunction,
}

fn tool_call_type_default() -> String {
    "function".into()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    /// Arguments serialized as a JSON string by the LLM, matching the
    /// OpenAI wire format. Callers parse to `serde_json::Value` before
    /// dispatching.
    pub arguments: String,
}

impl ToolCallFunction {
    /// Parse `arguments` from its on-wire string form into a JSON
    /// value. Empty string is treated as `{}`.
    pub fn parsed_arguments(&self) -> Result<Value, serde_json::Error> {
        if self.arguments.is_empty() {
            Ok(Value::Object(serde_json::Map::new()))
        } else {
            serde_json::from_str(&self.arguments)
        }
    }
}

/// Token + cost usage for a single LLM call. Optional because
/// providers don't always return it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: u32,
    #[serde(default)]
    pub completion_tokens: u32,
    /// Some providers return a pre-computed cost. Prefer this over
    /// per-model rate calculation when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
}

/// What an LLM call returns. `message` is the assistant turn; if it
/// carries `tool_calls`, the loop dispatches each and re-calls the
/// LLM with the results appended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmResponse {
    pub message: Message,
    #[serde(default)]
    pub usage: Usage,
    /// Provider's `finish_reason` (`stop` / `tool_calls` / `length` /
    /// `content_filter`). Kept as a string so unknown values from new
    /// providers pass through.
    pub finish_reason: Option<String>,
}

/// LLM client errors. Surface enough detail for the loop to
/// distinguish actionable failures (auth, rate limit, model
/// unavailable) from transient transport issues worth retrying.
#[derive(Debug, Error)]
pub enum LlmError {
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("rate limited: {0}")]
    RateLimit(String),
    /// 200 with an empty assistant message + no tool_calls — the
    /// OpenRouter silent-no-op signature we saw with opus-4.6.
    /// Persisting this would leave the session with an empty turn
    /// and no error.
    #[error("empty LLM response (finish_reason={finish_reason:?}, upstream_error={upstream_error:?})")]
    EmptyResponse {
        finish_reason: Option<String>,
        upstream_error: Option<Value>,
    },
    #[error("LLM HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error("LLM transport error: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("malformed response: {0}")]
    Malformed(String),
}

/// One call against an LLM provider.
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Send the conversation, optionally with a tool catalog, get
    /// back the assistant turn.
    async fn chat(
        &self,
        request: ChatRequest<'_>,
    ) -> Result<LlmResponse, LlmError>;
}

/// One LLM round-trip. Borrows the conversation so the loop can
/// build it once per turn without cloning.
#[derive(Debug)]
pub struct ChatRequest<'a> {
    pub model: &'a str,
    pub messages: &'a [Message],
    /// Tool catalog (output of [`crate::tool_registry::ToolRegistry::catalog`])
    /// already filtered to the agent's `:TOOLS:` list. Empty when the
    /// agent isn't using tools this turn.
    pub tools: &'a [Value],
}

/// OpenRouter (and any other OpenAI-compatible endpoint).
///
/// Construct with [`OpenRouterClient::new`] using the
/// `OPENROUTER_API_KEY` env var, or with an explicit key for tests /
/// embedded use.
pub struct OpenRouterClient {
    api_key: String,
    base_url: String,
    http: reqwest::Client,
}

impl OpenRouterClient {
    pub const DEFAULT_BASE_URL: &'static str = "https://openrouter.ai/api/v1";

    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: Self::DEFAULT_BASE_URL.into(),
            http: reqwest::Client::new(),
        }
    }

    /// Override the base URL — used in tests against a mock server.
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Override the underlying client — useful for shared connection
    /// pools.
    pub fn with_http_client(mut self, http: reqwest::Client) -> Self {
        self.http = http;
        self
    }
}

#[async_trait]
impl LlmClient for OpenRouterClient {
    async fn chat(&self, request: ChatRequest<'_>) -> Result<LlmResponse, LlmError> {
        let url = format!("{}/chat/completions", self.base_url);

        // OpenRouter accepts the OpenAI-compatible payload shape:
        // model, messages, optional tools. We embed `usage.include`
        // so the response carries a cost field when available.
        let mut body = serde_json::json!({
            "model": request.model,
            "messages": request.messages,
            "usage": { "include": true }
        });
        if !request.tools.is_empty() {
            // The catalog entries are name/description/input_schema —
            // wrap each as an OpenAI `tools[].function` entry.
            let wrapped: Vec<Value> = request
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t["name"],
                            "description": t["description"],
                            "parameters": t["input_schema"]
                        }
                    })
                })
                .collect();
            body["tools"] = Value::Array(wrapped);
            body["tool_choice"] = Value::String("auto".into());
        }

        let response = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        let text = response.text().await?;

        match status.as_u16() {
            200 => parse_chat_completions(&text),
            401 | 403 => Err(LlmError::Auth(text)),
            429 => Err(LlmError::RateLimit(text)),
            other => Err(LlmError::Http {
                status: other,
                body: text,
            }),
        }
    }
}

/// Parse an OpenAI chat-completions response body. Public so binary
/// tests can exercise the parser without a live HTTP round-trip.
pub fn parse_chat_completions(body: &str) -> Result<LlmResponse, LlmError> {
    let json: Value =
        serde_json::from_str(body).map_err(|e| LlmError::Malformed(e.to_string()))?;

    let choices = json
        .get("choices")
        .and_then(|c| c.as_array())
        .ok_or_else(|| LlmError::Malformed("missing choices array".into()))?;

    if choices.is_empty() {
        // Mirrors wb-wru8's Elixir branch — surface the upstream
        // error payload so callers can distinguish rate-limit from
        // model-returned-nothing.
        return Err(LlmError::EmptyResponse {
            finish_reason: None,
            upstream_error: json.get("error").cloned(),
        });
    }

    let choice = &choices[0];
    let finish_reason = choice
        .get("finish_reason")
        .and_then(|v| v.as_str())
        .map(String::from);

    let message_val = choice
        .get("message")
        .ok_or_else(|| LlmError::Malformed("choice missing message".into()))?;
    let message: Message = serde_json::from_value(message_val.clone())
        .map_err(|e| LlmError::Malformed(format!("message: {e}")))?;

    // wb-wru8 silent-no-op check: 200 with assistant message that
    // has neither content nor tool_calls.
    if is_empty_assistant(&message) {
        return Err(LlmError::EmptyResponse {
            finish_reason,
            upstream_error: json.get("error").cloned(),
        });
    }

    let usage: Usage = json
        .get("usage")
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    Ok(LlmResponse {
        message,
        usage,
        finish_reason,
    })
}

fn is_empty_assistant(msg: &Message) -> bool {
    let has_tool_calls = msg
        .tool_calls
        .as_ref()
        .map(|c| !c.is_empty())
        .unwrap_or(false);
    if has_tool_calls {
        return false;
    }
    match &msg.content {
        Some(Value::String(s)) if !s.is_empty() => false,
        Some(Value::Array(a)) if !a.is_empty() => false,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_only_response() {
        let body = r#"{
            "choices": [{
                "message": {"role": "assistant", "content": "hi"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 1}
        }"#;
        let resp = parse_chat_completions(body).unwrap();
        assert_eq!(resp.message.role, "assistant");
        assert_eq!(resp.finish_reason.as_deref(), Some("stop"));
        assert_eq!(resp.usage.prompt_tokens, 10);
    }

    #[test]
    fn parses_tool_call_response() {
        let body = r#"{
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "echo", "arguments": "{\"text\":\"hi\"}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }"#;
        let resp = parse_chat_completions(body).unwrap();
        let calls = resp.message.tool_calls.unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "echo");
        assert_eq!(
            calls[0].function.parsed_arguments().unwrap(),
            serde_json::json!({"text": "hi"})
        );
    }

    #[test]
    fn empty_assistant_returns_empty_response_error() {
        // wb-wru8 — opus-4.6 silent no-op.
        let body = r#"{
            "choices": [{
                "message": {"role": "assistant"},
                "finish_reason": "stop"
            }]
        }"#;
        match parse_chat_completions(body).unwrap_err() {
            LlmError::EmptyResponse { finish_reason, .. } => {
                assert_eq!(finish_reason.as_deref(), Some("stop"));
            }
            other => panic!("expected EmptyResponse, got {other:?}"),
        }
    }

    #[test]
    fn empty_choices_array_returns_empty_response_error_with_upstream() {
        // OpenRouter's shape when the downstream provider rate-limits.
        let body = r#"{
            "choices": [],
            "error": {"message": "Rate limited", "code": 429}
        }"#;
        match parse_chat_completions(body).unwrap_err() {
            LlmError::EmptyResponse {
                upstream_error: Some(err),
                ..
            } => {
                assert_eq!(err["code"], 429);
            }
            other => panic!("expected EmptyResponse with upstream, got {other:?}"),
        }
    }

    #[test]
    fn message_helpers_set_expected_roles() {
        assert_eq!(Message::system("foo").role, "system");
        assert_eq!(Message::user("bar").role, "user");
        let tr = Message::tool_result("c1", "echo", Value::String("ok".into()));
        assert_eq!(tr.role, "tool");
        assert_eq!(tr.tool_call_id.as_deref(), Some("c1"));
    }

    #[test]
    fn parsed_arguments_handles_empty_string() {
        let f = ToolCallFunction {
            name: "x".into(),
            arguments: "".into(),
        };
        assert_eq!(f.parsed_arguments().unwrap(), serde_json::json!({}));
    }
}
