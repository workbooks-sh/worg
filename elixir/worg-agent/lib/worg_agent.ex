defmodule WorgAgent do
  @moduledoc """
  worg-agent — a reference Elixir runtime for WORG-defined agents.

  Lives at `packages/worg/elixir/worg-agent/`, sibling to `Worg`
  (`packages/worg/elixir/worg/`). worg-agent is native to worg, not a
  Workbooks-specific package — it's the canonical example of how to
  build a runtime plugin on the worg substrate. Workbooks can dogfood
  it; so can any other consumer that wants Pi-shaped agents driven
  from `.org` files.

  Substrate-vs-runtime split: `Worg` owns parsing + querying + lint as
  a pure-functional library. `WorgAgent` owns *execution* — load an
  agent definition + task DAG from an orchestrator board, prompt an
  LLM, dispatch tools, write runs back.

  At the time of this scaffold, the runtime is empty. Each child issue
  under `wb-nlln.21` adds one module:

  - `WorgAgent.Loader` (wb-nlln.21.2) — read `.wb-orch/agents.json` +
    `.wb-orch/tasks/*.json` into structs.
  - `WorgAgent.Tool` + `WorgAgent.Tools.*` (wb-nlln.21.3) — pluggable
    tool behaviour + default set (Bash, Read, Write, LuaEval).
  - `WorgAgent.Llm` (wb-nlln.21.4) — OpenRouter client with prompt
    caching, default model `xiaomi/mimo-v2.5-pro`.
  - `WorgAgent.Loop` (wb-nlln.21.5) — pick task, prompt, dispatch
    tools, write run.
  - `WorgAgent.Sync` (wb-nlln.21.6) — fold runs back into the
    source `.org` file via `worg orch import runs`.

  The public API lands progressively; for now this module is a
  documentation anchor. The OTP application boots with an empty
  supervision tree (`WorgAgent.Application`) — children get added
  alongside the modules that need them.
  """
end
