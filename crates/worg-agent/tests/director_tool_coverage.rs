//! Integration test: `register_wavelet_director` must cover every
//! tool name in `agents/wavelet-director.org`'s `:TOOLS:` drawer,
//! minus tools whose phase hasn't landed yet.
//!
//! Catches "added a tool to the agent definition but forgot to wire
//! it into the runtime registry" — which would otherwise only fail
//! at the agent's first tool_call.

use std::collections::HashSet;
use std::path::PathBuf;

use worg_agent::tool_registry::ToolRegistry;
use worg_agent::tools;

/// Tools listed in director.org that we deliberately have NOT ported
/// yet. Empty as of Phase 4 (wb-ki6b.6) — frame_judge + video_judge
/// landed there.
const PHASE_DEFERRED: &[&str] = &[];

fn director_tool_list() -> HashSet<String> {
    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "..",
        "..",
        "proposed",
        "agents",
        "wavelet-director.org",
    ]
    .iter()
    .collect();

    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

    let line = src
        .lines()
        .find(|l| l.trim_start().starts_with(":TOOLS:"))
        .expect("no :TOOLS: line in director.org");
    let after = line
        .trim_start()
        .strip_prefix(":TOOLS:")
        .unwrap_or("")
        .trim();

    after
        .split_whitespace()
        .filter(|t| !PHASE_DEFERRED.contains(t))
        .map(String::from)
        .collect()
}

fn registry_tool_names() -> HashSet<String> {
    let mut registry = ToolRegistry::new();
    tools::register_wavelet_director(&mut registry);
    registry.names().map(String::from).collect()
}

#[test]
fn every_director_tool_is_registered() {
    let want = director_tool_list();
    let have = registry_tool_names();
    let missing: Vec<_> = want.difference(&have).cloned().collect();
    assert!(
        missing.is_empty(),
        "tools listed in wavelet-director.org but NOT registered by \
         register_wavelet_director (excluding PHASE_DEFERRED): {missing:?}\n\
         Either implement them, or add to PHASE_DEFERRED in this test."
    );
}

#[test]
fn no_unexpected_extra_tools_in_registry() {
    // Drift the other way: if we register a tool that the director
    // doesn't list, the agent will never call it and we're wasting
    // tokens describing it in the catalog. Allow a small set of
    // foundational extras we deliberately ship that director may
    // not list.
    let want = director_tool_list();
    let have = registry_tool_names();
    let allowed_extras: HashSet<String> = HashSet::new(); // none today
    let extras: Vec<_> = have
        .difference(&want)
        .filter(|t| !allowed_extras.contains(*t))
        .cloned()
        .collect();
    assert!(
        extras.is_empty(),
        "registered tools not listed in director.org (and not in \
         allowed_extras): {extras:?}"
    );
}
