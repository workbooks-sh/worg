//! worg-wasi — bare-metal wasm exports of the worg stack, callable from
//! any Wasm runtime that supports the C ABI. Built for
//! `wasm32-unknown-unknown` so that Wasmex (Wasmtime under Elixir) can load
//! the artifact directly and call into it without wasm-bindgen JS glue.
//!
//! ## Calling convention
//!
//! Strings cross the boundary as `(ptr: u32, len: u32)`. Returned strings
//! are packed into a single `u64`: high 32 bits are the pointer, low 32
//! are the length. Allocation is host-driven: the host calls `alloc(n)`,
//! writes input bytes at the returned ptr, calls a function, reads the
//! returned bytes, then calls `dealloc(ptr, len)` to free both buffers.
//!
//! This pattern matches the standard Wasmex example shape (see Wasmex
//! tutorials in hex.pm/packages/wasmex). Re-use of input ptr+len for
//! dealloc requires the host to remember the input length.

use std::alloc::{alloc, dealloc as raw_dealloc, Layout};
use worg_parse::Document;
use worg_query::Predicate;

// --- Memory management exports -------------------------------------------

/// Allocate `len` bytes inside the wasm linear memory and return a pointer
/// to the start of the block. The host is responsible for calling
/// `dealloc(ptr, len)` once it no longer needs the buffer.
#[no_mangle]
pub extern "C" fn worg_alloc(len: u32) -> u32 {
    let layout = match Layout::from_size_align(len as usize, 1) {
        Ok(l) => l,
        Err(_) => return 0,
    };
    let ptr = unsafe { alloc(layout) };
    ptr as u32
}

/// Free a buffer previously returned by `worg_alloc` (or a function that
/// returned a packed pointer). `len` MUST match the original allocation.
#[no_mangle]
pub extern "C" fn worg_dealloc(ptr: u32, len: u32) {
    if ptr == 0 {
        return;
    }
    let layout = match Layout::from_size_align(len as usize, 1) {
        Ok(l) => l,
        Err(_) => return,
    };
    unsafe { raw_dealloc(ptr as *mut u8, layout) };
}

// --- Helpers --------------------------------------------------------------

/// Read a UTF-8 string from `(ptr, len)` inside our linear memory.
///
/// Safety: relies on the host having populated the buffer via `worg_alloc`
/// + a write through wasm linear memory.
unsafe fn read_str(ptr: u32, len: u32) -> &'static str {
    let slice = std::slice::from_raw_parts(ptr as *const u8, len as usize);
    // For our use case, all inputs are author-controlled org-mode text;
    // a non-UTF-8 input is a host bug. Falling back to empty avoids a
    // panic that would trap the wasm instance.
    std::str::from_utf8(slice).unwrap_or("")
}

/// Leak a heap-allocated `String` into linear memory and return a packed
/// `u64` (`high 32 = ptr`, `low 32 = len`). The host MUST eventually call
/// `worg_dealloc(ptr, len)` to release the buffer.
fn write_str(s: String) -> u64 {
    let bytes = s.into_bytes();
    let len = bytes.len() as u32;
    if len == 0 {
        // Returning ptr=0 len=0 is a valid empty result the host should
        // not attempt to dealloc.
        return 0;
    }
    let layout = match Layout::from_size_align(len as usize, 1) {
        Ok(l) => l,
        Err(_) => return 0,
    };
    let ptr = unsafe { alloc(layout) };
    if ptr.is_null() {
        return 0;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, len as usize);
    }
    ((ptr as u64) << 32) | (len as u64)
}

// --- Public API ----------------------------------------------------------

/// Parse the org source and return a JSON array of headline summaries.
/// Each headline includes the full :PROPERTIES: drawer as a `props` map
/// (key→value, both stringified). The `id` field stays as a top-level
/// convenience equal to `props["ID"]`.
#[no_mangle]
pub extern "C" fn parse_headlines(src_ptr: u32, src_len: u32) -> u64 {
    let src = unsafe { read_str(src_ptr, src_len) };
    let doc = Document::parse(src);
    let summary: Vec<_> = doc
        .headlines()
        .iter()
        .map(|h| {
            let props: serde_json::Map<String, serde_json::Value> = h
                .properties()
                .map(|p| {
                    p.iter()
                        .map(|(k, v)| (k.to_string(), serde_json::Value::String(v.to_string())))
                        .collect()
                })
                .unwrap_or_default();

            serde_json::json!({
                "level": h.level(),
                "title": h.title_raw().trim(),
                "state": h.todo_keyword().map(|t| t.to_string()),
                "id": h.properties().and_then(|p| p.get("ID")).map(|t| t.to_string()),
                "tags": h.tags().map(|t| t.to_string()).collect::<Vec<_>>(),
                "props": props,
            })
        })
        .collect();
    write_str(serde_json::to_string(&summary).unwrap_or_else(|_| "[]".to_string()))
}

/// Round-trip invariant — returns 1 if `serialize(parse(src)) == src`,
/// 0 otherwise. Cheap host-side bool decoding.
#[no_mangle]
pub extern "C" fn round_trip_ok(src_ptr: u32, src_len: u32) -> u32 {
    let src = unsafe { read_str(src_ptr, src_len) };
    if Document::round_trip_ok(src) {
        1
    } else {
        0
    }
}

/// Run a query. `predicate_json` is a JSON-encoded `worg_query::Predicate`.
/// Returns a JSON array of matching headline summaries.
#[no_mangle]
pub extern "C" fn query_json(
    src_ptr: u32,
    src_len: u32,
    pred_ptr: u32,
    pred_len: u32,
) -> u64 {
    let src = unsafe { read_str(src_ptr, src_len) };
    let pred_json = unsafe { read_str(pred_ptr, pred_len) };
    let pred: Predicate = match serde_json::from_str(pred_json) {
        Ok(p) => p,
        Err(e) => return write_str(format!("ERR:invalid predicate: {e}")),
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
    write_str(serde_json::to_string(&summary).unwrap_or_else(|_| "[]".to_string()))
}

/// Lint the source and return diagnostics as a JSON array.
#[no_mangle]
pub extern "C" fn lint_json(src_ptr: u32, src_len: u32) -> u64 {
    let src = unsafe { read_str(src_ptr, src_len) };
    let doc = Document::parse(src);
    let diags = worg_lint::lint(&doc);
    write_str(serde_json::to_string(&diags).unwrap_or_else(|_| "[]".to_string()))
}
