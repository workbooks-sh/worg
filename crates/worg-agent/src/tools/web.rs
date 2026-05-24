//! HTTP tools — `web_fetch` and `web_search`. Both are in-process
//! (reqwest) rather than shelling out to curl, so they pick up
//! consistent timeout + TLS behavior.
//!
//! `web_search` proxies to Exa (the default), Brave, or a configured
//! alternative. The provider is selected via the `WEB_SEARCH_PROVIDER`
//! env var; API keys come from provider-specific env vars
//! (`EXA_API_KEY`, `BRAVE_SEARCH_API_KEY`, etc.). Tools fail with
//! `Execution` and an actionable message when the required key is
//! missing — the agent surfaces this back to the user.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tool::{Tool, ToolError};
use crate::types::{ToolCtx, ToolOutput};

pub struct WebFetchTool;

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &'static str {
        "web_fetch"
    }

    fn description(&self) -> &'static str {
        "GET a URL and return its body as text. 30-second hard timeout; \
         max response 5 MiB. Use for fetching public pages, JSON APIs, \
         or markdown documents the agent wants to read. Not for binaries."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "HTTP(S) URL to fetch."},
                "accept": {
                    "type": "string",
                    "description": "Optional Accept header (e.g. application/json)."
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let url = args
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::bad_args("missing `url`"))?;

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| ToolError::execution(format!("client build: {e}")))?;

        let mut req = client.get(url);
        if let Some(accept) = args.get("accept").and_then(|v| v.as_str()) {
            req = req.header("accept", accept);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| ToolError::execution(format!("GET {url}: {e}")))?;

        let status = resp.status();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| ToolError::execution(format!("read body: {e}")))?;
        if bytes.len() > 5 * 1024 * 1024 {
            return Err(ToolError::execution(format!(
                "response too large ({} bytes; cap is 5 MiB)",
                bytes.len()
            )));
        }
        let body = String::from_utf8_lossy(&bytes);
        Ok(format!("status: {status}\n{body}").into())
    }
}

pub struct WebSearchTool;

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &'static str {
        "web_search"
    }

    fn description(&self) -> &'static str {
        "Web search via the configured provider (Exa by default — set \
         WEB_SEARCH_PROVIDER=brave to switch). Returns top N results \
         with title, URL, and snippet. Provider API keys come from \
         EXA_API_KEY / BRAVE_SEARCH_API_KEY env vars."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Search query."},
                "limit": {
                    "type": "number",
                    "description": "Max results. Default 10, hard cap 25."
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::bad_args("missing `query`"))?;
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10)
            .min(25);

        let provider = std::env::var("WEB_SEARCH_PROVIDER")
            .ok()
            .unwrap_or_else(|| "exa".to_string());

        match provider.as_str() {
            "exa" => search_exa(query, limit).await,
            "brave" => search_brave(query, limit).await,
            other => Err(ToolError::execution(format!(
                "unknown WEB_SEARCH_PROVIDER: {other} (supported: exa, brave)"
            ))),
        }
    }
}

async fn search_exa(query: &str, limit: u64) -> Result<ToolOutput, ToolError> {
    let key = std::env::var("EXA_API_KEY")
        .map_err(|_| ToolError::execution("EXA_API_KEY not set"))?;
    let body = json!({
        "query": query,
        "numResults": limit,
        "type": "neural",
        "useAutoprompt": true
    });
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| ToolError::execution(format!("client: {e}")))?;
    let resp = client
        .post("https://api.exa.ai/search")
        .header("x-api-key", key)
        .json(&body)
        .send()
        .await
        .map_err(|e| ToolError::execution(format!("exa POST: {e}")))?;
    let text = resp
        .text()
        .await
        .map_err(|e| ToolError::execution(format!("exa read: {e}")))?;
    Ok(text.into())
}

async fn search_brave(query: &str, limit: u64) -> Result<ToolOutput, ToolError> {
    let key = std::env::var("BRAVE_SEARCH_API_KEY")
        .map_err(|_| ToolError::execution("BRAVE_SEARCH_API_KEY not set"))?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| ToolError::execution(format!("client: {e}")))?;
    let resp = client
        .get("https://api.search.brave.com/res/v1/web/search")
        .header("x-subscription-token", key)
        .header("accept", "application/json")
        .query(&[("q", query), ("count", &limit.to_string())])
        .send()
        .await
        .map_err(|e| ToolError::execution(format!("brave GET: {e}")))?;
    let text = resp
        .text()
        .await
        .map_err(|e| ToolError::execution(format!("brave read: {e}")))?;
    Ok(text.into())
}

pub fn register_all(registry: &mut crate::tool_registry::ToolRegistry) {
    registry.register(WebFetchTool);
    registry.register(WebSearchTool);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TrustLevel;
    use std::path::PathBuf;

    fn ctx() -> ToolCtx {
        ToolCtx {
            working_dir: PathBuf::from("/tmp"),
            trust_level: TrustLevel::Sandboxed,
            task_id: None,
            capabilities: Vec::new(),
        }
    }

    #[tokio::test]
    async fn web_fetch_requires_url() {
        let err = WebFetchTool
            .execute(json!({}), &ctx())
            .await
            .unwrap_err();
        assert_eq!(err.kind, crate::tool::ToolErrorKind::BadArgs);
    }

    #[tokio::test]
    async fn web_search_requires_query() {
        let err = WebSearchTool
            .execute(json!({}), &ctx())
            .await
            .unwrap_err();
        assert_eq!(err.kind, crate::tool::ToolErrorKind::BadArgs);
    }

    #[tokio::test]
    async fn web_search_surfaces_missing_provider_key() {
        // Save and unset to make the test deterministic regardless
        // of the dev env.
        let saved = std::env::var("EXA_API_KEY").ok();
        let saved_provider = std::env::var("WEB_SEARCH_PROVIDER").ok();
        unsafe {
            std::env::remove_var("EXA_API_KEY");
            std::env::remove_var("WEB_SEARCH_PROVIDER");
        }
        let err = WebSearchTool
            .execute(json!({"query": "test"}), &ctx())
            .await
            .unwrap_err();
        assert!(err.message.contains("EXA_API_KEY"));
        unsafe {
            if let Some(v) = saved {
                std::env::set_var("EXA_API_KEY", v);
            }
            if let Some(v) = saved_provider {
                std::env::set_var("WEB_SEARCH_PROVIDER", v);
            }
        }
    }
}
