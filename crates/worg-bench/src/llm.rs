//! OpenRouter client. Single provider — same surface the BEAM runtime uses
//! for the same model-pluggability reason. Reads OPENROUTER_API_KEY from env.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/chat/completions";

#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Debug, Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ResponseMessage {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: u32,
    #[serde(default)]
    pub completion_tokens: u32,
    #[serde(default)]
    pub total_tokens: u32,
}

#[derive(Debug, Clone)]
pub struct CallResult {
    pub text: String,
    pub latency_ms: u128,
    pub usage: Option<Usage>,
}

pub struct Client {
    inner: reqwest::Client,
    api_key: String,
}

impl Client {
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("OPENROUTER_API_KEY")
            .context("OPENROUTER_API_KEY not set — required to call models")?;
        let inner = reqwest::Client::builder()
            .timeout(Duration::from_secs(180))
            .build()?;
        Ok(Client { inner, api_key })
    }

    pub async fn complete(
        &self,
        model: &str,
        system: Option<&str>,
        user: &str,
    ) -> Result<CallResult> {
        let mut messages = Vec::with_capacity(2);
        if let Some(s) = system {
            messages.push(ChatMessage {
                role: "system",
                content: s,
            });
        }
        messages.push(ChatMessage {
            role: "user",
            content: user,
        });

        let req = ChatRequest {
            model,
            messages,
            temperature: Some(0.0),
            max_tokens: Some(2048),
        };

        let started = std::time::Instant::now();
        let resp = self
            .inner
            .post(OPENROUTER_URL)
            .bearer_auth(&self.api_key)
            .header("HTTP-Referer", "https://github.com/shinyobjectz-sh/workbooks")
            .header("X-Title", "worg-bench")
            .json(&req)
            .send()
            .await
            .context("OpenRouter request failed")?;

        let latency_ms = started.elapsed().as_millis();
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "OpenRouter {status}: {}",
                body.chars().take(500).collect::<String>()
            ));
        }

        let parsed: ChatResponse = resp
            .json()
            .await
            .context("OpenRouter returned non-JSON response")?;
        let text = parsed
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .unwrap_or_default();

        Ok(CallResult {
            text,
            latency_ms,
            usage: parsed.usage,
        })
    }
}

/// Strip surrounding ```...``` or ```org ... ``` fences if the model wrapped
/// the answer. Many models default to fenced output even when asked for plain
/// org text; we don't want fence noise to fail every parse.
pub fn strip_fences(s: &str) -> &str {
    let trimmed = s.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        // skip optional language tag on the first line
        let after_lang = match rest.find('\n') {
            Some(i) => &rest[i + 1..],
            None => return trimmed,
        };
        if let Some(end) = after_lang.rfind("```") {
            return after_lang[..end].trim_end();
        }
    }
    trimmed
}
