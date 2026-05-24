defmodule WorgAgent.Tool do
  @moduledoc """
  Behaviour for tools an agent can invoke. Tools are the leaves of the
  agent loop — the LLM emits tool-use calls, the runtime dispatches
  them through this behaviour, and results flow back into the
  conversation.

  Each implementation lives under `lib/worg_agent/tools/<name>.ex`.
  The default set (Bash, Read, Write, LuaEval) ships in this package;
  consumers register additional tools by extending
  `config :worg_agent, :tools, [...]` in their `config/config.exs`.

  ## Behaviour contract

  - `name/0` — canonical tool name. Matches the value the LLM uses in
    tool-use calls. Conventionally lowercase, underscore-separated.
  - `description/0` — single-paragraph description fed to the LLM as
    part of the tool-use schema. Tell the model when to use this tool
    and what the inputs/outputs mean.
  - `input_schema/0` — JSON Schema (as an Elixir map) describing the
    tool's arguments. Translates 1:1 to the provider's tool-use schema
    format; the LLM client converts this to OpenAI / Anthropic shape.
  - `execute/2` — run the tool with parsed args and a context. Returns
    `{:ok, String.t()}` on success (the string is fed back into the
    conversation as the tool result) or `{:error, term}` on failure.

  ## Context

  The `ctx` argument passed to `execute/2` carries cross-cutting
  state that tools can read but should not mutate:

  - `:working_dir` (String.t()) — directory path-style tools should
    resolve relative paths against.
  - `:trust_level` (`:sandboxed | :full`) — gate for tools that need
    elevated trust. Tools that aren't safe under `:sandboxed` should
    refuse execution and return `{:error, {:trust, "..."}}`.
  - `:task_id` (String.t() | nil) — the task this tool call is part
    of, for audit/logging purposes.

  Tools MAY ignore fields they don't need. Tools MUST NOT add fields
  to ctx (it's read-only).
  """

  @type ctx :: %{
          optional(:working_dir) => String.t(),
          optional(:trust_level) => :sandboxed | :full,
          optional(:task_id) => String.t() | nil
        }

  @doc "Canonical name. Matches the LLM tool-use name field."
  @callback name() :: String.t()

  @doc "One-paragraph description fed to the LLM."
  @callback description() :: String.t()

  @doc """
  JSON Schema for the tool's input parameters.
  Returned as an Elixir map matching the JSON Schema spec.
  """
  @callback input_schema() :: map()

  @typedoc """
  Anthropic-shaped content block for tool results. The LLM client
  translates these to OpenAI image_url blocks for OpenAI-compat
  providers (Gemini / Qwen / Kimi / GPT through OpenRouter). See
  `WorgAgent.Llm` for the transform details. wb-t274.
  """
  @type content_block ::
          %{required(String.t()) => term}

  @typedoc """
  Tool execute/2 return value. Most tools return a plain string;
  tools that emit image content (`frame_judge`, `video_judge`,
  `wavelet_shot_still`) return a list of Anthropic-shaped content
  blocks:

      [
        %{"type" => "text", "text" => "verdict json"},
        %{"type" => "image", "source" => %{
          "type" => "base64",
          "media_type" => "image/png",
          "data" => "iVBOR..."
        }}
      ]

  String returns are passed through unchanged. List returns are
  normalized by the LLM client before reaching the wire. wb-t274.
  """
  @type result :: String.t() | [content_block()]

  @doc """
  Run the tool. Returns `{:ok, result}` (string OR a list of
  Anthropic-shaped content blocks; see `t:result/0`) or
  `{:error, reason}`.
  """
  @callback execute(args :: map(), ctx :: ctx()) :: {:ok, result()} | {:error, term}
end
