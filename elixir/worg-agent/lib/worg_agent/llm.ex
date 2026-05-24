defmodule WorgAgent.Llm do
  @moduledoc """
  OpenRouter (OpenAI-compat) HTTP client for the agent loop. Builds
  the request from a message history + tool catalog, calls the
  provider, parses the response into a `%WorgAgent.Llm.Response{}`.

  ## Defaults

  - Model: `xiaomi/mimo-v2.5-pro` (per CLAUDE.md "Default model" rule).
  - Endpoint: `https://openrouter.ai/api/v1/chat/completions`.
  - API key: read from `OPENROUTER_API_KEY` env at call time. This is
    a genuine cross-process secret crossing into the LLM provider —
    Varlock or equivalent should manage it in production. The client
    never logs the key.

  ## Prompt caching

  When `opts[:cache]` is `true`, the client attaches Anthropic-style
  `cache_control: {type: "ephemeral"}` breakpoints to:
  - The system message (stable across the entire run).
  - The tools array (also stable).

  These fields are read by Anthropic-backed routes through OpenRouter
  and ignored silently by other providers. xiaomi/mimo (our default)
  doesn't currently honor cache_control, but adding the field costs
  nothing and lets us A/B against Anthropic without code changes.

  ## Image-bearing tool results (wb-t274)

  Tools that return image content (`frame_judge`, `video_judge`,
  `wavelet_shot_still`) emit results as a list of Anthropic-shaped
  content blocks instead of a plain string:

      %{"role" => "tool", "tool_call_id" => "...",
        "content" => [
          %{"type" => "text", "text" => "verdict json"},
          %{"type" => "image", "source" => %{
            "type" => "base64",
            "media_type" => "image/png",
            "data" => "iVBOR..."
          }}
        ]}

  Anthropic accepts image content inside tool_result blocks directly.
  OpenAI-compatible endpoints (Gemini / Qwen / Kimi / GPT through
  OpenRouter) do NOT — image content must arrive as `image_url` parts
  inside a separate user message. The client transforms image-bearing
  tool results before sending: the tool message keeps the text-only
  summary, and a follow-up synthetic user message carries the images
  as `image_url` blocks.

  This happens transparently — tools just return Anthropic-shaped
  content blocks, the client handles the per-provider shape.

  ## Testing

  All tests inject a Req test adapter via `opts[:req_options]`. No
  real HTTP calls happen in CI. Real network calls only happen at
  production-time invocation by the agent loop.
  """

  alias WorgAgent.Llm.{Response, Stream}
  alias WorgAgent.ToolRegistry

  @default_endpoint "https://openrouter.ai/api/v1/chat/completions"
  @default_model "xiaomi/mimo-v2.5-pro"

  @doc """
  Call the LLM with the given message history and tool catalog.

  ## Required

  - `messages` — list of message maps in OpenAI shape:
        %{"role" => "system" | "user" | "assistant" | "tool",
          "content" => binary,
          # optional, on assistant messages:
          "tool_calls" => [%{...}],
          # optional, on tool messages:
          "tool_call_id" => binary}

  - `tools` — list of tool descriptors from
    `WorgAgent.ToolRegistry.catalog/0`, or `[]` to disable tool use.

  ## Optional `opts`

  - `:model` — override the default `xiaomi/mimo-v2.5-pro`.
  - `:cache` — `true` to attach Anthropic-style cache_control to the
    system message + tools (silently ignored by other providers).
  - `:endpoint` — override the default OpenRouter URL.
  - `:api_key` — override the OPENROUTER_API_KEY env lookup (used in
    tests with a fake key).
  - `:req_options` — keyword passed verbatim to `Req.new/1`. Used in
    tests to inject `Req.Test` adapters; do not use in production.

  Returns `{:ok, %Response{}}` on success or `{:error, term}` on HTTP
  failures, non-2xx responses, or unparseable bodies.
  """
  @spec call(list(map), list(map), keyword) :: {:ok, Response.t()} | {:error, term}
  def call(messages, tools, opts \\ []) when is_list(messages) and is_list(tools) do
    model = Keyword.get(opts, :model, @default_model)
    endpoint = Keyword.get(opts, :endpoint, @default_endpoint)
    api_key = Keyword.get(opts, :api_key) || System.get_env("OPENROUTER_API_KEY")
    cache? = Keyword.get(opts, :cache, false)
    req_options = Keyword.get(opts, :req_options, [])

    cond do
      api_key in [nil, ""] ->
        {:error, :missing_api_key}

      true ->
        body = build_request(model, messages, tools, cache?)
        req = build_req(endpoint, api_key, req_options)

        try do
          case Req.post(req, json: body) do
            {:ok, %Req.Response{status: status, body: resp_body}} when status in 200..299 ->
              Response.from_wire(resp_body)

            {:ok, %Req.Response{status: status, body: resp_body}} ->
              {:error, {:http, status, resp_body}}

            {:error, exception} ->
              {:error, {:transport, exception}}
          end
        rescue
          # Req's plug adapter (used in tests) propagates plug
          # exceptions instead of returning {:error, _}. Real
          # transport errors usually come through as {:error,
          # %Mint.TransportError{}} — but a bug-induced exception
          # path should fail safe rather than crash the caller.
          exception -> {:error, {:transport, exception}}
        end
    end
  end

  @doc """
  Convenience: call with the default tool catalog from `ToolRegistry`.
  """
  @spec call_with_default_tools(list(map), keyword) :: {:ok, Response.t()} | {:error, term}
  def call_with_default_tools(messages, opts \\ []) do
    call(messages, ToolRegistry.catalog() |> wrap_tools_for_openai(), opts)
  end

  @doc """
  Streaming variant of `call/3`. Issues a request with `stream: true`
  and consumes the SSE response, optionally invoking `opts[:on_delta]`
  per chunk for live-UI updates. Returns the SAME `{:ok, %Response{}}`
  shape as `call/3` once the stream terminates — callers can swap in
  `stream/3` without changing downstream handling.

  ## Per-delta callback

  When `opts[:on_delta]` is a function of arity 1, it is invoked for
  each parsed SSE event:

    * `{:content, fragment}` — a text chunk
    * `{:tool_call, index, tc_delta}` — a partial tool-call (id+name
      arrive in the first delta; subsequent deltas extend
      `function.arguments`)
    * `{:done, %Response{}}` — terminal sentinel with the final
      aggregated response (the same value returned by stream/3)

  ## Telemetry

  Emits `[:worg_agent, :llm, :delta]` events per content/tool_call
  chunk so live subscribers (Phoenix channel, debug printer) can see
  tokens land in real time without owning the SSE buffer.

      [:worg_agent, :llm, :delta]
        :start  → %{session_id, kind: :content | :tool_call}
        :stop   → %{session_id, kind, byte_count, index: nil | integer}

  `session_id` is forwarded from `opts[:session_id]` to match the
  metadata shape of the `:llm, :turn` span.

  Errors return `{:error, reason}` in the same shape as `call/3`.
  """
  @spec stream(list(map), list(map), keyword) :: {:ok, Response.t()} | {:error, term}
  def stream(messages, tools, opts \\ []) when is_list(messages) and is_list(tools) do
    model = Keyword.get(opts, :model, @default_model)
    endpoint = Keyword.get(opts, :endpoint, @default_endpoint)
    api_key = Keyword.get(opts, :api_key) || System.get_env("OPENROUTER_API_KEY")
    cache? = Keyword.get(opts, :cache, false)
    req_options = Keyword.get(opts, :req_options, [])
    on_delta = Keyword.get(opts, :on_delta)
    session_id = Keyword.get(opts, :session_id)

    cond do
      api_key in [nil, ""] ->
        {:error, :missing_api_key}

      true ->
        body =
          build_request(model, messages, tools, cache?)
          |> Map.put("stream", true)
          |> Map.put("stream_options", %{"include_usage" => true})

        req = build_req(endpoint, api_key, req_options)
        consume_stream(req, body, on_delta, session_id)
    end
  end

  defp consume_stream(req, body, on_delta, session_id) do
    state0 = %{
      acc: Stream.new_acc(),
      buffer: "",
      on_delta: on_delta,
      session_id: session_id,
      done?: false
    }

    try do
      case Req.post(req, json: body, into: stream_collector(state0)) do
        {:ok, %Req.Response{status: status, private: %{worg_stream: final_state}}}
        when status in 200..299 ->
          response = Stream.finalize(final_state.acc)
          if final_state.on_delta, do: final_state.on_delta.({:done, response})
          {:ok, response}

        {:ok, %Req.Response{status: status, body: body}} ->
          {:error, {:http, status, body}}

        {:error, exception} ->
          {:error, {:transport, exception}}
      end
    rescue
      exception -> {:error, {:transport, exception}}
    end
  end

  # Per-chunk SSE collector. Req invokes this for each {:data, chunk}
  # tuple. State is threaded via the response struct's :private map
  # since the callback can only return {acc, {req, resp}}-style data
  # via the {:cont, ...} pattern.
  defp stream_collector(initial) do
    fn {:data, chunk}, {req, resp} ->
      prior = (resp.private[:worg_stream] || initial)
      {events, new_buffer} = Stream.parse_frames(chunk, prior.buffer)

      new_state =
        Enum.reduce(events, %{prior | buffer: new_buffer}, fn event, st ->
          handle_event(event, st)
        end)

      {:cont, {req, %{resp | private: Map.put(resp.private, :worg_stream, new_state)}}}
    end
  end

  defp handle_event(:done, state), do: %{state | done?: true}

  defp handle_event({:invalid, _raw}, state), do: state

  defp handle_event({:event, frame} = event, state) do
    new_acc = Stream.apply_event(state.acc, event)

    if state.on_delta || state.session_id do
      emit_delta_events(frame, state)
    end

    %{state | acc: new_acc}
  end

  # Fan out content + tool_call deltas. Each emits a telemetry event
  # AND (if present) the user callback. Telemetry stays span-shaped
  # so consumers can attach the same way as :llm, :turn.
  defp emit_delta_events(%{"choices" => [choice | _]}, state) do
    case choice["delta"]["content"] do
      text when is_binary(text) and text != "" ->
        meta = %{session_id: state.session_id, kind: :content, byte_count: byte_size(text), index: nil}
        :telemetry.execute([:worg_agent, :llm, :delta], %{system_time: System.system_time()}, meta)
        if state.on_delta, do: state.on_delta.({:content, text})

      _ ->
        :ok
    end

    case choice["delta"]["tool_calls"] do
      list when is_list(list) ->
        Enum.each(list, fn tc ->
          idx = tc["index"] || 0
          frag = get_in(tc, ["function", "arguments"]) || ""
          meta = %{session_id: state.session_id, kind: :tool_call, byte_count: byte_size(frag), index: idx}
          :telemetry.execute([:worg_agent, :llm, :delta], %{system_time: System.system_time()}, meta)
          if state.on_delta, do: state.on_delta.({:tool_call, idx, tc})
        end)

      _ ->
        :ok
    end
  end

  defp emit_delta_events(_, _), do: :ok

  # ── Internal: request building ────────────────────────────────────

  defp build_req(endpoint, api_key, extra_options) do
    base = [
      url: endpoint,
      headers: [
        {"authorization", "Bearer #{api_key}"},
        {"content-type", "application/json"},
        # OpenRouter site/app attribution headers — optional, not
        # secret. Self-identify as worg-agent.
        {"http-referer", "https://github.com/workbooks-sh/worg"},
        {"x-title", "worg-agent"}
      ]
    ]

    Req.new(Keyword.merge(base, extra_options))
  end

  defp build_request(model, messages, tools, cache?) do
    normalized_messages =
      messages
      |> normalize_tool_image_results()
      |> maybe_cache_system(cache?)

    payload = %{
      "model" => model,
      "messages" => normalized_messages,
      "tools" => maybe_cache_tools(tools, cache?)
    }

    # Drop "tools" entirely when the catalog is empty — some
    # providers reject an empty array.
    if tools == [] do
      Map.delete(payload, "tools")
    else
      payload
    end
  end

  # Tool results carrying image content (Anthropic content-block
  # shape: `%{"type" => "image", "source" => %{...}}`) must be split
  # for OpenAI-compat endpoints. The tool message keeps the text-only
  # summary; a synthetic user message immediately after carries the
  # images as `image_url` content parts.
  #
  # Tool messages with plain string content pass through unchanged.
  # Tool messages with list content containing only text blocks are
  # collapsed back to a string for OpenAI's spec (which wants string
  # content on tool messages when no images are involved).
  defp normalize_tool_image_results(messages) do
    Enum.flat_map(messages, &split_tool_message/1)
  end

  defp split_tool_message(%{"role" => "tool", "content" => content} = msg)
       when is_list(content) do
    {image_blocks, text_blocks} =
      Enum.split_with(content, fn block -> block["type"] in ["image", "image_url"] end)

    text_summary =
      text_blocks
      |> Enum.map(&block_to_text/1)
      |> Enum.join("\n")
      |> case do
        "" when image_blocks != [] -> "[image content attached in next message]"
        "" -> ""
        t -> t
      end

    tool_msg = Map.put(msg, "content", text_summary)

    if image_blocks == [] do
      [tool_msg]
    else
      image_url_blocks = Enum.map(image_blocks, &to_image_url_block/1)
      label = label_for_tool_image(Map.get(msg, "tool_call_id"))

      user_msg = %{
        "role" => "user",
        "content" => [%{"type" => "text", "text" => label} | image_url_blocks]
      }

      [tool_msg, user_msg]
    end
  end

  defp split_tool_message(other), do: [other]

  # Anthropic image block → OpenAI image_url block.
  defp to_image_url_block(%{
         "type" => "image",
         "source" => %{"type" => "base64", "media_type" => media_type, "data" => data}
       }) do
    %{"type" => "image_url", "image_url" => %{"url" => "data:#{media_type};base64,#{data}"}}
  end

  defp to_image_url_block(%{
         "type" => "image",
         "source" => %{"type" => "url", "url" => url}
       }) do
    %{"type" => "image_url", "image_url" => %{"url" => url}}
  end

  # Already OpenAI shape — pass through.
  defp to_image_url_block(%{"type" => "image_url"} = block), do: block

  defp block_to_text(%{"type" => "text", "text" => text}) when is_binary(text), do: text
  defp block_to_text(_), do: ""

  defp label_for_tool_image(nil), do: "Image content from tool call:"
  defp label_for_tool_image(""), do: "Image content from tool call:"
  defp label_for_tool_image(id), do: "Image content from tool call #{id}:"

  # ToolRegistry.catalog/0 returns the WORG-internal shape with
  # `name` / `description` / `input_schema`. Convert each to the
  # OpenAI tool descriptor wrapping.
  defp wrap_tools_for_openai(catalog) do
    Enum.map(catalog, fn entry ->
      %{
        "type" => "function",
        "function" => %{
          "name" => entry["name"],
          "description" => entry["description"],
          "parameters" => entry["input_schema"]
        }
      }
    end)
  end

  # Anthropic prompt caching: attach cache_control to a stable
  # breakpoint. The system message is the canonical first breakpoint.
  defp maybe_cache_system(messages, false), do: messages

  defp maybe_cache_system([%{"role" => "system"} = sys | rest], true) do
    cached = Map.update(sys, "content", sys["content"], &cached_content/1)
    [cached | rest]
  end

  defp maybe_cache_system(messages, true), do: messages

  defp maybe_cache_tools(tools, false), do: tools

  defp maybe_cache_tools(tools, true) when is_list(tools) and tools != [] do
    # Mark the last tool with cache_control — Anthropic spec says the
    # breakpoint applies to everything UP TO and INCLUDING that point.
    {init, [last]} = Enum.split(tools, length(tools) - 1)
    init ++ [Map.put(last, "cache_control", %{"type" => "ephemeral"})]
  end

  defp maybe_cache_tools(tools, true), do: tools

  # Wrap a plain string content into the structured content blocks
  # Anthropic expects, attaching cache_control to the text block.
  # OpenAI-compat providers accept either a string OR a list of
  # content blocks; the list form is what carries cache_control.
  defp cached_content(content) when is_binary(content) do
    [%{"type" => "text", "text" => content, "cache_control" => %{"type" => "ephemeral"}}]
  end

  defp cached_content(content) when is_list(content) do
    # Already a list of blocks — mark the last one.
    case Enum.split(content, length(content) - 1) do
      {init, [last]} -> init ++ [Map.put(last, "cache_control", %{"type" => "ephemeral"})]
      _ -> content
    end
  end
end
