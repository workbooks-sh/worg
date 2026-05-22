//! worg-lint — enforce WORG.md conventions.
//!
//! Returns a list of [`Diagnostic`]s, each with a code (`E001…`, `W001…`),
//! severity, message, and optional location. The CLI prints these; the
//! runtime can also consume them programmatically.
//!
//! Codes match WORG.md "Linter rules" section. Not all rules are
//! implemented yet — the most load-bearing ones land first. Stubs are
//! marked with TODO and will not falsely report.

#![forbid(unsafe_code)]

use orgize::rowan::ast::AstNode;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use worg_parse::Document;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warn,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: String,
    pub severity: Severity,
    pub message: String,
    /// Best-effort 1-based line number. `None` if not localizable.
    pub line: Option<usize>,
    /// Best-effort headline ID for context.
    pub headline_id: Option<String>,
}

/// Standardized property names from WORG.md. Used by W001 to detect drift.
const STANDARDIZED_PROPERTIES: &[&str] = &[
    // identity + ownership
    "ID",
    "ASSIGNED_AGENT",
    "RUN_ID",
    "CATEGORY",
    // scheduling
    "DEPENDS_ON",
    "STAGE_ORDER",
    "TIMEOUT_MS",
    // budgets + retries
    "TOOL_BUDGET",
    "RETRY_POLICY",
    "COST_USD",
    // tools + artifacts
    "TOOL",
    "TOOLS_AVAILABLE",
    "ARTIFACT",
    "ARTIFACTS_IN",
    "ARTIFACTS_OUT",
    // executor dispatch
    "TRUST_LEVEL",
    "DERIVED",
    "KIND",
    // common standard org properties we permit silently
    "CUSTOM_ID",
    "ARCHIVE",
    "VISIBILITY",
];

/// Source-block languages worg can dispatch. Other languages with `:results`
/// trigger E004; without `:results`, just W005.
const SUPPORTED_EXEC_LANGS: &[&str] = &["shell", "bash", "sh", "elixir", "lua"];

/// Lint a parsed document.
pub fn lint(doc: &Document) -> Vec<Diagnostic> {
    let mut out = Vec::new();

    let known_ids: HashSet<String> = doc
        .headlines()
        .iter()
        .filter_map(|h| {
            h.properties()
                .and_then(|p| p.get("ID"))
                .map(|t| t.to_string())
        })
        .collect();

    let extensions = collect_extensions(doc);

    for hl in doc.headlines() {
        let id = hl
            .properties()
            .and_then(|p| p.get("ID"))
            .map(|t| t.to_string());

        // W001: unknown uppercase property
        if let Some(props) = hl.properties() {
            for (k, _v) in props.iter() {
                let key = k.to_string();
                let key_upper = key.to_ascii_uppercase();
                if key == key_upper
                    && !STANDARDIZED_PROPERTIES.contains(&key_upper.as_str())
                    && !extensions.contains(&key_upper)
                {
                    out.push(Diagnostic {
                        code: "W001".into(),
                        severity: Severity::Warn,
                        message: format!(
                            "unknown uppercase property `{key}` — possible drift. Add to `#+WORG_EXTENSIONS:` if intentional."
                        ),
                        line: None,
                        headline_id: id.clone(),
                    });
                }
            }
        }

        // E003: dangling [[id:...]] in :DEPENDS_ON:
        if let Some(deps) = hl
            .properties()
            .and_then(|p| p.get("DEPENDS_ON"))
            .map(|t| t.to_string())
        {
            for dep_id in worg_query::parse_depends_on(&deps) {
                if !known_ids.contains(&dep_id) {
                    out.push(Diagnostic {
                        code: "E003".into(),
                        severity: Severity::Error,
                        message: format!(
                            ":DEPENDS_ON: references missing id `{dep_id}`"
                        ),
                        line: None,
                        headline_id: id.clone(),
                    });
                }
            }
        }

        // W006: validator headline missing :KIND:
        let tags = worg_query::headline_tags(&hl);
        if tags.iter().any(|t| t == "validator")
            && hl
                .properties()
                .and_then(|p| p.get("KIND"))
                .is_none()
        {
            out.push(Diagnostic {
                code: "W006".into(),
                severity: Severity::Warn,
                message: "validator headline missing `:KIND:` property".into(),
                line: None,
                headline_id: id.clone(),
            });
        }

        // W005 + E004: source block language checks
        if let Some(section) = hl.section() {
            use orgize::ast::SourceBlock;
            use orgize::SyntaxKind;
            for child in section.syntax().children() {
                if child.kind() != SyntaxKind::SOURCE_BLOCK {
                    continue;
                }
                let Some(block) = SourceBlock::cast(child) else { continue };
                let lang = block.language().map(|t| t.to_string().to_ascii_lowercase());
                let has_results = block
                    .parameters()
                    .map(|p| p.to_string().contains(":results"))
                    .unwrap_or(false);
                if let Some(l) = lang.as_deref() {
                    if !SUPPORTED_EXEC_LANGS.contains(&l) && l != "json" && l != "markdown" {
                        let (code, severity) = if has_results {
                            ("E004", Severity::Error)
                        } else {
                            ("W005", Severity::Warn)
                        };
                        out.push(Diagnostic {
                            code: code.into(),
                            severity,
                            message: format!(
                                "source block language `{l}` is not in worg's dispatch table (shell/bash/sh, elixir, lua). Use shell to invoke external tools."
                            ),
                            line: None,
                            headline_id: id.clone(),
                        });
                    }
                }
            }
        }
    }

    out
}

/// Read `#+WORG_EXTENSIONS:` file keyword — whitespace-separated list of
/// extension property names a project has declared.
fn collect_extensions(doc: &Document) -> HashSet<String> {
    let src = doc.serialize();
    let mut out = HashSet::new();
    for line in src.lines() {
        if let Some(rest) = line.strip_prefix("#+WORG_EXTENSIONS:") {
            for token in rest.split_whitespace() {
                out.insert(token.to_ascii_uppercase());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn w001_unknown_property() {
        let doc = Document::parse(
            "* TODO Task
:PROPERTIES:
:ID: t1
:WEIRD_PROP: yes
:END:
",
        );
        let diags = lint(&doc);
        assert!(diags.iter().any(|d| d.code == "W001"));
    }

    #[test]
    fn w001_accepts_documented_extensions() {
        let doc = Document::parse(
            "#+WORG_EXTENSIONS: WEIRD_PROP CUSTOM_THING
* TODO Task
:PROPERTIES:
:ID: t1
:WEIRD_PROP: yes
:END:
",
        );
        let diags = lint(&doc);
        assert!(!diags.iter().any(|d| d.code == "W001"));
    }

    #[test]
    fn e003_dangling_depends_on() {
        let doc = Document::parse(
            "* TODO Task A
:PROPERTIES:
:ID: a
:DEPENDS_ON: [[id:nonexistent]]
:END:
",
        );
        let diags = lint(&doc);
        let e003 = diags.iter().find(|d| d.code == "E003").expect("E003");
        assert!(e003.message.contains("nonexistent"));
    }

    #[test]
    fn w006_validator_without_kind() {
        let doc = Document::parse(
            "* TODO Validator :validator:
:PROPERTIES:
:ID: v1
:END:
",
        );
        let diags = lint(&doc);
        assert!(diags.iter().any(|d| d.code == "W006"));
    }

    #[test]
    fn no_diags_on_clean_document() {
        let doc = Document::parse(
            "* DONE Stage 1
:PROPERTIES:
:ID: s1
:ASSIGNED_AGENT: workhorse
:END:

* TODO Stage 2
:PROPERTIES:
:ID: s2
:DEPENDS_ON: [[id:s1]]
:END:

** TODO Validator :validator:
:PROPERTIES:
:KIND: artifact_exists
:END:
",
        );
        let diags = lint(&doc);
        assert!(diags.is_empty(), "expected no diags, got: {diags:#?}");
    }
}
