# worg

worg is **canonical org-mode** plus a fixed set of conventions for
multi-agent orchestration. Every `.org` file worg parses is also a
valid org-mode file that opens cleanly in any org viewer. We do not
invent syntax — we standardize property names, tag meanings, and
how source blocks dispatch to executors.

## Canonical sources

This README is a pointer file. The substance lives elsewhere:

- **Runtime semantics, vocabulary, lint codes** — [`w.org`](./w.org)
  is the authoritative spec. Status enum, classification tags, payload
  properties, trust enum, validator KIND registry, plugin registry,
  source-block dispatch table, file-level keywords, lint rules,
  extension policy. The worg linter reads this file at startup; edits
  to `w.org` change lint behavior on the next run.

- **Grammar** — [orgmode.org/manual](https://orgmode.org/manual/) is
  the org-mode reference. worg is a strict subset; anything that
  parses there parses here.

- **Plugin registry** — see `w.org`'s `* Plugin registry` section.
  Each plugin's behavior is documented in
  [`packages/worg-plugin-<name>/manifest.org`](../) (sibling crates
  to this one).

- **Dependency layers + ownership** — [`CLAUDE.md`](../../CLAUDE.md)
  "Dependency layers (what eats what)" describes how worg fits with
  Watershed, the Workbooks runtime, and the orchestrator protocol.
  worg is **runtime- and consumer-agnostic**; Watershed-specific
  concerns (runtime targets, dispatch, sandboxing) live in
  Watershed's glossary extension, not in worg itself.

## Crates

| Crate | Purpose |
|---|---|
| `crates/worg-parse` | Parser + AST + edit-preserving serializer |
| `crates/worg-query` | Predicate-based queries over the AST |
| `crates/worg-lint`  | Lint rules driven by `w.org` glossary |
| `crates/worg-orch`  | Wire-format types + walkers for the Workbooks Orchestrator Protocol |
| `crates/worg-cli`   | `worg` command-line tool |
| `crates/worg-nif`   | Rustler NIF bindings for Elixir (`Worg.Parser`) |
| `crates/worg-wasm`  | wasm-bindgen targets for nodejs + web |
| `crates/worg-wasi`  | Bare-metal wasm exports for Wasmex (BEAM) |
| `crates/worg-bench` | Benchmark harness |
| `elixir/worg`       | Elixir API consuming the NIF |

## Extension policy

worg's standardized vocabulary is deliberately small. Projects that
need additional property names, tags, or plugins should ship them in
a separate glossary file and reference it from individual files via
`#+GLOSSARY: ./path/to/extra.org` at the top. See `w.org`'s
`* Extension policy` for the full rules.
