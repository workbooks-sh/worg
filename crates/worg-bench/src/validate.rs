//! Validators. Each validator takes the model's output text and either
//! returns Ok(()) or an error string describing what went wrong.
//!
//! All structural validators run after the output is parsed by worg-parse.
//! `Parses` is the load-bearing gate — if the output doesn't parse, all
//! structural validators downstream are short-circuited as "gated".

use orgize::rowan::ast::AstNode;
use orgize::ast::{Drawer, Headline, PropertyDrawer};
use regex::Regex;
use worg_parse::Document;

use crate::spec::ValidatorSpec;

#[derive(Debug, Clone)]
pub enum Outcome {
    Pass,
    Fail(String),
    Gated,
}

pub fn check(spec: &ValidatorSpec, output: &str, parses: bool) -> Outcome {
    // Every structural check requires parses==true.
    if !matches!(
        spec,
        ValidatorSpec::Parses
            | ValidatorSpec::Regex { .. }
            | ValidatorSpec::EqualsNormalized { .. }
            | ValidatorSpec::Contains { .. }
    ) && !parses
    {
        return Outcome::Gated;
    }

    match spec {
        ValidatorSpec::Parses => {
            if parses {
                Outcome::Pass
            } else {
                Outcome::Fail("worg-parse round-trip drift or unparsable".into())
            }
        }
        ValidatorSpec::HeadlineCount { count } => {
            let doc = Document::parse(output);
            let n = doc.headlines().len();
            if n == *count {
                Outcome::Pass
            } else {
                Outcome::Fail(format!("expected {count} headlines, got {n}"))
            }
        }
        ValidatorSpec::StateMatch {
            headline_index,
            state,
        } => with_headline(output, *headline_index, |h| {
            let actual = h.todo_keyword().map(|s| s.to_string()).unwrap_or_default();
            if actual == *state {
                Ok(())
            } else {
                Err(format!(
                    "headline[{headline_index}] state: expected {state:?}, got {actual:?}"
                ))
            }
        }),
        ValidatorSpec::HasProperty {
            headline_index,
            name,
            value,
        } => with_headline(output, *headline_index, |h| {
            let props = headline_properties(&h);
            let upper = name.to_uppercase();
            match props.iter().find(|(k, _)| k.to_uppercase() == upper) {
                None => Err(format!(
                    "headline[{headline_index}] missing property :{name}:"
                )),
                Some((_, v)) => match value {
                    None => Ok(()),
                    Some(expected) => {
                        if v.trim() == expected.trim() {
                            Ok(())
                        } else {
                            Err(format!(
                                "headline[{headline_index}] property :{name}: expected {expected:?}, got {:?}",
                                v
                            ))
                        }
                    }
                },
            }
        }),
        ValidatorSpec::HasDrawer {
            headline_index,
            name,
        } => with_headline(output, *headline_index, |h| {
            let upper = name.to_uppercase();
            let found = headline_drawers(&h)
                .into_iter()
                .any(|d| d.to_uppercase() == upper);
            if found {
                Ok(())
            } else {
                Err(format!("headline[{headline_index}] missing drawer :{name}:"))
            }
        }),
        ValidatorSpec::TagsContain {
            headline_index,
            tags,
        } => with_headline(output, *headline_index, |h| {
            let actual: Vec<String> = h.tags().map(|t| t.to_string()).collect();
            let missing: Vec<&String> = tags
                .iter()
                .filter(|want| !actual.iter().any(|a| a == *want))
                .collect();
            if missing.is_empty() {
                Ok(())
            } else {
                Err(format!(
                    "headline[{headline_index}] missing tags {missing:?} (had {actual:?})"
                ))
            }
        }),
        ValidatorSpec::PriorityMatch {
            headline_index,
            priority,
        } => with_headline(output, *headline_index, |h| {
            let actual = h.priority().map(|p| p.to_string()).unwrap_or_default();
            if actual == *priority {
                Ok(())
            } else {
                Err(format!(
                    "headline[{headline_index}] priority: expected {priority:?}, got {actual:?}"
                ))
            }
        }),
        ValidatorSpec::LevelMatch {
            headline_index,
            level,
        } => with_headline(output, *headline_index, |h| {
            let actual = h.level();
            if actual == *level {
                Ok(())
            } else {
                Err(format!(
                    "headline[{headline_index}] level: expected {level}, got {actual}"
                ))
            }
        }),
        ValidatorSpec::Regex { pattern } => match Regex::new(pattern) {
            Err(e) => Outcome::Fail(format!("bad regex {pattern:?}: {e}")),
            Ok(re) => {
                if re.is_match(output) {
                    Outcome::Pass
                } else {
                    Outcome::Fail(format!("regex {pattern:?} did not match"))
                }
            }
        },
        ValidatorSpec::EqualsNormalized { expected } => {
            if normalize_ws(output) == normalize_ws(expected) {
                Outcome::Pass
            } else {
                Outcome::Fail("output != expected after whitespace normalization".into())
            }
        }
        ValidatorSpec::Contains { substring } => {
            if output.contains(substring) {
                Outcome::Pass
            } else {
                Outcome::Fail(format!("output does not contain {substring:?}"))
            }
        }
    }
}

/// Helper — index into the parsed document and run a closure against the
/// requested headline. Returns Gated if the index is out of range (so callers
/// see a clear "couldn't reach this check" signal vs. a structural failure).
fn with_headline<F>(output: &str, idx: usize, f: F) -> Outcome
where
    F: FnOnce(Headline) -> Result<(), String>,
{
    let doc = Document::parse(output);
    let headlines = doc.headlines();
    match headlines.into_iter().nth(idx) {
        None => Outcome::Fail(format!("headline[{idx}] out of range (no such headline)")),
        Some(h) => match f(h) {
            Ok(()) => Outcome::Pass,
            Err(msg) => Outcome::Fail(msg),
        },
    }
}

fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn headline_properties(h: &Headline) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let node = h.syntax();
    for child in node.descendants() {
        if let Some(drawer) = PropertyDrawer::cast(child.clone()) {
            for (k, v) in drawer.iter() {
                out.push((k.to_string(), v.to_string()));
            }
            break;
        }
    }
    out
}

fn headline_drawers(h: &Headline) -> Vec<String> {
    let mut out = Vec::new();
    let node = h.syntax();
    // Only collect drawers whose nearest enclosing headline ancestor is `h`.
    // Without this, the recursion would also include child-headline drawers.
    let h_offset = node.text_range().start();
    for desc in node.descendants() {
        if let Some(d) = Drawer::cast(desc.clone()) {
            // Walk up to confirm the nearest Headline ancestor is `h`.
            let mut cur = desc.parent();
            while let Some(p) = cur {
                if Headline::can_cast(p.kind()) {
                    if p.text_range().start() == h_offset {
                        out.push(d.name().to_string());
                    }
                    break;
                }
                cur = p.parent();
            }
        }
    }
    out
}

/// Check the load-bearing parse-and-round-trip property in one call.
/// `true` means worg-parse accepted the text AND serialize(parse(text)) == text.
pub fn parses_ok(text: &str) -> bool {
    Document::round_trip_ok(text)
}
