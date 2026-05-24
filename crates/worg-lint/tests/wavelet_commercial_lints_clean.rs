//! Wavelet-adoption contract: linting `wavelet-commercial.org` must
//! produce ZERO `W010 unknown validator kind` findings.
//!
//! This is the load-bearing gate that lets the wavelet project move
//! its agent architecture onto WORG. Before the 7 wavelet kinds were
//! registered, every validator headline in the plan tripped W010 and
//! the plan couldn't be loaded by a strict-glossary runtime.
//!
//! If a wavelet-specific kind gets dropped from the registry in
//! `w.org` / `worg_lint::Glossary::default()`, this test fails and
//! the regression is caught immediately.

use std::path::PathBuf;
use worg_lint::lint;
use worg_parse::Document;

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is .../packages/worg/crates/worg-lint; the worg
    // workspace root is two levels up.
    let crate_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(crate_dir).ancestors().nth(2).unwrap().to_path_buf()
}

#[test]
fn wavelet_commercial_plan_lints_without_w010() {
    let plan_path = workspace_root().join("proposed/plans/wavelet-commercial.org");
    assert!(
        plan_path.is_file(),
        "fixture missing at {} — the wavelet-adoption test cannot run",
        plan_path.display()
    );

    let src = std::fs::read_to_string(&plan_path).expect("read wavelet-commercial.org");
    let doc = Document::parse(&src);
    let diags = lint(&doc);

    let w010: Vec<_> = diags.iter().filter(|d| d.code == "W010").collect();
    assert!(
        w010.is_empty(),
        "wavelet-commercial.org tripped W010 (unknown validator kind) — \
         a wavelet kind regressed out of the registry. Findings:\n{}",
        w010.iter()
            .map(|d| format!("  - {}", d.message))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
