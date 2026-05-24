defmodule WorgAgent.Tools.ShellWrapper do
  @moduledoc """
  `use`-macro for typed wrappers around shell-out tools (wavelet CLI,
  brandwork CLI, etc.). Each wrapper is a thin module that declares
  the CLI verb path + the arg map; the macro generates the `Tool`
  behaviour callbacks. wb-fcs3.

  ## Usage

      defmodule WorgAgent.Tools.WaveletLint do
        use WorgAgent.Tools.ShellWrapper,
          name: "wavelet_lint",
          binary: "wavelet",
          argv_prefix: ["lint"],
          description: "Run wavelet lint on a composition HTML...",
          input_schema: %{
            "type" => "object",
            "properties" => %{
              "path" => %{"type" => "string"},
              "platform" => %{"type" => "string"},
              "mp4" => %{"type" => "string"}
            },
            "required" => ["path"]
          },
          arg_map: [
            {"path", :positional},
            {"platform", "--platform"},
            {"mp4", "--mp4"}
          ]
      end

  ## What the macro generates

  - `@behaviour WorgAgent.Tool`
  - `name/0`, `description/0`, `input_schema/0` callbacks reading from
    the use-block options
  - `execute/2` that:
    1. Builds `argv` from `argv_prefix` + arg_map walked against args
    2. Runs `System.cmd(binary, argv, stderr_to_stdout: true, cd: ctx[:working_dir])`
    3. Returns `{:ok, "exit=<n>\\n<stdout>"}` — same shape as Bash tool

  ## arg_map shape

  Each entry is `{arg_key, flag_spec}` where flag_spec is either:
  - `:positional` — value appended as a positional argv element
  - `:positional_list` — list value, each element appended in order
  - `"--flag-name"` — value passed as `--flag-name <value>`
  - `{"--flag-name", :boolean}` — flag emitted only when value is truthy
  - `{"--flag-name", :repeat}` — list value, flag repeated per element

  Args missing from the input map are skipped (no flag emitted).

  ## Env injection

  Wrappers for tools that need env vars (BRANDWORK_BASE_URL for the
  brandwork CLI) can pass `env: [{"BRANDWORK_BASE_URL", System.get_env("BRANDWORK_BASE_URL")}, ...]`
  in the use-block. Env is forwarded to System.cmd.
  """

  defmacro __using__(opts) do
    name = Keyword.fetch!(opts, :name)
    binary = Keyword.fetch!(opts, :binary)
    argv_prefix = Keyword.get(opts, :argv_prefix, [])
    description = Keyword.fetch!(opts, :description)
    input_schema = Keyword.fetch!(opts, :input_schema)
    arg_map = Keyword.get(opts, :arg_map, [])
    env = Keyword.get(opts, :env, [])

    quote do
      @behaviour WorgAgent.Tool

      @impl true
      def name, do: unquote(name)

      @impl true
      def description, do: unquote(description)

      @impl true
      def input_schema, do: unquote(input_schema)

      @impl true
      def execute(args, ctx) when is_map(args) do
        WorgAgent.Tools.ShellWrapper.run(
          unquote(binary),
          unquote(argv_prefix),
          unquote(arg_map),
          unquote(env),
          args,
          ctx
        )
      end
    end
  end

  @doc """
  Build argv from the spec + run System.cmd. Returns `{:ok, output}`
  matching the Bash tool's exit=N marker convention. Errors during
  System.cmd (binary not found, etc.) come back as `{:error, term}`.
  """
  @spec run(String.t(), [String.t()], list(), list(), map(), map()) ::
          {:ok, String.t()} | {:error, term}
  def run(binary, argv_prefix, arg_map, env, args, ctx) do
    argv = argv_prefix ++ build_args(arg_map, args)
    cwd = Map.get(ctx, :working_dir, File.cwd!())

    # Resolve env vars from the spec's keyword list. Each entry can
    # be {name, literal_value} or {name, {:env, ENV_NAME}} for
    # dynamic resolution at call time.
    resolved_env =
      env
      |> Enum.map(fn
        {k, {:env, env_name}} -> {k, System.get_env(env_name) || ""}
        {k, v} when is_binary(v) -> {k, v}
      end)
      |> Enum.reject(fn {_k, v} -> v == "" end)

    cmd_opts = [stderr_to_stdout: true, cd: cwd]
    cmd_opts = if resolved_env == [], do: cmd_opts, else: Keyword.put(cmd_opts, :env, resolved_env)

    try do
      {output, exit_code} = System.cmd(binary, argv, cmd_opts)
      {:ok, "exit=#{exit_code}\n#{output}"}
    rescue
      e in [ErlangError] ->
        case e do
          %ErlangError{original: :enoent} ->
            {:error, "binary not found on PATH: #{binary}"}

          other ->
            {:error, {:cmd_failed, Exception.message(other)}}
        end

      e ->
        {:error, {:cmd_failed, Exception.message(e)}}
    end
  end

  @doc """
  Walk arg_map against args, producing the argv suffix.

  Skips entries whose key is absent from args. Honors arg_map entry
  shapes: `:positional`, `:positional_list`, `"--flag"`,
  `{"--flag", :boolean}`, `{"--flag", :repeat}`.
  """
  @spec build_args(list(), map()) :: [String.t()]
  def build_args(arg_map, args) when is_list(arg_map) and is_map(args) do
    Enum.flat_map(arg_map, fn entry -> entry_argv(entry, args) end)
  end

  defp entry_argv({key, :positional}, args) do
    case Map.get(args, key) do
      nil -> []
      val -> [to_string(val)]
    end
  end

  defp entry_argv({key, :positional_list}, args) do
    case Map.get(args, key) do
      nil -> []
      list when is_list(list) -> Enum.map(list, &to_string/1)
      single -> [to_string(single)]
    end
  end

  defp entry_argv({key, {flag, :boolean}}, args) when is_binary(flag) do
    case Map.get(args, key) do
      true -> [flag]
      _ -> []
    end
  end

  defp entry_argv({key, {flag, :repeat}}, args) when is_binary(flag) do
    case Map.get(args, key) do
      nil -> []
      list when is_list(list) -> Enum.flat_map(list, fn v -> [flag, to_string(v)] end)
      single -> [flag, to_string(single)]
    end
  end

  defp entry_argv({key, flag}, args) when is_binary(flag) do
    case Map.get(args, key) do
      nil -> []
      val -> [flag, to_string(val)]
    end
  end
end
