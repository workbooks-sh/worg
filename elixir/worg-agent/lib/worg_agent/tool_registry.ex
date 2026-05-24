defmodule WorgAgent.ToolRegistry do
  @moduledoc """
  Looks up tool implementations by name. Tools are registered via
  `config :worg_agent, :tools, [...]` in `config/config.exs`.

  The Loop (wb-nlln.21.5) consults this registry to (a) build the
  tool-use schema sent to the LLM and (b) dispatch tool_call results
  by name back into the right implementation.

  Extending: consumer apps (Watershed, third parties) add their own
  tool modules to the config list. Order doesn't matter — lookup is
  by `name/0` not list position.
  """

  @doc """
  Return the configured list of tool modules. Defaults to the built-in
  set (Bash, Read, Write, LuaEval) if no config is set.
  """
  @spec all() :: [module()]
  def all do
    Application.get_env(:worg_agent, :tools, [
      WorgAgent.Tools.Bash,
      WorgAgent.Tools.Read,
      WorgAgent.Tools.Write,
      WorgAgent.Tools.LuaEval
    ])
  end

  @doc """
  Look up a tool module by its canonical `name/0` value.
  Returns `nil` if no tool matches.
  """
  @spec lookup(String.t()) :: module() | nil
  def lookup(name) when is_binary(name) do
    Enum.find(all(), fn module -> module.name() == name end)
  end

  @doc """
  Build the tool-use catalog: a list of maps with `name`,
  `description`, and `input_schema`. The Llm client (wb-nlln.21.4)
  converts this into provider-specific tool-use payloads (OpenAI's
  `tools.function` shape, Anthropic's bare-tools shape, etc.).
  """
  @spec catalog() :: [map()]
  def catalog do
    Enum.map(all(), fn module ->
      %{
        "name" => module.name(),
        "description" => module.description(),
        "input_schema" => module.input_schema()
      }
    end)
  end

  @doc """
  Dispatch a tool call by name. Returns whatever the tool's
  `execute/2` returned, or `{:error, {:unknown_tool, name}}` if no
  matching tool is registered.
  """
  @spec dispatch(String.t(), map(), WorgAgent.Tool.ctx()) ::
          {:ok, String.t()} | {:error, term}
  def dispatch(name, args, ctx) do
    case lookup(name) do
      nil -> {:error, {:unknown_tool, name}}
      module -> module.execute(args, ctx)
    end
  end
end
