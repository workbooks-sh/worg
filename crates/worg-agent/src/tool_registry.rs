//! Looks up tool implementations by name. Mirrors Elixir's
//! `WorgAgent.ToolRegistry`.
//!
//! Unlike the Elixir version (which reads `:tools` from
//! `Application.get_env`), the Rust registry is constructed
//! explicitly by the binary entry point. This trades a bit of
//! ergonomics for explicit dependency injection — useful in tests
//! and when embedding the runtime in another Rust process.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use crate::tool::{Tool, ToolError};
use crate::types::{ToolCtx, ToolOutput};

/// A name → tool map. Cheap to clone (uses `Arc` internally).
#[derive(Clone, Default)]
pub struct ToolRegistry {
    by_name: HashMap<&'static str, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            by_name: HashMap::new(),
        }
    }

    /// Register a tool. Last-registration-wins for duplicate names;
    /// debug builds panic on duplicates to catch wiring bugs early.
    pub fn register<T: Tool + 'static>(&mut self, tool: T) -> &mut Self {
        let name = tool.name();
        if cfg!(debug_assertions) && self.by_name.contains_key(name) {
            panic!(
                "tool {name} registered twice — registry construction order is wrong"
            );
        }
        self.by_name.insert(name, Arc::new(tool));
        self
    }

    /// Look up by canonical name. `None` if not registered.
    pub fn lookup(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.by_name.get(name).cloned()
    }

    /// Build the tool-use catalog: a list of `{name, description,
    /// input_schema}` objects. The LLM client converts this into the
    /// provider-specific tool-use payload (OpenAI's
    /// `tools[].function`, Anthropic's bare-tools shape).
    pub fn catalog(&self) -> Vec<Value> {
        self.by_name
            .values()
            .map(|t| {
                serde_json::json!({
                    "name": t.name(),
                    "description": t.description(),
                    "input_schema": t.input_schema()
                })
            })
            .collect()
    }

    /// Dispatch a tool call by name. Returns
    /// `Err(DispatchError::UnknownTool)` if no matching tool is
    /// registered, otherwise propagates the tool's result.
    pub async fn dispatch(
        &self,
        name: &str,
        args: Value,
        ctx: &ToolCtx,
    ) -> Result<ToolOutput, DispatchError> {
        let tool = self
            .lookup(name)
            .ok_or_else(|| DispatchError::UnknownTool(name.to_string()))?;
        tool.execute(args, ctx)
            .await
            .map_err(DispatchError::ToolFailed)
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.by_name.keys().copied()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    #[error(transparent)]
    ToolFailed(#[from] ToolError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct NoopTool;

    #[async_trait]
    impl Tool for NoopTool {
        fn name(&self) -> &'static str {
            "noop"
        }
        fn description(&self) -> &'static str {
            "does nothing"
        }
        fn input_schema(&self) -> Value {
            serde_json::json!({"type": "object"})
        }
        async fn execute(&self, _args: Value, _ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
            Ok("ok".into())
        }
    }

    #[tokio::test]
    async fn unknown_tool_returns_dispatch_error() {
        let registry = ToolRegistry::new();
        let ctx = ToolCtx::sandboxed("/tmp");
        let err = registry
            .dispatch("missing", serde_json::json!({}), &ctx)
            .await
            .unwrap_err();
        match err {
            DispatchError::UnknownTool(name) => assert_eq!(name, "missing"),
            other => panic!("expected UnknownTool, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn registered_tool_dispatches_through_registry() {
        let mut registry = ToolRegistry::new();
        registry.register(NoopTool);
        let ctx = ToolCtx::sandboxed("/tmp");
        let out = registry
            .dispatch("noop", serde_json::json!({}), &ctx)
            .await
            .unwrap();
        assert!(matches!(out, ToolOutput::Text(ref t) if t == "ok"));
    }

    #[test]
    fn catalog_contains_one_entry_per_registered_tool() {
        let mut registry = ToolRegistry::new();
        registry.register(NoopTool);
        let cat = registry.catalog();
        assert_eq!(cat.len(), 1);
        assert_eq!(cat[0]["name"], "noop");
    }
}
