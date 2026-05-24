//! `ShellTool` — generic Tool impl that shells out to a CLI binary.
//! Mirrors Elixir's `WorgAgent.Tools.ShellWrapper`. Each typed wrapper
//! (wavelet_lint, brandwork_brief, …) is a small constructor function
//! that returns a configured `ShellTool`.
//!
//! Argv shape:
//!     [binary] + argv_prefix + walk(arg_map against args)
//!
//! `arg_map` is a list of [`ArgSpec`] entries; each can emit zero or
//! more argv elements depending on whether the corresponding key is
//! present in the LLM-supplied `args`. Missing keys are skipped (no
//! flag emitted), matching the Elixir semantics.
//!
//! Output shape: `exit=<n>\n<combined stdout+stderr>` — same as Bash
//! tool so downstream LLM consumers parse uniformly.

use async_trait::async_trait;
use serde_json::Value;
use tokio::process::Command;

use crate::tool::{Tool, ToolError};
use crate::types::{ToolCtx, ToolOutput};

/// One argv-mapping entry. The shape mirrors Elixir's `arg_map` tuple
/// vocabulary; the Rust types are explicit so the compiler catches
/// invalid combinations.
#[derive(Debug, Clone)]
pub enum ArgSpec {
    /// `args[key]` value appended as a positional argv element.
    Positional { key: &'static str },
    /// `args[key]` (a list) appended in order as positional elements.
    /// Scalar values are treated as a single-element list.
    PositionalList { key: &'static str },
    /// `args[key]` emitted as `flag <value>`.
    Flag {
        key: &'static str,
        flag: &'static str,
    },
    /// `args[key]` is a boolean — emit `flag` when truthy, nothing otherwise.
    BoolFlag {
        key: &'static str,
        flag: &'static str,
    },
    /// `args[key]` is a list — emit `flag <value>` per element.
    /// Scalar values are treated as a single-element list.
    RepeatedFlag {
        key: &'static str,
        flag: &'static str,
    },
}

/// Env-var injection. `Literal` is a fixed value; `FromEnv` resolves
/// the parent process env at call time (used by brandwork wrappers
/// to forward BRANDWORK_BASE_URL into the child).
#[derive(Debug, Clone)]
pub enum EnvSpec {
    Literal {
        name: &'static str,
        value: &'static str,
    },
    FromEnv {
        name: &'static str,
        env_name: &'static str,
    },
}

/// Configurable shell-out tool. Construct via [`ShellTool::new`] +
/// builder methods, register the result with [`crate::tool_registry::ToolRegistry`].
pub struct ShellTool {
    name: &'static str,
    description: &'static str,
    input_schema: Value,
    binary: &'static str,
    argv_prefix: Vec<&'static str>,
    arg_map: Vec<ArgSpec>,
    env: Vec<EnvSpec>,
}

impl ShellTool {
    pub fn new(
        name: &'static str,
        description: &'static str,
        input_schema: Value,
        binary: &'static str,
    ) -> Self {
        Self {
            name,
            description,
            input_schema,
            binary,
            argv_prefix: Vec::new(),
            arg_map: Vec::new(),
            env: Vec::new(),
        }
    }

    pub fn with_argv_prefix<I: IntoIterator<Item = &'static str>>(mut self, prefix: I) -> Self {
        self.argv_prefix = prefix.into_iter().collect();
        self
    }

    pub fn with_positional(mut self, key: &'static str) -> Self {
        self.arg_map.push(ArgSpec::Positional { key });
        self
    }

    pub fn with_positional_list(mut self, key: &'static str) -> Self {
        self.arg_map.push(ArgSpec::PositionalList { key });
        self
    }

    pub fn with_flag(mut self, key: &'static str, flag: &'static str) -> Self {
        self.arg_map.push(ArgSpec::Flag { key, flag });
        self
    }

    pub fn with_bool_flag(mut self, key: &'static str, flag: &'static str) -> Self {
        self.arg_map.push(ArgSpec::BoolFlag { key, flag });
        self
    }

    pub fn with_repeated_flag(mut self, key: &'static str, flag: &'static str) -> Self {
        self.arg_map.push(ArgSpec::RepeatedFlag { key, flag });
        self
    }

    pub fn with_env_from(mut self, name: &'static str, env_name: &'static str) -> Self {
        self.env.push(EnvSpec::FromEnv { name, env_name });
        self
    }

    pub fn with_env_literal(mut self, name: &'static str, value: &'static str) -> Self {
        self.env.push(EnvSpec::Literal { name, value });
        self
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &'static str {
        self.name
    }
    fn description(&self) -> &'static str {
        self.description
    }
    fn input_schema(&self) -> Value {
        self.input_schema.clone()
    }
    async fn execute(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let mut argv: Vec<String> = self.argv_prefix.iter().map(|s| s.to_string()).collect();
        for spec in &self.arg_map {
            argv.extend(build_arg(spec, &args)?);
        }

        let mut cmd = Command::new(self.binary);
        cmd.args(&argv).current_dir(&ctx.working_dir);

        for env in &self.env {
            match env {
                EnvSpec::Literal { name, value } => {
                    cmd.env(*name, *value);
                }
                EnvSpec::FromEnv { name, env_name } => {
                    if let Ok(v) = std::env::var(env_name) {
                        if !v.is_empty() {
                            cmd.env(*name, v);
                        }
                    }
                }
            }
        }

        let output = cmd.output().await.map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                ToolError::execution(format!("binary not found on PATH: {}", self.binary))
            }
            _ => ToolError::execution(format!("{} spawn failed: {e}", self.binary)),
        })?;

        let exit = output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "?".into());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Combine like the Elixir version's stderr_to_stdout flag.
        let body = if stderr.is_empty() {
            format!("exit={exit}\n{stdout}")
        } else {
            format!("exit={exit}\n{stdout}{stderr}")
        };

        Ok(body.into())
    }
}

fn build_arg(spec: &ArgSpec, args: &Value) -> Result<Vec<String>, ToolError> {
    Ok(match spec {
        ArgSpec::Positional { key } => match args.get(*key) {
            None | Some(Value::Null) => Vec::new(),
            Some(v) => vec![value_to_string(v)],
        },
        ArgSpec::PositionalList { key } => match args.get(*key) {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(arr)) => arr.iter().map(value_to_string).collect(),
            Some(v) => vec![value_to_string(v)],
        },
        ArgSpec::Flag { key, flag } => match args.get(*key) {
            None | Some(Value::Null) => Vec::new(),
            Some(v) => vec![flag.to_string(), value_to_string(v)],
        },
        ArgSpec::BoolFlag { key, flag } => match args.get(*key) {
            Some(Value::Bool(true)) => vec![flag.to_string()],
            _ => Vec::new(),
        },
        ArgSpec::RepeatedFlag { key, flag } => match args.get(*key) {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(arr)) => arr
                .iter()
                .flat_map(|v| vec![flag.to_string(), value_to_string(v)])
                .collect(),
            Some(v) => vec![flag.to_string(), value_to_string(v)],
        },
    })
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
        // Pass-through for arrays/objects — the LLM probably meant a
        // JSON literal; let the downstream CLI parse.
        other => other.to_string(),
    }
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

    #[test]
    fn build_arg_skips_missing_positional() {
        let spec = ArgSpec::Positional { key: "path" };
        assert_eq!(build_arg(&spec, &serde_json::json!({})).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn build_arg_emits_present_positional() {
        let spec = ArgSpec::Positional { key: "path" };
        assert_eq!(
            build_arg(&spec, &serde_json::json!({"path": "x.html"})).unwrap(),
            vec!["x.html".to_string()]
        );
    }

    #[test]
    fn build_arg_emits_flag_with_value() {
        let spec = ArgSpec::Flag {
            key: "platform",
            flag: "--platform",
        };
        assert_eq!(
            build_arg(&spec, &serde_json::json!({"platform": "tiktok"})).unwrap(),
            vec!["--platform".to_string(), "tiktok".to_string()]
        );
    }

    #[test]
    fn build_arg_emits_bool_flag_only_when_true() {
        let spec = ArgSpec::BoolFlag {
            key: "json",
            flag: "--json",
        };
        assert_eq!(
            build_arg(&spec, &serde_json::json!({"json": true})).unwrap(),
            vec!["--json".to_string()]
        );
        assert_eq!(
            build_arg(&spec, &serde_json::json!({"json": false})).unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            build_arg(&spec, &serde_json::json!({})).unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn build_arg_repeats_flag_per_list_element() {
        let spec = ArgSpec::RepeatedFlag {
            key: "tag",
            flag: "-t",
        };
        assert_eq!(
            build_arg(&spec, &serde_json::json!({"tag": ["a", "b"]})).unwrap(),
            vec![
                "-t".to_string(),
                "a".to_string(),
                "-t".to_string(),
                "b".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn shell_tool_runs_a_real_binary() {
        // Use `/bin/echo` so we don't depend on wavelet being installed.
        let tool = ShellTool::new(
            "echo_tool",
            "echo a positional arg",
            serde_json::json!({"type":"object","properties":{"msg":{"type":"string"}}}),
            "/bin/echo",
        )
        .with_positional("msg");
        let out = tool
            .execute(serde_json::json!({"msg": "hi"}), &ctx())
            .await
            .unwrap();
        match out {
            ToolOutput::Text(s) => {
                assert!(s.contains("exit=0"));
                assert!(s.contains("hi"));
            }
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn missing_binary_surfaces_clear_error() {
        let tool = ShellTool::new(
            "nope",
            "missing binary",
            serde_json::json!({"type":"object"}),
            "/this/binary/does/not/exist",
        );
        let err = tool.execute(serde_json::json!({}), &ctx()).await.unwrap_err();
        assert!(err.message.contains("binary not found") || err.message.contains("spawn failed"));
    }
}
