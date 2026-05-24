defmodule WorgAgent.Tools.Read do
  @moduledoc """
  Read a file's contents. Path is resolved relative to ctx.working_dir
  if relative; absolute paths are honored as-is.

  No trust gate — reads are inert. The path resolution prevents the
  LLM from accidentally escaping the working dir via relative paths,
  but does NOT prevent absolute paths from being read; that's
  intentional (tool users are trusted to ask for the right paths).
  """

  @behaviour WorgAgent.Tool

  @impl true
  def name, do: "read"

  @impl true
  def description do
    """
    Read a file's contents and return them as a UTF-8 string. Path is
    resolved relative to the agent's working directory; absolute
    paths are honored. Returns the file contents on success or an
    error string describing what went wrong (file missing,
    permissions, etc.).
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
        }
      },
      "required" => ["path"]
    }
  end

  @impl true
  def execute(%{"path" => path}, ctx) when is_binary(path) do
    resolved = resolve_path(path, ctx)

    case File.read(resolved) do
      {:ok, contents} -> {:ok, contents}
      {:error, reason} -> {:error, {:read_failed, resolved, reason}}
    end
  end

  def execute(args, _ctx),
    do: {:error, {:bad_args, "expected %{\"path\" => string}; got #{inspect(args)}"}}

  defp resolve_path(path, ctx) do
    if Path.type(path) == :absolute do
      path
    else
      Path.expand(path, Map.get(ctx, :working_dir, File.cwd!()))
    end
  end
end
