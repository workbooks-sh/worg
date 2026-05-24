//! Typed `ShellTool` wrappers around the `wavelet` CLI. Each function
//! returns a fully-configured `ShellTool` ready for the registry.
//!
//! Phase 3 of wb-ki6b: ports the four existing Elixir
//! `WorgAgent.Tools.Wavelet*` wrappers. The 11 additional wavelet_*
//! tools listed in wavelet-director.org's `:TOOLS:` (brief_check,
//! screenplay_parse, velocity_*, storyboard_*, continuity_check,
//! transitions_classify, shot_still, shot_txt2vid, music_gen,
//! dialogue_tts) land alongside as the underlying `wavelet` CLI
//! subcommands stabilize — each is a 10-line constructor in this file.

use serde_json::json;

use crate::tools::shell::ShellTool;

/// `wavelet lint` — pre-render structural rules; post-render frame
/// checks when `mp4` is supplied.
pub fn lint() -> ShellTool {
    ShellTool::new(
        "wavelet_lint",
        "Run wavelet lint on a composition HTML. Pre-render mode (path only) \
         runs layout-walk rules — safe-zone, hallucinated-attrs, halo-contrast \
         on text scenes. Post-render mode (path + mp4) additionally runs \
         frame-level rules. Use platform to scope rules to a delivery target \
         (instagram_reels, tiktok, youtube_shorts). Returns wavelet stdout \
         with an exit=<n> marker. Exit 0 = clean; exit 3 = rule failed; \
         exit 2 = arg parse error.",
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Path to the composition HTML."},
                "platform": {
                    "type": "string",
                    "description": "Optional platform slug. instagram_reels | tiktok | youtube_shorts."
                },
                "mp4": {
                    "type": "string",
                    "description": "Optional MP4 path for post-render frame-level checks."
                }
            },
            "required": ["path"]
        }),
        "wavelet",
    )
    .with_argv_prefix(["lint"])
    .with_positional("path")
    .with_flag("platform", "--platform")
    .with_flag("mp4", "--mp4")
}

/// `wavelet render` — composition HTML → MP4.
pub fn render() -> ShellTool {
    ShellTool::new(
        "wavelet_render",
        "Render a wavelet composition HTML to an MP4. Composition must reference \
         scene HTMLs that each contain an inline <video src=\"../shots/shot-N.mp4\"> \
         (NOT data-video-bg — that doesn't render, wb-a2z2). Outputs `out` \
         (default commercial.mp4). Sidecar WAV is auto-muxed when audio cues exist.",
        json!({
            "type": "object",
            "properties": {
                "comp": {"type": "string", "description": "Path to commercial.html."},
                "out": {"type": "string", "description": "Output MP4 path. Default commercial.mp4."},
                "no_audio": {"type": "boolean", "description": "Skip audio muxing. Default false."},
                "aspects": {
                    "type": "string",
                    "description": "Optional aspect ratios to emit. Comma-separated. e.g., 9:16,1:1."
                },
                "frame_budget_secs": {
                    "type": "number",
                    "description": "Optional per-frame budget; render aborts if exceeded."
                }
            },
            "required": ["comp"]
        }),
        "wavelet",
    )
    .with_argv_prefix(["render"])
    .with_positional("comp")
    .with_flag("out", "-o")
    .with_bool_flag("no_audio", "--no-audio")
    .with_flag("aspects", "--aspects")
    .with_flag("frame_budget_secs", "--frame-budget-secs")
}

/// `wavelet screenplay validate` — copy-density vs declared duration.
pub fn screenplay_validate() -> ShellTool {
    ShellTool::new(
        "wavelet_screenplay_validate",
        "Validate that a Fountain screenplay's copy density fits the declared \
         duration. Computes VO time + caption dwell + shot floor against the \
         target with a ±10% tolerance band. Use as the gate BEFORE generating \
         any paid assets — too much copy in too short a spot is unrecoverable. \
         Returns JSON report on stdout. Exit 0 = fits/under_budget; exit 3 = over_budget.",
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Path to the .fountain file."},
                "duration": {"type": "number", "description": "Declared spot duration in seconds."},
                "pretty": {"type": "boolean", "description": "Pretty-print JSON. Default false."}
            },
            "required": ["path", "duration"]
        }),
        "wavelet",
    )
    .with_argv_prefix(["screenplay", "validate"])
    .with_positional("path")
    .with_flag("duration", "--duration")
    .with_bool_flag("pretty", "--pretty")
}

/// `wavelet character define` — register a named character with refs.
pub fn character_define() -> ShellTool {
    ShellTool::new(
        "wavelet_character_define",
        "Register a named character with reference images. Writes a clip-HTML at \
         <workdir>/refs/character/<slug>.clip.html that the storyboard planner \
         reads at plan time. Multiple character_type values (full-body / hands / \
         product-hands) coexist for the same name — the planner picks the right one \
         per shot. The flag is --character-type, NOT --type (clap rejects).",
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Canonical character name. Matches Fountain CHARACTER cues."
                },
                "reference": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Reference image paths or HTTPS URLs. Pass 1-3 (Gemini cap)."
                },
                "character_type": {
                    "type": "string",
                    "enum": ["full-body", "hands", "product-hands"],
                    "description": "full-body | hands | product-hands. Default full-body."
                },
                "workdir": {"type": "string", "description": "Workdir for clip-HTML. Default cwd."}
            },
            "required": ["name", "reference"]
        }),
        "wavelet",
    )
    .with_argv_prefix(["character", "define"])
    .with_positional("name")
    .with_repeated_flag("reference", "--reference")
    .with_flag("character_type", "--character-type")
    .with_flag("workdir", "--workdir")
}

/// `wavelet brief check <path>` — parse + sanity-check a brief markdown.
pub fn brief_check() -> ShellTool {
    ShellTool::new(
        "wavelet_brief_check",
        "Parse a creative brief markdown file and surface any structural \
         issues (missing brand, duration, platform; ambiguous CTA; etc.). \
         Returns a structured report so the agent can decide whether to \
         proceed with screenplay generation or push back on the brief.",
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Path to brief.md."},
                "json": {"type": "boolean", "description": "Emit parsed brief as JSON."},
                "pretty": {"type": "boolean", "description": "Pretty-print JSON."}
            },
            "required": ["path"]
        }),
        "wavelet",
    )
    .with_argv_prefix(["brief", "check"])
    .with_positional("path")
    .with_bool_flag("json", "--json")
    .with_bool_flag("pretty", "--pretty")
}

/// `wavelet screenplay parse <path>` — Fountain → structured JSON tree.
pub fn screenplay_parse() -> ShellTool {
    ShellTool::new(
        "wavelet_screenplay_parse",
        "Parse a Fountain screenplay into structured JSON (scenes, \
         characters, dialogue, action). Emits per-scene files under \
         <workdir>/screenplay/ by default; use legacy_json for a single blob.",
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Path to the Fountain source file."},
                "workdir": {"type": "string", "description": "Workdir (defaults to parent of path)."},
                "legacy_json": {"type": "boolean", "description": "Emit legacy single-blob screenplay.json."},
                "pretty": {"type": "boolean", "description": "Pretty-print JSON."},
                "out": {"type": "string", "description": "Output path for legacy_json mode."}
            },
            "required": ["path"]
        }),
        "wavelet",
    )
    .with_argv_prefix(["screenplay", "parse"])
    .with_positional("path")
    .with_flag("workdir", "--workdir")
    .with_bool_flag("legacy_json", "--legacy-json")
    .with_bool_flag("pretty", "--pretty")
    .with_flag("out", "--out")
}

/// `wavelet velocity propose <screenplay>` — pacing profile from a script.
pub fn velocity_propose() -> ShellTool {
    ShellTool::new(
        "wavelet_velocity_propose",
        "Propose a pacing/BPM velocity profile from a Fountain screenplay. \
         Output drives storyboard plan + music gen so they share the same \
         beat-grid. Returns a JSON velocity profile.",
        json!({
            "type": "object",
            "properties": {
                "screenplay": {"type": "string", "description": "Path to Fountain source."},
                "out": {"type": "string", "description": "Optional output path."},
                "pretty": {"type": "boolean", "description": "Pretty-print JSON."}
            },
            "required": ["screenplay"]
        }),
        "wavelet",
    )
    .with_argv_prefix(["velocity", "propose"])
    .with_positional("screenplay")
    .with_flag("out", "--out")
    .with_bool_flag("pretty", "--pretty")
}

/// `wavelet velocity validate <profile> --against <music>` — BPM agreement.
pub fn velocity_validate() -> ShellTool {
    ShellTool::new(
        "wavelet_velocity_validate",
        "Validate that a velocity profile's BPM matches a real music track \
         within tolerance. Emits a cuts.edl by default for the renderer to \
         snap shot boundaries onto musical onsets.",
        json!({
            "type": "object",
            "properties": {
                "profile": {"type": "string", "description": "Path to velocity profile JSON."},
                "against": {"type": "string", "description": "Path to music file."},
                "tolerance": {"type": "number", "description": "BPM delta threshold. Default 5.0."},
                "window": {"type": "number", "description": "Window radius in seconds. Default 2.0."},
                "fps": {"type": "number", "description": "Frame rate for cuts.edl. Default 30."},
                "no_emit_edl": {"type": "boolean", "description": "Suppress cuts.edl emission."},
                "pretty": {"type": "boolean", "description": "Pretty-print JSON."}
            },
            "required": ["profile", "against"]
        }),
        "wavelet",
    )
    .with_argv_prefix(["velocity", "validate"])
    .with_positional("profile")
    .with_flag("against", "--against")
    .with_flag("tolerance", "--tolerance")
    .with_flag("window", "--window")
    .with_flag("fps", "--fps")
    .with_bool_flag("no_emit_edl", "--no-emit-edl")
    .with_bool_flag("pretty", "--pretty")
}

/// `wavelet storyboard plan <screenplay> --velocity <profile>` — shot list.
pub fn storyboard_plan() -> ShellTool {
    ShellTool::new(
        "wavelet_storyboard_plan",
        "Plan a shot-list storyboard from a Fountain screenplay + velocity \
         profile. Each shot gets duration, aspect, transition type, and a \
         generation prompt. Optionally snaps shot boundaries to music onsets \
         and loads character references from <workdir>/refs/character/.",
        json!({
            "type": "object",
            "properties": {
                "screenplay": {"type": "string", "description": "Path to Fountain source."},
                "velocity": {"type": "string", "description": "Path to velocity profile JSON."},
                "fps": {"type": "number", "description": "Target FPS. Default 30."},
                "resolution": {"type": "string", "description": "WxH. Default 1920x1080."},
                "aspect": {"type": "string", "description": "Aspect ratio (16:9, 9:16, 1:1, ...)."},
                "onsets": {"type": "string", "description": "Pre-rendered music track for snap."},
                "no_snap": {"type": "boolean", "description": "Disable onset snapping."},
                "match_runtime": {"type": "number", "description": "Target runtime in seconds."},
                "workdir": {"type": "string", "description": "Project workdir for char refs."},
                "no_characters": {"type": "boolean", "description": "Disable character-ref auto-load."},
                "out": {"type": "string", "description": "Optional output path."},
                "pretty": {"type": "boolean", "description": "Pretty-print JSON."}
            },
            "required": ["screenplay", "velocity"]
        }),
        "wavelet",
    )
    .with_argv_prefix(["storyboard", "plan"])
    .with_positional("screenplay")
    .with_flag("velocity", "--velocity")
    .with_flag("fps", "--fps")
    .with_flag("resolution", "--resolution")
    .with_flag("aspect", "--aspect")
    .with_flag("onsets", "--onsets")
    .with_bool_flag("no_snap", "--no-snap")
    .with_flag("match_runtime", "--match-runtime")
    .with_flag("workdir", "--workdir")
    .with_bool_flag("no_characters", "--no-characters")
    .with_flag("out", "--out")
    .with_bool_flag("pretty", "--pretty")
}

/// `wavelet storyboard verify <storyboard>` — structural rule pass.
pub fn storyboard_verify() -> ShellTool {
    ShellTool::new(
        "wavelet_storyboard_verify",
        "Verify a storyboard JSON against structural rules: every shot has \
         a generation spec, durations sum to total, character refs resolve, \
         transitions are valid. Returns a verdict report.",
        json!({
            "type": "object",
            "properties": {
                "storyboard": {"type": "string", "description": "Path to storyboard JSON."},
                "json": {"type": "boolean", "description": "Emit report as JSON."}
            },
            "required": ["storyboard"]
        }),
        "wavelet",
    )
    .with_argv_prefix(["storyboard", "verify"])
    .with_positional("storyboard")
    .with_bool_flag("json", "--json")
}

/// `wavelet continuity check <storyboard>` — cross-shot continuity rules.
pub fn continuity_check() -> ShellTool {
    ShellTool::new(
        "wavelet_continuity_check",
        "Check shot-to-shot continuity rules across a storyboard: character \
         appearance consistency, wardrobe carryover, prop placement, scene \
         lighting. Surfaces violations the storyboard planner missed.",
        json!({
            "type": "object",
            "properties": {
                "storyboard": {"type": "string", "description": "Path to storyboard JSON."},
                "json": {"type": "boolean", "description": "Emit full report as JSON."}
            },
            "required": ["storyboard"]
        }),
        "wavelet",
    )
    .with_argv_prefix(["continuity", "check"])
    .with_positional("storyboard")
    .with_bool_flag("json", "--json")
}

/// `wavelet transitions classify <screenplay> --velocity <profile>` — pick
/// per-cut transition kinds.
pub fn transitions_classify() -> ShellTool {
    ShellTool::new(
        "wavelet_transitions_classify",
        "Classify per-cut transitions across a screenplay (hard cut, \
         crossfade, wipe, etc.) using the velocity profile as pacing context. \
         The storyboard planner picks transitions per-shot; this is the \
         standalone classifier used at brief-review time.",
        json!({
            "type": "object",
            "properties": {
                "screenplay": {"type": "string", "description": "Path to Fountain source."},
                "velocity": {"type": "string", "description": "Path to velocity profile JSON."},
                "out": {"type": "string", "description": "Optional output path."},
                "pretty": {"type": "boolean", "description": "Pretty-print JSON."}
            },
            "required": ["screenplay", "velocity"]
        }),
        "wavelet",
    )
    .with_argv_prefix(["transitions", "classify"])
    .with_positional("screenplay")
    .with_flag("velocity", "--velocity")
    .with_flag("out", "--out")
    .with_bool_flag("pretty", "--pretty")
}

/// `wavelet shot still <prompt>` — single still image via configured backend.
pub fn shot_still() -> ShellTool {
    ShellTool::new(
        "wavelet_shot_still",
        "Generate a single still image from a prompt via the configured \
         backend (fal-flux, gemini-imagen, etc.). Supports variants + VLM \
         selection so the agent can sample N and pick the best per a \
         criteria list. Hard cost cap via max_cost (default $0.05).",
        json!({
            "type": "object",
            "properties": {
                "prompt": {"type": "string", "description": "Generation prompt."},
                "backend": {"type": "string", "description": "Backend slug."},
                "image_size": {"type": "string", "description": "Default landscape_16_9."},
                "seed": {"type": "number", "description": "Random seed."},
                "variants": {"type": "number", "description": "Number of variants. Default 1."},
                "select": {"type": "string", "description": "Selection policy. Default max-vlm."},
                "criteria": {"type": "string", "description": "VLM criteria (comma-separated)."},
                "max_variants_cost": {"type": "number", "description": "Aggregate cost ceiling."},
                "dry_run": {"type": "boolean", "description": "Emit spec without API call."},
                "max_cost": {"type": "number", "description": "Max spend per still. Default 0.05."},
                "cache": {"type": "string", "description": "Cache root. Default .wavelet-cache."},
                "out": {"type": "string", "description": "Optional destination path."},
                "pretty": {"type": "boolean", "description": "Pretty-print JSON."}
            },
            "required": ["prompt"]
        }),
        "wavelet",
    )
    .with_argv_prefix(["shot", "still"])
    .with_positional("prompt")
    .with_flag("backend", "--backend")
    .with_flag("image_size", "--image-size")
    .with_flag("seed", "--seed")
    .with_flag("variants", "--variants")
    .with_flag("select", "--select")
    .with_flag("criteria", "--criteria")
    .with_flag("max_variants_cost", "--max-variants-cost")
    .with_bool_flag("dry_run", "--dry-run")
    .with_flag("max_cost", "--max-cost")
    .with_flag("cache", "--cache")
    .with_flag("out", "--out")
    .with_bool_flag("pretty", "--pretty")
}

/// `wavelet shot txt2vid <prompt>` — single text-to-video clip.
pub fn shot_txt2vid() -> ShellTool {
    ShellTool::new(
        "wavelet_shot_txt2vid",
        "Generate a single video clip from a prompt via the configured \
         backend (fal-veo3, fal-kling, etc.). Supports character references \
         (repeatable) for identity-preserving generation. Hard cost cap via \
         max_cost. Optional freezedetect trim removes static head/tail frames.",
        json!({
            "type": "object",
            "properties": {
                "prompt": {"type": "string", "description": "Generation prompt."},
                "backend": {"type": "string", "description": "Backend slug."},
                "duration": {"type": "number", "description": "Clip duration in seconds. Default 4.0."},
                "aspect": {"type": "string", "description": "Aspect ratio. Default 16:9."},
                "negative": {"type": "string", "description": "Negative prompt."},
                "no_default_negatives": {"type": "boolean", "description": "Skip default negatives."},
                "seed": {"type": "number", "description": "Random seed."},
                "variants": {"type": "number", "description": "Number of variants. Default 1."},
                "select": {"type": "string", "description": "Selection policy. Default max-vlm."},
                "max_variants_cost": {"type": "number", "description": "Aggregate cost ceiling."},
                "dry_run": {"type": "boolean", "description": "Emit spec without API call."},
                "max_cost": {"type": "number", "description": "Max spend per clip."},
                "cache": {"type": "string", "description": "Cache root. Default .wavelet-cache."},
                "out": {"type": "string", "description": "Optional destination path."},
                "pretty": {"type": "boolean", "description": "Pretty-print JSON."},
                "no_trim_static": {"type": "boolean", "description": "Skip freezedetect trim."},
                "reference": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Reference images for identity preservation."
                }
            },
            "required": ["prompt"]
        }),
        "wavelet",
    )
    .with_argv_prefix(["shot", "txt2vid"])
    .with_positional("prompt")
    .with_flag("backend", "--backend")
    .with_flag("duration", "--duration")
    .with_flag("aspect", "--aspect")
    .with_flag("negative", "--negative")
    .with_bool_flag("no_default_negatives", "--no-default-negatives")
    .with_flag("seed", "--seed")
    .with_flag("variants", "--variants")
    .with_flag("select", "--select")
    .with_flag("max_variants_cost", "--max-variants-cost")
    .with_bool_flag("dry_run", "--dry-run")
    .with_flag("max_cost", "--max-cost")
    .with_flag("cache", "--cache")
    .with_flag("out", "--out")
    .with_bool_flag("pretty", "--pretty")
    .with_bool_flag("no_trim_static", "--no-trim-static")
    .with_repeated_flag("reference", "--reference")
}

/// `wavelet music gen [--prompt ... | --velocity ...]` — generated track.
pub fn music_gen() -> ShellTool {
    ShellTool::new(
        "wavelet_music_gen",
        "Generate a music track from a prompt or a velocity profile via \
         the configured backend (fal-stable-audio, suno-ai, elevenlabs-music). \
         Either prompt+duration OR velocity must be provided. Hard cost cap.",
        json!({
            "type": "object",
            "properties": {
                "prompt": {"type": "string", "description": "Free-text prompt."},
                "velocity": {"type": "string", "description": "Path to velocity profile JSON."},
                "style": {"type": "string", "description": "Style descriptor. Default cinematic."},
                "duration": {"type": "number", "description": "Duration in seconds."},
                "bpm": {"type": "number", "description": "Target BPM."},
                "backend": {"type": "string", "description": "Backend slug."},
                "variant": {"type": "string", "description": "Model variant override."},
                "seed": {"type": "number", "description": "Random seed."},
                "dry_run": {"type": "boolean", "description": "Emit spec without API call."},
                "max_cost": {"type": "number", "description": "Max spend."},
                "cache": {"type": "string", "description": "Cache root. Default .wavelet-cache."},
                "out": {"type": "string", "description": "Optional destination path."},
                "pretty": {"type": "boolean", "description": "Pretty-print JSON."}
            }
        }),
        "wavelet",
    )
    .with_argv_prefix(["music", "gen"])
    .with_flag("prompt", "--prompt")
    .with_flag("velocity", "--velocity")
    .with_flag("style", "--style")
    .with_flag("duration", "--duration")
    .with_flag("bpm", "--bpm")
    .with_flag("backend", "--backend")
    .with_flag("variant", "--variant")
    .with_flag("seed", "--seed")
    .with_bool_flag("dry_run", "--dry-run")
    .with_flag("max_cost", "--max-cost")
    .with_flag("cache", "--cache")
    .with_flag("out", "--out")
    .with_bool_flag("pretty", "--pretty")
}

/// `wavelet dialogue tts <text>` — text-to-speech for VO/dialogue.
pub fn dialogue_tts() -> ShellTool {
    ShellTool::new(
        "wavelet_dialogue_tts",
        "Synthesize speech from text via the configured TTS backend \
         (ElevenLabs by default). Tunable stability, similarity, and style \
         exaggeration. Hard cost cap.",
        json!({
            "type": "object",
            "properties": {
                "text": {"type": "string", "description": "Text to synthesize."},
                "voice": {"type": "string", "description": "Voice id. Default ElevenLabs Rachel."},
                "backend": {"type": "string", "description": "Backend slug."},
                "model": {"type": "string", "description": "Model id override."},
                "stability": {"type": "number", "description": "Voice stability (0.0-1.0)."},
                "similarity": {"type": "number", "description": "Voice similarity boost (0.0-1.0)."},
                "style": {"type": "number", "description": "Voice style exaggeration (0.0-1.0)."},
                "dry_run": {"type": "boolean", "description": "Emit spec without API call."},
                "max_cost": {"type": "number", "description": "Max spend."},
                "cache": {"type": "string", "description": "Cache root. Default .wavelet-cache."},
                "out": {"type": "string", "description": "Optional destination path."},
                "pretty": {"type": "boolean", "description": "Pretty-print JSON."}
            },
            "required": ["text"]
        }),
        "wavelet",
    )
    .with_argv_prefix(["dialogue", "tts"])
    .with_positional("text")
    .with_flag("voice", "--voice")
    .with_flag("backend", "--backend")
    .with_flag("model", "--model")
    .with_flag("stability", "--stability")
    .with_flag("similarity", "--similarity")
    .with_flag("style", "--style")
    .with_bool_flag("dry_run", "--dry-run")
    .with_flag("max_cost", "--max-cost")
    .with_flag("cache", "--cache")
    .with_flag("out", "--out")
    .with_bool_flag("pretty", "--pretty")
}

/// Register every wavelet ShellTool wrapper in this module. Covers the
/// 15 `wavelet_*` tools listed in `agents/wavelet-director.org`.
pub fn register_all(registry: &mut crate::tool_registry::ToolRegistry) {
    // From Phase 1 (already had Elixir wrappers).
    registry.register(lint());
    registry.register(render());
    registry.register(screenplay_validate());
    registry.register(character_define());
    // Added in Phase 3 — port of the rest of the director's TOOLS list.
    registry.register(brief_check());
    registry.register(screenplay_parse());
    registry.register(velocity_propose());
    registry.register(velocity_validate());
    registry.register(storyboard_plan());
    registry.register(storyboard_verify());
    registry.register(continuity_check());
    registry.register(transitions_classify());
    registry.register(shot_still());
    registry.register(shot_txt2vid());
    registry.register(music_gen());
    registry.register(dialogue_tts());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::Tool;

    #[test]
    fn each_wrapper_has_unique_name() {
        let names: Vec<_> = vec![lint(), render(), screenplay_validate(), character_define()]
            .into_iter()
            .map(|t| t.name())
            .collect();
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "duplicate tool name");
    }

    #[test]
    fn each_wrapper_schema_validates() {
        for tool in [lint(), render(), screenplay_validate(), character_define()] {
            let schema = tool.input_schema();
            assert_eq!(schema["type"], "object", "{}", tool.name());
            assert!(schema["properties"].is_object(), "{}", tool.name());
        }
    }
}
