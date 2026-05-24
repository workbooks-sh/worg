defmodule WorgAgent.Tools.WaveletScreenplayValidate do
  @moduledoc """
  Typed wrapper for `wavelet screenplay validate`. Validates that a
  Fountain screenplay's copy density fits the declared spot duration.
  Exit 0 = fits, exit 3 = over budget (unrecoverable downstream).
  Idempotent on identical content via a sha-cache.
  """

  use WorgAgent.Tools.ShellWrapper,
    name: "wavelet_screenplay_validate",
    binary: "wavelet",
    argv_prefix: ["screenplay", "validate"],
    description: """
    Validate that a Fountain screenplay's copy density fits the
    declared duration. Computes VO time + caption dwell + shot floor
    against the target with a ±10% tolerance band. Use as the gate
    BEFORE generating any paid assets — too much copy in too short
    a spot is unrecoverable.

    Returns JSON report on stdout. Exit 0 on fits/under_budget;
    exit 3 on over_budget.
    """,
    input_schema: %{
      "type" => "object",
      "properties" => %{
        "path" => %{"type" => "string", "description" => "Path to the .fountain file."},
        "duration" => %{
          "type" => "number",
          "description" => "Declared spot duration in seconds."
        },
        "pretty" => %{
          "type" => "boolean",
          "description" => "Pretty-print the emitted JSON. Default false."
        }
      },
      "required" => ["path", "duration"]
    },
    arg_map: [
      {"path", :positional},
      {"duration", "--duration"},
      {"pretty", {"--pretty", :boolean}}
    ]
end
