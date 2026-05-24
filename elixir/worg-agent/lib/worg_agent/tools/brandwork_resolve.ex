defmodule WorgAgent.Tools.BrandworkResolve do
  @moduledoc """
  Typed wrapper for `brandwork resolve <query>`. Resolves a brand
  name or product description to a canonical domain via four
  parallel sources (direct slug, Exa neural search, Wikipedia
  infobox, LLM fallback). Use FIRST when the user's brief names a
  brand without a domain, or names a parent brand whose product
  ships under a sub-brand (e.g. "Whirlpool stand mixer" →
  kitchenaid.com).
  """

  use WorgAgent.Tools.ShellWrapper,
    name: "brandwork_resolve",
    binary: "brandwork",
    argv_prefix: ["resolve"],
    description: """
    Resolve a brand name or product description to a canonical
    domain. Runs four sources in parallel (5s cap each): direct
    slug guesses, Exa neural search, Wikipedia infobox parse,
    LLM fallback (only when < 2 verified results). Each candidate
    is HEAD-verified via the tiered fetcher.

    Returns ranked candidates with confidence scores. Use the top
    `accepted` result as the domain for downstream brandwork calls.
    """,
    input_schema: %{
      "type" => "object",
      "properties" => %{
        "query" => %{
          "type" => "string",
          "description" => "Brand name or product description. e.g., 'Bubble Skincare' or 'Whirlpool stand mixer'."
        },
        "json" => %{
          "type" => "boolean",
          "description" => "Return structured JSON. Default false."
        }
      },
      "required" => ["query"]
    },
    arg_map: [
      {"query", :positional},
      {"json", {"--json", :boolean}}
    ],
    env: [
      {"BRANDWORK_BASE_URL", {:env, "BRANDWORK_BASE_URL"}}
    ]
end
