//! worg-parse — parser, AST, and edit-preserving serializer for worg.
//!
//! Built on top of [`orgize`]. Spec: `packages/worg/WORG.md`.
//!
//! Three load-bearing operations beyond plain parse/serialize:
//!
//! - [`Document::transition_todo`] — change a headline's TODO keyword
//! - [`Document::append_logbook`] — append an entry to a headline's `:LOGBOOK:` drawer
//! - [`Document::write_results`] — set or replace a `#+RESULTS:` block for a tool source block
//!
//! All mutations are text-range replacements over the underlying [`orgize::Org`],
//! so round-trip for unedited regions is byte-identical by construction.

#![forbid(unsafe_code)]

use orgize::ast::{Drawer, Headline, SourceBlock};
use orgize::rowan::ast::AstNode;
use orgize::rowan::{TextRange, TextSize};
use orgize::{Org, ParseConfig, SyntaxKind, SyntaxNode};
use thiserror::Error;

/// GTD-aligned TODO keyword set, used as the default for every worg document.
///
/// org-mode core only recognizes `TODO` and `DONE` out of the box; orgize
/// inherits that behavior, so a headline like `* NEXT pick a thing` parses
/// "NEXT pick a thing" as the title rather than extracting `NEXT` as the
/// state keyword. To recover the org-gtd vocabulary (which is what an LLM
/// reading a worg file expects, and what the wb-0mqz migration adopts) we
/// build orgize's [`ParseConfig`] with the full keyword set.
///
/// The split:
/// - **active** (left of `|` in `#+TODO:` syntax): `TODO NEXT WAITING DOING
///   SOMEDAY`. SOMEDAY is included even though most plans don't use it —
///   adding it costs nothing and a plan that does declare it gets the
///   expected behavior.
/// - **done** (right of `|`): `DONE CANCELED FAILED`. `BLOCKED` and
///   `ABANDONED` are kept on the active side as legacy back-compat —
///   pre-GTD worg files using them continue to parse with the same
///   semantics as before this change.
fn gtd_parse_config() -> ParseConfig {
    ParseConfig {
        todo_keywords: (
            vec![
                "TODO".into(),
                "NEXT".into(),
                "WAITING".into(),
                "DOING".into(),
                "SOMEDAY".into(),
                "BLOCKED".into(),
            ],
            vec![
                "DONE".into(),
                "CANCELED".into(),
                "FAILED".into(),
                "ABANDONED".into(),
            ],
        ),
        ..ParseConfig::default()
    }
}

/// One source block under a headline — language tag, body, and a stable
/// 0-based index of where it sits among the headline's source blocks.
/// The body is the verbatim content between `#+BEGIN_SRC` and `#+END_SRC`,
/// no markers, no leading/trailing newlines normalized.
#[derive(Debug, Clone)]
pub struct SourceBlockInfo {
    pub language: Option<String>,
    pub body: String,
    pub index: usize,
}

#[derive(Debug, Error)]
pub enum WorgError {
    #[error("headline with id `{0}` not found")]
    HeadlineNotFound(String),
    #[error("headline has no TODO keyword to transition")]
    NoTodoKeyword,
    #[error("round-trip drift: input != serialize(parse(input))")]
    RoundTripDrift,
    #[error("invalid argument: {0}")]
    InvalidArg(String),
    /// CAS check failed — current state is not what the caller expected.
    /// wb-nlln.18: prevents concurrent claims / transitions from silently
    /// stomping each other.
    #[error("state mismatch on `{id}`: expected one of {expected:?}, actual {actual:?}")]
    StateMismatch {
        id: String,
        expected: Vec<String>,
        actual: Option<String>,
    },
}

/// One parsed worg document. Wraps [`orgize::Org`] with worg-specific helpers.
pub struct Document {
    org: Org,
}

impl Document {
    /// Parse a worg/org document from source text.
    ///
    /// Uses a GTD-aligned [`ParseConfig`] so headline keywords like
    /// `NEXT` / `WAITING` / `DOING` / `CANCELED` are extracted as state
    /// keywords rather than swallowed into the title.
    pub fn parse(src: &str) -> Self {
        Document { org: gtd_parse_config().parse(src) }
    }

    /// Serialize the document back to org-mode text.
    ///
    /// For an unedited document, `Document::parse(s).serialize() == s` is a
    /// load-bearing invariant — see [`Document::round_trip_ok`] for the check.
    pub fn serialize(&self) -> String {
        self.org.to_org()
    }

    /// Check the round-trip invariant against the original source.
    pub fn round_trip_ok(original: &str) -> bool {
        Document::parse(original).serialize() == original
    }

    /// Iterate over all headlines in document order, descending into nested
    /// headlines depth-first.
    pub fn headlines(&self) -> Vec<Headline> {
        let mut out = Vec::new();
        let root = SyntaxNode::new_root(self.org.green().clone());
        walk_headlines(&root, &mut out);
        out
    }

    /// Return every source block under the headline identified by `id`,
    /// in document order. Used by agents reading a plan and dispatching
    /// `#+BEGIN_SRC <lang>` blocks to the right runtime (lua_eval / bash /
    /// js_eval). Returns an empty vec if the headline has no source
    /// blocks. Returns an error if the headline isn't found.
    pub fn source_blocks_for(&self, id: &str) -> Result<Vec<SourceBlockInfo>, WorgError> {
        let hl = self
            .find_by_id(id)
            .ok_or_else(|| WorgError::HeadlineNotFound(id.to_string()))?;
        Ok(extract_source_blocks(&hl))
    }

    /// Find a headline whose `:ID:` property equals `id`.
    pub fn find_by_id(&self, id: &str) -> Option<Headline> {
        self.headlines()
            .into_iter()
            .find(|h| headline_id(h).as_deref() == Some(id))
    }

    /// Replace a headline's TODO keyword with `new_state`.
    ///
    /// The new state must already be a member of the document's `#+TODO:`
    /// declaration — worg-parse does not validate that here (the runtime checks
    /// against the document's keyword set before calling).
    ///
    /// Returns an error if the headline doesn't exist or has no current TODO
    /// keyword.
    pub fn transition_todo(&mut self, id: &str, new_state: &str) -> Result<(), WorgError> {
        let hl = self
            .find_by_id(id)
            .ok_or_else(|| WorgError::HeadlineNotFound(id.to_string()))?;
        let keyword = hl.todo_keyword().ok_or(WorgError::NoTodoKeyword)?;
        self.org
            .replace_range(keyword.syntax().text_range(), new_state);
        Ok(())
    }

    /// CAS variant of [`Document::transition_todo`] — only transitions
    /// when the headline's current TODO keyword is in `expected`.
    /// Returns [`WorgError::StateMismatch`] otherwise.
    ///
    /// wb-nlln.18: when paired with file-level locking in the CLI, two
    /// concurrent `claim` invocations against the same task can no
    /// longer both succeed silently — exactly one wins, the other gets
    /// a clear "state changed under us" error.
    pub fn transition_todo_cas(
        &mut self,
        id: &str,
        expected: &[&str],
        new_state: &str,
    ) -> Result<(), WorgError> {
        let hl = self
            .find_by_id(id)
            .ok_or_else(|| WorgError::HeadlineNotFound(id.to_string()))?;
        let current = hl.todo_keyword().map(|t| t.to_string());

        let matched = match &current {
            None => expected.is_empty(),
            Some(state) => expected.iter().any(|e| *e == state),
        };

        if !matched {
            return Err(WorgError::StateMismatch {
                id: id.to_string(),
                expected: expected.iter().map(|s| s.to_string()).collect(),
                actual: current,
            });
        }

        // Re-find the headline after the borrow so the mutable replace
        // doesn't conflict with the immutable read above.
        let hl = self
            .find_by_id(id)
            .ok_or_else(|| WorgError::HeadlineNotFound(id.to_string()))?;
        let keyword = hl.todo_keyword().ok_or(WorgError::NoTodoKeyword)?;
        self.org
            .replace_range(keyword.syntax().text_range(), new_state);
        Ok(())
    }

    /// Append an entry to a headline's `:LOGBOOK:` drawer.
    ///
    /// If the headline has no `:LOGBOOK:` drawer yet, one is created
    /// immediately after the `:PROPERTIES:` drawer (or immediately after the
    /// headline title if no properties exist).
    ///
    /// `entry` should be the body of a single list item, e.g.
    /// `r#"State "DONE" from "DOING" [2026-05-20 Tue 14:30:58]"#`. worg-parse
    /// prepends `"- "` and trailing newline.
    pub fn append_logbook(&mut self, id: &str, entry: &str) -> Result<(), WorgError> {
        self.append_drawer(id, "LOGBOOK", entry)
    }

    /// Append an entry to a named drawer under the headline identified by `id`.
    ///
    /// Generic over `:LOGBOOK:`, `:NOTES:`, `:CONSTRAINTS:`, or any custom
    /// drawer name. If the drawer doesn't exist yet, it's created at the
    /// canonical insertion point (after `:PROPERTIES:` if present, else
    /// directly under the headline title).
    ///
    /// `entry` is the body of a single list item. A leading `"- "` is
    /// stripped defensively (callers often include the bullet because they've
    /// seen drawer entries in the wild and assume the bullet is part of the
    /// payload). worg-parse then prepends a single `"- "` and trailing newline.
    pub fn append_drawer(
        &mut self,
        id: &str,
        drawer_name: &str,
        entry: &str,
    ) -> Result<(), WorgError> {
        let hl = self
            .find_by_id(id)
            .ok_or_else(|| WorgError::HeadlineNotFound(id.to_string()))?;

        let entry = entry.trim_start_matches('-').trim_start();
        let upper = drawer_name.to_uppercase();

        if let Some(drawer) = find_named_drawer(&hl, &upper) {
            let insert_at = drawer.content_end();
            self.org
                .replace_range(TextRange::empty(insert_at), &format!("- {entry}\n"));
        } else {
            let insert_at = insertion_point_for_new_drawer(&hl);
            let drawer_text = format!(":{upper}:\n- {entry}\n:END:\n");
            self.org
                .replace_range(TextRange::empty(insert_at), &drawer_text);
        }
        Ok(())
    }

    /// Set or update a property in the `:PROPERTIES:` drawer of the headline
    /// identified by `id`.
    ///
    /// If the drawer doesn't exist, it's created. If the named property already
    /// exists, its value is replaced. The `:ID:` property is set automatically
    /// by orgize when this headline was created and CANNOT be changed via this
    /// method (would invalidate the find_by_id contract). Use this for
    /// `:BLOCKER:`, `:ASSIGNED_TO:`, `:CAPABILITIES:`, custom keys, etc.
    pub fn set_property(
        &mut self,
        id: &str,
        name: &str,
        value: &str,
    ) -> Result<(), WorgError> {
        if name.eq_ignore_ascii_case("ID") {
            return Err(WorgError::InvalidArg(
                ":ID: cannot be set via set_property — it's the lookup key. Use add_child or write the file directly.".into(),
            ));
        }

        let hl = self
            .find_by_id(id)
            .ok_or_else(|| WorgError::HeadlineNotFound(id.to_string()))?;
        let upper = name.to_uppercase();

        if let Some(props) = hl.properties() {
            // Look for an existing :KEY: in the drawer
            for (k, v) in props.iter() {
                if k.to_string().to_uppercase() == upper {
                    self.org
                        .replace_range(v.text_range(), value);
                    return Ok(());
                }
            }
            // Drawer exists, key absent — append before :END:
            let insert_at = props.content_end();
            self.org
                .replace_range(
                    TextRange::empty(insert_at),
                    &format!(":{upper}: {value}\n"),
                );
        } else {
            // No properties drawer — create one
            let insert_at = if let Some(section) = hl.section() {
                section.syntax().text_range().start()
            } else {
                hl.syntax().text_range().end()
            };
            let drawer = format!(":PROPERTIES:\n:{upper}: {value}\n:END:\n");
            self.org
                .replace_range(TextRange::empty(insert_at), &drawer);
        }
        Ok(())
    }

    /// Insert a new child headline under the headline identified by
    /// `parent_id`. The child is added at the END of the parent's subtree
    /// (after all existing descendants), which matches the natural
    /// "append child" semantic.
    ///
    /// `level` is computed automatically (parent.level() + 1). `state` is an
    /// optional TODO keyword (e.g. "TODO", "NEXT", "DONE"); pass `None` for a
    /// plain headline. `child_id` becomes the new headline's `:ID:` property
    /// so it's reachable by `find_by_id`.
    pub fn add_child(
        &mut self,
        parent_id: &str,
        title: &str,
        state: Option<&str>,
        child_id: &str,
    ) -> Result<(), WorgError> {
        if child_id.is_empty() {
            return Err(WorgError::InvalidArg(
                "add_child: child_id must be non-empty (becomes the :ID: property)".into(),
            ));
        }

        let parent_level = self
            .find_by_id(parent_id)
            .ok_or_else(|| WorgError::HeadlineNotFound(parent_id.to_string()))?
            .level();

        let child_level = parent_level + 1;
        let stars = "*".repeat(child_level);
        let title_line = match state {
            Some(s) if !s.is_empty() => format!("{stars} {s} {title}"),
            _ => format!("{stars} {title}"),
        };
        let child_text = format!(
            "{title_line}\n:PROPERTIES:\n:ID: {child_id}\n:END:\n"
        );

        // Insertion point: end of parent's subtree (just before the next
        // headline at level <= parent_level, or end-of-doc if none).
        let insert_at = subtree_end(self, parent_id, parent_level);

        // Make sure we land on a fresh line: if the byte just before the
        // insert position isn't a newline, prepend one so we don't collide
        // with prior content.
        let serialized = self.org.to_org();
        let needs_newline = insert_at > TextSize::from(0)
            && {
                let pos = u32::from(insert_at) as usize;
                serialized.as_bytes().get(pos.saturating_sub(1)) != Some(&b'\n')
            };

        let composed = if needs_newline {
            format!("\n{child_text}")
        } else {
            child_text
        };

        self.org
            .replace_range(TextRange::empty(insert_at), &composed);
        Ok(())
    }

    /// Write or replace a `#+RESULTS:` block following the first source block
    /// under the headline identified by `id`.
    ///
    /// If a `#+RESULTS:` block already exists immediately after the source
    /// block, its content is replaced. Otherwise a new block is inserted.
    ///
    /// `results` is the verbatim block body — one or more lines, each prefixed
    /// with `": "` per org convention. worg-parse does not add the prefix; the
    /// caller controls formatting.
    pub fn write_results(&mut self, id: &str, results: &str) -> Result<(), WorgError> {
        let hl = self
            .find_by_id(id)
            .ok_or_else(|| WorgError::HeadlineNotFound(id.to_string()))?;
        let src_block = find_first_source_block(&hl)
            .ok_or_else(|| WorgError::HeadlineNotFound(format!("source block under {id}")))?;
        let after_block = src_block.text_range().end();

        // Look at the text immediately after the source block — does a #+RESULTS: block follow?
        let full_text = self.org.to_org();
        let after_str = &full_text[usize::from(after_block)..];

        if let Some(existing) = existing_results_range(after_block, after_str) {
            let new_block = format_results_block(results);
            self.org.replace_range(existing, &new_block);
        } else {
            let new_block = format!("\n{}", format_results_block(results));
            self.org
                .replace_range(TextRange::empty(after_block), &new_block);
        }
        Ok(())
    }
}

// ───── traversal helpers ─────

fn walk_headlines(node: &SyntaxNode, out: &mut Vec<Headline>) {
    for child in node.children() {
        if let Some(hl) = Headline::cast(child.clone()) {
            out.push(hl);
        }
        walk_headlines(&child, out);
    }
}

/// Compute the byte offset where a headline's subtree ends — i.e. the start
/// of the next headline at level <= `parent_level`, or end-of-document if
/// there isn't one.
///
/// Used by `add_child` to append new children at the natural "end of
/// subtree" position rather than as the first child.
fn subtree_end(doc: &Document, parent_id: &str, parent_level: usize) -> TextSize {
    let headlines = doc.headlines();
    let mut seen_parent = false;
    for hl in &headlines {
        if !seen_parent {
            if headline_id(hl).as_deref() == Some(parent_id) {
                seen_parent = true;
            }
            continue;
        }
        // After parent: look for the first headline at level <= parent_level.
        if hl.level() <= parent_level {
            return hl.syntax().text_range().start();
        }
    }
    // No subsequent sibling/ancestor — append at document end.
    let serialized = doc.org.to_org();
    TextSize::from(serialized.len() as u32)
}

fn headline_id(hl: &Headline) -> Option<String> {
    hl.properties()
        .and_then(|p| p.get("ID"))
        .map(|t| t.to_string())
}

// ───── drawer helpers ─────

/// Find a drawer by name (case-insensitive) under a headline's section.
///
/// orgize's `Headline::properties()` returns the `:PROPERTIES:` drawer
/// specifically; for `:LOGBOOK:` and other named drawers we use the
/// [`Drawer`] AST type.
fn find_named_drawer(hl: &Headline, name: &str) -> Option<Drawer> {
    let section = hl.section()?;
    let want = name.to_uppercase();
    section
        .syntax()
        .children()
        .filter_map(Drawer::cast)
        .find(|d| d.name().to_string().to_uppercase() == want)
}

/// Where to insert a new drawer (e.g. `:LOGBOOK:`) under a headline.
///
/// Prefers immediately after `:PROPERTIES:` if it exists, else after the
/// headline title line.
fn insertion_point_for_new_drawer(hl: &Headline) -> TextSize {
    if let Some(props) = hl.properties() {
        return props.syntax().text_range().end();
    }
    if let Some(section) = hl.section() {
        return section.syntax().text_range().start();
    }
    hl.syntax().text_range().end()
}

// ───── source block + results helpers ─────

fn find_first_source_block(hl: &Headline) -> Option<SyntaxNode> {
    let section = hl.section()?;
    section
        .syntax()
        .children()
        .find(|n| n.kind() == SyntaxKind::SOURCE_BLOCK)
}

/// Extract every source block directly under a headline's section.
/// Does NOT recurse into child-headline sections — each headline owns
/// its own blocks. Order matches document order.
fn extract_source_blocks(hl: &Headline) -> Vec<SourceBlockInfo> {
    let mut out = Vec::new();
    if let Some(section) = hl.section() {
        for (idx, node) in section
            .syntax()
            .children()
            .filter(|n| n.kind() == SyntaxKind::SOURCE_BLOCK)
            .enumerate()
        {
            if let Some(block) = SourceBlock::cast(node) {
                out.push(SourceBlockInfo {
                    language: block.language().map(|t| t.to_string()),
                    body: block.value(),
                    index: idx,
                });
            }
        }
    }
    out
}

/// If the next non-blank line after `start` is a `#+RESULTS:` block, return
/// its full text range (so we can replace it).
fn existing_results_range(start: TextSize, after_str: &str) -> Option<TextRange> {
    let trimmed = after_str.trim_start_matches(|c: char| c == '\n' || c == ' ');
    let leading_ws = after_str.len() - trimmed.len();

    let lower = trimmed
        .lines()
        .next()
        .map(|l| l.trim_start().to_ascii_uppercase())?;
    if !lower.starts_with("#+RESULTS:") {
        return None;
    }

    // Span: from after the headline-source-block end, through the last
    // line that begins with ": " or is the #+RESULTS: header itself.
    let mut byte_len = 0usize;
    let mut lines = trimmed.lines().enumerate();
    let mut last_line_end = 0usize;
    while let Some((i, line)) = lines.next() {
        let is_results_header = i == 0;
        let is_result_body = line.trim_start().starts_with(": ") || line.trim().is_empty();
        if !(is_results_header || is_result_body) {
            break;
        }
        // empty line ends the block
        if i > 0 && line.trim().is_empty() {
            byte_len = last_line_end;
            break;
        }
        last_line_end += line.len() + 1; // +1 for \n
        byte_len = last_line_end;
    }
    let start_offset = TextSize::from((usize::from(start) + leading_ws) as u32);
    let end_offset = start_offset + TextSize::from(byte_len as u32);
    Some(TextRange::new(start_offset, end_offset))
}

fn format_results_block(results: &str) -> String {
    let body: String = results
        .lines()
        .map(|l| {
            if l.is_empty() {
                String::from("\n")
            } else if l.starts_with(": ") {
                format!("{l}\n")
            } else {
                format!(": {l}\n")
            }
        })
        .collect();
    format!("#+RESULTS:\n{body}")
}

// ───── tests ─────

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
#+TITLE: round-trip sample
#+TODO: TODO DOING | DONE

* TODO First task
:PROPERTIES:
:ID: task-1
:END:

Body text.

* DONE Second task
:PROPERTIES:
:ID: task-2
:END:
:LOGBOOK:
- State \"DONE\" from \"DOING\" [2026-05-20 Tue 10:00:00]
:END:

#+begin_src shell
echo hello
#+end_src

#+RESULTS:
: hello
";

    #[test]
    fn round_trip_byte_identical() {
        assert_eq!(Document::parse(SAMPLE).serialize(), SAMPLE);
        assert!(Document::round_trip_ok(SAMPLE));
    }

    #[test]
    fn find_by_id() {
        let doc = Document::parse(SAMPLE);
        let hl = doc.find_by_id("task-1").unwrap();
        assert_eq!(hl.title_raw().trim(), "First task");
    }

    #[test]
    fn transition_todo_changes_keyword() {
        let mut doc = Document::parse(SAMPLE);
        doc.transition_todo("task-1", "DOING").unwrap();
        let out = doc.serialize();
        assert!(out.contains("* DOING First task"));
        assert!(!out.contains("* TODO First task"));
    }

    #[test]
    fn transition_todo_missing_id_errs() {
        let mut doc = Document::parse(SAMPLE);
        let err = doc.transition_todo("nope", "DOING").unwrap_err();
        matches!(err, WorgError::HeadlineNotFound(_));
    }

    #[test]
    fn transition_todo_cas_accepts_when_expected_matches() {
        let mut doc = Document::parse(SAMPLE);
        doc.transition_todo_cas("task-1", &["TODO", "NEXT"], "DOING").unwrap();
        let out = doc.serialize();
        assert!(out.contains("* DOING First task"));
    }

    #[test]
    fn transition_todo_cas_rejects_when_state_mismatches() {
        let mut doc = Document::parse(SAMPLE);
        // First transition takes the task to DOING.
        doc.transition_todo_cas("task-1", &["TODO"], "DOING").unwrap();
        // Second concurrent claimer expects TODO but finds DOING.
        let err = doc
            .transition_todo_cas("task-1", &["TODO"], "DOING")
            .unwrap_err();
        match err {
            WorgError::StateMismatch { id, actual, .. } => {
                assert_eq!(id, "task-1");
                assert_eq!(actual.as_deref(), Some("DOING"));
            }
            other => panic!("expected StateMismatch, got {other:?}"),
        }
    }

    #[test]
    fn transition_todo_cas_propagates_missing_id() {
        let mut doc = Document::parse(SAMPLE);
        let err = doc
            .transition_todo_cas("nope", &["TODO"], "DOING")
            .unwrap_err();
        matches!(err, WorgError::HeadlineNotFound(_));
    }

    #[test]
    fn append_logbook_creates_drawer_when_absent() {
        let mut doc = Document::parse(SAMPLE);
        doc.append_logbook("task-1", "State \"DOING\" from \"TODO\" [2026-05-20 Tue 11:00:00]")
            .unwrap();
        let out = doc.serialize();
        assert!(out.contains(":LOGBOOK:"));
        assert!(out.contains("State \"DOING\" from \"TODO\""));
        // First headline now has a logbook
        let doc2 = Document::parse(&out);
        let hl = doc2.find_by_id("task-1").unwrap();
        assert!(find_named_drawer(&hl, "LOGBOOK").is_some());
    }

    #[test]
    fn append_logbook_appends_to_existing_drawer() {
        let mut doc = Document::parse(SAMPLE);
        doc.append_logbook("task-2", "State \"ABANDONED\" from \"DONE\" [2026-05-21 Wed 09:00:00]")
            .unwrap();
        let out = doc.serialize();
        // both entries present in the same drawer
        let logbook = out.find(":LOGBOOK:").unwrap();
        let end = out[logbook..].find(":END:").unwrap();
        let body = &out[logbook..logbook + end];
        assert!(body.contains("State \"DONE\" from \"DOING\""));
        assert!(body.contains("State \"ABANDONED\" from \"DONE\""));
    }

    #[test]
    fn write_results_replaces_existing_block() {
        let mut doc = Document::parse(SAMPLE);
        doc.write_results("task-2", "world").unwrap();
        let out = doc.serialize();
        assert!(out.contains("#+RESULTS:\n: world\n"));
        assert!(!out.contains(": hello\n"));
    }

    #[test]
    fn mini_coffee_round_trip() {
        let src = include_str!("../../../examples/mini-coffee-mapped.org");
        let doc = Document::parse(src);
        let out = doc.serialize();
        if out != src {
            // diff for failure context
            let first_diff = src
                .chars()
                .zip(out.chars())
                .position(|(a, b)| a != b)
                .unwrap_or_else(|| src.len().min(out.len()));
            let ctx_start = first_diff.saturating_sub(40);
            let ctx_end = (first_diff + 40).min(src.len()).min(out.len());
            panic!(
                "round-trip drift at byte {first_diff}\n  src: {:?}\n  out: {:?}",
                &src.get(ctx_start..ctx_end).unwrap_or(""),
                &out.get(ctx_start..ctx_end).unwrap_or(""),
            );
        }
    }
}
