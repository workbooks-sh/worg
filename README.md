# worg

**Canonical org-mode for multi-agent orchestration.** Workbooks
ships planning code as `.org` files that open in Emacs *and* drive a
supervised Elixir runtime. Spec: [`WORG.md`](./WORG.md).

This directory is a `git subtree`, mirrored to
[github.com/workbooks-sh/worg](https://github.com/workbooks-sh/worg).
Author changes here in the workbooks monorepo and commit normally.
Periodic `git subtree push --prefix=packages/worg worg main` releases
upstream.

## Layout

```
packages/worg/
├── WORG.md                    spec — what worg parses + standardized conventions
├── Cargo.toml                 Rust workspace root
├── crates/
│   ├── worg-parse/            parser + AST + serializer (wb-4vhr.1, .3)
│   ├── worg-query/            agenda-style queries (wb-4vhr.4) [planned]
│   ├── worg-cli/              native `worg` binary (wb-4vhr.5) [planned]
│   ├── worg-nif/              Rustler NIF for Elixir (wb-4vhr.6) [planned]
│   └── worg-wasm/             wasm-pack targets (wb-4vhr.7, .8) [planned]
├── elixir/
│   └── worg/                  Elixir LIBRARY (not a runtime — no Application module)
├── bindings/                  generated JS bindings from worg-wasm
│   ├── node/                  wasm-pack output, target=nodejs
│   └── browser/               wasm-pack output, target=web
└── examples/
    └── mini-coffee-mapped.org  hand-mapped from gamut/001-mini-coffee dryrun
                                (the wb-4vhr.19 validation gate)
```

## Build

```bash
cd packages/worg
cargo build
cargo test
```

The workspace builds independently of the rest of the monorepo —
worg has no Rust dependency on gamut, rvst, or workbook-cli. It is
*consumed* by them (workbook-cli embeds plans, gamut seeds from
`.org` files), but it doesn't depend on them.

## Architecture

**Worg is a library, not a runtime.** There is no daemon, no supervised
runtime, no GenServer per document. The agent process — whatever it is in
whatever context — links worg as a library and calls its functions when it
needs to. When the agent stops, nothing keeps running.

Three contexts, same library:

| Context | How worg is loaded | What writes the file |
|---|---|---|
| **Studio** (Elixir agent on Fly.io) | NIF (`Worg.Parser` → `worg-nif`) | The agent process, in one `Ecto.Multi` (file to R2 + derived rows to Postgres + PubSub broadcast) |
| **Workbook CLI** | WASM (Node bindings → `bindings/node/`) | `workbook plan <subcommand>` writes locally via temp+rename |
| **Standalone / OSS use** | crates.io (Rust crate dep) or WASM | `worg <subcommand>` writes locally |

The `.org` file is canonical. Postgres (Studio only) is a derived reactive
index — rebuildable from the file, never the source of truth.

## Status

Work tracked under epic [wb-4vhr](../../.beads/issues.jsonl). See the
epic description for full handoff state, completed work, and what's next.
