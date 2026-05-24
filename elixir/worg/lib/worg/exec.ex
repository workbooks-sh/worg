defmodule Worg.Exec do
  @moduledoc """
  Source-block executors. Three languages, three pure functions. No
  supervisor, no GenServer — the caller (agent process, CLI invocation)
  owns the Task that invokes these and supervises it under its own tree.

  Languages dispatch:

    * `:shell` / `"bash"` / `"sh"` → `shell/2` — `System.cmd/3` with timeout
    * `:lua` → `lua/2` — Luerl interpreter on BEAM, sandboxed by
      construction (no `os`/`io`/`file` modules in the global env)
    * `:elixir` → `elixir/2` — `Code.eval_string/3`, FULL TRUST

  Any other language is inert. Worg does not invent dispatch for `:python`,
  `:javascript`, etc. — callers that need those bind them at the agent's
  tool layer, not through worg source blocks.

  All three functions return `{:ok, stdout_or_value_string}` on success
  or `{:error, term}` on failure. The string convention keeps the result
  cleanly embeddable in a `#+RESULTS:` block via `Worg.write_results/3`.
  """

  @type block :: %{
          optional(:language) => String.t(),
          optional(:body) => String.t(),
          optional(:header_args) => %{optional(String.t()) => String.t()},
          optional(any) => any
        }

  @type exec_opts :: keyword
  @type result :: {:ok, String.t()} | {:error, term}

  @default_timeout_ms 30_000

  # ───── shell ──────────────────────────────────────────────────────

  @doc """
  Run a shell block. Opts:
    * `:cwd` — working directory (defaults to current process cwd)
    * `:env` — list of `{"NAME", "value"}` env pairs (passed verbatim,
      no inheritance suppression — caller controls)
    * `:timeout_ms` — wall-clock cap, default 30_000.
      Process is killed (`SIGKILL`) on timeout.

  Stdout is captured + returned as a UTF-8 string. Stderr is merged into
  stdout (`:stderr_to_stdout` true), matching the org-babel default.
  Non-zero exit returns `{:error, {:exit_status, n, captured_output}}`.
  """
  @spec shell(block, exec_opts) :: result
  def shell(%{body: body} = block, opts \\ []) when is_binary(body) do
    cmd_opts =
      [stderr_to_stdout: true]
      |> Keyword.merge(if cwd = opts[:cwd], do: [cd: cwd], else: [])
      |> Keyword.merge(if env = opts[:env], do: [env: env], else: [])

    timeout_ms = Keyword.get(opts, :timeout_ms, @default_timeout_ms)
    interpreter = shell_interpreter(block)

    # Run System.cmd in a Task so we can enforce a wall-clock timeout.
    # System.cmd itself blocks forever; the Task wrapper + Task.yield
    # gives us a kill switch when the script hangs.
    task =
      Task.async(fn ->
        try do
          System.cmd(interpreter, ["-c", body], cmd_opts)
        rescue
          e -> {:error, e}
        end
      end)

    case Task.yield(task, timeout_ms) || Task.shutdown(task, :brutal_kill) do
      {:ok, {output, 0}} ->
        {:ok, output}

      {:ok, {output, status}} when is_integer(status) ->
        {:error, {:exit_status, status, output}}

      {:ok, {:error, e}} ->
        {:error, {:spawn_failed, e}}

      nil ->
        {:error, {:timeout, timeout_ms}}
    end
  end

  defp shell_interpreter(block) do
    case Map.get(block, :language, "bash") do
      "sh" -> "sh"
      _ -> "bash"
    end
  end

  # ───── elixir ─────────────────────────────────────────────────────

  @doc """
  Run an Elixir block via `Code.eval_string/3`. **FULL TRUST** — only call
  when the document's `:TRUST_LEVEL:` is `full` and the caller has
  verified provenance.

  Opts:
    * `:timeout_ms` — wall-clock cap (default 30_000)
    * `:bindings` — keyword list bound into the eval context

  Returns `{:ok, inspected_value}` — the eval result is rendered via
  `Kernel.inspect/1` so the caller always gets a string suitable for a
  `#+RESULTS:` block.
  """
  @spec elixir(block, exec_opts) :: result
  def elixir(%{body: body}, opts \\ []) when is_binary(body) do
    timeout_ms = Keyword.get(opts, :timeout_ms, @default_timeout_ms)
    bindings = Keyword.get(opts, :bindings, [])

    task =
      Task.async(fn ->
        try do
          {value, _new_bindings} = Code.eval_string(body, bindings, [])
          {:ok, inspect(value)}
        rescue
          e -> {:error, {:raise, Exception.message(e)}}
        catch
          kind, value -> {:error, {kind, value}}
        end
      end)

    case Task.yield(task, timeout_ms) || Task.shutdown(task, :brutal_kill) do
      {:ok, result} -> result
      nil -> {:error, {:timeout, timeout_ms}}
    end
  end

  # ───── lua ────────────────────────────────────────────────────────

  @doc """
  Run a Lua block under Luerl. Starts from Luerl's default state.

  Opts:
    * `:timeout_ms` — wall-clock cap (default 30_000)

  Returns `{:ok, last_expression_value_as_string}`. The last expression
  is `inspect`-ed for consistent string output.

  ## Sandbox caveat

  Luerl's default `init/0` state EXPOSES `os`, `io`, `package`, and
  `debug` modules — scripts CAN call `os.execute(...)` to spawn host
  processes, `io.open(...)` to read files, etc. The sandboxing the
  module-level docs aspire to is NOT implemented at the time of this
  writing — stripping these modules via `set_table_keys/3` triggers
  Luerl-internal `:badrecord` errors that need API-version research
  to resolve. Until that lands:

    * Treat `Worg.Exec.lua/2` as full-trust (same as `elixir/2`).
    * Only invoke from callers that have verified the source-block
      provenance.
    * Future work: add `:sandbox true` opt that strips dangerous
      modules + adds an `:inject` map for pre-bound globals.

  Sandboxing is the right long-term shape — Luerl is genuinely
  sandboxable, the API mismatch is fixable. Tracked in the same
  epic (wb-4vhr).
  """
  @spec lua(block, exec_opts) :: result
  def lua(%{body: body}, opts \\ []) when is_binary(body) do
    timeout_ms = Keyword.get(opts, :timeout_ms, @default_timeout_ms)

    task =
      Task.async(fn ->
        try do
          state = :luerl.init()

          # Luerl 1.x returns:
          #   {:ok, results, state}            — success
          #   {:lua_error, reason, state}      — script raised via error()
          #   {:error, reason, state}          — runtime / parser error
          case :luerl.do(body, state) do
            {:ok, [], _new_state} ->
              {:ok, ""}

            {:ok, [first | _rest], _new_state} ->
              {:ok, inspect(first)}

            {:lua_error, reason, _new_state} ->
              {:error, {:lua_error, reason}}

            {:error, reason, _new_state} ->
              {:error, {:lua_error, reason}}

            other ->
              {:error, {:lua_unexpected, other}}
          end
        rescue
          e -> {:error, {:raise, Exception.message(e)}}
        catch
          kind, value -> {:error, {kind, value}}
        end
      end)

    case Task.yield(task, timeout_ms) || Task.shutdown(task, :brutal_kill) do
      {:ok, result} -> result
      nil -> {:error, {:timeout, timeout_ms}}
    end
  end
end
