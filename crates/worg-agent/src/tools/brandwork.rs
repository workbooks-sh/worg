//! Typed `ShellTool` wrappers around the `brandwork` CLI.
//!
//! Phase 3 of wb-ki6b: ports the two existing Elixir
//! `WorgAgent.Tools.Brandwork*` wrappers. The three additional
//! brandwork_brand_fetch / brandwork_brand_product / brandwork_ads_search
//! wrappers land alongside as the brandwork CLI surface stabilizes.
//!
//! All wrappers forward `BRANDWORK_BASE_URL` from the parent env so
//! evals can pin the brandwork service per-run.

use serde_json::json;

use crate::tools::shell::ShellTool;

/// `brandwork brief <domain>` — one-call cross-channel brand brief.
pub fn brief() -> ShellTool {
    ShellTool::new(
        "brandwork_brief",
        "One-shot cross-channel brand brief. Returns brand identity \
         (logo URL, palette, typography, tagline), social handles \
         (Instagram / TikTok / YouTube / Twitter / Facebook), recent \
         Meta ads, and Google ad-transparency snapshots — all in one \
         call. Pass json=true to get structured output. \
         Phase 1 brand-research gate: this call's output is one of \
         three required inputs (alongside brand_fetch and ads_search) \
         to satisfy the brandwork_research_done validator.",
        json!({
            "type": "object",
            "properties": {
                "domain": {
                    "type": "string",
                    "description": "Domain to brief, e.g. newbalance.com."
                },
                "json": {
                    "type": "boolean",
                    "description": "Return structured JSON. Default false."
                }
            },
            "required": ["domain"]
        }),
        "brandwork",
    )
    .with_argv_prefix(["brief"])
    .with_positional("domain")
    .with_bool_flag("json", "--json")
    .with_env_from("BRANDWORK_BASE_URL", "BRANDWORK_BASE_URL")
}

/// `brandwork resolve <query>` — brand name / product → canonical domain.
pub fn resolve() -> ShellTool {
    ShellTool::new(
        "brandwork_resolve",
        "Resolve a brand name or product description to a canonical \
         domain. Runs four sources in parallel (5s cap each): direct \
         slug guesses, Exa neural search, Wikipedia infobox parse, \
         LLM fallback (only when < 2 verified results). Each candidate \
         is HEAD-verified via the tiered fetcher. Returns ranked \
         candidates with confidence scores. Use the top accepted result \
         as the domain for downstream brandwork calls.",
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Brand name or product description. e.g., 'Bubble Skincare' or 'Whirlpool stand mixer'."
                },
                "json": {
                    "type": "boolean",
                    "description": "Return structured JSON. Default false."
                }
            },
            "required": ["query"]
        }),
        "brandwork",
    )
    .with_argv_prefix(["resolve"])
    .with_positional("query")
    .with_bool_flag("json", "--json")
    .with_env_from("BRANDWORK_BASE_URL", "BRANDWORK_BASE_URL")
}

/// `brandwork brand fetch <domain>` — single-source brand identity.
pub fn brand_fetch() -> ShellTool {
    ShellTool::new(
        "brandwork_brand_fetch",
        "Fetch the canonical brand identity for a domain: logo URL, color \
         palette, typography, tagline. Single-source (no Meta / Google ads / \
         social handles — use `brandwork_brief` for the fan-out call). Use \
         when you've already resolved the domain and just need identity.",
        json!({
            "type": "object",
            "properties": {
                "domain": {"type": "string", "description": "Brand domain, e.g. newbalance.com."},
                "json": {"type": "boolean", "description": "Return structured JSON."}
            },
            "required": ["domain"]
        }),
        "brandwork",
    )
    .with_argv_prefix(["brand", "fetch"])
    .with_positional("domain")
    .with_bool_flag("json", "--json")
    .with_env_from("BRANDWORK_BASE_URL", "BRANDWORK_BASE_URL")
}

/// `brandwork brand product <domain> <product>` — product-page intelligence.
pub fn brand_product() -> ShellTool {
    ShellTool::new(
        "brandwork_brand_product",
        "Fetch product-page intelligence for a specific SKU on a brand's \
         site: product images (canonical PDP shots), price, copy, key \
         features. The agent uses these as references for shot generation.",
        json!({
            "type": "object",
            "properties": {
                "domain": {"type": "string", "description": "Brand domain."},
                "product": {"type": "string", "description": "Product slug or name."},
                "json": {"type": "boolean", "description": "Return structured JSON."}
            },
            "required": ["domain", "product"]
        }),
        "brandwork",
    )
    .with_argv_prefix(["brand", "product"])
    .with_positional("domain")
    .with_positional("product")
    .with_bool_flag("json", "--json")
    .with_env_from("BRANDWORK_BASE_URL", "BRANDWORK_BASE_URL")
}

/// `brandwork ads search <domain>` — Meta + Google ad library snapshot.
pub fn ads_search() -> ShellTool {
    ShellTool::new(
        "brandwork_ads_search",
        "Search Meta Ad Library + Google Ads Transparency for a brand's \
         recent paid creative. Returns ad copy, image URLs, target geos, \
         spend tier. Use to ground the spot in what's actually been \
         shipping in market — informs tone + format choices.",
        json!({
            "type": "object",
            "properties": {
                "domain": {"type": "string", "description": "Brand domain."},
                "limit": {"type": "number", "description": "Max ads per source. Default 10."},
                "since_days": {"type": "number", "description": "Restrict to ads from the last N days."},
                "json": {"type": "boolean", "description": "Return structured JSON."}
            },
            "required": ["domain"]
        }),
        "brandwork",
    )
    .with_argv_prefix(["ads", "search"])
    .with_positional("domain")
    .with_flag("limit", "--limit")
    .with_flag("since_days", "--since-days")
    .with_bool_flag("json", "--json")
    .with_env_from("BRANDWORK_BASE_URL", "BRANDWORK_BASE_URL")
}

/// Register every brandwork ShellTool wrapper.
pub fn register_all(registry: &mut crate::tool_registry::ToolRegistry) {
    registry.register(brief());
    registry.register(resolve());
    registry.register(brand_fetch());
    registry.register(brand_product());
    registry.register(ads_search());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::Tool;

    #[test]
    fn brandwork_wrappers_unique_named() {
        let names: Vec<_> = vec![brief(), resolve()].into_iter().map(|t| t.name()).collect();
        assert_eq!(names.len(), 2);
        assert!(names.iter().all(|n| n.starts_with("brandwork_")));
    }
}
