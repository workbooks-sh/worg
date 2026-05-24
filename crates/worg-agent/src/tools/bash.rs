//! `bash` — run a shell command, capture stdout/stderr/exit. Sibling
//! to Elixir's `WorgAgent.Tools.Bash`.
//!
//! Trust posture: `bash` is the canonical escape hatch. The agent's
//! `:CAPABILITIES:` list must include `bash` or the call is rejected
//! up front. Trust level is NOT checked here — the agent author opted
//! in by listing the capability.
//!
//! The command runs with `cwd = ctx.working_dir`. No PATH munging, no
//! env scrubbing — inherits the parent process's environment.

use async_trait::async_trait;
use serde_json::Value;
use tokio::process::Command;

use crate::tool::{Tool, ToolError};
use crate::types::{ToolCtx, ToolOutput};

pub struct BashTool;

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &'static str {
        "bash"
    }

    fn description(&self) -> &'static str {
        "Run a shell command via /bin/sh -c. Returns combined stdout/stderr \
         and the exit code. Use when no typed tool covers the operation. \
         Working directory is the agent's workdir."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Shell command to execute via /bin/sh -c."
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Maximum wall-clock time, in milliseconds. Default 60000.",
                    "minimum": 1,
                    "maximum": 600000
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        if !ctx.has_capability("bash") {
            return Err(ToolError::capability_denied(
                "bash tool requires the `bash` capability on the agent",
            ));
        }
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::bad_args("missing string field `command`"))?;
        let timeout_ms = args
            .get("timeout_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(60_000);

        let fut = Command::new("/bin/sh")
            .arg("-c")
            .arg(command)
            .current_dir(&ctx.working_dir)
            .output();

        let output = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), fut)
            .await
            .map_err(|_| {
                ToolError::execution(format!("command timed out after {timeout_ms}ms"))
            })?
            .map_err(|e| ToolError::execution(format!("spawn failed: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let exit = output.status.code();

        // Bash tool result is a single string: exit + stdout + stderr.
        // Mirrors the Elixir tool's output shape.
        let body = format!(
            "exit: {exit}\n--- stdout ---\n{stdout}--- stderr ---\n{stderr}",
            exit = exit.map(|c| c.to_string()).unwrap_or_else(|| "?".into()),
        );

        Ok(body.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TrustLevel;
    use std::path::PathBuf;

    fn cap_ctx(caps: &[&str]) -> ToolCtx {
        ToolCtx {
            working_dir: PathBuf::from("/tmp"),
            trust_level: TrustLevel::Sandboxed,
            task_id: None,
            capabilities: caps.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[tokio::test]
    async fn echo_command_returns_stdout() {
        let tool = BashTool;
        let ctx = cap_ctx(&["bash"]);
        let out = tool
            .execute(serde_json::json!({"command": "echo hello"}), &ctx)
            .await
            .unwrap();
        match out {
            ToolOutput::Text(s) => {
                assert!(s.contains("hello"));
                assert!(s.contains("exit: 0"));
            }
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn non_zero_exit_surfaces_in_output() {
        let tool = BashTool;
        let ctx = cap_ctx(&["bash"]);
        let out = tool
            .execute(serde_json::json!({"command": "exit 7"}), &ctx)
            .await
            .unwrap();
        match out {
            ToolOutput::Text(s) => assert!(s.contains("exit: 7")),
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn missing_capability_is_denied() {
        let tool = BashTool;
        let ctx = cap_ctx(&["read"]); // bash not granted
        let err = tool
            .execute(serde_json::json!({"command": "echo ok"}), &ctx)
            .await
            .unwrap_err();
        assert_eq!(err.kind, crate::tool::ToolErrorKind::CapabilityDenied);
    }

    #[tokio::test]
    async fn missing_command_arg_is_bad_args() {
        let tool = BashTool;
        let ctx = cap_ctx(&["bash"]);
        let err = tool
            .execute(serde_json::json!({}), &ctx)
            .await
            .unwrap_err();
        assert_eq!(err.kind, crate::tool::ToolErrorKind::BadArgs);
    }

    #[tokio::test]
    async fn timeout_is_enforced() {
        let tool = BashTool;
        let ctx = cap_ctx(&["bash"]);
        let err = tool
            .execute(
                serde_json::json!({"command": "sleep 5", "timeout_ms": 200}),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.message.contains("timed out"));
    }
}
