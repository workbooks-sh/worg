defmodule WorgAgent.MixProject do
  use Mix.Project

  @description """
  worg-agent — a reference Elixir runtime for WORG-defined agents.

  Lives at packages/worg/elixir/worg-agent/, sibling to the Worg Elixir
  library. Native to worg, not a Workbooks-specific package — the
  canonical example of how to build a runtime plugin on the worg
  substrate.

  Substrate-vs-runtime split: Worg owns parsing, querying, and lint
  (pure-functional library). WorgAgent owns execution — load an agent
  definition + task DAG from the orchestrator board, prompt an LLM,
  dispatch tools, write runs back. Workbooks can dogfood this via
  Watershed; Pi remains an alternate runtime via the pi-orchestrator
  adapter; Wavelet keeps its own pipeline (different shape).
  """

  def project do
    [
      app: :worg_agent,
      version: "0.1.0",
      elixir: "~> 1.17",
      elixirc_options: [warnings_as_errors: true],
      start_permanent: Mix.env() == :prod,
      deps: deps(),
      description: @description,
      package: package()
    ]
  end

  def application do
    [
      mod: {WorgAgent.Application, []},
      extra_applications: [:logger]
    ]
  end

  defp deps do
    # Each dep earns its place when a child issue actually consumes it.
    # Speculative deps DO NOT belong here.
    [
      # wb-nlln.21.2: JSON parsing of .wb-orch/agents.json + tasks/*.json.
      {:jason, "~> 1.4"},
      # wb-nlln.21.4: HTTP client for OpenRouter. Req's test adapter
      # (Req.Test) handles HTTP mocking in tests — no real network
      # calls in CI.
      {:req, "~> 0.5"},
      # Plug provides the Conn struct Req's `plug:` test option uses.
      # Test-only — no runtime cost.
      {:plug, "~> 1.16", only: :test},
      # wb-qk6l.2: Lua-on-BEAM evaluator for the lua_eval tool. Luerl
      # provides a sandboxed Lua VM in pure Erlang — no OS execution,
      # no file I/O, no network from Lua. Same engine the WORG-Lua
      # executor will use, so a Watershed-side host can reuse this
      # tool wholesale.
      {:luerl, "~> 1.5"}
    ]
  end

  defp package do
    [
      maintainers: ["workbooks-sh"],
      licenses: ["MIT OR Apache-2.0"],
      links: %{
        "Source" => "https://github.com/workbooks-sh/workbooks-mono",
        "Spec" => "https://github.com/workbooks-sh/worg"
      }
    ]
  end
end
