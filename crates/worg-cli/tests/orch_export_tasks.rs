//! Integration test for `worg orch export tasks`.
//!
//! Runs the CLI binary against the canonical watershed-autoloop DAG
//! fixture at packages/worg/proposed/examples/skills/watershed-autoloop/skill.org
//! and asserts the expected wire-format JSON lands in the output dir.
//!
//! Determinism: `--created-at` is pinned so the byte-diff against the
//! fixture is reproducible across machines and clocks.

use std::process::Command;

const WORG_PKG_REL: &str = "../..";
const FIXED_TS: &str = "2026-05-23T20:00:00Z";

#[test]
fn export_tasks_against_watershed_autoloop_skill() {
    let crate_dir = env!("CARGO_MANIFEST_DIR");
    let worg_pkg = std::path::Path::new(crate_dir).join(WORG_PKG_REL);
    let skill_org = worg_pkg.join("proposed/examples/skills/watershed-autoloop/skill.org");
    let fixture_path = std::path::Path::new(crate_dir)
        .join("tests/fixtures/task_autoloop_iteration_expected.json");

    assert!(
        skill_org.is_file(),
        "watershed-autoloop/skill.org missing at {}",
        skill_org.display()
    );
    assert!(
        fixture_path.is_file(),
        "fixture missing at {}",
        fixture_path.display()
    );

    let out_dir = tempdir();
    let bin = env!("CARGO_BIN_EXE_worg");
    let status = Command::new(bin)
        .args([
            "orch",
            "export",
            "tasks",
        ])
        .arg(&skill_org)
        .args(["--to"])
        .arg(&out_dir)
        .args(["--created-at", FIXED_TS])
        .status()
        .expect("running worg binary");
    assert!(status.success(), "worg orch export tasks failed: {status:?}");

    // The watershed-autoloop skill.org has exactly these 9 stages.
    // If this list drifts, the skill file changed shape — update the
    // fixture and this list together.
    let expected_ids = [
        "autoloop-iteration",
        "orient",
        "pick-issue",
        "claim-issue",
        "implement",
        "deploy",
        "verify",
        "commit-push",
        "close-issue",
    ];
    for id in expected_ids {
        let p = out_dir.join(format!("{id}.json"));
        assert!(p.is_file(), "missing exported task: {}", p.display());
        let raw = std::fs::read_to_string(&p).unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(&raw).expect("task JSON parses");
        assert_eq!(parsed["id"], serde_json::json!(id), "id mismatch in {}", p.display());
        assert_eq!(
            parsed["created_by"],
            serde_json::json!("worg-exporter"),
            "created_by drifted in {}",
            p.display()
        );
        assert_eq!(
            parsed["created_at"],
            serde_json::json!(FIXED_TS),
            "created_at drifted in {}",
            p.display()
        );
        // Application-layer leakage check: retry_policy and budget
        // are runtime-shape concerns and must NOT appear in the
        // exported JSON. (blocker USED to be on this list — as
        // of wb-qk6l.3 it's a documented extension field, surfaced
        // when the source declared :BLOCKER:. See the dedicated
        // blocker assertion below.)
        assert!(
            !raw.contains("retry_policy"),
            "retry_policy leaked into wire export at {}",
            p.display()
        );
        assert!(
            !raw.contains("budget") || raw.contains("\"tags\""),
            "budget leaked into wire JSON at {}",
            p.display()
        );
    }

    // blocker extension field (wb-qk6l.3): tasks WITH a
    // `:BLOCKER:` declaration in the .org source must surface it
    // in the JSON; tasks without it must omit the key entirely
    // (skip_serializing_if equivalent).
    let expected_deps: &[(&str, &[&str])] = &[
        ("autoloop-iteration", &[]),
        ("orient", &[]),
        ("pick-issue", &["orient"]),
        ("claim-issue", &["pick-issue"]),
        ("implement", &["claim-issue"]),
        ("deploy", &["implement"]),
        ("verify", &["deploy"]),
        ("commit-push", &["verify"]),
        ("close-issue", &["commit-push"]),
    ];
    for (id, deps) in expected_deps {
        let p = out_dir.join(format!("{id}.json"));
        let raw = std::fs::read_to_string(&p).unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(&raw).expect("task JSON parses");
        if deps.is_empty() {
            assert!(
                parsed.get("blocker").is_none(),
                "blocker appeared on a task that declared none: {}",
                p.display()
            );
        } else {
            let got = parsed["blocker"]
                .as_array()
                .unwrap_or_else(|| panic!("blocker absent on {}", p.display()))
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect::<Vec<_>>();
            let want: Vec<String> = deps.iter().map(|s| s.to_string()).collect();
            assert_eq!(
                got,
                want,
                "blocker mismatch on {}",
                p.display()
            );
        }
    }

    // Outline-ancestry parent edges: every non-root stage should have
    // parent = "autoloop-iteration".
    for id in &expected_ids[1..] {
        let p = out_dir.join(format!("{id}.json"));
        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(
            parsed["parent"],
            serde_json::json!("autoloop-iteration"),
            "parent edge missing or wrong in {}",
            p.display()
        );
    }

    // Byte-diff the canonical fixture against autoloop-iteration.json.
    let actual = std::fs::read_to_string(out_dir.join("autoloop-iteration.json"))
        .expect("read actual");
    let expected = std::fs::read_to_string(&fixture_path).expect("read fixture");
    let actual_n = actual.trim_end();
    let expected_n = expected.trim_end();
    if actual_n != expected_n {
        eprintln!("=== EXPECTED ===\n{expected_n}\n");
        eprintln!("=== ACTUAL ===\n{actual_n}\n");
        panic!(
            "autoloop-iteration.json drifted from fixture. \
             If this is intentional, regenerate the fixture at {}.",
            fixture_path.display()
        );
    }
}

fn tempdir() -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    let unique = format!(
        "worg-cli-tasks-test-{}-{}",
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

/// Exporter surfaces `:TRIGGER:` as a `trigger` extension JSON key
/// when the source declared it (wb-0mqz.4). Mirrors the `:BLOCKER:`
/// extension behavior — both org-edna properties round-trip through
/// the wire format.
#[test]
fn export_tasks_emits_trigger_extension_when_source_declares_it() {
    let dir = tempdir();
    let plan_path = dir.join("plan.org");
    std::fs::write(
        &plan_path,
        "\
* TODO A
:PROPERTIES:
:ID: a
:TRIGGER: [[id:b]] [[id:c]]
:END:

* TODO B
:PROPERTIES:
:ID: b
:END:

* TODO C
:PROPERTIES:
:ID: c
:END:
",
    )
    .expect("writing plan");

    let out_dir = dir.join("out");
    let bin = env!("CARGO_BIN_EXE_worg");
    let status = Command::new(bin)
        .args(["orch", "export", "tasks"])
        .arg(&plan_path)
        .args(["--to"])
        .arg(&out_dir)
        .args(["--created-at", FIXED_TS])
        .status()
        .expect("running worg binary");
    assert!(status.success(), "export must succeed: {status:?}");

    let a_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out_dir.join("a.json")).unwrap()).unwrap();
    let triggers: Vec<_> = a_json["trigger"]
        .as_array()
        .expect("trigger array")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(triggers, vec!["b".to_string(), "c".to_string()]);

    // Tasks without :TRIGGER: omit the key entirely.
    let b_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out_dir.join("b.json")).unwrap()).unwrap();
    assert!(
        b_json.get("trigger").is_none(),
        "trigger key should be absent on tasks that declared none, got: {b_json}"
    );
}

/// `worg orch export tasks` must refuse to run if the plan contains
/// a :BLOCKER: cycle (wb-0mqz.6). Failing at export time gives the
/// author immediate feedback instead of producing a JSON dump that
/// the orchestrator would silently stall on.
#[test]
fn export_tasks_refuses_to_export_a_cyclic_plan() {
    let dir = tempdir();
    let plan_path = dir.join("cyclic.org");
    std::fs::write(
        &plan_path,
        "\
* TODO A
:PROPERTIES:
:ID: a
:BLOCKER: [[id:b]]
:END:

* TODO B
:PROPERTIES:
:ID: b
:BLOCKER: [[id:a]]
:END:
",
    )
    .expect("writing plan");

    let out_dir = dir.join("out");
    let bin = env!("CARGO_BIN_EXE_worg");
    let output = Command::new(bin)
        .args(["orch", "export", "tasks"])
        .arg(&plan_path)
        .args(["--to"])
        .arg(&out_dir)
        .args(["--created-at", FIXED_TS])
        .output()
        .expect("running worg binary");

    assert!(
        !output.status.success(),
        "worg orch export tasks must FAIL on a cyclic plan, got success"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("E007") && stderr.contains("cycle"),
        "expected E007 cycle diagnostic in stderr, got: {stderr}"
    );

    // And the output directory should be empty — no JSON written.
    if out_dir.exists() {
        let entries: Vec<_> = std::fs::read_dir(&out_dir)
            .expect("reading out dir")
            .filter_map(|e| e.ok())
            .collect();
        assert!(
            entries.is_empty(),
            "out dir should be empty after refused export, got: {entries:?}"
        );
    }
}
