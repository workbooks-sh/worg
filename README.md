<p align="center">
  <img src=".github/banner.png" alt="worg" width="540">
</p>

# worg

**Canonical org-mode for multi-agent orchestration.**

worg parses, queries, and executes plain `.org` files as the source of
truth for agents, plans, task graphs, skills, and validators. The files
open in Emacs and drive a supervised runtime. No new DSL, no proprietary
schema — the agent's plan *is* an org document, and the document is the
program.

## Why org-mode

Agents need a planning substrate that humans can read, diff, edit,
and version without tooling. Most "agent frameworks" invent a YAML or
JSON dialect for the same job and then bolt on viewers, editors, and
linters around it. Org-mode already has all of that — plus tags,
properties, agenda queries, source blocks, and 20 years of editor
support. worg standardizes a small set of conventions on top so the
same file works in Emacs, in a CLI, and as input to a supervised
runtime.

## What worg gives you

- **Parser + AST + serializer** — round-trip-safe org reader (Rust,
  `syn`-style API). Survives edits without reformatting.
- **Agenda-style queries** — pull tasks by tag, property, deadline,
  state across a tree of files.
- **DAG executor** — walks a plan, resolves dependencies, dispatches
  work, records run state back into the file.
- **Glossary system** — `#+GLOSSARY:` lets consumers extend the
  vocabulary (runtime targets, dispatch hints, environment shape)
  without forking worg.
- **Bindings** — native Rust crate, Node + browser WASM, and a
  Rustler NIF for Elixir hosts.

## Design

**worg is a library, not a runtime.** There is no daemon, no GenServer
per document, nothing keeping state when your process exits. The host
links worg, calls its functions, and writes the file. When the host
stops, nothing stays running.

This makes worg usable in three shapes from the same codebase:

| Host | How worg is loaded |
|---|---|
| Local CLI (`worg`) | Native binary |
| Node / browser tool | WASM bindings |
| Elixir / BEAM service | Rustler NIF |

The `.org` file is canonical. Any database the host maintains (an
agenda index, a run log) is a derived reactive view — rebuildable from
the file, never the source of truth.

## Layout

```
worg/
├── WORG.md                 spec — parser conventions + glossary system
├── crates/
│   ├── worg-parse/         parser + AST + serializer
│   ├── worg-query/         agenda-style queries
│   ├── worg-cli/           `worg` native binary
│   ├── worg-nif/           Rustler NIF for Elixir
│   ├── worg-wasm/          wasm-pack targets
│   └── worg-orch/          Orchestrator Protocol types (run import / export)
├── elixir/worg/            Elixir library wrapper around the NIF
├── bindings/{node,browser} generated WASM bindings
└── examples/               reference org files
```

## Build

```bash
cargo build
cargo test
```

## Status

Parser + query + executor + glossary are stable. CLI and bindings are
under active development — see [`WORG.md`](./WORG.md) for the current
spec and roadmap.
