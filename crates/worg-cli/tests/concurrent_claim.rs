//! wb-nlln.18 — verify that two concurrent `worg claim` invocations
//! cannot both succeed on the same task.
//!
//! Before this work, both processes would read the file in their TODO
//! state, both would transition in-memory to DOING, and the second
//! writer's atomic_write would silently clobber the first. After this
//! work, an advisory file lock + state CAS guarantees exactly one
//! claim wins; the other surfaces a clean StateMismatch error.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

fn worg_bin() -> PathBuf {
    // CARGO_BIN_EXE_<name> is set by Cargo for integration tests.
    PathBuf::from(env!("CARGO_BIN_EXE_worg"))
}

fn write_fixture(tmp: &std::path::Path) -> PathBuf {
    let plan = tmp.join("plan.org");
    fs::write(
        &plan,
        "* TODO Cake\n:PROPERTIES:\n:ID:       cake-1\n:END:\n\n",
    )
    .unwrap();
    plan
}

#[test]
fn concurrent_claim_yields_exactly_one_winner() {
    let tmp = tempdir();
    let plan = write_fixture(&tmp);
    let bin = worg_bin();

    // Spawn N concurrent claim attempts. Without the lock + CAS, you'd
    // typically see all N succeed (and silently clobber each other).
    // With both in place, exactly one wins; the others report
    // StateMismatch via non-zero exit + stderr.
    const N: usize = 6;
    let (tx, rx) = mpsc::channel();

    for i in 0..N {
        let bin = bin.clone();
        let plan = plan.clone();
        let tx = tx.clone();
        thread::spawn(move || {
            let agent = format!("agent-{i}");
            let out = Command::new(&bin)
                .args([
                    "claim",
                    plan.to_str().unwrap(),
                    "cake-1",
                    &format!("--agent={agent}"),
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .expect("spawn worg");
            tx.send((agent, out)).unwrap();
        });
    }
    drop(tx);

    let mut winners = Vec::new();
    let mut losers = Vec::new();
    while let Ok((agent, out)) = rx.recv_timeout(Duration::from_secs(20)) {
        if out.status.success() {
            winners.push(agent);
        } else {
            losers.push((agent, String::from_utf8_lossy(&out.stderr).into_owned()));
        }
    }

    assert_eq!(
        winners.len(),
        1,
        "exactly one claim must win (winners={winners:?}, losers={losers:?})"
    );
    assert_eq!(
        losers.len(),
        N - 1,
        "the rest must fail with StateMismatch"
    );

    // The winning agent's slug should be the one stamped on disk.
    let final_contents = fs::read_to_string(&plan).unwrap();
    let winner_slug = winners.first().unwrap();
    assert!(
        final_contents.contains("* DOING Cake"),
        "expected DOING in final file:\n{final_contents}"
    );
    assert!(
        final_contents.contains(&format!(":ASSIGNED_AGENT: {winner_slug}")),
        "expected :ASSIGNED_AGENT: {winner_slug} in final file:\n{final_contents}"
    );

    // Loser errors should mention state mismatch or claim failure
    // (the exact wording is anyhow-wrapped — assert on the substring
    // the human-facing CLI emits).
    for (_, stderr) in &losers {
        assert!(
            stderr.contains("state mismatch") || stderr.contains("claim"),
            "loser stderr should explain the failure, got: {stderr}"
        );
    }
}

#[test]
fn claim_already_done_task_fails_cleanly() {
    let tmp = tempdir();
    let plan = write_fixture(&tmp);
    let bin = worg_bin();

    // First claim wins.
    Command::new(&bin)
        .args([
            "claim",
            plan.to_str().unwrap(),
            "cake-1",
            "--agent=first",
        ])
        .status()
        .unwrap()
        .success()
        .then_some(())
        .expect("first claim must succeed");

    // Force the task into DONE.
    Command::new(&bin)
        .args([
            "transition",
            plan.to_str().unwrap(),
            "cake-1",
            "DONE",
        ])
        .status()
        .unwrap();

    // A claim against a DONE task must fail with a state-mismatch
    // error (not silently clobber).
    let out = Command::new(&bin)
        .args([
            "claim",
            plan.to_str().unwrap(),
            "cake-1",
            "--agent=second",
        ])
        .output()
        .unwrap();

    assert!(
        !out.status.success(),
        "claiming an already-DONE task must not succeed"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("state mismatch") || stderr.contains("claim"),
        "claim-of-DONE stderr should explain: {stderr}"
    );

    // File still says DONE — no silent clobbering.
    let after = fs::read_to_string(&plan).unwrap();
    assert!(after.contains("* DONE Cake"));
    // The ASSIGNED_AGENT from the first claim is still there.
    assert!(after.contains(":ASSIGNED_AGENT: first"));
}

// Minimal tempdir without pulling in the tempfile crate — keeps the
// test suite's dep footprint flat.
struct TempDir {
    path: PathBuf,
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

impl std::ops::Deref for TempDir {
    type Target = std::path::Path;
    fn deref(&self) -> &std::path::Path {
        &self.path
    }
}

fn tempdir() -> TempDir {
    let mut p = env::temp_dir();
    p.push(format!(
        "worg-cli-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&p).unwrap();
    TempDir { path: p }
}
