//! frame_judge + video_judge tools + shared video/VLM helpers.
//!
//! Phase 4 of wb-ki6b. Mirrors Elixir's `WorgAgent.Tools.VideoHelpers`,
//! `FrameJudge`, and `VideoJudge`. Both tools:
//!
//! 1. Extract JPEG frames from an MP4 via ffmpeg subprocess.
//! 2. Send the frames + a prompt to the configured video VLM
//!    (default `google/gemini-2.5-pro` via OpenRouter) as an
//!    OpenAI-shaped vision request — works against Gemini, Qwen3-VL,
//!    Kimi K2.5, Claude (vision-capable), GPT-4o.
//! 3. Return Anthropic-shaped content blocks: a text block with the
//!    VLM verdict followed by the extracted frames as image blocks
//!    (so the LLM sees both the judgment and the source frames).
//!
//! Model selection lives at the runtime edge, not in the agent — the
//! agent doesn't know whether a Gemini call or a Kimi call serviced
//! the request. Override the default via `WORG_AGENT_VIDEO_MODEL` env.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use serde_json::{json, Value};

use crate::llm::Message;
use crate::tool::{Tool, ToolError};
use crate::types::{ContentBlock, ImageSource, ToolCtx, ToolOutput};

const DEFAULT_MODEL: &str = "google/gemini-2.5-pro";
const FFMPEG_TIMEOUT: Duration = Duration::from_secs(30);
const VLM_TIMEOUT: Duration = Duration::from_secs(90);

/// Resolve the video-judge model. Env override takes precedence so
/// ops can swap Gemini → Kimi at runtime without recompiling.
fn configured_model() -> String {
    std::env::var("WORG_AGENT_VIDEO_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string())
}

/// Extract a single JPEG frame at `ts_sec` from `mp4_path`. Returns
/// the base64-encoded bytes + the media type.
async fn extract_frame(
    mp4_path: &std::path::Path,
    ts_sec: f64,
) -> Result<(String, &'static str), ToolError> {
    let tmp_path = {
        let mut p = std::env::temp_dir();
        let rand = format!(
            "worg_agent_frame_{}_{}.jpg",
            std::process::id(),
            // not cryptographic; collision-free enough for short-lived temps
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        );
        p.push(rand);
        p
    };

    let ts_str = format!("{ts_sec:.3}");
    let args = [
        "-y",
        "-ss",
        &ts_str,
        "-i",
        mp4_path.to_str().ok_or_else(|| {
            ToolError::bad_args(format!("mp4_path is not valid UTF-8: {mp4_path:?}"))
        })?,
        "-frames:v",
        "1",
        "-q:v",
        "5",
        tmp_path
            .to_str()
            .ok_or_else(|| ToolError::execution("temp path is not valid UTF-8"))?,
    ];

    let output = tokio::time::timeout(
        FFMPEG_TIMEOUT,
        tokio::process::Command::new("ffmpeg")
            .args(args)
            .stderr(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .output(),
    )
    .await
    .map_err(|_| ToolError::execution("ffmpeg timed out (30s)"))?
    .map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => {
            ToolError::execution("ffmpeg not found on PATH")
        }
        _ => ToolError::execution(format!("ffmpeg spawn: {e}")),
    })?;

    if !output.status.success() {
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(ToolError::execution(format!(
            "ffmpeg exit {}: {combined}",
            output.status.code().unwrap_or(-1)
        )));
    }

    let bytes = tokio::fs::read(&tmp_path)
        .await
        .map_err(|e| ToolError::execution(format!("read extracted frame: {e}")))?;
    let _ = tokio::fs::remove_file(&tmp_path).await;

    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok((b64, "image/jpeg"))
}

/// Send the prompt + frames to the configured video VLM via
/// OpenRouter and return the assistant's text response.
async fn judge_frames(
    prompt: &str,
    frames: &[(String, &'static str)],
) -> Result<String, ToolError> {
    let api_key = std::env::var("OPENROUTER_API_KEY")
        .map_err(|_| ToolError::execution("OPENROUTER_API_KEY not set"))?;
    let model = configured_model();

    let mut content_parts: Vec<Value> =
        vec![json!({"type": "text", "text": prompt})];
    for (b64, mt) in frames {
        content_parts.push(json!({
            "type": "image_url",
            "image_url": { "url": format!("data:{mt};base64,{b64}") }
        }));
    }

    let body = json!({
        "model": model,
        "messages": [{
            "role": "user",
            "content": content_parts
        }]
    });

    let client = reqwest::Client::builder()
        .timeout(VLM_TIMEOUT)
        .build()
        .map_err(|e| ToolError::execution(format!("client: {e}")))?;

    let resp = client
        .post("https://openrouter.ai/api/v1/chat/completions")
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| ToolError::execution(format!("VLM POST: {e}")))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| ToolError::execution(format!("VLM body: {e}")))?;
    if !status.is_success() {
        return Err(ToolError::execution(format!(
            "VLM HTTP {status}: {text}"
        )));
    }
    let parsed: Value = serde_json::from_str(&text)
        .map_err(|e| ToolError::execution(format!("VLM JSON: {e}\nbody: {text}")))?;
    let verdict = parsed["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| ToolError::execution(format!("VLM returned no content: {text}")))?;
    if verdict.is_empty() {
        return Err(ToolError::execution("VLM returned empty content"));
    }
    Ok(verdict.to_string())
}

/// Convert (b64, media_type) frames into ContentBlock::Image entries.
fn frames_as_blocks(frames: &[(String, &'static str)]) -> Vec<ContentBlock> {
    frames
        .iter()
        .map(|(b64, mt)| ContentBlock::Image {
            source: ImageSource::Base64 {
                media_type: (*mt).to_string(),
                data: b64.clone(),
            },
        })
        .collect()
}

fn resolve_mp4_path(raw: &str, ctx: &ToolCtx) -> PathBuf {
    let p = std::path::Path::new(raw);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        ctx.working_dir.join(p)
    }
}

// ── frame_judge ────────────────────────────────────────────────────────

pub struct FrameJudgeTool;

#[async_trait]
impl Tool for FrameJudgeTool {
    fn name(&self) -> &'static str {
        "frame_judge"
    }

    fn description(&self) -> &'static str {
        "Extract frames from an MP4 at the given timestamps (seconds) and \
         judge them against a prompt question or rubric using the configured \
         video-capable VLM. Returns the VLM verdict text plus each frame as \
         an image content block so the calling agent sees both the judgment \
         and the source frames. Use after rendering a clip to validate visual \
         quality before declaring DONE."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "mp4_path": {"type": "string", "description": "Path to the MP4 to inspect."},
                "timestamps_sec": {
                    "type": "array",
                    "items": {"type": "number"},
                    "description": "Seconds into the video to extract frames from. e.g., [1.5, 6.0, 11.0, 16.0]."
                },
                "prompt": {"type": "string", "description": "What to ask the VLM about the frames."}
            },
            "required": ["mp4_path", "timestamps_sec", "prompt"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let mp4_path_raw = args
            .get("mp4_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::bad_args("missing `mp4_path`"))?;
        let mp4_path = resolve_mp4_path(mp4_path_raw, ctx);
        let timestamps: Vec<f64> = args
            .get("timestamps_sec")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ToolError::bad_args("missing `timestamps_sec` array"))?
            .iter()
            .map(|v| {
                v.as_f64()
                    .ok_or_else(|| ToolError::bad_args(format!("non-number timestamp: {v}")))
            })
            .collect::<Result<_, _>>()?;
        if timestamps.is_empty() {
            return Err(ToolError::bad_args("timestamps_sec is empty"));
        }
        if timestamps.len() > 12 {
            return Err(ToolError::bad_args(format!(
                "too many frames ({}); cap is 12",
                timestamps.len()
            )));
        }
        let prompt = args
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::bad_args("missing `prompt`"))?;

        if !tokio::fs::try_exists(&mp4_path).await.unwrap_or(false) {
            return Err(ToolError::execution(format!(
                "mp4_path does not exist: {}",
                mp4_path.display()
            )));
        }

        let mut frames = Vec::with_capacity(timestamps.len());
        for ts in &timestamps {
            frames.push(extract_frame(&mp4_path, *ts).await?);
        }

        let verdict = judge_frames(prompt, &frames).await?;

        let mut blocks = vec![ContentBlock::Text { text: verdict }];
        blocks.extend(frames_as_blocks(&frames));
        Ok(ToolOutput::Blocks(blocks))
    }
}

// ── video_judge ────────────────────────────────────────────────────────

pub struct VideoJudgeTool;

#[async_trait]
impl Tool for VideoJudgeTool {
    fn name(&self) -> &'static str {
        "video_judge"
    }

    fn description(&self) -> &'static str {
        "Hand a whole MP4 to the configured video-capable VLM and ask it \
         to score against a rubric. Samples 4 evenly-spaced frames \
         (approximately quartiles) and runs them as a single VLM call. \
         Cheaper than letting the agent decide which timestamps to inspect. \
         Returns verdict + the sampled frames as image content blocks."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "mp4_path": {"type": "string", "description": "Path to the MP4."},
                "prompt": {"type": "string", "description": "Rubric / question to evaluate the spot against."},
                "frame_count": {
                    "type": "number",
                    "description": "Number of evenly-spaced frames to sample. Default 4, cap 8."
                }
            },
            "required": ["mp4_path", "prompt"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let mp4_path_raw = args
            .get("mp4_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::bad_args("missing `mp4_path`"))?;
        let mp4_path = resolve_mp4_path(mp4_path_raw, ctx);
        let prompt = args
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::bad_args("missing `prompt`"))?;
        let count = args
            .get("frame_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(4)
            .clamp(1, 8) as usize;

        if !tokio::fs::try_exists(&mp4_path).await.unwrap_or(false) {
            return Err(ToolError::execution(format!(
                "mp4_path does not exist: {}",
                mp4_path.display()
            )));
        }

        let duration = probe_duration_secs(&mp4_path).await?;
        let timestamps = evenly_spaced(duration, count);

        let mut frames = Vec::with_capacity(timestamps.len());
        for ts in &timestamps {
            frames.push(extract_frame(&mp4_path, *ts).await?);
        }

        let verdict = judge_frames(prompt, &frames).await?;
        let mut blocks = vec![ContentBlock::Text { text: verdict }];
        blocks.extend(frames_as_blocks(&frames));
        Ok(ToolOutput::Blocks(blocks))
    }
}

/// Run `ffprobe` to read the MP4 duration in seconds.
async fn probe_duration_secs(mp4_path: &std::path::Path) -> Result<f64, ToolError> {
    let output = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(mp4_path)
        .output()
        .await
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                ToolError::execution("ffprobe not found on PATH")
            }
            _ => ToolError::execution(format!("ffprobe spawn: {e}")),
        })?;
    if !output.status.success() {
        return Err(ToolError::execution(format!(
            "ffprobe exit {}: {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let s = String::from_utf8_lossy(&output.stdout);
    s.trim()
        .parse::<f64>()
        .map_err(|e| ToolError::execution(format!("parse duration {s:?}: {e}")))
}

/// Evenly-spaced timestamps inside (0, duration) — never at 0 or
/// `duration` exactly, since those are often black/transition frames.
fn evenly_spaced(duration: f64, count: usize) -> Vec<f64> {
    if count == 0 || duration <= 0.0 {
        return Vec::new();
    }
    // Quartile-style sampling: avoid the very first/last frames.
    (1..=count)
        .map(|i| duration * (i as f64) / ((count + 1) as f64))
        .collect()
}

// Eliminate the dead-code warning on the imported Message — the
// trait is kept available for any follow-up that extends judge_frames
// to receive a full Message struct from the loop.
#[allow(dead_code)]
fn _keep_message_in_scope(_: Message) {}

pub fn register_all(registry: &mut crate::tool_registry::ToolRegistry) {
    registry.register(FrameJudgeTool);
    registry.register(VideoJudgeTool);
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
    fn evenly_spaced_returns_count_timestamps_inside_duration() {
        let ts = evenly_spaced(20.0, 4);
        assert_eq!(ts.len(), 4);
        // (1/5)*20, (2/5)*20, (3/5)*20, (4/5)*20 = 4, 8, 12, 16
        assert!((ts[0] - 4.0).abs() < 1e-9);
        assert!((ts[3] - 16.0).abs() < 1e-9);
        assert!(ts.iter().all(|&t| t > 0.0 && t < 20.0));
    }

    #[test]
    fn evenly_spaced_empty_on_zero_count_or_duration() {
        assert!(evenly_spaced(10.0, 0).is_empty());
        assert!(evenly_spaced(0.0, 4).is_empty());
    }

    #[tokio::test]
    async fn frame_judge_missing_args_fails() {
        let err = FrameJudgeTool
            .execute(json!({}), &ctx())
            .await
            .unwrap_err();
        assert_eq!(err.kind, crate::tool::ToolErrorKind::BadArgs);
    }

    #[tokio::test]
    async fn frame_judge_missing_mp4_file_fails_loudly() {
        let err = FrameJudgeTool
            .execute(
                json!({
                    "mp4_path": "/nonexistent/x.mp4",
                    "timestamps_sec": [1.0],
                    "prompt": "anything"
                }),
                &ctx(),
            )
            .await
            .unwrap_err();
        assert!(err.message.contains("does not exist"));
    }

    #[tokio::test]
    async fn frame_judge_rejects_too_many_frames() {
        // Reach the 12-frame cap.
        let stamps: Vec<f64> = (0..15).map(|i| i as f64).collect();
        let err = FrameJudgeTool
            .execute(
                json!({
                    "mp4_path": "x.mp4",
                    "timestamps_sec": stamps,
                    "prompt": "test"
                }),
                &ctx(),
            )
            .await
            .unwrap_err();
        assert_eq!(err.kind, crate::tool::ToolErrorKind::BadArgs);
        assert!(err.message.contains("cap is 12"));
    }
}
