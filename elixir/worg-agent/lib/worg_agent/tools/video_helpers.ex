defmodule WorgAgent.Tools.VideoHelpers do
  @moduledoc """
  Shared utilities for the `frame_judge` and `video_judge` tools:
  ffmpeg frame extraction, base64 encoding, and the VLM call shape.

  Both tools delegate to this module so they share the same model
  configuration and the same image-encoding conventions. The video
  model is configured at app-level — the agent doesn't pick it, the
  runtime does (wb-lw3z):

      config :worg_agent, :video_judge_model, "google/gemini-2.5-pro"

  Override per-call via `Application.put_env/3` in tests or per
  runtime profile.
  """

  alias WorgAgent.Llm

  @default_model "google/gemini-2.5-pro"

  @doc """
  Returns the configured video-judge model slug (OpenRouter format,
  e.g. `google/gemini-2.5-pro`). Falls back to `google/gemini-2.5-pro`
  when no config is set.

  Note: the user named Gemini 3.5 / Kimi K2.5 / Qwen3-VL as viable
  alternatives. Swap via `config :worg_agent, :video_judge_model,
  "moonshotai/kimi-k2.5"` (or whichever slug). The frame-extraction
  + image-encoding path is provider-agnostic; only the model slug
  changes.
  """
  @spec configured_model() :: String.t()
  def configured_model do
    Application.get_env(:worg_agent, :video_judge_model, @default_model)
  end

  @doc """
  Extract a JPEG frame from `mp4_path` at `timestamp_sec` to a temp
  file and return its base64-encoded bytes (no `data:` prefix).

  Uses `ffmpeg -ss <ts> -i <mp4> -frames:v 1 -y <out>.jpg` with a
  quality flag tuned for fast, modest-sized JPEGs (q:v 5). On
  ffmpeg failure returns `{:error, {:ffmpeg, exit_code, output}}`.

  The temp file is created via `System.tmp_dir/0` and deleted on
  success — failures leave it for inspection.
  """
  @spec extract_frame(String.t(), number()) ::
          {:ok, String.t(), media_type :: String.t()} | {:error, term}
  def extract_frame(mp4_path, timestamp_sec)
      when is_binary(mp4_path) and is_number(timestamp_sec) do
    tmp_dir = System.tmp_dir!()
    rand = :rand.uniform(1_000_000_000)
    out_path = Path.join(tmp_dir, "wa_frame_#{rand}.jpg")
    ts_str = :erlang.float_to_binary(timestamp_sec / 1, decimals: 3)

    args = ["-y", "-ss", ts_str, "-i", mp4_path, "-frames:v", "1", "-q:v", "5", out_path]

    case System.cmd("ffmpeg", args, stderr_to_stdout: true) do
      {_output, 0} ->
        try do
          case File.read(out_path) do
            {:ok, bytes} ->
              File.rm(out_path)
              {:ok, Base.encode64(bytes), "image/jpeg"}

            {:error, posix} ->
              {:error, {:read_extracted_frame, posix}}
          end
        catch
          kind, reason -> {:error, {kind, reason}}
        end

      {output, code} ->
        {:error, {:ffmpeg, code, output}}
    end
  end

  @doc """
  Build an Anthropic-shaped image block for a base64-encoded JPEG.
  This is the shape tool results emit (per wb-t274); the LLM client
  re-encodes to OpenAI `image_url` automatically.
  """
  @spec image_block(String.t(), String.t()) :: map()
  def image_block(base64_data, media_type) when is_binary(base64_data) and is_binary(media_type) do
    %{
      "type" => "image",
      "source" => %{
        "type" => "base64",
        "media_type" => media_type,
        "data" => base64_data
      }
    }
  end

  @doc """
  Call the configured video-judge VLM with a prompt + a list of
  `{base64_data, media_type}` frame tuples. Returns the VLM's textual
  response on success.

  The frames go as a single user message with content blocks
  (`text` + N `image_url`). This is the OpenAI/OpenRouter vision-
  input shape — accepted by Gemini, Qwen3-VL, Kimi K2.5, Claude (in
  Anthropic-shape mode), etc.

  Test-mode: pass `req_options` (typically `[plug: fake_plug]`) to
  inject a stub Req adapter.
  """
  @spec judge_frames(String.t(), [{String.t(), String.t()}], keyword) ::
          {:ok, String.t()} | {:error, term}
  def judge_frames(prompt, frames, opts \\ []) when is_binary(prompt) and is_list(frames) do
    image_parts =
      Enum.map(frames, fn {b64, mt} ->
        %{"type" => "image_url", "image_url" => %{"url" => "data:#{mt};base64,#{b64}"}}
      end)

    messages = [
      %{
        "role" => "user",
        "content" => [%{"type" => "text", "text" => prompt} | image_parts]
      }
    ]

    # Test-mode hook: tests inject a stub Req plug via the
    # `:video_judge_req_options` app env key; the tool path doesn't
    # take user-passed opts (it's invoked from the agent loop, which
    # doesn't know about the LLM transport). Production reads the
    # real OPENROUTER_API_KEY env var; tests inject {plug: fake} +
    # an explicit api_key.
    env_req_opts = Application.get_env(:worg_agent, :video_judge_req_options, [])
    env_api_key = Application.get_env(:worg_agent, :video_judge_api_key)

    llm_opts =
      [model: configured_model()]
      |> Keyword.merge(if env_req_opts != [], do: [req_options: env_req_opts], else: [])
      |> Keyword.merge(if env_api_key, do: [api_key: env_api_key], else: [])
      |> Keyword.merge(Keyword.take(opts, [:api_key, :endpoint, :req_options]))

    case Llm.call(messages, [], llm_opts) do
      {:ok, %{content: content}} when is_binary(content) and content != "" ->
        {:ok, content}

      {:ok, %{content: nil}} ->
        {:error, :empty_vlm_response}

      {:error, _} = err ->
        err
    end
  end
end
