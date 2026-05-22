//! worg-nif — Rustler bindings for the worg parser, query, and lint stack.
//!
//! Loaded from the `WorgRuntime.Parser` Elixir module via Rustler's
//! `:rustler.load_nif` glue. All exported NIFs run on dirty CPU schedulers
//! since parsing is CPU-bound (and document files can be large) — keeps the
//! main BEAM schedulers responsive.
//!
//! The Elixir side is in `elixir/worg_runtime/lib/worg_runtime/parser.ex`.
//!
//! ## Data model across the boundary
//!
//! We use strings on both sides — no Rust-owned references handed to Elixir.
//! The Elixir runtime holds the source text; for every mutation it calls
//! a NIF, gets back the new text, and writes that to disk. This is the
//! simplest correct model and aligns with worg-parse's text-range-based
//! mutation API (the parser is effectively stateless from Elixir's POV).

use rustler::{Env, Term};
use worg_parse::Document;
use worg_query::Predicate;

mod atoms {
    rustler::atoms! {
        ok,
        error,
        not_found,
        parse_error,
        no_todo_keyword,
        invalid_predicate,
        invalid_severity,
        invalid_arg,
    }
}

/// Round-trip check: returns true if `Document::parse(src).serialize() == src`.
/// The Elixir runtime calls this on every document load as a defense against
/// orgize regressions.
#[rustler::nif(schedule = "DirtyCpu")]
fn round_trip_ok(src: String) -> bool {
    Document::round_trip_ok(&src)
}

/// Parse + emit a JSON summary of headlines. The Elixir runtime decodes this
/// into Document.Server's in-memory representation.
#[rustler::nif(schedule = "DirtyCpu")]
fn parse_headlines_json(src: String) -> String {
    let doc = Document::parse(&src);
    let summary: Vec<_> = doc
        .headlines()
        .iter()
        .map(|h| {
            serde_json::json!({
                "level": h.level(),
                "title": h.title_raw().trim(),
                "state": h.todo_keyword().map(|t| t.to_string()),
                "id": h.properties().and_then(|p| p.get("ID")).map(|t| t.to_string()),
                "tags": h.tags().map(|t| t.to_string()).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::to_string(&summary).unwrap_or_else(|_| "[]".to_string())
}

/// Apply a TODO transition. Returns the updated document text on success.
#[rustler::nif(schedule = "DirtyCpu")]
fn transition_todo<'a>(
    env: Env<'a>,
    src: String,
    id: String,
    new_state: String,
) -> Term<'a> {
    let mut doc = Document::parse(&src);
    match doc.transition_todo(&id, &new_state) {
        Ok(()) => (atoms::ok(), doc.serialize()).encode(env),
        Err(worg_parse::WorgError::HeadlineNotFound(_)) => {
            (atoms::error(), atoms::not_found()).encode(env)
        }
        Err(worg_parse::WorgError::NoTodoKeyword) => {
            (atoms::error(), atoms::no_todo_keyword()).encode(env)
        }
        Err(_) => (atoms::error(), atoms::parse_error()).encode(env),
    }
}

/// Append an entry to a headline's `:LOGBOOK:` drawer. Returns updated text.
#[rustler::nif(schedule = "DirtyCpu")]
fn append_logbook<'a>(
    env: Env<'a>,
    src: String,
    id: String,
    entry: String,
) -> Term<'a> {
    let mut doc = Document::parse(&src);
    match doc.append_logbook(&id, &entry) {
        Ok(()) => (atoms::ok(), doc.serialize()).encode(env),
        Err(worg_parse::WorgError::HeadlineNotFound(_)) => {
            (atoms::error(), atoms::not_found()).encode(env)
        }
        Err(_) => (atoms::error(), atoms::parse_error()).encode(env),
    }
}

/// Write a `#+RESULTS:` block under the first source block of a headline.
/// Returns updated text.
#[rustler::nif(schedule = "DirtyCpu")]
fn write_results<'a>(
    env: Env<'a>,
    src: String,
    id: String,
    results: String,
) -> Term<'a> {
    let mut doc = Document::parse(&src);
    match doc.write_results(&id, &results) {
        Ok(()) => (atoms::ok(), doc.serialize()).encode(env),
        Err(worg_parse::WorgError::HeadlineNotFound(_)) => {
            (atoms::error(), atoms::not_found()).encode(env)
        }
        Err(_) => (atoms::error(), atoms::parse_error()).encode(env),
    }
}

/// Run a JSON-encoded predicate. Returns a JSON array of matching headline
/// summaries (id, title, state).
#[rustler::nif(schedule = "DirtyCpu")]
fn query_json<'a>(env: Env<'a>, src: String, predicate_json: String) -> Term<'a> {
    let Ok(pred): Result<Predicate, _> = serde_json::from_str(&predicate_json) else {
        return (atoms::error(), atoms::invalid_predicate()).encode(env);
    };
    let doc = Document::parse(&src);
    let matches = worg_query::query(&doc, &pred);
    let summary: Vec<_> = matches
        .iter()
        .map(|h| {
            serde_json::json!({
                "id": h.properties().and_then(|p| p.get("ID")).map(|t| t.to_string()),
                "title": h.title_raw().trim(),
                "state": h.todo_keyword().map(|t| t.to_string()),
            })
        })
        .collect();
    let json = serde_json::to_string(&summary).unwrap_or_else(|_| "[]".to_string());
    (atoms::ok(), json).encode(env)
}

/// Generic drawer append. Returns updated text. Use for NOTES, CONSTRAINTS,
/// custom drawers — LOGBOOK has its own NIF (`append_logbook`) for
/// backward-compat but routes through the same Document method.
#[rustler::nif(schedule = "DirtyCpu")]
fn append_drawer<'a>(
    env: Env<'a>,
    src: String,
    id: String,
    drawer_name: String,
    entry: String,
) -> Term<'a> {
    let mut doc = Document::parse(&src);
    match doc.append_drawer(&id, &drawer_name, &entry) {
        Ok(()) => (atoms::ok(), doc.serialize()).encode(env),
        Err(worg_parse::WorgError::HeadlineNotFound(_)) => {
            (atoms::error(), atoms::not_found()).encode(env)
        }
        Err(_) => (atoms::error(), atoms::parse_error()).encode(env),
    }
}

/// Set or update a property in the :PROPERTIES: drawer. The :ID: key is
/// reserved (returns invalid_arg) because it's the lookup key for
/// find_by_id; agents that want to "rename" should add_child a new headline
/// with the new ID and migrate manually.
#[rustler::nif(schedule = "DirtyCpu")]
fn set_property<'a>(
    env: Env<'a>,
    src: String,
    id: String,
    name: String,
    value: String,
) -> Term<'a> {
    let mut doc = Document::parse(&src);
    match doc.set_property(&id, &name, &value) {
        Ok(()) => (atoms::ok(), doc.serialize()).encode(env),
        Err(worg_parse::WorgError::HeadlineNotFound(_)) => {
            (atoms::error(), atoms::not_found()).encode(env)
        }
        Err(worg_parse::WorgError::InvalidArg(_)) => {
            (atoms::error(), atoms::invalid_arg()).encode(env)
        }
        Err(_) => (atoms::error(), atoms::parse_error()).encode(env),
    }
}

/// Add a child headline at the end of a parent's subtree. The child gets a
/// :PROPERTIES: drawer with the supplied :ID:. State is optional (None for
/// plain headlines).
#[rustler::nif(schedule = "DirtyCpu")]
fn add_child<'a>(
    env: Env<'a>,
    src: String,
    parent_id: String,
    title: String,
    state: Option<String>,
    child_id: String,
) -> Term<'a> {
    let mut doc = Document::parse(&src);
    let state_ref = state.as_deref();
    match doc.add_child(&parent_id, &title, state_ref, &child_id) {
        Ok(()) => (atoms::ok(), doc.serialize()).encode(env),
        Err(worg_parse::WorgError::HeadlineNotFound(_)) => {
            (atoms::error(), atoms::not_found()).encode(env)
        }
        Err(worg_parse::WorgError::InvalidArg(_)) => {
            (atoms::error(), atoms::invalid_arg()).encode(env)
        }
        Err(_) => (atoms::error(), atoms::parse_error()).encode(env),
    }
}

/// Return every source block under the headline identified by
/// `target_id` as a JSON array of `{language, body, index}` objects.
/// Used by the Worg.Tools dispatcher so agents reading plans can
/// extract `#+BEGIN_SRC <lang> ... #+END_SRC` content without falling
/// back to raw text + regex.
#[rustler::nif(schedule = "DirtyCpu")]
fn source_blocks_json<'a>(
    env: Env<'a>,
    src: String,
    target_id: String,
) -> Term<'a> {
    let doc = Document::parse(&src);
    match doc.source_blocks_for(&target_id) {
        Ok(blocks) => {
            let json: Vec<_> = blocks
                .into_iter()
                .map(|b| {
                    serde_json::json!({
                        "language": b.language,
                        "body": b.body,
                        "index": b.index,
                    })
                })
                .collect();
            let s = serde_json::to_string(&json).unwrap_or_else(|_| "[]".to_string());
            (atoms::ok(), s).encode(env)
        }
        Err(worg_parse::WorgError::HeadlineNotFound(_)) => {
            (atoms::error(), atoms::not_found()).encode(env)
        }
        Err(_) => (atoms::error(), atoms::parse_error()).encode(env),
    }
}

/// Run lints. Returns JSON array of diagnostics.
#[rustler::nif(schedule = "DirtyCpu")]
fn lint_json(src: String) -> String {
    let doc = Document::parse(&src);
    let diags = worg_lint::lint(&doc);
    serde_json::to_string(&diags).unwrap_or_else(|_| "[]".to_string())
}

// Encode helper. Manual because Rustler's auto-encoding has some quirks
// around String + tuple-of-atoms results.
use rustler::Encoder;

rustler::init!("Elixir.Worg.Parser");
