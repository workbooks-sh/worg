//! In-process tools for the WORG parser + query API. Unlike the
//! wavelet/brandwork wrappers (which shell out to external CLIs),
//! these call [`worg_parse`] and [`worg_query`] directly — same
//! Rust process, no subprocess overhead, no PATH dependency.
//!
//! Both tools accept either a file path (`path`) OR raw org source
//! (`source`). When both are given, `source` wins.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tool::{Tool, ToolError};
use crate::types::{ToolCtx, ToolOutput};

/// `worg_parse` — parse an org file, return one entry per headline.
pub struct WorgParseTool;

#[async_trait]
impl Tool for WorgParseTool {
    fn name(&self) -> &'static str {
        "worg_parse"
    }

    fn description(&self) -> &'static str {
        "Parse a WORG `.org` file and return its outline as a JSON array \
         of headlines. Each entry carries level, title, tags, properties, \
         and the raw body text. In-process — no subprocess overhead. \
         Pass either `path` (file on disk) or `source` (raw org text)."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Path to a .org file."},
                "source": {"type": "string", "description": "Raw org source text."}
            }
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let src = read_source(&args, ctx).await?;
        let doc = worg_parse::Document::parse(&src);
        let headlines: Vec<_> = doc
            .headlines()
            .into_iter()
            .map(|hl| serialize_headline(&hl))
            .collect();
        let body = serde_json::to_string_pretty(&json!({ "headlines": headlines }))
            .map_err(|e| ToolError::execution(format!("encode: {e}")))?;
        Ok(body.into())
    }
}

/// `worg_query` — apply a query predicate to a parsed org doc.
///
/// Predicate vocabulary mirrors [`worg_query::Predicate`]:
/// - `{"kind": "tag", "value": "ready"}`              → `Predicate::Tag`
/// - `{"kind": "state", "value": "NEXT"}`             → `Predicate::State`
/// - `{"kind": "property", "key": "ID", "value": "wb-foo"}` → `Predicate::Property`
/// - `{"kind": "has_property", "key": "ASSIGNED_AGENT"}` → `Predicate::HasProperty`
/// - `{"kind": "ready"}`                              → `Predicate::Ready`
/// - `{"kind": "assigned", "value": "<agent-id>"}`    → `Predicate::Assigned`
pub struct WorgQueryTool;

#[async_trait]
impl Tool for WorgQueryTool {
    fn name(&self) -> &'static str {
        "worg_query"
    }

    fn description(&self) -> &'static str {
        "Query a WORG `.org` file with a structured predicate. Returns \
         every matching headline as JSON. Use when the agent needs to \
         pick tasks by tag/keyword/property/assignment rather than \
         re-parsing the whole outline. In-process."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Path to a .org file."},
                "source": {"type": "string", "description": "Raw org source text."},
                "predicate": {
                    "type": "object",
                    "description": "Query predicate. {kind: tag|todo_keyword|property|has_property|assigned_to, ...}"
                }
            },
            "required": ["predicate"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let src = read_source(&args, ctx).await?;
        let predicate = parse_predicate(args.get("predicate").ok_or_else(|| {
            ToolError::bad_args("missing `predicate` object")
        })?)?;
        let doc = worg_parse::Document::parse(&src);
        let hits = worg_query::query(&doc, &predicate);
        let serialized: Vec<_> = hits.iter().map(serialize_headline).collect();
        let body = serde_json::to_string_pretty(&json!({ "matches": serialized }))
            .map_err(|e| ToolError::execution(format!("encode: {e}")))?;
        Ok(body.into())
    }
}

async fn read_source(args: &Value, ctx: &ToolCtx) -> Result<String, ToolError> {
    if let Some(s) = args.get("source").and_then(|v| v.as_str()) {
        return Ok(s.to_string());
    }
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::bad_args("provide either `path` or `source`"))?;
    let resolved = if std::path::Path::new(path).is_absolute() {
        std::path::PathBuf::from(path)
    } else {
        ctx.working_dir.join(path)
    };
    tokio::fs::read_to_string(&resolved)
        .await
        .map_err(|e| ToolError::execution(format!("read {}: {e}", resolved.display())))
}

fn parse_predicate(v: &Value) -> Result<worg_query::Predicate, ToolError> {
    let kind = v
        .get("kind")
        .and_then(|x| x.as_str())
        .ok_or_else(|| ToolError::bad_args("predicate missing `kind`"))?;
    let str_field = |name: &str| -> Result<String, ToolError> {
        v.get(name)
            .and_then(|x| x.as_str())
            .map(String::from)
            .ok_or_else(|| ToolError::bad_args(format!("{kind} predicate needs `{name}`")))
    };
    Ok(match kind {
        "tag" => worg_query::Predicate::Tag {
            tag: str_field("value")?,
        },
        "state" => worg_query::Predicate::State {
            state: str_field("value")?,
        },
        "property" => worg_query::Predicate::Property {
            key: str_field("key")?,
            value: str_field("value")?,
        },
        "has_property" => worg_query::Predicate::HasProperty {
            key: str_field("key")?,
        },
        "ready" => worg_query::Predicate::Ready,
        "assigned" => worg_query::Predicate::Assigned {
            agent: str_field("value")?,
        },
        other => {
            return Err(ToolError::bad_args(format!(
                "unknown predicate kind: {other}"
            )))
        }
    })
}

fn serialize_headline(hl: &orgize::ast::Headline) -> Value {
    use orgize::rowan::ast::AstNode as _;
    let level = hl.level() as u32;
    let title = hl.title_raw().trim().to_string();
    let tags = worg_query::headline_tags(hl);
    let mut props = serde_json::Map::new();
    if let Some(p) = hl.properties() {
        for (k, v) in p.iter() {
            props.insert(k.to_string(), Value::String(v.to_string()));
        }
    }
    let body = hl.syntax().text().to_string();
    json!({
        "level": level,
        "title": title,
        "tags": tags,
        "properties": props,
        "body": body,
    })
}

/// Register both WORG tools.
pub fn register_all(registry: &mut crate::tool_registry::ToolRegistry) {
    registry.register(WorgParseTool);
    registry.register(WorgQueryTool);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TrustLevel;

    fn ctx() -> ToolCtx {
        ToolCtx {
            working_dir: std::env::current_dir().unwrap(),
            trust_level: TrustLevel::Sandboxed,
            task_id: None,
            capabilities: Vec::new(),
        }
    }

    #[tokio::test]
    async fn parse_from_source_returns_one_entry_per_headline() {
        let src = "* First\nfoo\n* Second\nbar\n";
        let out = WorgParseTool
            .execute(json!({"source": src}), &ctx())
            .await
            .unwrap();
        let text = match out {
            ToolOutput::Text(s) => s,
            other => panic!("expected text, got {other:?}"),
        };
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["headlines"].as_array().unwrap().len(), 2);
        assert_eq!(parsed["headlines"][0]["title"], "First");
    }

    #[tokio::test]
    async fn query_by_tag_filters_to_matching_headlines() {
        let src = "* A                                                   :alpha:\n\
                   * B                                                   :beta:\n\
                   * C                                                   :alpha:\n";
        let out = WorgQueryTool
            .execute(
                json!({"source": src, "predicate": {"kind":"tag","value":"alpha"}}),
                &ctx(),
            )
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(&match out {
            ToolOutput::Text(s) => s,
            other => panic!("expected text, got {other:?}"),
        })
        .unwrap();
        let matches = parsed["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 2);
    }

    #[tokio::test]
    async fn missing_predicate_fails_cleanly() {
        let err = WorgQueryTool
            .execute(json!({"source": "* X"}), &ctx())
            .await
            .unwrap_err();
        assert_eq!(err.kind, crate::tool::ToolErrorKind::BadArgs);
    }
}
