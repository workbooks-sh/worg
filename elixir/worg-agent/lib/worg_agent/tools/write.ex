defmodule WorgAgent.Tools.Write do
  @moduledoc """
  Write content to a file, creating parent directories if missing.
  Overwrites any existing content. Path resolution mirrors `Read`.

  Trust gate: refuses execution if ctx is missing or trust_level is
  neither `:sandboxed` nor `:full`. Writes can mutate the workspace;
  the gate prevents accidental writes from unprivileged contexts.
  """

  @behaviour WorgAgent.Tool

  @impl true
  def name, do: "write"

  @impl true
  def description do
    """
    Write content to a file. Creates parent directories as needed.
    Overwrites any existing content. Path is resolved relative to
    the agent's working directory; absolute paths are honored.
    Returns "wrote N bytes to <path>" on success.
    """
  end

  @impl true
  def input_schema do
    %{
      "type" => "object",
      "properties" => %{
        "path" => %{
          "type" => "string",
          "description" => "Path to the file. Relative paths resolve against the agent's working directory."
        },
        "content" => %{
          "type" => "string",
          "description" => "The bytes to write. Overwrites any existing file content."
        }
      },
      "required" => ["path", "content"]
    }
  end

  @impl true
  def execute(%{"path" => path, "content" => content}, ctx)
      when is_binary(path) and is_binary(content) do
    with :ok <- check_trust(ctx) do
      resolved = resolve_path(path, ctx)
      parent = Path.dirname(resolved)

      with :ok <- File.mkdir_p(parent),
           :ok <- File.write(resolved, content) do
        {:ok, "wrote #{byte_size(content)} bytes to #{resolved}"}
      else
        {:error, reason} -> {:error, {:write_failed, resolved, reason}}
      end
    end
  end

  def execute(args, _ctx),
    do:
      {:error,
       {:bad_args, "expected %{\"path\" => string, \"content\" => string}; got #{inspect(args)}"}}

  defp check_trust(ctx) do
    case Map.get(ctx, :trust_level) do
      :sandboxed -> :ok
      :full -> :ok
      _ -> {:error, {:trust, "write requires ctx[:trust_level] of :sandboxed or :full"}}
    end
  end

  defp resolve_path(path, ctx) do
    if Path.type(path) == :absolute do
      path
    else
      Path.expand(path, Map.get(ctx, :working_dir, File.cwd!()))
    end
  end
end
