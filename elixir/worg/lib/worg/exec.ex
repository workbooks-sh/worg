defmodule Worg.Exec do
  @moduledoc """
  Source-block executors. Three languages, three pure functions. No supervisor,
  no GenServer — the caller (agent process, CLI invocation) owns the Task that
  invokes these and supervises it under its own tree.

  Languages dispatch:

    * `:shell` / `"bash"` / `"sh"` → `Worg.Exec.shell/2` — `Port` with timeout
    * `:lua`  → `Worg.Exec.lua/2`   — Luerl interpreter on BEAM, sandboxed by construction
    * `:elixir` → `Worg.Exec.elixir/2` — `Code.eval_string/3`, full-trust

  Any other language is inert. Worg does not invent dispatch for `:python`,
  `:javascript`, etc. — callers that need those bind them at the agent's tool
  layer, not through worg source blocks.

  This module is a stub at the time of handoff. Real implementation lands in
  wb-4vhr.15.
  """

  @type block :: %{
          language: String.t(),
          body: String.t(),
          header_args: %{optional(String.t()) => String.t()}
        }

  @type exec_opts :: keyword
  @type result :: {:ok, String.t()} | {:error, term}

  @doc "Run a shell block. opts: :cwd, :env, :timeout_ms (default 30_000)."
  @spec shell(block, exec_opts) :: result
  def shell(_block, _opts \\ []), do: raise("not yet implemented — wb-4vhr.15")

  @doc """
  Run a Lua block under Luerl. Sandboxed — no os/io/file modules. opts:
  :timeout_ms, :inject (map of safe values pre-bound in the Lua env).
  """
  @spec lua(block, exec_opts) :: result
  def lua(_block, _opts \\ []), do: raise("not yet implemented — wb-4vhr.15")

  @doc """
  Run an Elixir block via Code.eval_string. FULL TRUST — only call when the
  document's :TRUST_LEVEL: is `full` and the caller has verified provenance.
  opts: :timeout_ms, :bindings (keyword list passed to Code.eval_string).
  """
  @spec elixir(block, exec_opts) :: result
  def elixir(_block, _opts \\ []), do: raise("not yet implemented — wb-4vhr.15")
end
