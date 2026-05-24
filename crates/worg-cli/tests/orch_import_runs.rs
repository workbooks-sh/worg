//! Integration test for `worg orch import runs`.
//!
//! End-to-end round-trip:
//!   1. Copy proposed/examples/skills/watershed-autoloop/skill.org to a temp dir.
//!   2. Author a fake `.wb-orch/runs/<task-id>-1.json` and matching
//!      `.wb-orch/tasks/<task-id>.json` for one of the stages.
//!   3. Run `worg orch import runs <copy.org> --from <tempdir/.wb-orch/>`.
//!   4. Assert the LOGBOOK entry landed and the TODO keyword (where
//!      applicable) transitioned.
//!   5. Re-run; assert idempotency — no duplicate LOGBOOK lines, no
//!      additional state changes.

use std::process::Command;

const WORG_PKG_REL: &str = "../..";

#[test]
fn import_runs_appends_logbook_and_is_idempotent() {
    let crate_dir = env!("CARGO_MANIFEST_DIR");
    let worg_pkg = std::path::Path::new(crate_dir).join(WORG_PKG_REL);
    let source_skill_org =
        worg_pkg.join("proposed/examples/skills/watershed-autoloop/skill.org");
    assert!(source_skill_org.is_file());

    let work = tempdir();
    let plan = work.join("plan.org");
    std::fs::copy(&source_skill_org, &plan).expect("seeding plan.org");

    let wb_orch = work.join(".wb-orch");
    let runs = wb_orch.join("runs");
    let tasks = wb_orch.join("tasks");
    std::fs::create_dir_all(&runs).unwrap();
    std::fs::create_dir_all(&tasks).unwrap();

    // Fake a completed run for the "orient" stage.
    let run_json = r#"{
        "id": "orient-1",
        "task": "orient",
        "agent": "workhorse",
        "state": "completed",
        "attempt": 1,
        "started_at": "2026-05-23T20:00:00Z",
        "finished_at": "2026-05-23T20:00:14Z",
        "tokens": { "input": 1240, "output": 180 },
        "cost_usd": 0.0043,
        "result_summary": "working tree clean"
    }"#;
    std::fs::write(runs.join("orient-1.json"), run_json).unwrap();

    // And the corresponding task state — Done — so the import can
    // transition the TODO keyword. (The watershed-autoloop skill.org
    // template doesn't have explicit TODO keywords on its stages, so
    // the transition will silently no-op; we still verify the path
    // runs cleanly.)
    let task_json = r#"{
        "id": "orient",
        "title": "orient",
        "state": "done",
        "created_by": "worg-exporter",
        "created_at": "2026-05-23T20:00:00Z"
    }"#;
    std::fs::write(tasks.join("orient.json"), task_json).unwrap();

    let bin = env!("CARGO_BIN_EXE_worg");

    // First import.
    let out1 = Command::new(bin)
        .args(["orch", "import", "runs"])
        .arg(&plan)
        .args(["--from"])
        .arg(&wb_orch)
        .output()
        .expect("running worg binary");
    assert!(
        out1.status.success(),
        "first import failed: stderr={}",
        String::from_utf8_lossy(&out1.stderr)
    );
    let stderr1 = String::from_utf8_lossy(&out1.stderr).to_string();
    assert!(
        stderr1.contains("imported 1 logbook"),
        "first import should have written exactly one entry; got: {stderr1}"
    );

    let after_first = std::fs::read_to_string(&plan).expect("read updated plan");
    assert!(
        after_first.contains("run=orient-1"),
        "LOGBOOK should contain the run id marker; got:\n{after_first}"
    );
    assert!(
        after_first.contains("state=completed"),
        "LOGBOOK entry should record state=completed"
    );
    assert!(
        after_first.contains("agent=workhorse"),
        "LOGBOOK entry should record the agent"
    );
    assert!(
        after_first.contains("cost=$0.0043"),
        "LOGBOOK entry should record the cost"
    );
    assert!(
        after_first.contains("tokens_in=1240"),
        "LOGBOOK entry should record token counts"
    );
    assert!(
        after_first.contains("dur=14s"),
        "LOGBOOK entry should compute duration from start/finish"
    );

    // Second import — must be idempotent.
    let out2 = Command::new(bin)
        .args(["orch", "import", "runs"])
        .arg(&plan)
        .args(["--from"])
        .arg(&wb_orch)
        .output()
        .expect("running worg binary (second pass)");
    assert!(out2.status.success(), "second import failed");
    let stderr2 = String::from_utf8_lossy(&out2.stderr).to_string();
    assert!(
        stderr2.contains("imported 0 logbook"),
        "second import should have written zero entries; got: {stderr2}"
    );
    assert!(
        stderr2.contains("skipped 1 already-imported"),
        "second import should report 1 skipped; got: {stderr2}"
    );

    let after_second = std::fs::read_to_string(&plan).expect("read plan after second import");
    // File content should be byte-identical (no rewrite if nothing
    // changed) OR differ only in benign ways. Assert no duplicate
    // logbook entries by counting occurrences of the run-id marker.
    let occurrences = after_second.matches("run=orient-1").count();
    assert_eq!(
        occurrences, 1,
        "second import duplicated the LOGBOOK entry; full file:\n{after_second}"
    );
}

#[test]
fn import_runs_dry_run_does_not_modify_file() {
    let crate_dir = env!("CARGO_MANIFEST_DIR");
    let worg_pkg = std::path::Path::new(crate_dir).join(WORG_PKG_REL);
    let source_skill_org =
        worg_pkg.join("proposed/examples/skills/watershed-autoloop/skill.org");

    let work = tempdir();
    let plan = work.join("plan.org");
    std::fs::copy(&source_skill_org, &plan).unwrap();
    let before = std::fs::read_to_string(&plan).unwrap();

    let wb_orch = work.join(".wb-orch");
    let runs = wb_orch.join("runs");
    std::fs::create_dir_all(&runs).unwrap();
    let run_json = r#"{
        "id": "pick-issue-1",
        "task": "pick-issue",
        "agent": "workhorse",
        "state": "running",
        "attempt": 1,
        "started_at": "2026-05-23T20:00:00Z"
    }"#;
    std::fs::write(runs.join("pick-issue-1.json"), run_json).unwrap();

    let bin = env!("CARGO_BIN_EXE_worg");
    let out = Command::new(bin)
        .args(["orch", "import", "runs"])
        .arg(&plan)
        .args(["--from"])
        .arg(&wb_orch)
        .arg("--dry-run")
        .output()
        .expect("running worg binary");
    assert!(out.status.success());

    let after = std::fs::read_to_string(&plan).unwrap();
    assert_eq!(
        before, after,
        "dry-run must not modify the plan file on disk"
    );
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        stderr.contains("dry-run"),
        "dry-run output should announce itself; got: {stderr}"
    );
}

#[test]
fn import_runs_skips_unknown_tasks_silently() {
    // If the .wb-orch directory has runs for tasks the plan file
    // doesn't declare (or that don't carry :ID:), the import must NOT
    // error — it just skips those runs. This is the "orchestrator
    // knows more than this single plan" case.
    let crate_dir = env!("CARGO_MANIFEST_DIR");
    let worg_pkg = std::path::Path::new(crate_dir).join(WORG_PKG_REL);
    let source_skill_org =
        worg_pkg.join("proposed/examples/skills/watershed-autoloop/skill.org");

    let work = tempdir();
    let plan = work.join("plan.org");
    std::fs::copy(&source_skill_org, &plan).unwrap();

    let wb_orch = work.join(".wb-orch");
    let runs = wb_orch.join("runs");
    std::fs::create_dir_all(&runs).unwrap();
    let run_json = r#"{
        "id": "unknown-task-1",
        "task": "unknown-task",
        "agent": "workhorse",
        "state": "completed",
        "attempt": 1,
        "started_at": "2026-05-23T20:00:00Z",
        "finished_at": "2026-05-23T20:00:01Z"
    }"#;
    std::fs::write(runs.join("unknown-task-1.json"), run_json).unwrap();

    let bin = env!("CARGO_BIN_EXE_worg");
    let out = Command::new(bin)
        .args(["orch", "import", "runs"])
        .arg(&plan)
        .args(["--from"])
        .arg(&wb_orch)
        .output()
        .expect("running worg binary");
    assert!(
        out.status.success(),
        "import should succeed even with runs for unknown tasks; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    // No logbook entry for the unknown task, but the run for it was
    // also not "imported" — it was silently skipped because the org
    // file has no headline with that :ID:. Verify the marker is absent.
    let after = std::fs::read_to_string(&plan).unwrap();
    assert!(
        !after.contains("unknown-task-1"),
        "should not have appended a logbook entry for an unknown task"
    );
}

/// CLOCK lines (wb-0mqz.13): import_runs emits a native org-mode
/// CLOCK line for each completed run, alongside the custom kvp
/// entry. Format `CLOCK: [start]--[end] =>  H:MM`, recognized by
/// org-clock-report and any LLM with org-mode training data.
///
/// In-progress runs (no finished_at) do NOT get a CLOCK line —
/// you can't close a clock you haven't seen the end of.
#[test]
fn import_runs_emits_native_clock_lines_for_completed_runs() {
    let crate_dir = env!("CARGO_MANIFEST_DIR");
    let worg_pkg = std::path::Path::new(crate_dir).join(WORG_PKG_REL);
    let source_skill_org =
        worg_pkg.join("proposed/examples/skills/watershed-autoloop/skill.org");

    let work = tempdir();
    let plan = work.join("plan.org");
    std::fs::copy(&source_skill_org, &plan).unwrap();

    let wb_orch = work.join(".wb-orch");
    let runs = wb_orch.join("runs");
    std::fs::create_dir_all(&runs).unwrap();

    // Completed run: 14-minute duration, 2026-05-23 Saturday 20:00 → 20:14.
    let completed = r#"{
        "id": "orient-1",
        "task": "orient",
        "agent": "workhorse",
        "state": "completed",
        "attempt": 1,
        "started_at": "2026-05-23T20:00:00Z",
        "finished_at": "2026-05-23T20:14:00Z",
        "result_summary": "clean"
    }"#;
    std::fs::write(runs.join("orient-1.json"), completed).unwrap();

    // In-progress run: no finished_at → no CLOCK line.
    let in_progress = r#"{
        "id": "pick-issue-1",
        "task": "pick-issue",
        "agent": "workhorse",
        "state": "running",
        "attempt": 1,
        "started_at": "2026-05-23T20:14:00Z"
    }"#;
    std::fs::write(runs.join("pick-issue-1.json"), in_progress).unwrap();

    let bin = env!("CARGO_BIN_EXE_worg");
    let out = Command::new(bin)
        .args(["orch", "import", "runs"])
        .arg(&plan)
        .args(["--from"])
        .arg(&wb_orch)
        .output()
        .expect("running worg binary");
    assert!(out.status.success(), "stderr={}", String::from_utf8_lossy(&out.stderr));

    let after = std::fs::read_to_string(&plan).unwrap();

    // Completed run: CLOCK line present with right shape + duration.
    assert!(
        after.contains("CLOCK: [2026-05-23 Sat 20:00]--[2026-05-23 Sat 20:14] =>  0:14"),
        "expected native CLOCK line for completed run, got:\n{after}"
    );

    // In-progress run: custom entry landed but NO CLOCK line for it.
    // We can't easily assert "no CLOCK line for pick-issue" with simple
    // substring matching since the CLOCK line itself doesn't carry the
    // run id. Instead assert the total CLOCK count is exactly 1.
    let clock_count = after.matches("CLOCK:").count();
    assert_eq!(
        clock_count, 1,
        "expected exactly 1 CLOCK line (only the completed run); got {clock_count}\n{after}"
    );

    // Custom entry for the in-progress run did land (it's marker-
    // tracked, not clock-tracked).
    assert!(
        after.contains("run=pick-issue-1"),
        "in-progress custom entry should still land"
    );
}

fn tempdir() -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    let unique = format!(
        "worg-cli-import-test-{}-{}",
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
