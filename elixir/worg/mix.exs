defmodule Worg.MixProject do
  use Mix.Project

  @description """
  Worg — canonical org-mode for multi-agent orchestration.

  This is a LIBRARY, not a runtime. It exposes pure functions for parsing,
  querying, mutating, and executing source blocks in `.org` files. There is
  no Application module, no supervision tree, no GenServer per document.
  The caller (an agent process, a CLI, a LiveView, etc.) owns its own
  lifecycle and calls Worg functions when it needs to.

  See packages/worg/WORG.md for the file-format spec and packages/worg/README.md
  for the project layout.
  """

  def project do
    [
      app: :worg,
      version: "0.1.0",
      elixir: "~> 1.19",
      start_permanent: false,
      description: @description,
      deps: deps(),
      rustler_crates: rustler_crates()
    ]
  end

  # Library mode: no `mod:` key, no Application module. Pulling in extra_apps
  # only so the BEAM has them at runtime when callers use logging / crypto.
  def application do
    [
      extra_applications: [:logger]
    ]
  end

  defp deps do
    [
      # Rust NIF for parse / mutate / query / lint.
      {:rustler, "~> 0.36.0"},
      # Sandboxed Lua executor (Worg.Exec.lua/2). On BEAM, no subprocess.
      {:luerl, "~> 1.4"},
      # JSON for Worg.query/2 — encodes predicate maps before handing
      # them to Parser.query_json/2 and decodes the result back to maps.
      {:jason, "~> 1.4"}
    ]
  end

  # Rustler tells mix where the worg-nif Rust source lives. Compile path is
  # ../../crates/worg-nif relative to this Mix project.
  defp rustler_crates do
    [
      worg_nif: [
        path: "../../crates/worg-nif",
        mode: if(Mix.env() == :prod, do: :release, else: :debug)
      ]
    ]
  end
end
