# worg — Elixir library

This is a **library**, not a runtime. There is no Application module, no
supervision tree, no GenServer per document.

The caller — an agent process, a workbook CLI invocation, a LiveView, a
test — calls `Worg` functions when it needs to. The caller owns its own
lifecycle. When the caller exits, nothing keeps running.

## Surface

| Module | Purpose |
|---|---|
| `Worg` | Public API — parse / serialize / mutate / query |
| `Worg.Parser` | Rustler NIF wrapper (thin) |
| `Worg.Exec` | Source-block executors (`shell` / `lua` / `elixir`) |
| `Worg.Tangle` | `:tangle path` block materializer |
| `Worg.AtomicFile` | Local-disk atomic write helper |

For Studio: the agent process is an Elixir process in the Fly.io cluster.
It loads this library, mutates an `.org` document string, writes the new
string to R2, upserts derived rows in Postgres via Ecto, broadcasts on
`Phoenix.PubSub`. All in the same agent process, in one transaction. When
the session ends, nothing persists.

For workbook CLI / standalone: same library, called from `workbook plan`
subcommands or the standalone `worg` CLI. Atomic write to local disk via
`Worg.AtomicFile.write/2`. No DB.

## Status

Most modules are **stubs** at the time of handoff. Real implementations
land in wb-4vhr.15 (executors + tangle) and a new issue for the library
surface.

The Rust NIF (`crates/worg-nif`) is built and tested. `Worg.Parser` only
needs the Rustler glue to start working — see the `use Rustler` line in
`lib/worg/parser.ex`.

## Build

```bash
cd packages/worg/elixir/worg
mix deps.get
mix compile  # triggers Rustler build of ../../crates/worg-nif
mix test
```
