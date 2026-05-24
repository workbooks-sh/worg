defmodule WorgAgent.Tools.Bash do
  @moduledoc """
  Execute a shell command. Combined stdout/stderr is returned with
  the exit code prepended on a marker line so the LLM can detect
  failures unambiguously.

  Trust gate: refuses execution if ctx is missing or trust_level is
  neither `:sandboxed` nor `:full`. The Loop wires a real ctx with the
  task's :TRUST_LEVEL: property; in tests, pass an explicit
  `%{trust_level: :sandboxed, working_dir: tmp}` ctx.
  """

  @behaviour WorgAgent.Tool

  @impl true
  def name, do: "bash"

  @impl true
  def description do
    """
    Run a shell command. Use for: running CLIs, inspecting filesystem,
    invoking build tools, anything that fits on a POSIX command line.
    Returns combined stdout/stderr with a leading `exit=<n>` marker
    line so non-zero exits are unambiguous.
    """
  end

  @impl true
  def input_schema do
    %{
      "type" => "object",
      "properties" => %{
        "command" => %{
          "type" => "string",
          "description" => "The shell command to execute. Run with `sh -c`."
        }
      },
      "required" => ["command"]
    }
  end

  @impl true
  def execute(%{"command" => command}, ctx) when is_binary(command) do
    with :ok <- check_trust(ctx) do
      cwd = Map.get(ctx, :working_dir, File.cwd!())
      {output, exit_code} = System.cmd("sh", ["-c", command], stderr_to_stdout: true, cd: cwd)
      {:ok, "exit=#{exit_code}\n#{output}"}
    end
  end

  def execute(args, _ctx),
    do: {:error, {:bad_args, "expected %{\"command\" => string}; got #{inspect(args)}"}}

  defp check_trust(ctx) do
    case Map.get(ctx, :trust_level) do
      :sandboxed -> :ok
      :full -> :ok
      _ -> {:error, {:trust, "bash requires ctx[:trust_level] of :sandboxed or :full"}}
    end
  end
end
