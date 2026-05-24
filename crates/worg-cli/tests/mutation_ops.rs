//! Integration test for the six standalone-CLI mutation subcommands
//! shipped in wb-4vhr.26: `worg ready / claim / transition / log /
//! result / close`.
//!
//! Strategy: write a small fixture, run each subcommand in sequence,
//! assert the file mutates as documented. Each command does its own
//! atomic write (tempfile + rename), so this also verifies the writes
//! survive a fresh load on the next invocation.

use std::process::Command;

fn worg_bin() -> &'static str {
    env!("CARGO_BIN_EXE_worg")
}

fn read(path: &std::path::Path) -> String {
    std::fs::read_to_string(path).expect("reading fixture")
}

#[test]
fn mutation_subcommands_roundtrip_through_the_file() {
    let tmp = tempdir();
    let plan = tmp.as_path().join("plan.org");
    std::fs::write(
        &plan,
        "\
#+TITLE: Probe
#+TODO: TODO NEXT WAITING DOING | DONE CANCELED FAILED

* NEXT Pick a thing :stage:
:PROPERTIES:
:ID: pick
:END:

#+begin_src bash
echo hello
#+end_src

* TODO Verify :stage:
:PROPERTIES:
:ID: verify
:END:
",
    )
    .unwrap();

    // ─── ready: lists NEXT + TODO, filter by --agent ──────────────
    let out = Command::new(worg_bin())
        .args(["ready"])
        .arg(&plan)
        .output()
        .expect("worg ready");
    assert!(out.status.success(), "ready failed: {out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("\"pick\""), "ready missing pick: {stdout}");
    assert!(stdout.contains("\"verify\""), "ready missing verify: {stdout}");

    // ─── claim pick --agent=workhorse ─────────────────────────────
    let out = Command::new(worg_bin())
        .args(["claim"])
        .arg(&plan)
        .args(["pick", "--agent=workhorse"])
        .output()
        .expect("worg claim");
    assert!(out.status.success(), "claim failed: {:?}", String::from_utf8_lossy(&out.stderr));
    let body = read(&plan);
    assert!(body.contains("* DOING Pick a thing"), "claim should transition to DOING: {body}");
    assert!(
        body.contains(":ASSIGNED_AGENT: workhorse"),
        "claim should set :ASSIGNED_AGENT:: {body}"
    );

    // ─── log pick "started" — appends to :LOGBOOK: ────────────────
    let out = Command::new(worg_bin())
        .args(["log"])
        .arg(&plan)
        .args(["pick", "started research"])
        .output()
        .expect("worg log");
    assert!(out.status.success(), "log failed: {:?}", String::from_utf8_lossy(&out.stderr));
    let body = read(&plan);
    assert!(body.contains(":LOGBOOK:"), "log should create :LOGBOOK: drawer: {body}");
    assert!(body.contains("started research"), "log entry missing: {body}");

    // ─── result pick "42" — writes #+RESULTS: block ──────────────
    let out = Command::new(worg_bin())
        .args(["result"])
        .arg(&plan)
        .args(["pick", "42"])
        .output()
        .expect("worg result");
    assert!(out.status.success(), "result failed: {:?}", String::from_utf8_lossy(&out.stderr));
    let body = read(&plan);
    assert!(body.contains("#+RESULTS:"), "result should emit #+RESULTS: block: {body}");

    // ─── close pick --reason="shipped" ───────────────────────────
    let out = Command::new(worg_bin())
        .args(["close"])
        .arg(&plan)
        .args(["pick", "--reason=shipped"])
        .output()
        .expect("worg close");
    assert!(out.status.success(), "close failed: {:?}", String::from_utf8_lossy(&out.stderr));
    let body = read(&plan);
    assert!(body.contains("* DONE Pick a thing"), "close should transition to DONE: {body}");
    assert!(body.contains("shipped"), "close reason should land in :LOGBOOK:: {body}");

    // ─── transition verify DOING — generic state setter ──────────
    let out = Command::new(worg_bin())
        .args(["transition"])
        .arg(&plan)
        .args(["verify", "DOING"])
        .output()
        .expect("worg transition");
    assert!(out.status.success(), "transition failed: {:?}", String::from_utf8_lossy(&out.stderr));
    let body = read(&plan);
    assert!(body.contains("* DOING Verify"), "transition should set DOING: {body}");
}

#[test]
fn ready_filters_by_agent_when_flag_present() {
    let tmp = tempdir();
    let plan = tmp.as_path().join("plan.org");
    std::fs::write(
        &plan,
        "\
#+TITLE: Probe
#+TODO: TODO NEXT WAITING DOING | DONE CANCELED FAILED

* NEXT Mine :stage:
:PROPERTIES:
:ID: mine
:ASSIGNED_AGENT: workhorse
:END:

* NEXT Theirs :stage:
:PROPERTIES:
:ID: theirs
:ASSIGNED_AGENT: someone-else
:END:

* TODO Unassigned :stage:
:PROPERTIES:
:ID: unassigned
:END:
",
    )
    .unwrap();

    let out = Command::new(worg_bin())
        .args(["ready"])
        .arg(&plan)
        .args(["--agent=workhorse"])
        .output()
        .expect("worg ready --agent");
    assert!(out.status.success(), "ready failed: {out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("\"mine\""), "should include mine: {stdout}");
    assert!(!stdout.contains("\"theirs\""), "should exclude theirs: {stdout}");
    assert!(!stdout.contains("\"unassigned\""), "should exclude unassigned: {stdout}");
}

#[test]
fn transition_unknown_id_returns_nonzero() {
    let tmp = tempdir();
    let plan = tmp.as_path().join("plan.org");
    std::fs::write(
        &plan,
        "* TODO Thing :stage:\n:PROPERTIES:\n:ID: thing\n:END:\n",
    )
    .unwrap();

    let out = Command::new(worg_bin())
        .args(["transition"])
        .arg(&plan)
        .args(["does-not-exist", "DONE"])
        .output()
        .expect("worg transition");
    // worg-parse's transition_todo returns HeadlineNotFound when the
    // target :ID: isn't in the file; we surface that as a non-zero exit
    // so callers (scripts, CI, agents) can branch on success vs failure.
    assert!(!out.status.success(), "expected failure for unknown id, got success: {out:?}");
}

fn tempdir() -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    let unique = format!(
        "worg-cli-mutation-test-{}-{}",
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
