//! worg-query — agenda-style queries over a [`worg_parse::Document`].
//!
//! Typed Rust API rather than a string DSL. The runtime, CLI, NIF, and WASM
//! bindings all call into the same predicate types. The CLI exposes a JSON
//! query syntax (see `Predicate` serde impl) so non-Rust consumers can
//! compose queries without touching the AST.
//!
//! Predicates compose with `And`, `Or`, `Not`. Six leaves are provided —
//! enough to satisfy the standardized vocabulary in WORG.md. Add new leaves
//! when patterns emerge; do not invent an expression DSL.

#![forbid(unsafe_code)]

use orgize::ast::Headline;
use serde::{Deserialize, Serialize};
use worg_parse::Document;

/// A composable predicate over headlines.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Predicate {
    /// Match if the headline has this tag (inherited tags count).
    Tag { tag: String },
    /// Match if the headline's TODO keyword equals this string.
    State { state: String },
    /// Match if a `:PROPERTIES:` drawer entry equals this value. The key is
    /// looked up case-insensitively (org-mode convention).
    Property { key: String, value: String },
    /// Match if the headline has any (non-empty) value for this property key.
    HasProperty { key: String },
    /// Match if this headline's `:DEPENDS_ON:` references all resolve to
    /// headlines whose state is in `DONE` or `ABANDONED` — i.e. the headline
    /// is unblocked. A headline with no `:DEPENDS_ON:` is always ready.
    Ready,
    /// Match if the headline's `:ASSIGNED_AGENT:` property equals this slug.
    /// Sugar for `Property { key: "ASSIGNED_AGENT", value: slug }`.
    Assigned { agent: String },
    /// Logical combinators.
    And { of: Vec<Predicate> },
    Or { of: Vec<Predicate> },
    Not { of: Box<Predicate> },
}

impl Predicate {
    /// Evaluate this predicate against a single headline within `doc`.
    pub fn matches(&self, doc: &Document, hl: &Headline) -> bool {
        match self {
            Predicate::Tag { tag } => headline_tags(hl).iter().any(|t| t == tag),
            Predicate::State { state } => {
                hl.todo_keyword().map(|t| t.to_string()) == Some(state.clone())
            }
            Predicate::Property { key, value } => headline_property(hl, key).as_deref() == Some(value),
            Predicate::HasProperty { key } => headline_property(hl, key).is_some(),
            Predicate::Ready => is_ready(doc, hl),
            Predicate::Assigned { agent } => {
                headline_property(hl, "ASSIGNED_AGENT").as_deref() == Some(agent.as_str())
            }
            Predicate::And { of } => of.iter().all(|p| p.matches(doc, hl)),
            Predicate::Or { of } => of.iter().any(|p| p.matches(doc, hl)),
            Predicate::Not { of } => !of.matches(doc, hl),
        }
    }
}

/// Run a predicate over an entire document. Returns headlines in document order.
pub fn query(doc: &Document, predicate: &Predicate) -> Vec<Headline> {
    doc.headlines()
        .into_iter()
        .filter(|hl| predicate.matches(doc, hl))
        .collect()
}

// ───── helpers (kept private — these are *queries*, not mutations) ─────

/// Collect tags for a headline, including inherited tags from ancestor
/// headlines (org-mode tag inheritance semantics).
pub fn headline_tags(hl: &Headline) -> Vec<String> {
    let mut out: Vec<String> = hl.tags().map(|t| t.to_string()).collect();

    // Walk up the syntax tree collecting parent headlines' tags.
    use orgize::rowan::ast::AstNode;
    let mut node = hl.syntax().parent();
    while let Some(n) = node {
        if let Some(parent_hl) = Headline::cast(n.clone()) {
            for t in parent_hl.tags() {
                let s = t.to_string();
                if !out.contains(&s) {
                    out.push(s);
                }
            }
        }
        node = n.parent();
    }
    out
}

fn headline_property(hl: &Headline, key: &str) -> Option<String> {
    let props = hl.properties()?;
    let want = key.to_ascii_uppercase();
    props
        .iter()
        .find(|(k, _)| k.to_string().to_ascii_uppercase() == want)
        .map(|(_, v)| v.to_string())
}

/// Parse a `:DEPENDS_ON:` property value into a list of headline IDs.
/// Accepts both bare ids (`task-1 task-2`) and `[[id:task-1]] [[id:task-2]]`.
pub fn parse_depends_on(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    for token in value.split_whitespace() {
        if let Some(rest) = token.strip_prefix("[[id:") {
            if let Some(id) = rest.strip_suffix("]]") {
                out.push(id.to_string());
                continue;
            }
        }
        if !token.is_empty() {
            out.push(token.to_string());
        }
    }
    out
}

fn is_ready(doc: &Document, hl: &Headline) -> bool {
    let deps = match headline_property(hl, "DEPENDS_ON") {
        None => return true, // no deps → always ready
        Some(v) => parse_depends_on(&v),
    };
    deps.iter().all(|id| {
        match doc.find_by_id(id) {
            None => false, // dangling dep → not ready (worg lint also flags this)
            Some(referent) => {
                let state = referent.todo_keyword().map(|t| t.to_string());
                matches!(state.as_deref(), Some("DONE") | Some("ABANDONED"))
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: org tags are alphanumeric + `_@` only. Agent slugs with hyphens
    // (e.g. `gamut-director`) must go in :ASSIGNED_AGENT: property, NOT in
    // tags. WORG.md reflects this — :agent:workhorse: works as a tag because
    // `workhorse` is alphanumeric; :agent:gamut_director: would too.
    const SAMPLE: &str = "\
#+TITLE: query sample
#+TODO: TODO DOING | DONE ABANDONED

* DONE Stage 1 :stage:agent:workhorse:
:PROPERTIES:
:ID: stage-1
:ASSIGNED_AGENT: workhorse
:END:

* TODO Stage 2 :stage:agent:director:
:PROPERTIES:
:ID: stage-2
:ASSIGNED_AGENT: gamut-director
:DEPENDS_ON: [[id:stage-1]]
:END:

* TODO Stage 3 :stage:
:PROPERTIES:
:ID: stage-3
:DEPENDS_ON: [[id:stage-2]]
:END:

** TODO Validator: artifact_exists :validator:
:PROPERTIES:
:KIND: artifact_exists
:END:
";

    #[test]
    fn tag_query() {
        let doc = Document::parse(SAMPLE);
        let q = Predicate::Tag { tag: "stage".into() };
        // 3 stages + the validator (which inherits :stage: from Stage 3)
        assert_eq!(query(&doc, &q).len(), 4);
    }

    #[test]
    fn tag_inheritance() {
        let doc = Document::parse(SAMPLE);
        // The validator headline inherits :stage: from its parent Stage 3.
        let q = Predicate::Tag { tag: "validator".into() };
        let validators = query(&doc, &q);
        assert_eq!(validators.len(), 1);
        // And it also matches :stage: via inheritance.
        let tags = headline_tags(&validators[0]);
        assert!(tags.contains(&"validator".to_string()));
        assert!(tags.contains(&"stage".to_string()));
    }

    #[test]
    fn state_query() {
        let doc = Document::parse(SAMPLE);
        let q = Predicate::State { state: "TODO".into() };
        assert_eq!(query(&doc, &q).len(), 3); // 2 stages + 1 validator
    }

    #[test]
    fn ready_query() {
        let doc = Document::parse(SAMPLE);
        // Stage 1 is DONE (no deps, always ready).
        // Stage 2's only dep is stage-1 (DONE) → ready.
        // Stage 3's only dep is stage-2 (TODO) → NOT ready.
        let q = Predicate::And {
            of: vec![
                Predicate::State { state: "TODO".into() },
                Predicate::Ready,
            ],
        };
        let ready_todos = query(&doc, &q);
        let ids: Vec<_> = ready_todos
            .iter()
            .map(|h| {
                h.properties()
                    .and_then(|p| p.get("ID"))
                    .map(|t| t.to_string())
                    .unwrap_or_default()
            })
            .collect();
        assert!(ids.contains(&"stage-2".to_string()));
        assert!(!ids.contains(&"stage-3".to_string()));
    }

    #[test]
    fn assigned_query() {
        let doc = Document::parse(SAMPLE);
        let q = Predicate::Assigned { agent: "gamut-director".into() };
        assert_eq!(query(&doc, &q).len(), 1);
    }

    #[test]
    fn predicate_serde_roundtrip() {
        let q = Predicate::And {
            of: vec![
                Predicate::State { state: "TODO".into() },
                Predicate::Ready,
                Predicate::Not {
                    of: Box::new(Predicate::Tag { tag: "skip".into() }),
                },
            ],
        };
        let json = serde_json::to_string(&q).unwrap();
        let back: Predicate = serde_json::from_str(&json).unwrap();
        // Re-serialize and compare strings — Predicate doesn't impl PartialEq.
        assert_eq!(json, serde_json::to_string(&back).unwrap());
    }

    #[test]
    fn parse_depends_on_handles_both_forms() {
        assert_eq!(parse_depends_on("a b"), vec!["a", "b"]);
        assert_eq!(
            parse_depends_on("[[id:a]] [[id:b]]"),
            vec!["a", "b"]
        );
        assert_eq!(parse_depends_on("a [[id:b]] c"), vec!["a", "b", "c"]);
    }
}
