defmodule WorgAgent.Tools.WaveletLint do
  @moduledoc """
  Typed wrapper for `wavelet lint`. Pre-render lint runs structural
  rules against a composition HTML; post-render lint with `--mp4`
  also runs frame-level checks (halo-contrast, baked-text-OCR,
  static-frame detection).
  """

  use WorgAgent.Tools.ShellWrapper,
    name: "wavelet_lint",
    binary: "wavelet",
    argv_prefix: ["lint"],
    description: """
    Run wavelet lint on a composition HTML. Pre-render mode (`path`
    only) runs layout-walk rules — safe-zone, hallucinated-attrs,
    halo-contrast on text scenes. Post-render mode (`path` + `mp4`)
    additionally runs frame-level rules. Use `platform` to scope
    rules to a delivery target (instagram_reels, tiktok, youtube_shorts).

    Returns wavelet's stdout/stderr with an `exit=<n>` marker.
    Exit 0 = clean; exit 3 = at least one rule failed; exit 2 = arg
    parse error.
    """,
    input_schema: %{
      "type" => "object",
      "properties" => %{
        "path" => %{"type" => "string", "description" => "Path to the composition HTML."},
        "platform" => %{
          "type" => "string",
          "description" =>
            "Optional platform slug to scope rules. Valid: instagram_reels, tiktok, youtube_shorts."
        },
        "mp4" => %{
          "type" => "string",
          "description" => "Optional MP4 path for post-render frame-level checks."
        }
      },
      "required" => ["path"]
    },
    arg_map: [
      {"path", :positional},
      {"platform", "--platform"},
      {"mp4", "--mp4"}
    ]
end
