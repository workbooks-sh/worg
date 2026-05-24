//! Integration test for `worg orch export agents`.
//!
//! Hand-authored expected JSON lives at
//! `tests/fixtures/agents_workhorse_expected.json` — derived from the
//! canonical Workhorse Elixir struct in
//! `apps/workbooks-runtime/lib/workbooks_runtime/agent_catalog.ex`,
//! projected through the orchestrator-protocol Agent wire format
//! (application-layer fields — tagline / icon / model / tools /
//! system_prompt / emitted_render_kinds / runtime_targets — are
//! intentionally absent from the wire).
//!
//! The test runs the CLI binary against
//! `packages/worg/proposed/agents/workhorse.org` and compares the
//! generated `agents.json` byte-for-byte against the fixture. If the
//! export drifts from the orchestrator-core wire format (or the
//! workhorse.org file changes shape), this test catches it.

use std::process::Command;

/// Path from the cli crate root up to the worg package root.
const WORG_PKG_REL: &str = "../..";

#[test]
fn export_agents_matches_workhorse_fixture() {
    // Resolve paths relative to the cli crate's manifest dir so the
    // test works whether invoked from the workspace root or this crate.
    let crate_dir = env!("CARGO_MANIFEST_DIR");
    let worg_pkg = std::path::Path::new(crate_dir).join(WORG_PKG_REL);
    let workhorse_org = worg_pkg.join("proposed/agents/workhorse.org");
    let fixture_path = std::path::Path::new(crate_dir)
        .join("tests/fixtures/agents_workhorse_expected.json");

    assert!(
        workhorse_org.is_file(),
        "workhorse.org missing at {} — did proposed/agents/workhorse.org get moved?",
        workhorse_org.display()
    );
    assert!(
        fixture_path.is_file(),
        "fixture missing at {} — regenerate it from the workhorse Elixir struct",
        fixture_path.display()
    );

    let out_dir = tempdir();
    let bin = env!("CARGO_BIN_EXE_worg");
    let status = Command::new(bin)
        .arg("orch")
        .arg("export")
        .arg("agents")
        .arg(&workhorse_org)
        .arg("--to")
        .arg(&out_dir)
        .status()
        .expect("running worg binary");
    assert!(status.success(), "worg orch export agents failed: {status:?}");

    let actual_path = out_dir.join("agents.json");
    let actual = std::fs::read_to_string(&actual_path)
        .expect("reading generated agents.json");
    let expected = std::fs::read_to_string(&fixture_path)
        .expect("reading fixture agents.json");

    // Normalize trailing whitespace for diff-stability.
    let actual_n = actual.trim_end();
    let expected_n = expected.trim_end();

    if actual_n != expected_n {
        eprintln!("=== EXPECTED ===\n{expected_n}\n");
        eprintln!("=== ACTUAL ===\n{actual_n}\n");
        panic!(
            "exported agents.json drifted from the workhorse fixture.\n\
             If this is intentional (e.g. orchestrator-core wire format \
             changed, or workhorse.org gained/lost a field), regenerate \
             the fixture at {} and re-run.",
            fixture_path.display()
        );
    }

    // Sanity: the protocol-version field is present and equals the
    // PROTOCOL_VERSION from worg-orch. If the protocol bumps, this
    // assert flags it for explicit handling.
    let parsed: serde_json::Value =
        serde_json::from_str(&actual).expect("agents.json parses as JSON");
    assert_eq!(parsed["version"], serde_json::json!(1));

    // Sanity: application-layer fields MUST NOT leak into the wire
    // export. These checks complement the wire-strict tests in
    // worg-orch but verify the assembled file end-to-end.
    assert!(
        !actual.contains("openrouter"),
        ":MODEL: leaked into wire export"
    );
    assert!(
        !actual.contains("substrate_push"),
        ":TOOLS: leaked into wire export"
    );
    assert!(
        !actual.contains("system_prompt"),
        "system prompt leaked into wire export"
    );
    assert!(
        !actual.contains("runtime_targets"),
        "runtime_targets leaked — should be Watershed-side only"
    );
}

/// Tiny temp-dir helper. Avoids pulling in the `tempfile` crate for
/// one usage; the dir is cleaned up at process exit (test runner
/// invocations are short-lived).
fn tempdir() -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    let unique = format!(
        "worg-cli-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    p.push(unique);
    std::fs::create_dir_all(&p).expect("creating tempdir");
    p
}
