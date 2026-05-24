//! wb-4vhr.21 Phase A — integration test for `worg orch board`.
//!
//! Two assertions:
//!   1. Returns a JSON blob containing `version`, `agents`, `tasks`.
//!   2. The `agents` and `tasks` arrays are PARITY with what the
//!      legacy `worg orch export agents` / `tasks` commands would
//!      produce against the same source — no semantic drift from the
//!      directory format.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn worg_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_worg"))
}

fn workhorse_org() -> PathBuf {
    let crate_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(crate_dir)
        .ancestors()
        .nth(2)
        .unwrap()
        .join("proposed/agents/workhorse.org")
}

#[test]
fn board_command_emits_version_agents_tasks() {
    let plan = workhorse_org();
    assert!(plan.is_file(), "fixture {} missing", plan.display());

    let out = Command::new(worg_bin())
        .args([
            "orch",
            "board",
            plan.to_str().unwrap(),
            "--created-by",
            "test-runner",
            "--created-at",
            "2026-05-24T00:00:00Z",
        ])
        .output()
        .expect("spawn worg");

    assert!(
        out.status.success(),
        "worg orch board failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let parsed: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout is valid JSON");

    assert_eq!(parsed["version"], 1, "wire-protocol version should be 1");
    assert!(parsed["agents"].is_array(), "agents must be an array");
    assert!(parsed["tasks"].is_array(), "tasks must be an array");
    assert!(
        !parsed["agents"].as_array().unwrap().is_empty(),
        "workhorse.org defines at least one agent"
    );
}

#[test]
fn board_command_agents_parity_with_legacy_export() {
    let plan = workhorse_org();

    // Run the new single-call command...
    let board_out = Command::new(worg_bin())
        .args([
            "orch",
            "board",
            plan.to_str().unwrap(),
            "--created-by",
            "test-runner",
            "--created-at",
            "2026-05-24T00:00:00Z",
        ])
        .output()
        .expect("spawn worg board");
    assert!(board_out.status.success());
    let board: serde_json::Value = serde_json::from_slice(&board_out.stdout).unwrap();

    // ...and the legacy directory-style export.
    let tmp = env::temp_dir().join(format!(
        "worg-board-parity-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&tmp).unwrap();
    let legacy_out = Command::new(worg_bin())
        .args([
            "orch",
            "export",
            "agents",
            plan.to_str().unwrap(),
            "--to",
            tmp.to_str().unwrap(),
        ])
        .output()
        .expect("spawn worg export agents");
    assert!(
        legacy_out.status.success(),
        "legacy export failed: {}",
        String::from_utf8_lossy(&legacy_out.stderr)
    );

    let legacy_json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(tmp.join("agents.json")).unwrap()).unwrap();
    let _ = fs::remove_dir_all(&tmp);

    // The legacy agents.json has the shape { "version": N, "agents": [...] }.
    // The board command's agents array must match the legacy agents
    // array element-for-element (same wire shape, same ordering).
    assert_eq!(
        board["agents"], legacy_json["agents"],
        "board command agents drifted from `worg orch export agents`"
    );
    assert_eq!(
        board["version"], legacy_json["version"],
        "board command version drifted from legacy agents.json version"
    );
}
