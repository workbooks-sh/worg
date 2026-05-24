defmodule WorgAgent.Tools.VideoJudge do
  @moduledoc """
  Score a full MP4 against a rubric using the configured video VLM.
  Samples 4 evenly-spaced timestamps (avoiding the first/last 5% of
  the clip to skip transition artifacts) and delegates to the same
  pipeline `FrameJudge` uses. Returns the VLM's scorecard text plus
  the sampled frames as image content blocks.

  Default rubric prompt is the 8-dimension wavelet rubric (matches
  the existing `rubric.passes` eval check kind). Override via the
  `:prompt` argument when scoring against a different schema.

  wb-lw3z.
  """

  @behaviour WorgAgent.Tool

  alias WorgAgent.Tools.VideoHelpers

  # 8-dim rubric matches the existing wavelet eval kind. Inline'd
  # rather than templated — the agent can override per-call via the
  # `:prompt` arg if it needs something tighter.
  @default_rubric """
  Score the attached video frames against the following 8-dimension
  rubric. For each dimension, return a JSON object with `score`
  (0-3) and `reason` (one sentence). At the end, return `overall`:
  PASS if every dimension scores ≥ 2 AND the sum is ≥ 0.75 × 24,
  otherwise FAIL.

  Dimensions:
  1. character_consistency — same subject visible across frames
  2. product_visible — product appears and is recognizable
  3. brand_register — visual register matches brand identity
  4. text_overlays_clean — any on-screen text is readable, not garbled
  5. composition — framing reads well at 9:16, no awkward letterbox
  6. motion_quality — real motion, no static stills, no broken physics
  7. final_artifact — no glitches, frozen frames, broken chroma
  8. brief_adherence — frames match what the brief described

  Return strictly valid JSON. No prose outside the JSON.
  """

  @impl true
  def name, do: "video_judge"

  @impl true
  def description do
    """
    Run the configured video VLM over a full MP4. Samples 4 evenly
    spaced frames (skipping the first/last 5% to avoid transition
    artifacts) and asks the VLM to score against a rubric. Returns
    JSON scorecard text plus the sampled frames as image content
    blocks.

    Use as the FINAL gate before declaring an MP4 shippable. One
    call per spot. The 8-dimension default rubric matches wavelet's
    existing `rubric.passes` eval kind; override the `prompt` arg
    to score against a different schema.

    Params:
    - mp4_path: filesystem path to the MP4.
    - prompt: optional override of the default 8-dim rubric.
    """
  end

  @impl true
  def input_schema do
    %{
      "type" => "object",
      "properties" => %{
        "mp4_path" => %{
          "type" => "string",
          "description" => "Filesystem path to the MP4 to score."
        },
        "prompt" => %{
          "type" => "string",
          "description" =>
            "Optional rubric/prompt for the VLM. If omitted, the default 8-dim wavelet rubric is used."
        }
      },
      "required" => ["mp4_path"]
    }
  end

  @impl true
  def execute(%{"mp4_path" => mp4_path} = args, _ctx) when is_binary(mp4_path) do
    prompt = Map.get(args, "prompt", @default_rubric)

    with :ok <- check_mp4(mp4_path),
         {:ok, duration} <- probe_duration(mp4_path),
         timestamps <- sample_timestamps(duration),
         {:ok, frames} <- extract_all(mp4_path, timestamps),
         {:ok, verdict} <- VideoHelpers.judge_frames(prompt, frames) do
      verdict_block = %{"type" => "text", "text" => verdict}
      image_blocks = Enum.map(frames, fn {b64, mt} -> VideoHelpers.image_block(b64, mt) end)
      {:ok, [verdict_block | image_blocks]}
    end
  end

  def execute(_args, _ctx) do
    {:error, "video_judge: missing arg. Required: mp4_path (string). Optional: prompt (string)."}
  end

  defp check_mp4(path) do
    cond do
      not File.exists?(path) -> {:error, "mp4_path not found: #{path}"}
      not File.regular?(path) -> {:error, "mp4_path is not a regular file: #{path}"}
      true -> :ok
    end
  end

  # ffprobe duration in seconds. Falls back to a wide error if ffprobe
  # isn't on PATH or fails.
  defp probe_duration(mp4_path) do
    args = ["-v", "error", "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1:nokey=1", mp4_path]

    case System.cmd("ffprobe", args, stderr_to_stdout: true) do
      {output, 0} ->
        case Float.parse(String.trim(output)) do
          {dur, _} when dur > 0.0 -> {:ok, dur}
          _ -> {:error, {:bad_duration_parse, output}}
        end

      {output, code} ->
        {:error, {:ffprobe, code, output}}
    end
  end

  # 4 evenly-spaced samples avoiding the first/last 5% of the clip.
  # For a 25-second clip: 1.25, 8.33, 15.42, 22.5 (or thereabouts).
  defp sample_timestamps(duration) when is_number(duration) and duration > 0 do
    lead = duration * 0.05
    tail = duration * 0.95
    span = tail - lead
    Enum.map(0..3, fn i -> lead + span * i / 3.0 end)
  end

  defp extract_all(mp4_path, timestamps) do
    Enum.reduce_while(timestamps, {:ok, []}, fn ts, {:ok, acc} ->
      case VideoHelpers.extract_frame(mp4_path, ts) do
        {:ok, b64, mt} -> {:cont, {:ok, [{b64, mt} | acc]}}
        {:error, _} = err -> {:halt, err}
      end
    end)
    |> case do
      {:ok, frames} -> {:ok, Enum.reverse(frames)}
      err -> err
    end
  end
end
