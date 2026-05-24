//! worg-lint — enforce WORG.md conventions.
//!
//! Returns a list of [`Diagnostic`]s, each with a code (`E001…`, `W001…`),
//! severity, message, and optional location. The CLI prints these; the
//! runtime can also consume them programmatically.
//!
//! Codes match WORG.md "Linter rules" section. Not all rules are
//! implemented yet — the most load-bearing ones land first. Stubs are
//! marked with TODO and will not falsely report.

#![forbid(unsafe_code)]

use orgize::rowan::ast::AstNode;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use worg_parse::Document;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warn,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: String,
    pub severity: Severity,
    pub message: String,
    /// Best-effort 1-based line number. `None` if not localizable.
    pub line: Option<usize>,
    /// Best-effort headline ID for context.
    pub headline_id: Option<String>,
}

/// A registered plugin entry — what slot the plugin fills and where
/// to find its `manifest.org`. The manifest path is absolute, resolved
/// at glossary-load time against the source file's directory.
#[derive(Debug, Clone)]
pub struct PluginEntry {
    pub slot: Option<String>,
    pub manifest: Option<PathBuf>,
}

/// A registered agent entry — captured from any `:agent:`-tagged
/// level-1 headline encountered while loading the glossary. Keyed by
/// `:ID:` (falling back to the title lowercased if `:ID:` is absent).
/// Future lint rules can use this to validate `:ASSIGNED_AGENT:`
/// references against the known agent set.
#[derive(Debug, Clone, Default)]
pub struct AgentEntry {
    /// `:TYPE:` value (e.g. `ai`, `human`). Mirrors orchestrator-core
    /// `Agent.kind`; named `kind` here because `type` is reserved.
    pub kind: Option<String>,
    /// `:MODEL:` value (e.g. `openrouter/xiaomi/mimo-v2.5-pro`).
    pub model: Option<String>,
    /// `:CAPABILITIES:` split on whitespace.
    pub capabilities: Vec<String>,
    /// `:TOOLS:` split on whitespace.
    pub tools: Vec<String>,
}

/// The glossary the linter enforces — known property names, executor
/// languages, validator kinds, registered plugins, and registered
/// agents. Loaded from a `w.org` file or constructed from the built-in
/// defaults. Composable via merge: a file may declare `#+GLOSSARY:
/// extra.org` to layer additional definitions on top.
#[derive(Debug, Clone)]
pub struct Glossary {
    pub properties: HashSet<String>,
    pub langs: HashSet<String>,
    pub kinds: HashSet<String>,
    pub plugins: HashMap<String, PluginEntry>,
    pub agents: HashMap<String, AgentEntry>,
    /// Org extension slots a plugin can legitimately fill. Populated
    /// from `w.org`'s `* Plugin slot registry` (`:registry:slot:`)
    /// section, or from the built-in canonical set when no `w.org` is
    /// discoverable.
    pub slots: HashSet<String>,
}

impl Glossary {
    /// Built-in default glossary. Used when no `w.org` is discoverable —
    /// matches the historical hardcoded set, so behavior is unchanged
    /// in projects that haven't adopted `w.org` yet.
    pub fn default() -> Self {
        let properties = [
            // identity + ownership
            "ID",
            "ASSIGNED_AGENT",
            "RUN_ID",
            "CATEGORY",
            // scheduling
            "BLOCKER",
            "TRIGGER",
            "ORDERED",
            "EFFORT",
            "PRIORITY",
            "STAGE_ORDER",
            "TIMEOUT_MS",
            // budgets + retries
            "BUDGET",
            "TOOL_BUDGET",
            "RETRY_POLICY",
            "COST_USD",
            // tools + artifacts
            "TOOL",
            "TOOLS_AVAILABLE",
            "ARTIFACT",
            "ARTIFACTS_IN",
            "ARTIFACTS_OUT",
            // executor dispatch
            "TRUST_LEVEL",
            "DERIVED",
            "KIND",
            "SLOT",
            "MANIFEST",
            // agent definitions
            "TYPE",
            "MODEL",
            "CAPABILITIES",
            "TOOLS",
            // common standard org properties we permit silently
            "CUSTOM_ID",
            "ARCHIVE",
            "VISIBILITY",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let langs = ["shell", "bash", "sh", "elixir", "lua", "json", "markdown"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        // Default kinds match the registry in w.org.
        let kinds = [
            "artifact_exists",
            "cmd_zero_exit",
            "screenplay_parse_clean",
            "storyboard_verify_passes",
            "c2pa_verify_passes",
            "vlm_verify_passes_per_shot",
            // Wavelet-specific kinds — registered so
            // wavelet-commercial.org and its sibling plans lint clean.
            // Implementations live in the wavelet runtime
            // (packages/wavelet/), not here; the registry just
            // declares the kinds are legitimate. See w.org §"Validator
            // KIND registry" for arg contracts.
            "brief_check_passes",
            "brandwork_research_done",
            "screenplay_duration_fits",
            "continuity_check_zero_errors",
            "cost_below_usd",
            "comp_verify_passes",
            "wavelet_lint_passes",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        // Canonical org extension slots a plugin can fill. Matches
        // w.org's `* Plugin slot registry` section.
        let slots = ["dynamic-block-writer", "babel-language", "link-resolver", "validator-kind"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        Self {
            properties,
            langs,
            kinds,
            plugins: HashMap::new(),
            agents: HashMap::new(),
            slots,
        }
    }

    /// Empty glossary — used as the accumulator for recursive merges.
    fn empty() -> Self {
        Self {
            properties: HashSet::new(),
            langs: HashSet::new(),
            kinds: HashSet::new(),
            plugins: HashMap::new(),
            agents: HashMap::new(),
            slots: HashSet::new(),
        }
    }

    /// Extend this glossary with everything from `other`. Set fields
    /// union; plugins + agents HashMaps union with later entries
    /// winning on duplicate name.
    pub fn merge(&mut self, other: Glossary) {
        self.properties.extend(other.properties);
        self.langs.extend(other.langs);
        self.kinds.extend(other.kinds);
        self.slots.extend(other.slots);
        for (name, entry) in other.plugins {
            self.plugins.insert(name, entry);
        }
        for (name, entry) in other.agents {
            self.agents.insert(name, entry);
        }
    }

    /// Validate the glossary's internal consistency. Returns diagnostics
    /// for missing plugin manifests (E006), slot mismatches (W011),
    /// missing required manifest file-level keywords (W013), and
    /// missing required manifest sections (W014). Called by the CLI
    /// after load; failures are setup-level, not per-document.
    pub fn validate(&self) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        for (name, entry) in &self.plugins {
            let Some(manifest_path) = &entry.manifest else {
                continue;
            };
            if !manifest_path.is_file() {
                out.push(Diagnostic {
                    code: "E006".into(),
                    severity: Severity::Error,
                    message: format!(
                        "plugin `{name}` :MANIFEST: `{}` does not resolve to a file",
                        manifest_path.display()
                    ),
                    line: None,
                    headline_id: Some(name.clone()),
                });
                continue;
            }
            let src = match std::fs::read_to_string(manifest_path) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let manifest_slot = scan_keyword_value(&src, "#+SLOT:");
            if let (Some(declared), Some(actual)) = (&entry.slot, &manifest_slot) {
                if declared != actual {
                    out.push(Diagnostic {
                        code: "W011".into(),
                        severity: Severity::Warn,
                        message: format!(
                            "plugin `{name}` :SLOT: `{declared}` differs from manifest's `#+SLOT: {actual}`"
                        ),
                        line: None,
                        headline_id: Some(name.clone()),
                    });
                }
            }
            // W012: the declared :SLOT: value must be a registered
            // slot. Adding new slots is allowed (extend w.org's slot
            // registry) but declaring a misspelled one is a bug.
            if let Some(declared) = &entry.slot {
                if !self.slots.contains(&declared.to_ascii_lowercase()) {
                    out.push(Diagnostic {
                        code: "W012".into(),
                        severity: Severity::Warn,
                        message: format!(
                            "plugin `{name}` :SLOT: `{declared}` is not in the registered slot set. Add it to `w.org` under the plugin slot registry."
                        ),
                        line: None,
                        headline_id: Some(name.clone()),
                    });
                }
            }
            // W013: manifest must declare #+TITLE, #+VERSION, #+SLOT
            // as file-level keywords. These shape the plugin's
            // self-description; missing any of them means the manifest
            // is partial.
            for kw in &["#+TITLE:", "#+VERSION:", "#+SLOT:"] {
                if scan_keyword_value(&src, kw).is_none() {
                    out.push(Diagnostic {
                        code: "W013".into(),
                        severity: Severity::Warn,
                        message: format!(
                            "plugin `{name}` manifest missing required file-level keyword `{kw}`"
                        ),
                        line: None,
                        headline_id: Some(name.clone()),
                    });
                }
            }
            // W014: manifest must have an `* About` level-1 section
            // explaining what the plugin does. Convention-driven —
            // pragmatically the discoverability anchor for humans
            // reading the manifest cold.
            if !has_top_level_about_section(&src) {
                out.push(Diagnostic {
                    code: "W014".into(),
                    severity: Severity::Warn,
                    message: format!(
                        "plugin `{name}` manifest missing required `* About` section"
                    ),
                    line: None,
                    headline_id: Some(name.clone()),
                });
            }
        }
        out
    }

    /// Load a glossary from a file, recursively following any
    /// `#+GLOSSARY:` declarations inside it. Cycle-safe.
    pub fn from_file(path: &Path) -> std::io::Result<Self> {
        let mut visited = HashSet::new();
        Self::load_recursive(path, &mut visited)
    }

    /// Discover the canonical glossary for `start`. Walks up from
    /// `start`'s directory toward the filesystem root, returning the
    /// first `w.org` found. At each level, prefers a direct `w.org`
    /// child (the in-package case — linting a file inside
    /// `packages/worg/`), falling back to `packages/worg/w.org` as a
    /// sibling-of-ancestor (the monorepo case — linting a file in
    /// `apps/foo/` finds the canonical glossary at the repo's
    /// `packages/worg/`). None if no glossary is reachable.
    ///
    /// Discovery is configured by file layout, not env vars or flags.
    /// If a file genuinely needs a different glossary, it declares
    /// `#+GLOSSARY:` at the top — the in-band, in-org-file path that
    /// w.org's extension policy expects.
    pub fn discover(start: &Path) -> Option<PathBuf> {
        let mut dir = if start.is_file() {
            start.parent()?.to_path_buf()
        } else {
            start.to_path_buf()
        };
        loop {
            let direct = dir.join("w.org");
            if direct.is_file() {
                return Some(direct);
            }
            let canonical = dir.join("packages/worg/w.org");
            if canonical.is_file() {
                return Some(canonical);
            }
            if !dir.pop() {
                return None;
            }
        }
    }

    fn load_recursive(path: &Path, visited: &mut HashSet<PathBuf>) -> std::io::Result<Self> {
        let canonical = path.canonicalize()?;
        if !visited.insert(canonical.clone()) {
            return Ok(Self::empty());
        }

        let src = std::fs::read_to_string(path)?;
        let base_dir = canonical.parent().map(|p| p.to_path_buf());
        let mut combined = parse_glossary_sections(&src, base_dir.as_deref());

        let dir_for_includes = base_dir.as_deref().unwrap_or_else(|| Path::new("."));
        for token in scan_keyword_paths(&src, "#+GLOSSARY:") {
            let include_path = dir_for_includes.join(&token);
            if let Ok(included) = Self::load_recursive(&include_path, visited) {
                combined.merge(included);
            }
        }

        // Always allow the common org-mode standards even if w.org omits them.
        for std_prop in &["ID", "CUSTOM_ID", "CATEGORY", "ARCHIVE", "VISIBILITY"] {
            combined.properties.insert(std_prop.to_string());
        }

        Ok(combined)
    }
}

/// Parse glossary entries from a single file's contents.
///
/// Level-1 headlines do one of two things:
///   - Act as a *section container* whose level-2 children are
///     vocabulary items. Drives sections by tag:
///       - `:prop:` → property names (uppercased)
///       - `:lang:` → executor languages (lowercased)
///       - `:kind:` → validator kinds (lowercased)
///       - `:plugin:` → plugin entries with SLOT + MANIFEST properties
///   - Act as a *direct registration* of one item. Currently:
///       - `:agent:` → an agent entry keyed by `:ID:` (or title if
///         `:ID:` absent); captures `:TYPE:`, `:MODEL:`,
///         `:CAPABILITIES:`, `:TOOLS:`.
///
/// Direct-registration tags do NOT set the section tracker — their
/// children are documentation prose, not registry items.
fn parse_glossary_sections(src: &str, base_dir: Option<&Path>) -> Glossary {
    let doc = Document::parse(src);
    let mut g = Glossary::empty();
    let mut section: Option<&'static str> = None;

    for hl in doc.headlines() {
        let level = hl.level();
        let tags: Vec<String> = worg_query::headline_tags(&hl);
        let title = hl.title_raw().trim().to_string();
        if level == 1 {
            // Direct registration: :agent:-tagged level-1 IS the agent.
            if tags.iter().any(|t| t == "agent") {
                let props = hl.properties();
                let get = |key: &str| {
                    props
                        .as_ref()
                        .and_then(|p| p.get(key))
                        .map(|t| t.to_string())
                };
                let id = get("ID").unwrap_or_else(|| title.to_ascii_lowercase());
                let kind = get("TYPE");
                let model = get("MODEL");
                let capabilities = get("CAPABILITIES")
                    .map(|s| {
                        s.split_whitespace().map(str::to_string).collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let tools = get("TOOLS")
                    .map(|s| {
                        s.split_whitespace().map(str::to_string).collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                g.agents.insert(
                    id,
                    AgentEntry {
                        kind,
                        model,
                        capabilities,
                        tools,
                    },
                );
                // Agents don't act as section containers — their
                // children are prose, not registry items.
                section = None;
                continue;
            }
            section = if tags.iter().any(|t| t == "prop") {
                Some("prop")
            } else if tags.iter().any(|t| t == "lang") {
                Some("lang")
            } else if tags.iter().any(|t| t == "kind") {
                Some("kind")
            } else if tags.iter().any(|t| t == "plugin") {
                Some("plugin")
            } else if tags.iter().any(|t| t == "slot") {
                Some("slot")
            } else {
                None
            };
        } else if level == 2 {
            match section {
                Some("prop") => {
                    g.properties.insert(title.to_ascii_uppercase());
                }
                Some("lang") => {
                    g.langs.insert(title.to_ascii_lowercase());
                }
                Some("kind") => {
                    g.kinds.insert(title.to_ascii_lowercase());
                }
                Some("slot") => {
                    g.slots.insert(title.to_ascii_lowercase());
                }
                Some("plugin") => {
                    let slot = hl
                        .properties()
                        .and_then(|p| p.get("SLOT"))
                        .map(|t| t.to_string());
                    let manifest = hl
                        .properties()
                        .and_then(|p| p.get("MANIFEST"))
                        .map(|t| t.to_string())
                        .map(|rel| {
                            base_dir
                                .map(|d| d.join(&rel))
                                .unwrap_or_else(|| PathBuf::from(rel))
                        });
                    g.plugins.insert(title, PluginEntry { slot, manifest });
                }
                _ => {}
            }
        }
    }
    g
}

/// Whitespace-separated tokens following a given file-keyword.
fn scan_keyword_paths(src: &str, keyword: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in src.lines() {
        if let Some(rest) = line.strip_prefix(keyword) {
            for token in rest.split_whitespace() {
                out.push(token.to_string());
            }
        }
    }
    out
}

/// First value of a single-valued file-keyword (e.g. `#+SLOT: foo` → `foo`).
fn scan_keyword_value(src: &str, keyword: &str) -> Option<String> {
    src.lines()
        .filter_map(|l| l.strip_prefix(keyword))
        .map(|s| s.trim().to_string())
        .next()
}

/// True iff the document has a level-1 headline titled "About"
/// (case-insensitive). Plugin manifests are expected to carry such
/// a section as the discoverability anchor per worg's manifest
/// convention.
fn has_top_level_about_section(src: &str) -> bool {
    let doc = Document::parse(src);
    doc.headlines()
        .iter()
        .any(|h| h.level() == 1 && h.title_raw().trim().eq_ignore_ascii_case("About"))
}

/// Lint a parsed document against the default glossary.
pub fn lint(doc: &Document) -> Vec<Diagnostic> {
    lint_with_glossary(doc, &Glossary::default())
}

/// Lint a parsed document against a specific glossary. Used by the CLI
/// after discovering and loading a project's `w.org` (plus any
/// `#+GLOSSARY:` declarations the target file layers on top).
pub fn lint_with_glossary(doc: &Document, glossary: &Glossary) -> Vec<Diagnostic> {
    let mut out = Vec::new();

    let known_ids: HashSet<String> = doc
        .headlines()
        .iter()
        .filter_map(|h| {
            h.properties()
                .and_then(|p| p.get("ID"))
                .map(|t| t.to_string())
        })
        .collect();

    let extensions = collect_extensions(doc);

    for hl in doc.headlines() {
        let id = hl
            .properties()
            .and_then(|p| p.get("ID"))
            .map(|t| t.to_string());

        // W001: unknown uppercase property.
        // `ARG_*` properties are validator-kind arguments — allowed
        // unconditionally; the KIND implementation rejects unknown
        // args at runtime.
        if let Some(props) = hl.properties() {
            for (k, _v) in props.iter() {
                let key = k.to_string();
                let key_upper = key.to_ascii_uppercase();
                if key == key_upper
                    && !glossary.properties.contains(&key_upper)
                    && !extensions.contains(&key_upper)
                    && !key_upper.starts_with("ARG_")
                {
                    out.push(Diagnostic {
                        code: "W001".into(),
                        severity: Severity::Warn,
                        message: format!(
                            "unknown uppercase property `{key}` — possible drift. Add to `#+EXTENSIONS:` if intentional."
                        ),
                        line: None,
                        headline_id: id.clone(),
                    });
                }
            }
        }

        // E003: dangling [[id:...]] in :BLOCKER:
        if let Some(deps) = hl
            .properties()
            .and_then(|p| p.get("BLOCKER"))
            .map(|t| t.to_string())
        {
            for dep_id in worg_query::parse_blocker(&deps) {
                if !known_ids.contains(&dep_id) {
                    out.push(Diagnostic {
                        code: "E003".into(),
                        severity: Severity::Error,
                        message: format!(
                            ":BLOCKER: references missing id `{dep_id}`"
                        ),
                        line: None,
                        headline_id: id.clone(),
                    });
                }
            }
        }

        // W008: dangling [[id:...]] in :TRIGGER: (wb-0mqz.7).
        //
        // Warning rather than error (distinct from E003 :BLOCKER:):
        // a dangling :TRIGGER: ref is a silent no-op at runtime
        // (`Sync.cascade_success` skips missing targets so a stale
        // link doesn't crash a successful Loop iteration). A dangling
        // :BLOCKER: ref, by contrast, is unsatisfiable — the task
        // can never become pickable. Different cost, different
        // severity.
        if let Some(targets) = hl
            .properties()
            .and_then(|p| p.get("TRIGGER"))
            .map(|t| t.to_string())
        {
            for target_id in worg_query::parse_blocker(&targets) {
                if !known_ids.contains(&target_id) {
                    out.push(Diagnostic {
                        code: "W008".into(),
                        severity: Severity::Warn,
                        message: format!(
                            ":TRIGGER: references missing id `{target_id}` — cascade will be a no-op"
                        ),
                        line: None,
                        headline_id: id.clone(),
                    });
                }
            }
        }

        // W006 / W010: validator KIND presence + value check.
        let tags = worg_query::headline_tags(&hl);
        if tags.iter().any(|t| t == "validator") {
            let kind = hl
                .properties()
                .and_then(|p| p.get("KIND"))
                .map(|t| t.to_string());
            match kind {
                None => {
                    out.push(Diagnostic {
                        code: "W006".into(),
                        severity: Severity::Warn,
                        message: "validator headline missing `:KIND:` property".into(),
                        line: None,
                        headline_id: id.clone(),
                    });
                }
                Some(k) if !glossary.kinds.contains(&k.to_ascii_lowercase()) => {
                    out.push(Diagnostic {
                        code: "W010".into(),
                        severity: Severity::Warn,
                        message: format!(
                            "validator `:KIND: {k}` is not in the registered KIND set. Add it to `w.org` under the validator KIND registry."
                        ),
                        line: None,
                        headline_id: id.clone(),
                    });
                }
                Some(_) => {}
            }
        }

        // E004: source block language not in dispatch table, but only
        // when `:results` is present (the block will actually be
        // executed). Documentation src blocks pass silently.
        if let Some(section) = hl.section() {
            use orgize::ast::SourceBlock;
            use orgize::SyntaxKind;
            for child in section.syntax().children() {
                if child.kind() != SyntaxKind::SOURCE_BLOCK {
                    continue;
                }
                let Some(block) = SourceBlock::cast(child) else { continue };
                let lang = block.language().map(|t| t.to_string().to_ascii_lowercase());
                let has_results = block
                    .parameters()
                    .map(|p| p.to_string().contains(":results"))
                    .unwrap_or(false);
                if !has_results {
                    continue;
                }
                if let Some(l) = lang.as_deref() {
                    if !glossary.langs.contains(l) {
                        out.push(Diagnostic {
                            code: "E004".into(),
                            severity: Severity::Error,
                            message: format!(
                                "source block language `{l}` with `:results` is not in worg's dispatch table. Use shell to invoke external tools."
                            ),
                            line: None,
                            headline_id: id.clone(),
                        });
                    }
                }
            }
        }
    }

    // E007: :BLOCKER: cycle. Whole-document analysis — runs once after
    // the per-headline loop. DFS with permanent-mark + on-stack
    // tracking; back-edge → cycle. Emits one diagnostic per detected
    // cycle, anchored on the node where the back-edge was discovered,
    // with the full cycle path in the message.
    out.extend(detect_blocker_cycles(doc));

    out
}

/// Run DFS over the `:BLOCKER:` graph and return one E007 diagnostic
/// per discovered cycle. Nodes are `:ID:` strings; edges are the
/// blocker references parsed from each headline's `:BLOCKER:`
/// property. Headlines without `:ID:` are silently skipped (they
/// can't be cycle participants because nothing can name them).
fn detect_blocker_cycles(doc: &Document) -> Vec<Diagnostic> {
    // Build the adjacency list: id → blocker ids.
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();
    let mut all_ids: Vec<String> = Vec::new();
    for hl in doc.headlines() {
        let Some(id) = hl
            .properties()
            .and_then(|p| p.get("ID"))
            .map(|t| t.to_string())
        else {
            continue;
        };
        all_ids.push(id.clone());
        let blockers = hl
            .properties()
            .and_then(|p| p.get("BLOCKER"))
            .map(|t| worg_query::parse_blocker(&t.to_string()))
            .unwrap_or_default();
        graph.insert(id, blockers);
    }

    // DFS state: White = unvisited, Gray = on the current path, Black
    // = fully processed. `Mark` lives at module level so dfs_visit
    // can reference it from outside this function.
    let mut marks: HashMap<String, Mark> =
        all_ids.iter().map(|id| (id.clone(), Mark::White)).collect();
    let mut path: Vec<String> = Vec::new();
    let mut reported: HashSet<Vec<String>> = HashSet::new();
    let mut out = Vec::new();

    // Recursive DFS via an explicit stack-of-iterators avoids the
    // implicit-recursion stack-overflow risk on pathological inputs,
    // but task graphs are small in practice — keep it readable. We
    // use a helper closure approximation via a worklist.
    for start in all_ids.iter().cloned() {
        if marks.get(&start) != Some(&Mark::White) {
            continue;
        }
        dfs_visit(
            &graph,
            &start,
            &mut marks,
            &mut path,
            &mut reported,
            &mut out,
        );
    }
    out
}

/// Recursive DFS visit. Pushes the current node onto `path` as Gray,
/// recurses into each blocker, pops back to Black on return. A back-
/// edge (encountering a Gray neighbor already on `path`) is a cycle:
/// emit one E007 diagnostic with the cycle path, and de-dup against
/// `reported` so the same cycle isn't reported once per traversal
/// entry point.
fn dfs_visit(
    graph: &HashMap<String, Vec<String>>,
    node: &str,
    marks: &mut HashMap<String, Mark>,
    path: &mut Vec<String>,
    reported: &mut HashSet<Vec<String>>,
    out: &mut Vec<Diagnostic>,
) {
    marks.insert(node.to_string(), Mark::Gray);
    path.push(node.to_string());

    if let Some(neighbors) = graph.get(node) {
        for nb in neighbors {
            match marks.get(nb).copied().unwrap_or(Mark::White) {
                Mark::White => {
                    dfs_visit(graph, nb, marks, path, reported, out);
                }
                Mark::Gray => {
                    // Back-edge → cycle. Extract from the path: from
                    // the first occurrence of `nb` through the
                    // current end, then close the cycle by appending
                    // `nb` again.
                    let Some(start_idx) = path.iter().position(|x| x == nb) else {
                        continue;
                    };
                    let mut cycle: Vec<String> = path[start_idx..].to_vec();
                    cycle.push(nb.to_string());
                    // Canonicalize for de-dup: rotate so the
                    // lexicographically smallest id is first
                    // (treating the cycle as a ring).
                    let canonical = canonical_cycle(&cycle);
                    if reported.insert(canonical) {
                        out.push(Diagnostic {
                            code: "E007".into(),
                            severity: Severity::Error,
                            message: format!(
                                ":BLOCKER: cycle detected: {}",
                                cycle.join(" → ")
                            ),
                            line: None,
                            headline_id: Some(node.to_string()),
                        });
                    }
                }
                Mark::Black => {
                    // Fully processed; no cycle reachable through it.
                }
            }
        }
    }

    path.pop();
    marks.insert(node.to_string(), Mark::Black);
}

/// Normalize a cycle path for de-duplication. Input includes the
/// closing repeat of the start node; we strip that, rotate so the
/// lexicographically smallest id leads, and re-append the close.
fn canonical_cycle(cycle: &[String]) -> Vec<String> {
    if cycle.len() <= 1 {
        return cycle.to_vec();
    }
    let ring = &cycle[..cycle.len() - 1];
    let (min_idx, _) = ring
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| a.cmp(b))
        .expect("non-empty");
    let mut rotated: Vec<String> = ring[min_idx..]
        .iter()
        .chain(ring[..min_idx].iter())
        .cloned()
        .collect();
    rotated.push(rotated[0].clone());
    rotated
}

#[derive(Clone, Copy, PartialEq)]
enum Mark {
    White,
    Gray,
    Black,
}

/// Read `#+EXTENSIONS:` file keyword — whitespace-separated list of
/// extension property names a project has declared inline. For more
/// than a few, prefer pointing at a separate file via `#+GLOSSARY:`.
fn collect_extensions(doc: &Document) -> HashSet<String> {
    let src = doc.serialize();
    let mut out = HashSet::new();
    for line in src.lines() {
        if let Some(rest) = line.strip_prefix("#+EXTENSIONS:") {
            for token in rest.split_whitespace() {
                out.insert(token.to_ascii_uppercase());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn w001_unknown_property() {
        let doc = Document::parse(
            "* TODO Task
:PROPERTIES:
:ID: t1
:WEIRD_PROP: yes
:END:
",
        );
        let diags = lint(&doc);
        assert!(diags.iter().any(|d| d.code == "W001"));
    }

    #[test]
    fn w001_accepts_documented_extensions() {
        let doc = Document::parse(
            "#+EXTENSIONS: WEIRD_PROP CUSTOM_THING
* TODO Task
:PROPERTIES:
:ID: t1
:WEIRD_PROP: yes
:END:
",
        );
        let diags = lint(&doc);
        assert!(!diags.iter().any(|d| d.code == "W001"));
    }

    #[test]
    fn w001_allows_arg_prefix_properties() {
        let doc = Document::parse(
            "* TODO Validator :validator:
:PROPERTIES:
:ID: v1
:KIND: artifact_exists
:ARG_PATH: /tmp/something
:ARG_RECURSIVE: true
:END:
",
        );
        let diags = lint(&doc);
        assert!(!diags.iter().any(|d| d.code == "W001"));
    }

    #[test]
    fn w010_unknown_validator_kind() {
        let doc = Document::parse(
            "* TODO Validator :validator:
:PROPERTIES:
:ID: v1
:KIND: totally_made_up
:END:
",
        );
        let diags = lint(&doc);
        let w010 = diags.iter().find(|d| d.code == "W010").expect("W010");
        assert!(w010.message.contains("totally_made_up"));
    }

    #[test]
    fn w010_passes_for_registered_kind() {
        let doc = Document::parse(
            "* TODO Validator :validator:
:PROPERTIES:
:ID: v1
:KIND: artifact_exists
:ARG_PATH: /tmp/x
:END:
",
        );
        let diags = lint(&doc);
        assert!(!diags.iter().any(|d| d.code == "W010"));
    }

    #[test]
    fn w010_passes_for_wavelet_validator_kinds() {
        // The seven wavelet-specific kinds must lint clean — they are
        // the contract that lets wavelet adopt WORG as its agent
        // architecture (the wavelet-commercial.org plan references all
        // seven; without registration each fires W010 and the plan
        // can't be loaded). See packages/worg/w.org §"Validator KIND
        // registry" and packages/worg/proposed/plans/wavelet-commercial.org.
        for kind in [
            "brief_check_passes",
            "brandwork_research_done",
            "screenplay_duration_fits",
            "continuity_check_zero_errors",
            "cost_below_usd",
            "comp_verify_passes",
            "wavelet_lint_passes",
        ] {
            let src = format!(
                "* TODO Validator :validator:
:PROPERTIES:
:ID: v1
:KIND: {kind}
:ARG_PATH: /tmp/x
:END:
"
            );
            let doc = Document::parse(&src);
            let diags = lint(&doc);
            assert!(
                !diags.iter().any(|d| d.code == "W010"),
                "kind `{kind}` should be registered; got W010: {:?}",
                diags.iter().filter(|d| d.code == "W010").collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn e006_missing_plugin_manifest() {
        let mut g = Glossary::default();
        g.plugins.insert(
            "ghost-plugin".to_string(),
            PluginEntry {
                slot: Some("dynamic-block-writer".to_string()),
                manifest: Some(PathBuf::from("/this/path/does/not/exist.org")),
            },
        );
        let diags = g.validate();
        let e006 = diags.iter().find(|d| d.code == "E006").expect("E006");
        assert!(e006.message.contains("ghost-plugin"));
    }

    #[test]
    fn w011_slot_mismatch() {
        let tmp = std::env::temp_dir().join("worg-lint-test-w011.org");
        std::fs::write(&tmp, "#+TITLE: test\n#+SLOT: babel-language\n").unwrap();
        let mut g = Glossary::default();
        g.plugins.insert(
            "mismatched".to_string(),
            PluginEntry {
                slot: Some("dynamic-block-writer".to_string()),
                manifest: Some(tmp.clone()),
            },
        );
        let diags = g.validate();
        let w011 = diags.iter().find(|d| d.code == "W011").expect("W011");
        assert!(w011.message.contains("dynamic-block-writer"));
        assert!(w011.message.contains("babel-language"));
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn parse_glossary_registers_agent_entries() {
        let src = "* Workhorse                                                       :agent:\n\
:PROPERTIES:\n\
:ID:           workhorse\n\
:TYPE:         ai\n\
:MODEL:        openrouter/xiaomi/mimo-v2.5-pro\n\
:CAPABILITIES: bash read write lua-eval\n\
:TOOLS:        bash read write substrate_push worg_parse\n\
:END:\n\
\n\
Body prose — should not be registered.\n\
\n\
** System prompt\n\
Some text.\n\
";
        let tmp = std::env::temp_dir().join("worg-lint-test-agent.org");
        std::fs::write(&tmp, src).unwrap();
        let g = Glossary::from_file(&tmp).unwrap();
        std::fs::remove_file(&tmp).ok();
        let workhorse = g
            .agents
            .get("workhorse")
            .expect("workhorse registered by :ID:");
        assert_eq!(workhorse.kind.as_deref(), Some("ai"));
        assert_eq!(
            workhorse.model.as_deref(),
            Some("openrouter/xiaomi/mimo-v2.5-pro")
        );
        assert_eq!(workhorse.capabilities, vec!["bash", "read", "write", "lua-eval"]);
        assert_eq!(
            workhorse.tools,
            vec!["bash", "read", "write", "substrate_push", "worg_parse"]
        );
    }

    #[test]
    fn agent_falls_back_to_title_when_id_missing() {
        let src = "* MysteryAgent                                                  :agent:\n\
:PROPERTIES:\n\
:TYPE: ai\n\
:END:\n\
";
        let tmp = std::env::temp_dir().join("worg-lint-test-agent-noid.org");
        std::fs::write(&tmp, src).unwrap();
        let g = Glossary::from_file(&tmp).unwrap();
        std::fs::remove_file(&tmp).ok();
        // No :ID:, so the registry key is the lowercased title.
        assert!(g.agents.contains_key("mysteryagent"));
    }

    #[test]
    fn agent_does_not_pollute_section_tracker() {
        // An :agent:-tagged headline followed by an :prop:-tagged one
        // must not leak section state — the prop section must still
        // collect its level-2 children as properties.
        let src = "* Workhorse :agent:\n\
:PROPERTIES:\n\
:ID: workhorse\n\
:END:\n\
\n\
** System prompt\n\
prose\n\
\n\
* Properties :prop:\n\
** CUSTOM_FIELD\n\
   A test property.\n\
";
        let tmp = std::env::temp_dir().join("worg-lint-test-agent-mixed.org");
        std::fs::write(&tmp, src).unwrap();
        let g = Glossary::from_file(&tmp).unwrap();
        std::fs::remove_file(&tmp).ok();
        assert!(g.agents.contains_key("workhorse"));
        assert!(g.properties.contains("CUSTOM_FIELD"));
        // The agent's "** System prompt" child must NOT have been
        // collected as a property.
        assert!(!g.properties.contains("SYSTEM PROMPT"));
        assert!(!g.properties.contains("System prompt"));
    }

    #[test]
    fn discover_finds_w_org_via_direct_walk_up() {
        let tmp = std::env::temp_dir().join(format!(
            "worg-lint-discover-direct-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let nested = tmp.join("a/b/c");
        std::fs::create_dir_all(&nested).unwrap();
        let w_org = tmp.join("a/w.org");
        std::fs::write(&w_org, "* x").unwrap();

        let found = Glossary::discover(&nested).expect("walk-up should find w.org");
        // Canonicalize both to handle macOS /tmp → /private/tmp symlink.
        assert_eq!(
            std::fs::canonicalize(&found).unwrap(),
            std::fs::canonicalize(&w_org).unwrap()
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn discover_monorepo_fallback_finds_canonical_via_sibling_path() {
        // Simulate a monorepo: packages/worg/w.org exists, the linted
        // file is at apps/foo/bar.org. Walk-up from apps/foo/ doesn't
        // find a direct w.org, but at the repo root it finds the
        // canonical packages/worg/w.org sibling.
        let tmp = std::env::temp_dir().join(format!(
            "worg-lint-discover-monorepo-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let app_dir = tmp.join("apps/foo");
        std::fs::create_dir_all(&app_dir).unwrap();
        let canonical = tmp.join("packages/worg/w.org");
        std::fs::create_dir_all(canonical.parent().unwrap()).unwrap();
        std::fs::write(&canonical, "* x").unwrap();

        let found = Glossary::discover(&app_dir).expect("should find canonical via sibling");
        assert_eq!(
            std::fs::canonicalize(&found).unwrap(),
            std::fs::canonicalize(&canonical).unwrap()
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn merge_combines_agent_registries() {
        let mut a = Glossary::empty();
        a.agents
            .insert("workhorse".into(), AgentEntry::default());
        let mut b = Glossary::empty();
        b.agents.insert("specialist".into(), AgentEntry::default());
        a.merge(b);
        assert!(a.agents.contains_key("workhorse"));
        assert!(a.agents.contains_key("specialist"));
    }

    #[test]
    fn w012_unknown_slot_value() {
        let tmp = std::env::temp_dir().join(format!(
            "worg-lint-test-w012-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(
            &tmp,
            "#+TITLE: x\n#+VERSION: 1\n#+SLOT: made-up-slot\n* About\nbody\n",
        )
        .unwrap();
        let mut g = Glossary::default();
        g.plugins.insert(
            "broken".to_string(),
            PluginEntry {
                slot: Some("made-up-slot".to_string()),
                manifest: Some(tmp.clone()),
            },
        );
        let diags = g.validate();
        let w012 = diags.iter().find(|d| d.code == "W012").expect("W012");
        assert!(w012.message.contains("made-up-slot"));
        assert!(w012.message.contains("broken"));
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn w012_canonical_slot_value_passes() {
        // All four canonical slots accept without W012.
        for slot in &[
            "dynamic-block-writer",
            "babel-language",
            "link-resolver",
            "validator-kind",
        ] {
            let tmp = std::env::temp_dir().join(format!(
                "worg-lint-test-w012-ok-{slot}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::write(
                &tmp,
                format!("#+TITLE: x\n#+VERSION: 1\n#+SLOT: {slot}\n* About\nbody\n"),
            )
            .unwrap();
            let mut g = Glossary::default();
            g.plugins.insert(
                format!("p-{slot}"),
                PluginEntry {
                    slot: Some(slot.to_string()),
                    manifest: Some(tmp.clone()),
                },
            );
            let diags = g.validate();
            assert!(
                !diags.iter().any(|d| d.code == "W012"),
                "canonical slot {slot} should pass; got: {diags:#?}"
            );
            std::fs::remove_file(&tmp).ok();
        }
    }

    #[test]
    fn w012_extends_via_glossary_slot_section() {
        // A glossary file that declares an extra slot via the
        // `:slot:` section makes that slot legal — no W012 even if
        // it's not in the built-in default set.
        let tmp_glossary = std::env::temp_dir().join(format!(
            "worg-lint-test-w012-ext-glossary-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(
            &tmp_glossary,
            "* Slots :registry:slot:\n** my-custom-slot\nA project-specific extension slot.\n",
        )
        .unwrap();
        // Load the extension glossary, merge with default.
        let extension = Glossary::from_file(&tmp_glossary).unwrap();
        let mut g = Glossary::default();
        g.merge(extension);
        assert!(g.slots.contains("my-custom-slot"));

        // Now a plugin claiming that slot should pass W012.
        let tmp_manifest = std::env::temp_dir().join(format!(
            "worg-lint-test-w012-ext-manifest-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(
            &tmp_manifest,
            "#+TITLE: x\n#+VERSION: 1\n#+SLOT: my-custom-slot\n* About\nbody\n",
        )
        .unwrap();
        g.plugins.insert(
            "custom".to_string(),
            PluginEntry {
                slot: Some("my-custom-slot".to_string()),
                manifest: Some(tmp_manifest.clone()),
            },
        );
        let diags = g.validate();
        assert!(
            !diags.iter().any(|d| d.code == "W012"),
            "extended slot should not trigger W012; got: {diags:#?}"
        );
        std::fs::remove_file(&tmp_glossary).ok();
        std::fs::remove_file(&tmp_manifest).ok();
    }

    #[test]
    fn w013_missing_required_manifest_keywords() {
        // Manifest missing #+VERSION + #+SLOT triggers W013 twice;
        // #+TITLE is present so only those two fire.
        let tmp = std::env::temp_dir().join(format!(
            "worg-lint-test-w013-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(&tmp, "#+TITLE: partial\n* About\nbody\n").unwrap();
        let mut g = Glossary::default();
        g.plugins.insert(
            "partial".to_string(),
            PluginEntry {
                slot: None,
                manifest: Some(tmp.clone()),
            },
        );
        let diags = g.validate();
        let w013_count = diags.iter().filter(|d| d.code == "W013").count();
        assert_eq!(
            w013_count, 2,
            "expected W013 for both missing keywords, got: {diags:#?}"
        );
        for d in diags.iter().filter(|d| d.code == "W013") {
            assert!(d.message.contains("partial"));
            assert!(d.message.contains("manifest missing required file-level keyword"));
        }
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn w014_missing_about_section() {
        let tmp = std::env::temp_dir().join(format!(
            "worg-lint-test-w014-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        // Has all required keywords but no * About section.
        std::fs::write(
            &tmp,
            "#+TITLE: bare\n#+VERSION: 1\n#+SLOT: dynamic-block-writer\n",
        )
        .unwrap();
        let mut g = Glossary::default();
        g.plugins.insert(
            "bare".to_string(),
            PluginEntry {
                slot: Some("dynamic-block-writer".to_string()),
                manifest: Some(tmp.clone()),
            },
        );
        let diags = g.validate();
        let w014 = diags
            .iter()
            .find(|d| d.code == "W014")
            .expect("expected W014");
        assert!(w014.message.contains("bare"));
        assert!(w014.message.contains("About"));
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn w014_about_case_insensitive_and_levels() {
        // `* about` (lowercase) at level 1 should still satisfy.
        // `** About` at level 2 should NOT satisfy — must be top-level.
        let tmp1 = std::env::temp_dir().join(format!(
            "worg-lint-test-w014-lower-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(
            &tmp1,
            "#+TITLE: t\n#+VERSION: 1\n#+SLOT: dynamic-block-writer\n* about\nbody\n",
        )
        .unwrap();
        let mut g = Glossary::default();
        g.plugins.insert(
            "lower".into(),
            PluginEntry {
                slot: Some("dynamic-block-writer".into()),
                manifest: Some(tmp1.clone()),
            },
        );
        let diags = g.validate();
        assert!(
            !diags.iter().any(|d| d.code == "W014"),
            "lowercase `about` at level 1 should satisfy: {diags:#?}"
        );
        std::fs::remove_file(&tmp1).ok();

        let tmp2 = std::env::temp_dir().join(format!(
            "worg-lint-test-w014-nested-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(
            &tmp2,
            "#+TITLE: t\n#+VERSION: 1\n#+SLOT: dynamic-block-writer\n* Other\n** About\nbody\n",
        )
        .unwrap();
        let mut g2 = Glossary::default();
        g2.plugins.insert(
            "nested".into(),
            PluginEntry {
                slot: Some("dynamic-block-writer".into()),
                manifest: Some(tmp2.clone()),
            },
        );
        let diags2 = g2.validate();
        assert!(
            diags2.iter().any(|d| d.code == "W014"),
            "level-2 `** About` must NOT satisfy: {diags2:#?}"
        );
        std::fs::remove_file(&tmp2).ok();
    }

    #[test]
    fn validate_passes_when_slots_match_and_manifest_complete() {
        // Manifest carrying all required fields (#+TITLE, #+VERSION,
        // #+SLOT) AND a top-level * About section AND matching :SLOT:
        // produces zero diagnostics.
        let tmp = std::env::temp_dir().join("worg-lint-test-ok.org");
        std::fs::write(
            &tmp,
            "#+TITLE: test\n#+VERSION: 1\n#+SLOT: dynamic-block-writer\n* About\nbody\n",
        )
        .unwrap();
        let mut g = Glossary::default();
        g.plugins.insert(
            "matched".to_string(),
            PluginEntry {
                slot: Some("dynamic-block-writer".to_string()),
                manifest: Some(tmp.clone()),
            },
        );
        let diags = g.validate();
        assert!(diags.is_empty(), "expected no diags, got: {diags:#?}");
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn e003_dangling_blocker() {
        let doc = Document::parse(
            "* TODO Task A
:PROPERTIES:
:ID: a
:BLOCKER: [[id:nonexistent]]
:END:
",
        );
        let diags = lint(&doc);
        let e003 = diags.iter().find(|d| d.code == "E003").expect("E003");
        assert!(e003.message.contains("nonexistent"));
    }

    // ── W008 :TRIGGER: dangling ref (wb-0mqz.7) ──────────────────────

    #[test]
    fn w008_dangling_trigger() {
        let doc = Document::parse(
            "* TODO Source
:PROPERTIES:
:ID: src
:TRIGGER: [[id:nonexistent]]
:END:
",
        );
        let diags = lint(&doc);
        let w008 = diags.iter().find(|d| d.code == "W008").expect("W008");
        assert!(matches!(w008.severity, Severity::Warn));
        assert!(w008.message.contains("nonexistent"));
        assert!(w008.message.contains("no-op"));
    }

    #[test]
    fn w008_silent_when_all_trigger_refs_resolve() {
        let doc = Document::parse(
            "* TODO Source
:PROPERTIES:
:ID: src
:TRIGGER: [[id:target]]
:END:

* TODO Target
:PROPERTIES:
:ID: target
:END:
",
        );
        let diags = lint(&doc);
        assert!(
            diags.iter().all(|d| d.code != "W008"),
            "resolvable :TRIGGER: should not fire W008, got: {diags:?}"
        );
    }

    #[test]
    fn w008_reports_each_missing_ref_separately() {
        let doc = Document::parse(
            "* TODO Source
:PROPERTIES:
:ID: src
:TRIGGER: [[id:a]] [[id:b]] [[id:exists]]
:END:

* TODO Exists
:PROPERTIES:
:ID: exists
:END:
",
        );
        let diags = lint(&doc);
        let w008s: Vec<_> = diags.iter().filter(|d| d.code == "W008").collect();
        assert_eq!(w008s.len(), 2, "expected 2 W008 for a + b, got: {w008s:?}");
        let messages: String = w008s.iter().map(|d| d.message.as_str()).collect::<Vec<_>>().join("\n");
        assert!(messages.contains("`a`"));
        assert!(messages.contains("`b`"));
        assert!(!messages.contains("`exists`"));
    }

    #[test]
    fn w008_does_not_fire_on_e003_blocker_path() {
        // Sanity check: :BLOCKER: with dangling ref fires E003 only,
        // not W008. The two rules look at different properties.
        let doc = Document::parse(
            "* TODO Source
:PROPERTIES:
:ID: src
:BLOCKER: [[id:nonexistent]]
:END:
",
        );
        let diags = lint(&doc);
        assert!(diags.iter().any(|d| d.code == "E003"));
        assert!(diags.iter().all(|d| d.code != "W008"));
    }

    // ── E007 :BLOCKER: cycle detection (wb-0mqz.6) ───────────────────

    #[test]
    fn e007_self_cycle() {
        let doc = Document::parse(
            "* TODO A
:PROPERTIES:
:ID: a
:BLOCKER: [[id:a]]
:END:
",
        );
        let diags = lint(&doc);
        let e007 = diags.iter().find(|d| d.code == "E007").expect("E007");
        assert!(e007.message.contains("cycle detected"));
        assert!(e007.message.contains("a → a"));
    }

    #[test]
    fn e007_two_node_cycle() {
        let doc = Document::parse(
            "* TODO A
:PROPERTIES:
:ID: a
:BLOCKER: [[id:b]]
:END:

* TODO B
:PROPERTIES:
:ID: b
:BLOCKER: [[id:a]]
:END:
",
        );
        let diags = lint(&doc);
        let cycles: Vec<_> = diags.iter().filter(|d| d.code == "E007").collect();
        assert_eq!(
            cycles.len(),
            1,
            "expected exactly 1 E007 for 2-node cycle, got: {cycles:?}"
        );
        // Canonicalization rotates the smallest id to start: "a → b → a".
        assert!(cycles[0].message.contains("a → b → a"));
    }

    #[test]
    fn e007_three_node_cycle_with_full_path() {
        let doc = Document::parse(
            "* TODO A
:PROPERTIES:
:ID: a
:BLOCKER: [[id:b]]
:END:

* TODO B
:PROPERTIES:
:ID: b
:BLOCKER: [[id:c]]
:END:

* TODO C
:PROPERTIES:
:ID: c
:BLOCKER: [[id:a]]
:END:
",
        );
        let diags = lint(&doc);
        let cycles: Vec<_> = diags.iter().filter(|d| d.code == "E007").collect();
        assert_eq!(cycles.len(), 1);
        assert!(cycles[0].message.contains("a → b → c → a"));
    }

    #[test]
    fn e007_acyclic_dag_is_silent() {
        let doc = Document::parse(
            "* TODO A
:PROPERTIES:
:ID: a
:END:

* TODO B
:PROPERTIES:
:ID: b
:BLOCKER: [[id:a]]
:END:

* TODO C
:PROPERTIES:
:ID: c
:BLOCKER: [[id:b]]
:END:
",
        );
        let diags = lint(&doc);
        assert!(
            diags.iter().all(|d| d.code != "E007"),
            "acyclic DAG should not fire E007, got: {diags:?}"
        );
    }

    #[test]
    fn e007_multiple_disjoint_cycles_all_reported() {
        // Two independent cycles in the same plan: a↔b and c↔d.
        let doc = Document::parse(
            "* TODO A
:PROPERTIES:
:ID: a
:BLOCKER: [[id:b]]
:END:

* TODO B
:PROPERTIES:
:ID: b
:BLOCKER: [[id:a]]
:END:

* TODO C
:PROPERTIES:
:ID: c
:BLOCKER: [[id:d]]
:END:

* TODO D
:PROPERTIES:
:ID: d
:BLOCKER: [[id:c]]
:END:
",
        );
        let diags = lint(&doc);
        let cycles: Vec<_> = diags.iter().filter(|d| d.code == "E007").collect();
        assert_eq!(
            cycles.len(),
            2,
            "expected 2 disjoint cycles, got: {cycles:?}"
        );
        let messages: String = cycles.iter().map(|d| d.message.as_str()).collect::<Vec<_>>().join("\n");
        assert!(messages.contains("a → b → a"), "missing a↔b cycle in {messages}");
        assert!(messages.contains("c → d → c"), "missing c↔d cycle in {messages}");
    }

    #[test]
    fn e007_does_not_fire_on_back_to_back_dfs_traversals() {
        // The DFS visits each node once; the canonicalization +
        // `reported` set ensures the same cycle isn't double-reported
        // even if the traversal happens to discover it from two
        // different entry points.
        let doc = Document::parse(
            "* TODO A
:PROPERTIES:
:ID: a
:BLOCKER: [[id:b]]
:END:

* TODO B
:PROPERTIES:
:ID: b
:BLOCKER: [[id:a]] [[id:c]]
:END:

* TODO C
:PROPERTIES:
:ID: c
:END:
",
        );
        let diags = lint(&doc);
        let cycles: Vec<_> = diags.iter().filter(|d| d.code == "E007").collect();
        assert_eq!(cycles.len(), 1);
    }

    #[test]
    fn w006_validator_without_kind() {
        let doc = Document::parse(
            "* TODO Validator :validator:
:PROPERTIES:
:ID: v1
:END:
",
        );
        let diags = lint(&doc);
        assert!(diags.iter().any(|d| d.code == "W006"));
    }

    #[test]
    fn no_diags_on_clean_document() {
        let doc = Document::parse(
            "* DONE Stage 1
:PROPERTIES:
:ID: s1
:ASSIGNED_AGENT: workhorse
:END:

* TODO Stage 2
:PROPERTIES:
:ID: s2
:BLOCKER: [[id:s1]]
:END:

** TODO Validator :validator:
:PROPERTIES:
:KIND: artifact_exists
:END:
",
        );
        let diags = lint(&doc);
        assert!(diags.is_empty(), "expected no diags, got: {diags:#?}");
    }
}
