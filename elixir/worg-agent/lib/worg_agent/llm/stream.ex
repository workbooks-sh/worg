defmodule WorgAgent.Llm.Stream do
  @moduledoc """
  SSE (Server-Sent Events) frame parsing + delta aggregation for
  OpenRouter / OpenAI-compatible chat-completion streams.

  ## Wire format

  OpenRouter emits one frame per token-ish event:

      data: {"choices":[{"delta":{"content":"hello"},"finish_reason":null}]}

      data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_x",
                                                 "function":{"name":"bash",
                                                             "arguments":""}}]}}]}

      data: {"choices":[{"delta":{"tool_calls":[{"index":0,
                                                 "function":{"arguments":"{\\""}}]}}]}

      ... more delta frames ...

      data: {"choices":[{"delta":{},"finish_reason":"stop"}],
             "usage":{"prompt_tokens":10,"completion_tokens":5}}

      data: [DONE]

  Tool-call fragments arrive piecewise across frames — `function.arguments`
  is JSON-string-fragmented, NOT individually parseable until concatenated.
  The aggregator merges by `index`.

  ## Public API

      iex> {events, _rest} = Stream.parse_frames("data: {\\"x\\":1}\\n\\n")
      iex> events
      [{:event, %{"x" => 1}}]

  Use `Stream.new_acc/0` + `Stream.apply_event/2` to accumulate frames
  into a final `%WorgAgent.Llm.Response{}` shape.
  """

  alias WorgAgent.Llm.{Response, ToolCall}

  @type acc :: %{
          content_parts: [binary],
          tool_calls: %{integer => map},
          stop_reason: String.t() | nil,
          usage: map | nil
        }

  @doc "Initial accumulator. Call once per stream."
  @spec new_acc() :: acc
  def new_acc do
    %{content_parts: [], tool_calls: %{}, stop_reason: nil, usage: nil}
  end

  @doc """
  Split a chunk of raw bytes into a list of SSE events and a remaining
  buffer (the trailing partial frame, if any).

  Returns `{events, remaining_buffer}` where `events` is a list of
    * `{:event, decoded_json_map}` — a `data: { ... json ... }` frame
    * `:done` — the terminal `data: [DONE]` sentinel
    * `{:invalid, raw_data_line}` — a `data: ` line that didn't decode

  Frames are separated by `\\n\\n` (or `\\r\\n\\r\\n`). Lines that
  don't start with `data:` (e.g., comments starting with `:`) are
  ignored.
  """
  @spec parse_frames(binary, binary) :: {[term], binary}
  def parse_frames(chunk, prior_buffer \\ "") when is_binary(chunk) do
    buf = prior_buffer <> chunk

    {complete, remaining} = split_complete_frames(buf)

    events =
      complete
      |> Enum.flat_map(&decode_frame/1)

    {events, remaining}
  end

  @doc """
  Apply one stream event to the accumulator. Acc-first for pipe-friendly
  use; passes through to `Enum.reduce` callers via a small wrapper.
  Ignores `:done` (the caller's signal to stop) and `{:invalid, _}`
  (silently dropped — unparseable frames don't sink the run).
  """
  @spec apply_event(acc, term) :: acc
  def apply_event(acc, {:event, %{"choices" => [choice | _]} = frame}) do
    acc =
      case choice do
        %{"finish_reason" => fr} when is_binary(fr) -> %{acc | stop_reason: fr}
        _ -> acc
      end

    acc =
      case choice["delta"]["content"] do
        nil -> acc
        "" -> acc
        text when is_binary(text) -> %{acc | content_parts: [text | acc.content_parts]}
        _ -> acc
      end

    acc =
      case choice["delta"]["tool_calls"] do
        nil -> acc
        list when is_list(list) -> merge_tool_call_deltas(acc, list)
        _ -> acc
      end

    case frame["usage"] do
      usage when is_map(usage) -> %{acc | usage: usage}
      _ -> acc
    end
  end

  def apply_event(acc, {:event, %{"usage" => usage}}) when is_map(usage) do
    %{acc | usage: usage}
  end

  def apply_event(acc, _other), do: acc

  @doc """
  Finalize the accumulator into a `%Response{}`. Concatenates content
  parts, sorts tool_calls by index, maps `stop_reason` to the same
  atoms `Response.from_wire/1` uses.
  """
  @spec finalize(acc) :: %Response{}
  def finalize(%{} = acc) do
    content =
      acc.content_parts
      |> Enum.reverse()
      |> IO.iodata_to_binary()

    tool_calls =
      acc.tool_calls
      |> Map.to_list()
      |> Enum.sort_by(fn {idx, _} -> idx end)
      |> Enum.map(fn {_, tc} ->
        %ToolCall{
          id: tc["id"] || "",
          name: tc["name"] || "",
          arguments: parse_arguments(tc["arguments"] || "")
        }
      end)

    %Response{
      content: nilify_empty(content),
      tool_calls: tool_calls,
      stop_reason: normalize_stop_reason(acc.stop_reason, tool_calls),
      usage: acc.usage || %{},
      raw: %{}
    }
  end

  # ── internals ─────────────────────────────────────────────────────

  # Split `buf` at frame boundaries (\n\n or \r\n\r\n). Returns
  # {complete_frames_as_strings, trailing_partial_frame}.
  defp split_complete_frames(buf) do
    do_split(buf, [], "")
  end

  defp do_split("", complete, partial), do: {Enum.reverse(complete), partial}

  defp do_split(rest, complete, partial) do
    case find_separator(rest) do
      {idx, sep_len} ->
        frame = binary_part(rest, 0, idx)
        next = binary_part(rest, idx + sep_len, byte_size(rest) - idx - sep_len)
        do_split(next, [partial <> frame | complete], "")

      :none ->
        {Enum.reverse(complete), partial <> rest}
    end
  end

  defp find_separator(s) do
    case :binary.match(s, ["\r\n\r\n", "\n\n"]) do
      :nomatch -> :none
      {idx, len} -> {idx, len}
    end
  end

  # A single SSE frame can contain one or more `data:` lines; we treat
  # each one as an independent event. (OpenRouter only ever emits one
  # data line per frame, but the spec allows multi-line.)
  defp decode_frame(frame) do
    frame
    |> String.split(["\r\n", "\n"], trim: true)
    |> Enum.flat_map(&decode_line/1)
  end

  defp decode_line("data: " <> data), do: decode_data(data)
  defp decode_line("data:" <> data), do: decode_data(String.trim_leading(data))
  defp decode_line(_), do: []

  defp decode_data("[DONE]"), do: [:done]

  defp decode_data(json) do
    case Jason.decode(json) do
      {:ok, decoded} -> [{:event, decoded}]
      {:error, _} -> [{:invalid, json}]
    end
  end

  # Per OpenAI tool-call streaming: each frame's tool_calls list
  # is keyed by `index`. id + function.name appear ONCE at the start
  # of a tool call; function.arguments is a string fragment per
  # frame that must be appended to the prior value at the same index.
  defp merge_tool_call_deltas(acc, deltas) do
    new_tool_calls =
      Enum.reduce(deltas, acc.tool_calls, fn delta, by_index ->
        idx = delta["index"] || 0
        existing = Map.get(by_index, idx, %{"arguments" => ""})
        merged = merge_one_tool_call(existing, delta)
        Map.put(by_index, idx, merged)
      end)

    %{acc | tool_calls: new_tool_calls}
  end

  defp merge_one_tool_call(existing, delta) do
    existing =
      case delta["id"] do
        nil -> existing
        id -> Map.put(existing, "id", id)
      end

    fn_delta = delta["function"] || %{}

    existing =
      case fn_delta["name"] do
        nil -> existing
        name -> Map.put(existing, "name", name)
      end

    case fn_delta["arguments"] do
      nil ->
        existing

      fragment when is_binary(fragment) ->
        prior = existing["arguments"] || ""
        Map.put(existing, "arguments", prior <> fragment)

      _ ->
        existing
    end
  end

  defp parse_arguments(""), do: %{}

  defp parse_arguments(args) when is_binary(args) do
    case Jason.decode(args) do
      {:ok, decoded} when is_map(decoded) -> decoded
      _ -> %{}
    end
  end

  defp nilify_empty(""), do: nil
  defp nilify_empty(s), do: s

  # If the wire says "tool_calls" we trust it; otherwise infer from the
  # presence of accumulated tool calls. Map known strings to the atoms
  # the existing Response uses; fall through to :other.
  defp normalize_stop_reason(nil, []), do: :end_turn
  defp normalize_stop_reason(nil, _tool_calls), do: :tool_calls
  defp normalize_stop_reason("stop", _), do: :end_turn
  defp normalize_stop_reason("length", _), do: :max_tokens
  defp normalize_stop_reason("tool_calls", _), do: :tool_calls
  defp normalize_stop_reason("function_call", _), do: :tool_calls
  defp normalize_stop_reason("content_filter", _), do: :content_filter
  defp normalize_stop_reason(_, _), do: :other
end
