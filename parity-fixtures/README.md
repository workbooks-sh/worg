# WORG runtime parity fixtures

Shared corpus that any runtime claiming WORG compatibility must produce
identical wire output for. Sibling to the Elixir + Rust runtime test
suites — both load these fixtures, drive a deterministic LLM stub
against them, and snapshot the resulting `messages_json` + Run wire JSON.

## What's normative

For a given fixture, **both runtimes must produce**:

1. **Identical conversation shape.** Same number of messages, same
   roles in the same order, same `content` strings, same `tool_calls`
   (id, name, arguments).
2. **Identical Run wire JSON.** `id`, `task`, `agent`, `state`,
   `attempt`, `tokens`, `commits`, `artifacts` byte-equal at the
   JSON level. `started_at` / `finished_at` are timestamps and so are
   *not* part of the parity check.
3. **Same termination cause.** Both return `Ok(TurnOutcome)` /
   `{:ok, %TurnOutcome{}}` with the same `rounds` count, or both
   return the same error variant (`RoundsExhausted`,
   `EmptyResponse`, etc.).

What is NOT normative:

- Token-cost values (provider-side, not deterministic across runs).
- Telemetry event ordering across the two runtimes (one uses
  `:telemetry`, the other `tracing`).
- Internal tool dispatch semantics that don't surface to the wire
  (e.g., whether bash uses `/bin/sh` or `bash`).

## Fixture layout

Each fixture is a directory:

```
parity-fixtures/<name>/
  agent.org           # the agent definition (loaded by both runtimes)
  plan.org            # optional — task graph if the fixture exercises wb-ki6b.7
  brief.txt           # the user message that kicks off the turn
  llm-script.json     # ordered list of canned LLM responses
  expected.json       # expected conversation shape + Run wire JSON
```

`llm-script.json` is an array of `LlmResponse` objects (per the Rust
crate's `llm::LlmResponse` schema — `{message, usage, finish_reason}`).
The Rust runtime's `QueueLlm` test helper pops one per call; the Elixir
runtime's equivalent stub does the same.

`expected.json` carries `messages` (the full conversation post-turn) +
`run` (the Run wire JSON). Parity-runner asserts deep equality.

## Adding fixtures

1. Drop a new directory under `parity-fixtures/<descriptive-name>/`.
2. Write `agent.org` + `brief.txt` + `llm-script.json`.
3. Run the Rust parity test once with `WORG_AGENT_PARITY_BLESS=1` to
   capture `expected.json`.
4. Verify against the Elixir runtime with the equivalent bless flag.
5. Both runtimes must now pass without bless.

Drift is a hard fail. If a fixture starts failing after a runtime
change, the question is *which runtime regressed* — re-blessing
without diagnosing is a bug-hider.

## Adopted by

- Rust runtime — `packages/worg/crates/worg-agent/tests/parity.rs`
- Elixir runtime — `packages/worg/elixir/worg-agent/test/integration/parity_test.exs` (pending)
