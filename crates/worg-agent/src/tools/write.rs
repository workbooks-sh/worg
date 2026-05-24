//! `write` — write a file. Mirrors `WorgAgent.Tools.Write`.
//!
//! Creates parent directories on demand. Overwrites without warning;
//! the agent is expected to read-then-write when preserving content
//! matters. Refuses to overwrite the workdir or its parent (`.` /
//! `..` / `""`) — those are bug shapes, not desired behaviors.

use async_trait::async_trait;
use serde_json::Value;

use crate::tool::{Tool, ToolError};
use crate::types::{ToolCtx, ToolOutput};

pub struct WriteTool;

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &'static str {
        "write"
    }

    fn description(&self) -> &'static str {
        "Write a UTF-8 string to a file. Parent directories are created \
         on demand. Path is resolved relative to the agent's workdir \
         unless absolute. Overwrites without warning."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path. Relative paths resolve against the workdir."
                },
                "content": {
                    "type": "string",
                    "description": "File body."
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::bad_args("missing string field `path`"))?;
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::bad_args("missing string field `content`"))?;

        if path.is_empty() || path == "." || path == ".." {
            return Err(ToolError::bad_args(format!(
                "refusing to write to suspicious path {path:?}"
            )));
        }

        let resolved = if std::path::Path::new(path).is_absolute() {
            std::path::PathBuf::from(path)
        } else {
            ctx.working_dir.join(path)
        };

        if let Some(parent) = resolved.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                ToolError::execution(format!("mkdir -p {}: {e}", parent.display()))
            })?;
        }

        tokio::fs::write(&resolved, content)
            .await
            .map_err(|e| ToolError::execution(format!("write {}: {e}", resolved.display())))?;

        Ok(format!("wrote {} bytes to {}", content.len(), resolved.display()).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TrustLevel;

    fn ctx_in(dir: &std::path::Path) -> ToolCtx {
        ToolCtx {
            working_dir: dir.to_path_buf(),
            trust_level: TrustLevel::Sandboxed,
            task_id: None,
            capabilities: Vec::new(),
        }
    }

    #[tokio::test]
    async fn writes_relative_path_and_creates_parents() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let _ = WriteTool
            .execute(
                serde_json::json!({"path": "nested/dir/out.txt", "content": "ok"}),
                &ctx,
            )
            .await
            .unwrap();
        let body = std::fs::read_to_string(dir.path().join("nested/dir/out.txt")).unwrap();
        assert_eq!(body, "ok");
    }

    #[tokio::test]
    async fn refuses_suspicious_paths() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let err = WriteTool
            .execute(serde_json::json!({"path": ".", "content": "x"}), &ctx)
            .await
            .unwrap_err();
        assert_eq!(err.kind, crate::tool::ToolErrorKind::BadArgs);
    }

    #[tokio::test]
    async fn missing_content_is_bad_args() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let err = WriteTool
            .execute(serde_json::json!({"path": "x.txt"}), &ctx)
            .await
            .unwrap_err();
        assert_eq!(err.kind, crate::tool::ToolErrorKind::BadArgs);
    }
}
