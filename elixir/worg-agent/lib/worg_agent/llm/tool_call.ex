defmodule WorgAgent.Llm.ToolCall do
  @moduledoc """
  A single tool invocation extracted from an LLM response.

  The LLM emits zero or more tool calls per assistant turn; the agent
  loop dispatches each to `WorgAgent.ToolRegistry.dispatch/3` and feeds
  results back into the conversation as `tool` role messages.

  `id` is the LLM-assigned call identifier. When responding with the
  tool result, the corresponding tool-role message must reference the
  same id via `tool_call_id`.
  """

  @enforce_keys [:id, :name, :arguments]
  defstruct [:id, :name, :arguments]

  @type t :: %__MODULE__{
          id: String.t(),
          name: String.t(),
          arguments: map()
        }

  @doc """
  Decode the OpenAI-shape tool_call object from a parsed response.

      %{
        "id" => "call_xyz",
        "type" => "function",
        "function" => %{
          "name" => "bash",
          "arguments" => "{\\"command\\": \\"ls\\"}"
        }
      }

  Arguments are a JSON-encoded string on the wire; we decode them
  eagerly to a map so callers don't need to remember.
  """
  @spec from_wire(map) :: {:ok, t()} | {:error, term}
  def from_wire(%{"id" => id, "function" => %{"name" => name, "arguments" => args_str}})
      when is_binary(id) and is_binary(name) and is_binary(args_str) do
    case Jason.decode(args_str) do
      {:ok, args} when is_map(args) -> {:ok, %__MODULE__{id: id, name: name, arguments: args}}
      {:ok, _other} -> {:error, {:arguments_not_object, args_str}}
      {:error, e} -> {:error, {:invalid_arguments_json, e}}
    end
  end

  def from_wire(other), do: {:error, {:invalid_tool_call_shape, other}}
end
