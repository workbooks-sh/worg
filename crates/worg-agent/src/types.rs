//! In-memory types the loop reads from. Distinct from [`crate::wire`]
//! which is the on-disk JSON shape — `AgentSpec` carries resolved
//! runtime fields (model, system_prompt, tool_names) that the loop
//! needs but that don't belong in the orchestrator-protocol wire
//! format.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// What a loaded `agents/<name>.org` file boils down to after parsing
/// the headline + `:PROPERTIES:` drawer. Mirrors the Elixir
/// `WorgAgent.Loader.Agent` struct field-for-field so a runtime can be
/// swapped without touching agent definitions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSpec {
    /// `:ID:` property — canonical handle ("wavelet-director").
    pub id: String,
    /// Headline title ("Wavelet Director").
    pub title: String,
    /// `:MODEL:` — provider-qualified slug
    /// ("openrouter/qwen/qwen3-vl-235b-a22b-instruct").
    pub model: String,
    /// Optional system prompt body (free text below the agent
    /// headline, up to but not including the next `*` headline).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// `:CAPABILITIES:` — space-separated tokens. The loop passes
    /// these as `Tool::ctx().capabilities` so tools can gate (e.g.
    /// `bash` requires `bash` capability).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    /// `:TOOLS:` — space-separated tool names. The loop intersects
    /// this with the registry to build the per-turn tool catalog
    /// sent to the LLM.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    /// Catch-all for properties the runtime doesn't interpret —
    /// useful for downstream tooling that wants to read e.g.
    /// `:STAGE_MODEL:` overrides without modifying this crate.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra_properties: BTreeMap<String, String>,
}

/// Trust level surfaced to tools via [`ToolCtx`]. Mirrors the Elixir
/// `WorgAgent.Tool.ctx.trust_level` enum.
///
/// `Sandboxed` is the default when the runtime can't independently
/// verify the workdir is isolated (eval runs in a tempdir, CI). Tools
/// that touch host state outside `working_dir` should refuse unless
/// `Full`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustLevel {
    Sandboxed,
    Full,
}

impl Default for TrustLevel {
    fn default() -> Self {
        Self::Sandboxed
    }
}

/// Cross-cutting context passed to every tool invocation. Read-only:
/// tools may consult fields they care about and MUST NOT mutate.
#[derive(Debug, Clone)]
pub struct ToolCtx {
    /// Directory tools should resolve relative paths against.
    pub working_dir: PathBuf,
    /// Whether the tool may touch host state beyond `working_dir`.
    pub trust_level: TrustLevel,
    /// Task this tool call is part of, for audit logging. `None`
    /// during free-form chat sessions that aren't tied to a plan.
    pub task_id: Option<String>,
    /// Capabilities the agent declared. A tool may inspect this to
    /// refuse execution if its required capability isn't granted.
    pub capabilities: Vec<String>,
}

impl ToolCtx {
    /// Convenience constructor for the common "sandboxed, no task" shape.
    pub fn sandboxed(working_dir: impl Into<PathBuf>) -> Self {
        Self {
            working_dir: working_dir.into(),
            trust_level: TrustLevel::Sandboxed,
            task_id: None,
            capabilities: Vec::new(),
        }
    }

    /// True if `cap` appears in the agent's capability list.
    pub fn has_capability(&self, cap: &str) -> bool {
        self.capabilities.iter().any(|c| c == cap)
    }
}

/// A single content block in a tool result. Mirrors Anthropic's
/// content-block shape; the LLM client translates to OpenAI's
/// `image_url` shape when the wire provider is OpenAI-compat.
///
/// Most tools return a single `Text` block; image-emitting tools
/// (`frame_judge`, `video_judge`, `wavelet_shot_still`) return a
/// `Text` block carrying the JSON verdict followed by one or more
/// `Image` blocks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        source: ImageSource,
    },
}

/// Anthropic-shaped image source. Only `Base64` is emitted by tools
/// today; the URL variant is reserved for future use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    Base64 {
        media_type: String,
        data: String,
    },
}

/// What a tool's `execute` returns on success. A plain `String`
/// short-hand is supported via [`From`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolOutput {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl From<String> for ToolOutput {
    fn from(s: String) -> Self {
        ToolOutput::Text(s)
    }
}

impl From<&str> for ToolOutput {
    fn from(s: &str) -> Self {
        ToolOutput::Text(s.to_string())
    }
}

impl From<Vec<ContentBlock>> for ToolOutput {
    fn from(b: Vec<ContentBlock>) -> Self {
        ToolOutput::Blocks(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_output_text_roundtrips_as_plain_string() {
        let out = ToolOutput::from("hello");
        let json = serde_json::to_string(&out).unwrap();
        // Untagged: a Text variant should serialize as just "hello".
        assert_eq!(json, "\"hello\"");
    }

    #[test]
    fn tool_output_blocks_roundtrips_as_array() {
        let out = ToolOutput::Blocks(vec![ContentBlock::Text {
            text: "verdict".into(),
        }]);
        let json = serde_json::to_string(&out).unwrap();
        assert_eq!(json, r#"[{"type":"text","text":"verdict"}]"#);
    }

    #[test]
    fn has_capability_matches_exact_token() {
        let ctx = ToolCtx {
            working_dir: PathBuf::from("/tmp"),
            trust_level: TrustLevel::Sandboxed,
            task_id: None,
            capabilities: vec!["bash".into(), "read".into()],
        };
        assert!(ctx.has_capability("bash"));
        assert!(!ctx.has_capability("write"));
    }
}
