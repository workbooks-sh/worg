defmodule WorgAgent.Tools.WaveletCharacterDefine do
  @moduledoc """
  Typed wrapper for `wavelet character define`. Registers a named
  character with 1..N reference images; the storyboard planner
  auto-discovers these refs and routes matching CHARACTER cues
  through fal-veo3-ref. wb-cx08.
  """

  use WorgAgent.Tools.ShellWrapper,
    name: "wavelet_character_define",
    binary: "wavelet",
    argv_prefix: ["character", "define"],
    description: """
    Register a named character with reference images. Writes a
    clip-HTML at `<workdir>/refs/character/<slug>.clip.html` that
    the storyboard planner reads at plan time. Multiple
    `character_type` values (full-body / hands / product-hands)
    coexist for the same name — the planner picks the right one
    per shot.

    The flag is `--character-type`, NOT `--type` (clap rejects).
    """,
    input_schema: %{
      "type" => "object",
      "properties" => %{
        "name" => %{
          "type" => "string",
          "description" => "Canonical character name. Matches Fountain CHARACTER cues (uppercase-normalized)."
        },
        "reference" => %{
          "type" => "array",
          "items" => %{"type" => "string"},
          "description" =>
            "Reference image paths or HTTPS URLs. Pass 1-3 (Gemini cap)."
        },
        "character_type" => %{
          "type" => "string",
          "description" => "full-body | hands | product-hands. Default full-body.",
          "enum" => ["full-body", "hands", "product-hands"]
        },
        "workdir" => %{
          "type" => "string",
          "description" => "Workdir to write the clip-HTML into. Default cwd."
        }
      },
      "required" => ["name", "reference"]
    },
    arg_map: [
      {"name", :positional},
      {"reference", {"--reference", :repeat}},
      {"character_type", "--character-type"},
      {"workdir", "--workdir"}
    ]
end
