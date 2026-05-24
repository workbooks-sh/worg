//! Bundled tool implementations.
//!
//! Each tool module exports either a unit struct that directly
//! implements [`crate::tool::Tool`] (foundational set — bash, read,
//! write) or constructor functions that return configured
//! [`shell::ShellTool`] instances (wavelet, brandwork wrappers).
//!
//! The conventional registration sites are:
//! - [`register_default_tools`] — bash + read + write (used by
//!   minimal-agent tests)
//! - [`register_wavelet_director`] — every tool listed in
//!   `agents/wavelet-director.org`'s `:TOOLS:` drawer that's been
//!   ported to Rust (used by the worg-agent CLI binary)

pub mod bash;
pub mod brandwork;
pub mod read;
pub mod shell;
pub mod substrate;
pub mod video;
pub mod wavelet;
pub mod web;
pub mod worg;
pub mod write;

pub use bash::BashTool;
pub use read::ReadTool;
pub use shell::ShellTool;
pub use write::WriteTool;

/// Register the foundational set: bash, read, write. Suitable for
/// minimal-agent tests that don't need wavelet/brandwork CLIs.
pub fn register_default_tools(registry: &mut crate::tool_registry::ToolRegistry) {
    registry.register(BashTool);
    registry.register(ReadTool);
    registry.register(WriteTool);
}

/// Register every Rust-side tool the wavelet-director agent expects
/// — foundational set + wavelet + brandwork wrappers. Tools listed
/// in director.org but not yet ported (frame_judge, video_judge,
/// wavelet_shot_*, music_gen, dialogue_tts, etc.) will fail dispatch
/// with `unknown tool` until their phases land.
pub fn register_wavelet_director(registry: &mut crate::tool_registry::ToolRegistry) {
    register_default_tools(registry);
    wavelet::register_all(registry);
    brandwork::register_all(registry);
    worg::register_all(registry);
    web::register_all(registry);
    substrate::register_all(registry);
    video::register_all(registry);
}
