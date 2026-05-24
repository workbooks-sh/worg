defmodule WorgAgent.Tools.BrandworkBrief do
  @moduledoc """
  Typed wrapper for `brandwork brief <domain>`. The Phase 1 gate's
  single-call cross-channel brand brief: brand identity + social
  profiles + Meta + Google ads, fanned out server-side.
  """

  use WorgAgent.Tools.ShellWrapper,
    name: "brandwork_brief",
    binary: "brandwork",
    argv_prefix: ["brief"],
    description: """
    One-shot cross-channel brand brief. Returns brand identity
    (logo URL, palette, typography, tagline), social handles
    (Instagram / TikTok / YouTube / Twitter / Facebook), recent
    Meta ads, and Google ad-transparency snapshots — all in one
    call. Pass `--json` to get structured output the agent can
    parse.

    Phase 1 brand-research gate: this call's output is one of
    three required inputs (alongside `brand fetch` and `ads
    search`) to satisfy the brandwork_research_done validator.
    """,
    input_schema: %{
      "type" => "object",
      "properties" => %{
        "domain" => %{"type" => "string", "description" => "Domain to brief, e.g. newbalance.com."},
        "json" => %{
          "type" => "boolean",
          "description" => "Return structured JSON. Default false (human-readable)."
        }
      },
      "required" => ["domain"]
    },
    arg_map: [
      {"domain", :positional},
      {"json", {"--json", :boolean}}
    ],
    env: [
      {"BRANDWORK_BASE_URL", {:env, "BRANDWORK_BASE_URL"}}
    ]
end
