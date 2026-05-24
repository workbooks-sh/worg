defmodule WorgAgent.Tools.WaveletRender do
  @moduledoc """
  Typed wrapper for `wavelet render`. Renders a composition HTML to
  MP4. HTML is the only accepted input — JSON inputs are rejected
  with exit 3 (data-video-bg is also unsupported; the renderer
  catches hallucinated attrs via wb-a2z2's lint rule).
  """

  use WorgAgent.Tools.ShellWrapper,
    name: "wavelet_render",
    binary: "wavelet",
    argv_prefix: ["render"],
    description: """
    Render a wavelet composition HTML to an MP4. The composition
    must reference scene HTMLs that each contain an inline `<video
    src="../shots/shot-N.mp4">` (NOT data-video-bg, which doesn't
    render — wb-a2z2).

    Outputs `out` (default `commercial.mp4` if omitted). Sidecar
    WAV is auto-muxed when audio cues are present. Returns wavelet
    stdout/stderr with an `exit=<n>` marker.
    """,
    input_schema: %{
      "type" => "object",
      "properties" => %{
        "comp" => %{"type" => "string", "description" => "Path to commercial.html."},
        "out" => %{"type" => "string", "description" => "Output MP4 path. Default commercial.mp4."},
        "no_audio" => %{
          "type" => "boolean",
          "description" => "Skip audio muxing. Default false."
        },
        "aspects" => %{
          "type" => "string",
          "description" =>
            "Optional aspect ratios to emit. Comma-separated. e.g., 9:16,1:1."
        },
        "frame_budget_secs" => %{
          "type" => "number",
          "description" => "Optional per-frame budget; render aborts if exceeded."
        }
      },
      "required" => ["comp"]
    },
    arg_map: [
      {"comp", :positional},
      {"out", "-o"},
      {"no_audio", {"--no-audio", :boolean}},
      {"aspects", "--aspects"},
      {"frame_budget_secs", "--frame-budget-secs"}
    ]
end
