defmodule WorgAgent.Tools.FrameJudge do
  @moduledoc """
  Extract frames from an MP4 at specified timestamps and run a video-
  capable VLM over them to score them against a prompt-supplied rubric
  or question. Returns the VLM's verdict text PLUS the frames as
  Anthropic-shaped image content blocks — the agent sees both the
  judgment and the raw frames (so it can sanity-check the verdict).

  Pairs with `WorgAgent.Tools.VideoJudge`, which samples 4
  evenly-spaced timestamps and delegates here. wb-lw3z.
  """

  @behaviour WorgAgent.Tool

  alias WorgAgent.Tools.VideoHelpers

  @impl true
  def name, do: "frame_judge"

  @impl true
  def description do
    """
    Extract frames from an MP4 at the given timestamps (seconds) and
    judge them against a prompt question/rubric using the configured
    video-capable VLM. Returns the VLM's verdict text plus each frame
    as image content so the calling agent sees both.

    Use after rendering a clip or full commercial to validate visual
    quality before declaring DONE. Cheaper than re-rolling: one
    frame_judge call costs ~one VLM image-token bill (~$0.005-0.02
    per frame on Gemini 2.5 Pro at the time of writing).

    Params:
    - mp4_path: filesystem path to the MP4 (must be readable, ffmpeg
      must be on PATH).
    - timestamps_sec: list of timestamps in seconds (floats accepted).
    - prompt: question/rubric for the VLM. Be specific — e.g.,
      "Is the woman in this frame the same person as the reference
      portrait? Score 0-3 with one-sentence reasoning."

    Returns Anthropic-shaped content blocks: a single text block with
    the VLM verdict, followed by one image block per extracted frame.
    The LLM client (wb-t274) re-encodes images to OpenAI image_url
    for non-Anthropic providers automatically.
    """
  end

  @impl true
  def input_schema do
    %{
      "type" => "object",
      "properties" => %{
        "mp4_path" => %{
          "type" => "string",
          "description" => "Filesystem path to the MP4 to inspect."
        },
        "timestamps_sec" => %{
          "type" => "array",
          "items" => %{"type" => "number"},
          "description" => "Seconds into the video to extract frames from. e.g., [1.5, 6.0, 11.0, 16.0]."
        },
        "prompt" => %{
          "type" => "string",
          "description" => "What to ask the VLM about the frames. Be specific."
        }
      },
      "required" => ["mp4_path", "timestamps_sec", "prompt"]
    }
  end

  @impl true
  def execute(
        %{"mp4_path" => mp4_path, "timestamps_sec" => timestamps, "prompt" => prompt},
        _ctx
      )
      when is_binary(mp4_path) and is_list(timestamps) and is_binary(prompt) do
    with :ok <- check_mp4(mp4_path),
         :ok <- check_timestamps(timestamps),
         {:ok, frames} <- extract_all(mp4_path, timestamps),
         {:ok, verdict} <- VideoHelpers.judge_frames(prompt, frames) do
      verdict_block = %{"type" => "text", "text" => verdict}
      image_blocks = Enum.map(frames, fn {b64, mt} -> VideoHelpers.image_block(b64, mt) end)
      {:ok, [verdict_block | image_blocks]}
    end
  end

  def execute(_args, _ctx) do
    {:error,
     "frame_judge: missing or wrong-typed arg. Required: mp4_path (string), timestamps_sec (array of numbers), prompt (string)."}
  end

  defp check_mp4(path) do
    cond do
      not File.exists?(path) -> {:error, "mp4_path not found: #{path}"}
      not File.regular?(path) -> {:error, "mp4_path is not a regular file: #{path}"}
      true -> :ok
    end
  end

  defp check_timestamps([]), do: {:error, "timestamps_sec must contain at least one timestamp"}

  defp check_timestamps(ts) when is_list(ts) do
    case Enum.find(ts, &(not is_number(&1))) do
      nil -> :ok
      bad -> {:error, "timestamps_sec contains non-number: #{inspect(bad)}"}
    end
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
