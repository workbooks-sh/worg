defmodule WorgAgent.Llm.Response do
  @moduledoc """
  Parsed LLM response. The agent loop reads this to decide whether to
  dispatch tools (when `tool_calls` is non-empty) or terminate the
  turn (when `stop_reason` is `:end_turn` or similar).

  Fields:
  - `content` — the assistant's text reply (may be nil if the model
    only returned tool_calls).
  - `tool_calls` — zero or more `ToolCall.t()` to dispatch. Order is
    preserved from the response.
  - `stop_reason` — atom describing why generation halted
    (`:end_turn`, `:tool_calls`, `:max_tokens`, `:other`). Mapped from
    OpenAI's `finish_reason` field.
  - `usage` — input/output token counts and (if reported) cache hit
    info, as a plain map keyed by the strings the provider returns.
  - `raw` — the full decoded JSON body, for cases the loop needs
    something we didn't surface as a struct field.
  """

  alias WorgAgent.Llm.ToolCall

  @enforce_keys [:tool_calls, :stop_reason, :usage, :raw]
  defstruct content: nil, tool_calls: [], stop_reason: :other, usage: %{}, raw: %{}

  @type stop_reason :: :end_turn | :tool_calls | :max_tokens | :other

  @type t :: %__MODULE__{
          content: String.t() | nil,
          tool_calls: [ToolCall.t()],
          stop_reason: stop_reason(),
          usage: map(),
          raw: map()
        }

  @doc """
  Parse the OpenAI-shape response body into a `%Response{}`.

  Returns `{:ok, response}` on success or `{:error, reason}` if the
  body doesn't have the expected shape (missing choices, missing
  message, malformed tool_calls). Other shapes (errors from the
  provider) should be detected by the caller before reaching this
  function.
  """
  @spec from_wire(map) :: {:ok, t()} | {:error, term}
  def from_wire(%{"choices" => [choice | _]} = body) do
    message = Map.get(choice, "message", %{})
    content = Map.get(message, "content")
    finish = Map.get(choice, "finish_reason")

    with {:ok, tool_calls} <- parse_tool_calls(Map.get(message, "tool_calls", [])) do
      {:ok,
       %__MODULE__{
         content: content,
         tool_calls: tool_calls,
         stop_reason: decode_finish_reason(finish),
         usage: Map.get(body, "usage", %{}),
         raw: body
       }}
    end
  end

  def from_wire(body), do: {:error, {:no_choices, body}}

  defp parse_tool_calls(nil), do: {:ok, []}

  defp parse_tool_calls(list) when is_list(list) do
    list
    |> Enum.reduce_while({:ok, []}, fn raw, {:ok, acc} ->
      case ToolCall.from_wire(raw) do
        {:ok, call} -> {:cont, {:ok, [call | acc]}}
        {:error, _} = err -> {:halt, err}
      end
    end)
    |> case do
      {:ok, reversed} -> {:ok, Enum.reverse(reversed)}
      other -> other
    end
  end

  defp decode_finish_reason("stop"), do: :end_turn
  defp decode_finish_reason("tool_calls"), do: :tool_calls
  defp decode_finish_reason("length"), do: :max_tokens
  defp decode_finish_reason(_), do: :other
end
