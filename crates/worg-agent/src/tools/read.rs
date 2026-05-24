//! `read` — read a UTF-8 file from disk. Mirrors `WorgAgent.Tools.Read`.
//!
//! Trust posture: paths are resolved relative to `ctx.working_dir`.
//! Absolute paths are accepted (an agent reading a known absolute
//! resource is in scope) but escaping the workdir via `..` is allowed
//! — sandboxing is the runtime host's responsibility, not the tool's.

use async_trait::async_trait;
use serde_json::Value;

use crate::tool::{Tool, ToolError};
use crate::types::{ToolCtx, ToolOutput};

pub struct ReadTool;

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        "read"
    }

    fn description(&self) -> &'static str {
        "Read a UTF-8 text file from disk. Path is resolved relative \
         to the agent's workdir unless absolute. Returns file contents \
         as a string."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path. Relative paths resolve against the workdir."
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::bad_args("missing string field `path`"))?;

        let resolved = if std::path::Path::new(path).is_absolute() {
            std::path::PathBuf::from(path)
        } else {
            ctx.working_dir.join(path)
        };

        match tokio::fs::read_to_string(&resolved).await {
            Ok(s) => Ok(s.into()),
            Err(e) => Err(ToolError::execution(format!(
                "read {}: {e}",
                resolved.display()
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TrustLevel;
    use std::io::Write as _;

    #[tokio::test]
    async fn reads_a_relative_path_against_the_workdir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hello.txt");
        std::fs::File::create(&path)
            .unwrap()
            .write_all(b"world")
            .unwrap();

        let ctx = ToolCtx {
            working_dir: dir.path().to_path_buf(),
            trust_level: TrustLevel::Sandboxed,
            task_id: None,
            capabilities: Vec::new(),
        };
        let out = ReadTool
            .execute(serde_json::json!({"path": "hello.txt"}), &ctx)
            .await
            .unwrap();
        match out {
            ToolOutput::Text(s) => assert_eq!(s, "world"),
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn missing_file_returns_execution_error() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolCtx {
            working_dir: dir.path().to_path_buf(),
            trust_level: TrustLevel::Sandboxed,
            task_id: None,
            capabilities: Vec::new(),
        };
        let err = ReadTool
            .execute(serde_json::json!({"path": "nope.txt"}), &ctx)
            .await
            .unwrap_err();
        assert_eq!(err.kind, crate::tool::ToolErrorKind::Execution);
    }
}
