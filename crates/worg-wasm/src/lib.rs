//! worg-wasm — wasm-bindgen wrapper for the worg stack.
//!
//! One Rust crate, two build targets:
//!
//!   wasm-pack build --target nodejs --out-dir ../../bindings/node    # workbook-cli (Node)
//!   wasm-pack build --target web    --out-dir ../../bindings/browser # workbook viewer (browser)
//!
//! Both bindings are read-only from JS — write-back mutations go through
//! Elixir + the NIF. The browser surface is purely observational (renders
//! plan state, doesn't mutate). The Node CLI uses it at build time to
//! embed parsed plans into compiled `.html` workbooks.

use wasm_bindgen::prelude::*;
use worg_parse::Document;
use worg_query::Predicate;

/// Parse and emit a JSON summary of headlines.
#[wasm_bindgen]
pub fn parse_headlines(src: &str) -> String {
    let doc = Document::parse(src);
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

/// Check the round-trip invariant.
#[wasm_bindgen]
pub fn round_trip_ok(src: &str) -> bool {
    Document::round_trip_ok(src)
}

/// Run a JSON-encoded predicate. Returns matching headline summaries as
/// JSON. Returns an error string prefixed with `"ERR:"` on invalid input.
#[wasm_bindgen]
pub fn query_json(src: &str, predicate_json: &str) -> String {
    let pred: Predicate = match serde_json::from_str(predicate_json) {
        Ok(p) => p,
        Err(e) => return format!("ERR:invalid predicate: {e}"),
    };
    let doc = Document::parse(src);
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
    serde_json::to_string(&summary).unwrap_or_else(|_| "[]".to_string())
}

/// Lint, returning a JSON array of diagnostics.
#[wasm_bindgen]
pub fn lint_json(src: &str) -> String {
    let doc = Document::parse(src);
    let diags = worg_lint::lint(&doc);
    serde_json::to_string(&diags).unwrap_or_else(|_| "[]".to_string())
}

/// Render to HTML via orgize's exporter.
#[wasm_bindgen]
pub fn render_html(src: &str) -> String {
    orgize::Org::parse(src).to_html()
}
