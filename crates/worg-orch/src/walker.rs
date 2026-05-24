//! org → orchestrator-protocol walkers.
//!
//! Extracts [`AgentDefinition`]s from a parsed [`worg_parse::Document`]
//! by finding `:agent:`-tagged level-1 headlines, reading their property
//! drawers + system-prompt subtrees, and emitting wire-compatible
//! [`Agent`] structs alongside richer application-layer fields.
//!
//! Used by `worg orch export agents` (wb-nlln.6) to produce
//! `.wb-orch/agents.json`. Watershed-side consumers (wb-6irl.21 et al.)
//! read the richer definition directly via the Wasmex bridge.
//!
//! Task-DAG walker emits one [`Task`] per `:stage:`-tagged headline,
//! mapping outline ancestry to `Task.parent` and TODO/status tags to
//! `TaskState`. Validators + tools nested inside a stage are NOT
//! emitted as tasks — they're application-layer gating concerns.

use crate::{
    Agent, AgentId, AgentStatus, AgentType, AgentsFile, ProtocolVersion, Task, TaskId, TaskState,
    PROTOCOL_VERSION,
};
use orgize::ast::Headline;
use orgize::rowan::ast::AstNode;
use time::OffsetDateTime;

/// Application-layer agent definition extracted from an `:agent:`-tagged
/// org headline. Carries both the wire-strict [`Agent`] (for
/// `worg orch export agents` → `.wb-orch/agents.json`) and richer
/// application-layer fields (`model`, `tools`, `system_prompt`) that the
/// orchestrator protocol does not transmit.
///
/// The wire export deliberately drops the richer fields; they live in
/// the WORG source-of-truth and are consumed by Watershed reading the
/// `.org` file directly (via the Wasmex bridge — see CLAUDE.md
/// "Dependency layers" for ownership).
#[derive(Debug, Clone, PartialEq)]
pub struct AgentDefinition {
    /// Orchestrator-protocol wire fields. Pass `.wire` to
    /// `serde_json::to_string` to emit one entry of `.wb-orch/agents.json`.
    pub wire: Agent,
    /// LLM model slug from `:MODEL:` (e.g.
    /// `openrouter/xiaomi/mimo-v2.5-pro`). None if the agent headline
    /// omitted the property.
    pub model: Option<String>,
    /// Tool function-name catalog from `:TOOLS:`, whitespace-split.
    pub tools: Vec<String>,
    /// Concatenated body of the agent's `** System prompt` subtree,
    /// or None if no such subtree exists. The `** System prompt` title
    /// line itself is stripped; descendant `***` headings + their
    /// bodies are kept verbatim so the dispatcher can feed it to the
    /// LLM unchanged.
    pub system_prompt: Option<String>,
}

/// Walk a parsed Document and emit every `:agent:`-tagged level-1
/// headline as an [`AgentDefinition`], in document order.
///
/// Properties consumed from each agent's drawer:
/// - `:ID:` → `Agent.id` (falls back to lowercased title if missing).
/// - `:TYPE:` → `Agent.kind`. `human` maps to [`AgentType::Human`];
///   anything else (including missing) defaults to [`AgentType::Ai`].
/// - `:CAPABILITIES:` (whitespace-split) → `Agent.capabilities`.
/// - `:MODEL:` → `AgentDefinition.model`.
/// - `:TOOLS:` (whitespace-split) → `AgentDefinition.tools`.
///
/// `Agent.name` is the headline title. `Agent.status` defaults to
/// [`AgentStatus::Active`] (the WORG file does not express agent
/// lifecycle; the orchestrator state machine owns that).
///
/// The `** System prompt` subtree (depth-first, including any `***`
/// sub-headlines + bodies) is concatenated as `system_prompt`, with
/// the leading `** System prompt` title line stripped. Convention
/// per workhorse.org's open-question resolution: option 1 (verbatim
/// subtree body).
pub fn agent_definitions(doc: &worg_parse::Document) -> Vec<AgentDefinition> {
    let mut out = Vec::new();
    let headlines = doc.headlines();
    for (idx, hl) in headlines.iter().enumerate() {
        if hl.level() != 1 {
            continue;
        }
        let tags = worg_query::headline_tags(hl);
        if !tags.iter().any(|t| t == "agent") {
            continue;
        }
        out.push(extract_agent(hl, &headlines, idx));
    }
    out
}

/// Find a single agent definition by `:ID:`. Returns `None` if no agent
/// in the document matches.
pub fn agent_definition_by_id(
    doc: &worg_parse::Document,
    id: &str,
) -> Option<AgentDefinition> {
    agent_definitions(doc)
        .into_iter()
        .find(|a| a.wire.id.as_str() == id)
}

/// Emit the full `.wb-orch/agents.json` content for a parsed Document.
/// Each `:agent:`-tagged level-1 headline becomes one wire-strict entry.
pub fn agents_file(doc: &worg_parse::Document) -> AgentsFile {
    AgentsFile {
        version: ProtocolVersion(PROTOCOL_VERSION),
        agents: agent_definitions(doc)
            .into_iter()
            .map(|a| a.wire)
            .collect(),
    }
}

/// wb-4vhr.21 Phase A — a single-call snapshot of the whole board from
/// one `.org` file. The eventual replacement for the per-file
/// `.wb-orch/{agents.json, tasks/*.json}` directory format.
///
/// Studio is already Postgres-first (spike confirmed: zero `.wb-orch/`
/// writes in `apps/studio/`), so the JSON-files-on-disk shape was a
/// vestigial export. Consumers (Pi extensions, the Studio orchestrator
/// HTTP controller, worg-agent's loader, third-party tools) should
/// migrate to:
///
/// ```ignore
/// let doc = worg_parse::Document::parse(&src);
/// let snap = worg_orch::board_snapshot(&doc, &Default::default());
/// // snap.agents : Vec<Agent>           — same shape as agents.json
/// // snap.tasks  : Vec<TaskDefinition>  — superset of one tasks/*.json
/// ```
///
/// The CLI exposes this as `worg orch board <file>` which prints a
/// single JSON blob to stdout, removing the need for a sibling
/// directory of files.
#[derive(Debug, Clone)]
pub struct BoardSnapshot {
    /// Wire-protocol version stamp. Same value as
    /// [`AgentsFile::version`] — the snapshot is a strict superset of
    /// the legacy `agents.json` payload.
    pub version: ProtocolVersion,
    /// Wire-strict agents (matches one entry per `.wb-orch/agents.json`).
    pub agents: Vec<Agent>,
    /// All tasks in document order. Each carries the application-layer
    /// fields the legacy per-file `tasks/<id>.json` exports surfaced.
    pub tasks: Vec<TaskDefinition>,
}

/// Walk a parsed Document once and produce the full
/// [`BoardSnapshot`]. Equivalent to calling [`agents_file`] +
/// [`task_definitions`] and collecting the results into one struct.
pub fn board_snapshot(
    doc: &worg_parse::Document,
    opts: &ExportOpts,
) -> BoardSnapshot {
    BoardSnapshot {
        version: ProtocolVersion(PROTOCOL_VERSION),
        agents: agent_definitions(doc)
            .into_iter()
            .map(|a| a.wire)
            .collect(),
        tasks: task_definitions(doc, opts),
    }
}

fn extract_agent(hl: &Headline, all: &[Headline], idx: usize) -> AgentDefinition {
    let props = hl.properties();
    // Property lookup is case-insensitive (org-mode convention —
    // authors write :Effort: but it's the same property as :EFFORT:
    // and :effort:). orgize stores keys as-written; we case-fold
    // both sides on lookup.
    let get = |k: &str| {
        let want = k.to_ascii_uppercase();
        props.as_ref().and_then(|p| {
            p.iter()
                .find(|(key, _)| key.to_string().to_ascii_uppercase() == want)
                .map(|(_, v)| v.to_string())
        })
    };
    let title = hl.title_raw().trim().to_string();
    let id = get("ID").unwrap_or_else(|| title.to_ascii_lowercase());
    let name = if title.is_empty() { id.clone() } else { title };
    let kind = match get("TYPE").as_deref() {
        Some("human") => AgentType::Human,
        _ => AgentType::Ai,
    };
    let capabilities = get("CAPABILITIES")
        .map(|s| s.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();
    let model = get("MODEL");
    let tools = get("TOOLS")
        .map(|s| s.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();
    let system_prompt = find_system_prompt(all, idx);

    let wire = Agent {
        id: AgentId::new(id),
        name,
        kind,
        status: AgentStatus::Active,
        runtime: None,
        role: None,
        capabilities,
        reports_to: None,
        heartbeat_sec: None,
    };
    AgentDefinition {
        wire,
        model,
        tools,
        system_prompt,
    }
}

/// Find the `** System prompt` headline directly under the agent at
/// `agent_idx` and return its subtree text minus the title line.
fn find_system_prompt(headlines: &[Headline], agent_idx: usize) -> Option<String> {
    // Scan forward from the agent for a level-2 headline whose title
    // is "system prompt" (case-insensitive). Stop if we hit another
    // level-1 (next agent or unrelated section).
    let mut sp_idx = None;
    for (i, hl) in headlines.iter().enumerate().skip(agent_idx + 1) {
        let lvl = hl.level();
        if lvl == 1 {
            break;
        }
        if lvl == 2 && hl.title_raw().trim().eq_ignore_ascii_case("system prompt") {
            sp_idx = Some(i);
            break;
        }
    }
    let sp_idx = sp_idx?;
    let sp = &headlines[sp_idx];
    // A Headline node's text covers the title line + body + all
    // descendant headlines/bodies until the next equal-or-higher
    // headline. Strip the first line to leave just the body content.
    let subtree = sp.syntax().text().to_string();
    let body = subtree.split_once('\n').map(|(_, rest)| rest).unwrap_or("");
    let trimmed = body.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

// ─────────────────────────── Task walker ─────────────────────────────────

/// Application-layer task definition extracted from a `:stage:`-tagged
/// org headline. Carries both the wire-strict [`Task`] (for
/// `worg orch export tasks` → `.wb-orch/tasks/<id>.json`) and richer
/// application-layer fields the orchestrator protocol does not transmit.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskDefinition {
    /// Orchestrator-protocol wire fields. Serialize `.wire` to JSON to
    /// produce one entry of `.wb-orch/tasks/<id>.json`.
    pub wire: Task,
    /// The full `:BLOCKER:` list from the org file, parsed as
    /// `[[id:...]]` references. The wire `.parent` field captures
    /// outline-tree ancestry only; richer cross-tree dependencies live
    /// here for application-layer consumers (Watershed scheduler).
    pub blocker: Vec<TaskId>,
    /// The full `:TRIGGER:` list from the org file (org-edna's
    /// success-side cascade). When this task transitions to DONE,
    /// every entry's referent that is currently `:blocked` is
    /// advanced to `:ready`. Tasks past `:blocked` are left alone.
    /// Same `[[id:...]]` syntax as `:BLOCKER:`.
    pub trigger: Vec<TaskId>,
    /// `:Effort:` property parsed into minutes (org-mode core; wb-0mqz.8).
    /// Accepts org-mode's standard formats:
    /// - `HH:MM` → hours * 60 + minutes
    /// - `Nm` → N minutes
    /// - `Nh` → N * 60 minutes (decimal allowed, e.g. `1.5h` → 90)
    /// - `Nd` → N * 8 * 60 minutes (org's default workday is 8h)
    /// - bare `N` → N minutes (org convention when no unit given)
    /// `None` when the property is absent or unparseable.
    pub effort_minutes: Option<u32>,
    /// `:BUDGET:` property verbatim (e.g.
    /// `tokens=200000 cost_usd=2.00 wallclock=600s`).
    pub budget: Option<String>,
    /// `:RETRY_POLICY:` property verbatim (e.g.
    /// `max=3 backoff=fixed fallback=mark_blocked`).
    pub retry_policy: Option<String>,
    /// `:STAGE_MODEL:` property verbatim (wb-6t1r). Per-stage model
    /// override; when set, the Loop dispatches THIS stage's LLM calls
    /// to the named model regardless of the agent's default `:MODEL:`.
    /// Use case: wavelet runs its orchestrator loop on a cheap-fast
    /// model (e.g. `xiaomi/mimo-v2.5-pro`) but escalates specific
    /// stages — `frame_judge` / `video_judge` to Gemini, brand-voice
    /// gate to Claude Opus. Single agent-level :MODEL: can't express
    /// this; STAGE_MODEL at the task node can. `None` when the
    /// property is absent (Loop falls through to agent.model).
    pub stage_model: Option<String>,
}

/// Per-export metadata the orchestrator-protocol Task requires but the
/// WORG file doesn't carry (the protocol needs a creator + timestamp for
/// every task; the WORG file just has the structural definition).
#[derive(Debug, Clone)]
pub struct ExportOpts {
    /// The agent slug to record as the task's `created_by`. The CLI
    /// defaults this to `worg-exporter` — callers can override.
    pub created_by: AgentId,
    /// The timestamp to record as `created_at`. The CLI defaults this
    /// to `OffsetDateTime::now_utc()`.
    pub exported_at: OffsetDateTime,
}

impl Default for ExportOpts {
    fn default() -> Self {
        Self {
            created_by: AgentId::new("worg-exporter"),
            exported_at: OffsetDateTime::now_utc(),
        }
    }
}

/// Walk a parsed Document and emit every `:stage:`-tagged headline as a
/// [`TaskDefinition`], in document order. Validators and tools nested
/// inside a stage are NOT emitted (they're gating concerns, not tasks).
///
/// Field mapping:
/// - `:ID:` → `Task.id` (fallback: title slugified)
/// - title → `Task.title`
/// - Closest `:stage:`-tagged ancestor by outline depth → `Task.parent`
/// - `:ASSIGNED_AGENT:` → `Task.assigned_to` (single-entry Vec)
/// - body prose (under the headline, before any child headline) → `Task.description`
/// - Status: status tag (`:active:`/`:done:`/etc.) preferred; falls back
///   to TODO keyword (TODO/DOING/BLOCKED/DONE/FAILED/ABANDONED); defaults
///   to `Backlog` if neither present.
/// - `:BLOCKER:` list → `TaskDefinition.blocker` (the wire `parent`
///   field uses outline ancestry, NOT the dependency list — the protocol
///   has only one parent slot per task)
/// - `:BUDGET:` / `:RETRY_POLICY:` → `TaskDefinition.budget` / `.retry_policy`
pub fn task_definitions(doc: &worg_parse::Document, opts: &ExportOpts) -> Vec<TaskDefinition> {
    let mut out = Vec::new();
    let headlines = doc.headlines();

    // Tag-inheritance exclusion list (wb-0mqz.11). Composed from:
    //   1. Built-in defaults — classification + status tags. These
    //      describe a single headline; inheriting them confuses the
    //      meaning. E.g. inheriting `:stage:` would make every
    //      descendant look like a task even if it's documentation.
    //   2. Author overrides from the file-level
    //      `#+TAG_EXCLUDE_INHERIT:` keyword (whitespace-separated).
    //
    // The headline's OWN tags are always included regardless of this
    // list — exclusion only affects inheritance from ancestors.
    let src = doc.serialize();
    let mut exclude_owned: Vec<String> = DEFAULT_TAG_EXCLUDE_INHERIT
        .iter()
        .map(|s| s.to_string())
        .collect();
    if let Some(line) = src
        .lines()
        .find_map(|l| l.strip_prefix("#+TAG_EXCLUDE_INHERIT:"))
    {
        exclude_owned.extend(line.split_whitespace().map(|s| s.to_string()));
    }
    let exclude_refs: Vec<&str> = exclude_owned.iter().map(|s| s.as_str()).collect();

    // First pass: collect (idx, level, id_or_slug, is_task) for ancestry lookups.
    let summary: Vec<HeadlineSummary> = headlines
        .iter()
        .map(|h| {
            let tags = worg_query::headline_tags_with_exclusions(h, &exclude_refs);
            let id = headline_id_or_title_slug(h);
            HeadlineSummary {
                level: h.level(),
                id,
                is_task: is_task_headline(h, &tags),
                ordered: read_ordered_property(h),
            }
        })
        .collect();

    for (idx, hl) in headlines.iter().enumerate() {
        if !summary[idx].is_task {
            continue;
        }
        out.push(extract_task(hl, &headlines, &summary, idx, opts, &exclude_refs));
    }
    out
}

/// Tags that NEVER inherit by default. Classification tags identify
/// what a single headline IS (a stage / tool / validator / etc.);
/// inheriting them mislabels descendants. Status tags reflect runtime
/// state of one headline; inheritance would lie about descendant
/// state. Authors who genuinely need one of these to cascade can
/// declare it on each child explicitly.
const DEFAULT_TAG_EXCLUDE_INHERIT: &[&str] = &[
    // Classification tags (from w.org's `* Classification` section).
    "stage",
    "validator",
    "tool",
    "input",
    "review",
    "agent",
    // Status tags (from w.org's `* Status` section).
    "pending",
    "active",
    "done",
    "failed",
    "abandoned",
];

/// Find a single task definition by `:ID:`.
pub fn task_definition_by_id(
    doc: &worg_parse::Document,
    id: &str,
    opts: &ExportOpts,
) -> Option<TaskDefinition> {
    task_definitions(doc, opts)
        .into_iter()
        .find(|t| t.wire.id.as_str() == id)
}

struct HeadlineSummary {
    level: usize,
    id: String,
    is_task: bool,
    /// True iff this headline's `:ORDERED:` property is set to a
    /// truthy value (`t`, `true`, `yes`, `1` — case-insensitive).
    /// org-mode's native semantic: when set on a parent, the
    /// parent's task children must transition to DONE in headline
    /// order. Carried per-headline (rather than per-task) so a
    /// non-task ORDERED container behaves the same — though we
    /// only consult it via the task hierarchy.
    ordered: bool,
}

fn extract_task(
    hl: &Headline,
    _headlines: &[Headline],
    summary: &[HeadlineSummary],
    idx: usize,
    opts: &ExportOpts,
    tag_exclude_inherit: &[&str],
) -> TaskDefinition {
    let props = hl.properties();
    // Property lookup is case-insensitive (org-mode convention —
    // authors write :Effort: but it's the same property as :EFFORT:
    // and :effort:). orgize stores keys as-written; we case-fold
    // both sides on lookup.
    let get = |k: &str| {
        let want = k.to_ascii_uppercase();
        props.as_ref().and_then(|p| {
            p.iter()
                .find(|(key, _)| key.to_string().to_ascii_uppercase() == want)
                .map(|(_, v)| v.to_string())
        })
    };
    let title = hl.title_raw().trim().to_string();
    let id = get("ID").unwrap_or_else(|| slugify(&title));
    let tags = worg_query::headline_tags_with_exclusions(hl, tag_exclude_inherit);
    let state = resolve_state(hl, &tags);

    let assigned_to = get("ASSIGNED_AGENT")
        .map(|s| vec![AgentId::new(s)])
        .unwrap_or_default();

    let parent_idx = nearest_task_ancestor_idx(summary, idx);
    let parent = parent_idx.map(|i| TaskId::new(summary[i].id.clone()));
    let description = section_text(hl);

    let mut blocker = get("BLOCKER")
        .map(|s| parse_blocker(&s))
        .unwrap_or_default();
    let trigger = get("TRIGGER")
        .map(|s| parse_blocker(&s))
        .unwrap_or_default();
    let effort_minutes = get("EFFORT").and_then(|s| parse_effort(&s));
    // DEADLINE: native org-mode timestamp → wire `due` (wb-0mqz.10).
    // org-mode authors write `DEADLINE: <2026-06-01 Mon>` on the line
    // after the headline; orgize parses it into a Timestamp. We
    // promote it to the wire `due` field (Option<OffsetDateTime>).
    // SCHEDULED: timestamps are deliberately ignored at the wire
    // level — they're informational ("ready to start on") rather
    // than a hard "must be done by" constraint.
    let due = hl.deadline().and_then(deadline_to_offset_datetime);
    // Priority (wb-0mqz.9, org-mode core): native inline `[#A]` syntax
    // takes precedence; falls back to the legacy :PRIORITY: property
    // when no inline marker is present. Mapping: A→1, B→2, C→3 (the
    // lower the number, the higher the priority — matches the wire
    // protocol's "0 = highest" convention).
    let priority = hl
        .priority()
        .map(|t| t.to_string())
        .and_then(|s| parse_priority_marker(&s))
        .or_else(|| get("PRIORITY").and_then(|s| s.trim().parse::<i32>().ok()));

    // ORDERED parent (wb-0mqz.5, org-mode core): inject a synthetic
    // :BLOCKER: edge to the immediate-predecessor sibling task. The
    // wire shape stays identical (still just a `blocker` list); Loop
    // can't tell the difference between an explicit author-declared
    // dependency and an inferred-from-ORDERED one. That's the point —
    // ORDERED collapses N explicit `:BLOCKER:` lines into one
    // property on the parent.
    if let Some(p_idx) = parent_idx {
        if summary[p_idx].ordered {
            if let Some(predecessor) = immediate_predecessor_sibling_task(summary, idx) {
                if !blocker.iter().any(|b| b.as_str() == predecessor.as_str()) {
                    blocker.push(predecessor);
                }
            }
        }
    }

    let budget = get("BUDGET");
    let retry_policy = get("RETRY_POLICY");
    let stage_model = get("STAGE_MODEL").map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

    let wire = Task {
        id: TaskId::new(id),
        title,
        state,
        created_by: opts.created_by.clone(),
        created_at: opts.exported_at,
        description,
        assigned_to,
        parent,
        capabilities: Vec::new(),
        priority,
        due,
        reviewer: None,
        tags: filter_runtime_tags(&tags),
        acceptance: None,
        result_summary: None,
        result_full: None,
        error: None,
        blocked_reason: None,
        input_required_prompt: None,
        comments: Vec::new(),
        updated_at: None,
    };

    TaskDefinition {
        wire,
        blocker,
        trigger,
        effort_minutes,
        budget,
        retry_policy,
        stage_model,
    }
}

/// Promote an orgize [`Timestamp`] (typically from `DEADLINE:`) into
/// a wire-protocol [`time::OffsetDateTime`]. Returns `None` if any
/// required component (year/month/day) is missing or unparseable.
/// Times without an explicit hour/minute default to 00:00 UTC.
///
/// Org-mode timestamps don't carry timezone info — we interpret the
/// naive date as UTC at the wire boundary. Authors who care about
/// timezone precision should encode the offset elsewhere (out of
/// scope for the wire format today).
fn deadline_to_offset_datetime(
    ts: orgize::ast::Timestamp,
) -> Option<time::OffsetDateTime> {
    let year: i32 = ts.year_start()?.to_string().parse().ok()?;
    let month_num: u8 = ts.month_start()?.to_string().parse().ok()?;
    let day: u8 = ts.day_start()?.to_string().parse().ok()?;
    let month = time::Month::try_from(month_num).ok()?;
    let date = time::Date::from_calendar_date(year, month, day).ok()?;

    let hour: u8 = ts
        .hour_start()
        .and_then(|t| t.to_string().parse().ok())
        .unwrap_or(0);
    let minute: u8 = ts
        .minute_start()
        .and_then(|t| t.to_string().parse().ok())
        .unwrap_or(0);
    let tod = time::Time::from_hms(hour, minute, 0).ok()?;

    Some(date.with_time(tod).assume_utc())
}

/// Parse an org-mode priority marker (the letter inside `[#X]`) into
/// the wire `Task.priority` integer. Mapping: A→1, B→2, C→3 — lower
/// numbers are higher priority, consistent with the wire protocol's
/// "0 = highest" convention.
///
/// Unrecognized markers (e.g. `[#破]` or extended `[#D]` from custom
/// `#+PRIORITIES:` declarations) return `None` rather than guessing.
/// Authors who need broader priority alphabets can fall back to the
/// legacy `:PRIORITY:` property.
fn parse_priority_marker(marker: &str) -> Option<i32> {
    match marker.trim() {
        "A" => Some(1),
        "B" => Some(2),
        "C" => Some(3),
        _ => None,
    }
}

/// Parse org-mode `:Effort:` value into minutes. Returns `None` for
/// empty / unparseable values. Lenient: accepts the canonical
/// formats (`HH:MM`, `Nm`, `Nh`, `Nd`, bare `N`) and rejects the
/// rest. See [`TaskDefinition::effort_minutes`] for the unit rules.
fn parse_effort(raw: &str) -> Option<u32> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }

    // HH:MM
    if let Some((h, m)) = s.split_once(':') {
        let h: u32 = h.trim().parse().ok()?;
        let m: u32 = m.trim().parse().ok()?;
        return Some(h * 60 + m);
    }

    // Nd → workday-hours = 8h
    if let Some(num) = s.strip_suffix('d') {
        let n: f64 = num.trim().parse().ok()?;
        if n.is_finite() && n >= 0.0 {
            return Some((n * 8.0 * 60.0) as u32);
        }
        return None;
    }
    // Nh → hours
    if let Some(num) = s.strip_suffix('h') {
        let n: f64 = num.trim().parse().ok()?;
        if n.is_finite() && n >= 0.0 {
            return Some((n * 60.0) as u32);
        }
        return None;
    }
    // Nm → minutes
    if let Some(num) = s.strip_suffix('m') {
        let n: u32 = num.trim().parse().ok()?;
        return Some(n);
    }

    // Bare integer → minutes (org-mode convention)
    s.parse().ok()
}

/// True if this headline is a task — i.e., should be exported as a
/// `:stage:` to the orchestrator board.
///
/// Two ways a headline qualifies as a task:
/// 1. **TODO keyword present** (org-mode-native discriminator,
///    canonical as of wb-0mqz.2). Any keyword from the GTD set
///    (TODO/NEXT/WAITING/DOING/SOMEDAY/DONE/CANCELED/FAILED) or
///    the legacy worg keywords (BLOCKED/ABANDONED). worg-parse's
///    `ParseConfig` extracts these reliably.
/// 2. **`:stage:` tag** (legacy worg discriminator, back-compat).
///    Existing files that haven't migrated to TODO keywords keep
///    working.
///
/// Exclusion classifications win in both cases. Per w.org,
/// classifications are mutually exclusive — a headline carries at
/// most one of `:tool:`, `:validator:`, `:input:`, `:review:`,
/// `:agent:`. If any of those is present, the headline is NOT a
/// task even if it has a TODO keyword or a `:stage:` tag (those
/// are inherited from an ancestor or set in error).
fn is_task_headline(hl: &Headline, tags: &[String]) -> bool {
    const EXCLUDED: &[&str] = &["validator", "tool", "input", "review", "agent"];
    if tags.iter().any(|t| EXCLUDED.contains(&t.as_str())) {
        return false;
    }
    hl.todo_keyword().is_some() || tags.iter().any(|t| t == "stage")
}

/// Status tag preferred; falls back to TODO keyword; defaults to `Backlog`.
///
/// Tag enum from w.org's `* Status` section is the canonical path —
/// `:pending:`/`:active:`/`:done:`/`:failed:`/`:abandoned:`.
///
/// TODO-keyword path: as of the wb-0mqz GTD migration, worg-parse
/// configures orgize with a GTD-aligned keyword set, so headlines
/// like `* NEXT Step` and `* WAITING Step` parse with the keyword
/// extracted as state. The mapping below covers the full GTD
/// vocabulary plus the legacy worg keywords (BLOCKED, ABANDONED)
/// for back-compat.
///
/// Wire-protocol note: orchestrator-core's `TaskState` enum does NOT
/// have a `Failed` variant — runs fail; tasks transition to
/// `Blocked` instead. The `FAILED` keyword preserves authorial
/// intent in the .org file while mapping to `Blocked` at the wire
/// boundary. The Failure cascade (wb-0mqz.14) is what writes the
/// blocked_reason metadata explaining *why* the task is blocked.
fn resolve_state(hl: &Headline, tags: &[String]) -> TaskState {
    for tag in tags {
        match tag.as_str() {
            "pending" => return TaskState::Ready,
            "active" => return TaskState::InProgress,
            "done" => return TaskState::Done,
            "failed" => return TaskState::Blocked,
            "abandoned" => return TaskState::Cancelled,
            _ => {}
        }
    }
    if let Some(kw) = hl.todo_keyword() {
        match kw.to_string().as_str() {
            // GTD-aligned vocabulary (wb-0mqz.1)
            "TODO" => return TaskState::Ready,
            "NEXT" => return TaskState::Ready,
            "WAITING" => return TaskState::Blocked,
            "DOING" => return TaskState::InProgress,
            "SOMEDAY" => return TaskState::Backlog,
            "DONE" => return TaskState::Done,
            "CANCELED" => return TaskState::Cancelled,
            // FAILED is an author-visible keyword but maps to Blocked
            // at the wire (orchestrator-core has no Failed state for
            // tasks). The blocked_reason carries the failure metadata.
            "FAILED" => return TaskState::Blocked,
            // Legacy worg keywords kept for back-compat with pre-GTD files.
            "BLOCKED" => return TaskState::Blocked,
            "ABANDONED" => return TaskState::Cancelled,
            _ => {}
        }
    }
    TaskState::Backlog
}

/// Walk backwards from `idx` looking for the nearest headline with a
/// strictly shallower level that's classified as a task (TODO keyword
/// or `:stage:` tag — see [`is_task_headline`]). Returns the index of
/// that ancestor in `summary` (so callers can look up not just its id
/// but also its `ordered` flag — see wb-0mqz.5).
fn nearest_task_ancestor_idx(summary: &[HeadlineSummary], idx: usize) -> Option<usize> {
    let cur_level = summary[idx].level;
    for i in (0..idx).rev() {
        if summary[i].level < cur_level && summary[i].is_task {
            return Some(i);
        }
    }
    None
}

/// Find the immediate-predecessor sibling task of `idx` — the most
/// recent task in document order that shares the same outline parent.
/// Returns `None` if `idx` is the first task under its parent (or has
/// no parent and no earlier top-level tasks).
///
/// Walk-back algorithm:
/// - skip headlines at strictly deeper level (descendants of a
///   sibling — those are NOT direct siblings of `idx`)
/// - skip headlines that aren't tasks
/// - return the first task at the SAME level we encounter
/// - stop and return `None` on a strictly shallower headline
///   (we've left `idx`'s subtree)
fn immediate_predecessor_sibling_task(
    summary: &[HeadlineSummary],
    idx: usize,
) -> Option<TaskId> {
    let cur_level = summary[idx].level;
    for i in (0..idx).rev() {
        let item = &summary[i];
        if item.level > cur_level {
            // descendant of a prior sibling — keep walking
            continue;
        }
        if item.level < cur_level {
            // we've left the subtree; no sibling exists before us
            return None;
        }
        // same level
        if item.is_task {
            return Some(TaskId::new(item.id.clone()));
        }
        // same level but not a task (e.g. documentation headline
        // intermixed with tasks); skip and keep looking
    }
    None
}

/// Read a headline's `:ORDERED:` property and interpret it leniently
/// as a boolean. Truthy values: `t`, `true`, `yes`, `1` (case-
/// insensitive). Anything else (including absent property) is false.
/// Matches Emacs org-mode's own loose truthiness for ORDERED.
fn read_ordered_property(hl: &Headline) -> bool {
    let Some(props) = hl.properties() else {
        return false;
    };
    let Some(raw) = props.get("ORDERED") else {
        return false;
    };
    let v = raw.to_string();
    let v = v.trim().to_ascii_lowercase();
    matches!(v.as_str(), "t" | "true" | "yes" | "1")
}

/// Tags not surfaced as `Task.tags` — runtime/state tags are noise in
/// the wire payload because they're already captured in `Task.state`.
fn filter_runtime_tags(tags: &[String]) -> Vec<String> {
    const STATUS: &[&str] = &["pending", "active", "done", "failed", "abandoned"];
    tags.iter()
        .filter(|t| !STATUS.contains(&t.as_str()))
        .cloned()
        .collect()
}

/// Parse a whitespace-separated list of `[[id:foo]]` link references
/// into bare ids. Tolerates trailing punctuation and stray text.
fn parse_blocker(value: &str) -> Vec<TaskId> {
    let mut out = Vec::new();
    let mut rest = value;
    while let Some(start) = rest.find("[[id:") {
        rest = &rest[start + 5..];
        if let Some(end) = rest.find("]]") {
            let id = rest[..end].trim();
            if !id.is_empty() {
                out.push(TaskId::new(id.to_string()));
            }
            rest = &rest[end + 2..];
        } else {
            break;
        }
    }
    out
}

/// Slugify a title for use as a fallback `:ID:`. Lowercases ASCII,
/// replaces whitespace with `-`, drops anything not in `[a-z0-9._-]`.
fn slugify(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    for ch in title.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() || lower == '.' || lower == '_' || lower == '-' {
            out.push(lower);
        } else if lower.is_whitespace() {
            out.push('-');
        }
    }
    if out.is_empty() {
        "untitled".to_string()
    } else {
        out
    }
}

/// Return the section body text of a headline (the prose between the
/// headline title and any child headlines), excluding the property
/// drawer. None if empty.
fn section_text(hl: &Headline) -> Option<String> {
    let section = hl.section()?;
    // Section text includes the property drawer if present; strip
    // ":PROPERTIES: ... :END:" before returning.
    let raw = section.syntax().text().to_string();
    let stripped = strip_property_drawer(&raw);
    let trimmed = stripped.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn strip_property_drawer(s: &str) -> String {
    let lines = s.lines().collect::<Vec<_>>();
    let mut out_lines = Vec::with_capacity(lines.len());
    let mut in_drawer = false;
    for line in lines {
        let trimmed = line.trim();
        if !in_drawer && trimmed.eq_ignore_ascii_case(":PROPERTIES:") {
            in_drawer = true;
            continue;
        }
        if in_drawer && trimmed.eq_ignore_ascii_case(":END:") {
            in_drawer = false;
            continue;
        }
        if !in_drawer {
            out_lines.push(line);
        }
    }
    out_lines.join("\n")
}

fn headline_id_or_title_slug(hl: &Headline) -> String {
    hl.properties()
        .and_then(|p| p.get("ID"))
        .map(|t| t.to_string())
        .unwrap_or_else(|| slugify(hl.title_raw().trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKHORSE_FIXTURE: &str = "\
* Workhorse                                                          :agent:
:PROPERTIES:
:ID:           workhorse
:TYPE:         ai
:MODEL:        openrouter/xiaomi/mimo-v2.5-pro
:CAPABILITIES: bash read write lua-eval js-eval git worg network
:TOOLS:        bash read write lua_eval substrate_push worg_parse worg_query worg_mutate
:END:

Intro prose under the agent — should not be confused with the system prompt.

** System prompt
You are Workhorse, the everyday Workbooks agent.

*** Path convention
Your VFS root =/= IS the working directory root.

*** Scope
Single-file =.html= workbooks.
";

    #[test]
    fn extracts_workhorse_wire_fields() {
        let doc = worg_parse::Document::parse(WORKHORSE_FIXTURE);
        let defs = agent_definitions(&doc);
        assert_eq!(defs.len(), 1, "should find exactly one agent");
        let a = &defs[0];
        assert_eq!(a.wire.id.as_str(), "workhorse");
        assert_eq!(a.wire.name, "Workhorse");
        assert_eq!(a.wire.kind, AgentType::Ai);
        assert_eq!(a.wire.status, AgentStatus::Active);
        assert_eq!(
            a.wire.capabilities,
            vec!["bash", "read", "write", "lua-eval", "js-eval", "git", "worg", "network"]
        );
    }

    #[test]
    fn extracts_workhorse_richer_fields() {
        let doc = worg_parse::Document::parse(WORKHORSE_FIXTURE);
        let a = &agent_definitions(&doc)[0];
        assert_eq!(
            a.model.as_deref(),
            Some("openrouter/xiaomi/mimo-v2.5-pro")
        );
        assert_eq!(
            a.tools,
            vec![
                "bash",
                "read",
                "write",
                "lua_eval",
                "substrate_push",
                "worg_parse",
                "worg_query",
                "worg_mutate"
            ]
        );
    }

    #[test]
    fn extracts_system_prompt_with_descendants() {
        let doc = worg_parse::Document::parse(WORKHORSE_FIXTURE);
        let prompt = agent_definitions(&doc)[0]
            .system_prompt
            .clone()
            .expect("system_prompt should be populated");
        assert!(prompt.contains("You are Workhorse"));
        assert!(prompt.contains("*** Path convention"));
        assert!(prompt.contains("Your VFS root"));
        assert!(prompt.contains("*** Scope"));
        assert!(
            !prompt.starts_with("** System prompt"),
            "title line should be stripped: {prompt:?}"
        );
    }

    #[test]
    fn id_falls_back_to_lowercased_title_when_missing() {
        let src = "* CustomAgent :agent:\n:PROPERTIES:\n:TYPE: ai\n:END:\n";
        let doc = worg_parse::Document::parse(src);
        let defs = agent_definitions(&doc);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].wire.id.as_str(), "customagent");
        assert_eq!(defs[0].wire.name, "CustomAgent");
    }

    #[test]
    fn agent_definition_by_id_finds_match_and_misses_correctly() {
        let doc = worg_parse::Document::parse(WORKHORSE_FIXTURE);
        assert!(agent_definition_by_id(&doc, "workhorse").is_some());
        assert!(agent_definition_by_id(&doc, "ghost").is_none());
    }

    #[test]
    fn type_human_recognized() {
        let src = "* Operator :agent:\n:PROPERTIES:\n:ID: shane\n:TYPE: human\n:END:\n";
        let doc = worg_parse::Document::parse(src);
        assert_eq!(agent_definitions(&doc)[0].wire.kind, AgentType::Human);
    }

    #[test]
    fn missing_type_defaults_to_ai() {
        let src = "* Mystery :agent:\n:PROPERTIES:\n:ID: mystery\n:END:\n";
        let doc = worg_parse::Document::parse(src);
        assert_eq!(agent_definitions(&doc)[0].wire.kind, AgentType::Ai);
    }

    #[test]
    fn non_agent_level1_headlines_skipped() {
        let src = "* Status :enum:\n\
                   ** pending\n\
                   ** active\n\
                   * Workhorse :agent:\n\
                   :PROPERTIES:\n:ID: workhorse\n:TYPE: ai\n:END:\n";
        let doc = worg_parse::Document::parse(src);
        let defs = agent_definitions(&doc);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].wire.id.as_str(), "workhorse");
    }

    #[test]
    fn multiple_agents_returned_in_document_order() {
        let src = "* First :agent:\n:PROPERTIES:\n:ID: first\n:TYPE: ai\n:END:\n\
                   * Second :agent:\n:PROPERTIES:\n:ID: second\n:TYPE: human\n:END:\n";
        let doc = worg_parse::Document::parse(src);
        let defs = agent_definitions(&doc);
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].wire.id.as_str(), "first");
        assert_eq!(defs[1].wire.id.as_str(), "second");
    }

    #[test]
    fn agents_file_wraps_with_protocol_version() {
        let doc = worg_parse::Document::parse(WORKHORSE_FIXTURE);
        let file = agents_file(&doc);
        assert_eq!(file.version, ProtocolVersion(PROTOCOL_VERSION));
        assert_eq!(file.agents.len(), 1);
        let json = serde_json::to_string(&file).unwrap();
        // Wire fields present.
        assert!(json.contains("\"id\":\"workhorse\""));
        assert!(json.contains("\"type\":\"ai\""));
        assert!(json.contains("\"status\":\"active\""));
        // Application-layer fields MUST NOT leak into the wire export.
        // The orchestrator-protocol Agent has no model/tools/system_prompt
        // fields — they live in the WORG source-of-truth.
        assert!(!json.contains("openrouter"), "model leaked into wire JSON");
        assert!(!json.contains("system_prompt"), "system_prompt leaked");
        assert!(!json.contains("substrate_push"), "tools leaked");
    }

    #[test]
    fn system_prompt_absent_returns_none() {
        let src = "* Bare :agent:\n:PROPERTIES:\n:ID: bare\n:TYPE: ai\n:END:\n";
        let doc = worg_parse::Document::parse(src);
        assert!(agent_definitions(&doc)[0].system_prompt.is_none());
    }

    #[test]
    fn system_prompt_search_stops_at_next_level1() {
        // The walker must not pick up a "** System prompt" that
        // belongs to a different agent.
        let src = "* First :agent:\n\
                   :PROPERTIES:\n:ID: first\n:TYPE: ai\n:END:\n\
                   * Second :agent:\n\
                   :PROPERTIES:\n:ID: second\n:TYPE: ai\n:END:\n\
                   ** System prompt\n\
                   This belongs to second, not first.\n";
        let doc = worg_parse::Document::parse(src);
        let defs = agent_definitions(&doc);
        assert!(defs[0].system_prompt.is_none(), "first agent should have no prompt");
        assert!(
            defs[1]
                .system_prompt
                .as_ref()
                .map(|p| p.contains("This belongs to second"))
                .unwrap_or(false),
            "second agent should have the prompt"
        );
    }

    // ───── Task walker tests ───────────────────────────────────────────

    /// Standard ExportOpts for tests — fixed timestamp so the fixture
    /// is deterministic.
    fn test_opts() -> ExportOpts {
        ExportOpts {
            created_by: AgentId::new("worg-exporter"),
            exported_at: time::macros::datetime!(2026-05-23 20:00:00 UTC),
        }
    }

    const TASK_DAG_FIXTURE: &str = "\
* Iteration                                                          :stage:
:PROPERTIES:
:ID:             autoloop-iteration
:ASSIGNED_AGENT: workhorse
:BUDGET:         tokens=200000 cost_usd=2.00 wallclock=600s
:RETRY_POLICY:   max=3 backoff=fixed fallback=mark_blocked
:END:

The top-level stage. Carries budget + retry for the whole loop.

** orient                                                            :stage:
:PROPERTIES:
:ID: orient
:END:

Pre-flight check.

*** clean working tree                                          :validator:
:PROPERTIES:
:KIND: cmd_zero_exit
:ARG_CMD: test -z \"$(git status --short)\"
:END:

** pick issue                                                        :stage:
:PROPERTIES:
:ID:         pick-issue
:BLOCKER: [[id:orient]]
:END:

** claim issue                                                       :stage:
:PROPERTIES:
:ID:         claim-issue
:BLOCKER: [[id:pick-issue]]
:END:
";

    #[test]
    fn task_definitions_emits_one_per_stage_only() {
        let doc = worg_parse::Document::parse(TASK_DAG_FIXTURE);
        let tasks = task_definitions(&doc, &test_opts());
        // Four :stage: headlines (Iteration + orient + pick issue +
        // claim issue). One :validator: child (clean working tree) is
        // NOT emitted as a task.
        assert_eq!(tasks.len(), 4);
        let ids: Vec<&str> = tasks.iter().map(|t| t.wire.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["autoloop-iteration", "orient", "pick-issue", "claim-issue"]
        );
    }

    #[test]
    fn task_state_defaults_to_backlog_when_no_keyword_or_status_tag() {
        let doc = worg_parse::Document::parse(TASK_DAG_FIXTURE);
        let tasks = task_definitions(&doc, &test_opts());
        for t in &tasks {
            assert_eq!(t.wire.state, TaskState::Backlog);
        }
    }

    #[test]
    fn task_state_uses_status_tag_when_present() {
        let src = "* Step :stage:active:\n:PROPERTIES:\n:ID: step\n:END:\n";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks[0].wire.state, TaskState::InProgress);
    }

    #[test]
    fn task_state_falls_back_to_todo_keyword_default_set() {
        // worg-parse configures orgize with a GTD-aligned keyword set
        // (wb-0mqz.1), so headlines with NEXT/WAITING/DOING/etc parse
        // with the keyword extracted. This test covers the canonical
        // TODO case.
        let src = "* TODO Step :stage:\n:PROPERTIES:\n:ID: step\n:END:\n";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks[0].wire.state, TaskState::Ready);
    }

    #[test]
    fn task_state_done_keyword_maps_to_done() {
        let src = "* DONE Step :stage:\n:PROPERTIES:\n:ID: step\n:END:\n";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks[0].wire.state, TaskState::Done);
    }

    // ── GTD TODO keyword coverage (wb-0mqz.1) ────────────────────────

    #[test]
    fn task_state_next_keyword_maps_to_ready() {
        let src = "* NEXT Step :stage:\n:PROPERTIES:\n:ID: step\n:END:\n";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks[0].wire.state, TaskState::Ready);
        // And the title is just "Step", not "NEXT Step" — proves
        // worg-parse's ParseConfig extracted the keyword.
        assert_eq!(tasks[0].wire.title, "Step");
    }

    #[test]
    fn task_state_waiting_keyword_maps_to_blocked() {
        let src = "* WAITING Step :stage:\n:PROPERTIES:\n:ID: step\n:END:\n";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks[0].wire.state, TaskState::Blocked);
        assert_eq!(tasks[0].wire.title, "Step");
    }

    #[test]
    fn task_state_doing_keyword_maps_to_in_progress() {
        let src = "* DOING Step :stage:\n:PROPERTIES:\n:ID: step\n:END:\n";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks[0].wire.state, TaskState::InProgress);
        assert_eq!(tasks[0].wire.title, "Step");
    }

    #[test]
    fn task_state_someday_keyword_maps_to_backlog() {
        let src = "* SOMEDAY Step :stage:\n:PROPERTIES:\n:ID: step\n:END:\n";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks[0].wire.state, TaskState::Backlog);
        assert_eq!(tasks[0].wire.title, "Step");
    }

    #[test]
    fn task_state_canceled_keyword_maps_to_cancelled() {
        let src = "* CANCELED Step :stage:\n:PROPERTIES:\n:ID: step\n:END:\n";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks[0].wire.state, TaskState::Cancelled);
        assert_eq!(tasks[0].wire.title, "Step");
    }

    #[test]
    fn task_state_failed_keyword_maps_to_blocked_at_wire() {
        // FAILED is GTD-recognized at the keyword level but
        // orchestrator-core's TaskState has no Failed variant — runs
        // fail; tasks transition to Blocked. The keyword preserves
        // authorial intent in the .org file.
        let src = "* FAILED Step :stage:\n:PROPERTIES:\n:ID: step\n:END:\n";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks[0].wire.state, TaskState::Blocked);
        assert_eq!(tasks[0].wire.title, "Step");
    }

    #[test]
    fn task_state_legacy_blocked_keyword_still_maps_to_blocked() {
        // Back-compat: pre-GTD worg files using BLOCKED keep working.
        let src = "* BLOCKED Step :stage:\n:PROPERTIES:\n:ID: step\n:END:\n";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks[0].wire.state, TaskState::Blocked);
    }

    #[test]
    fn task_state_legacy_abandoned_keyword_still_maps_to_cancelled() {
        let src = "* ABANDONED Step :stage:\n:PROPERTIES:\n:ID: step\n:END:\n";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks[0].wire.state, TaskState::Cancelled);
    }

    // ── Task discriminator: TODO keyword OR :stage: (wb-0mqz.2) ─────

    #[test]
    fn headline_with_todo_keyword_and_no_stage_tag_is_a_task() {
        // The GTD-native case: headline has a TODO keyword and no
        // :stage: tag. Should still be exported as a task.
        let src = "* NEXT Step\n:PROPERTIES:\n:ID: step\n:END:\n";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].wire.id.as_str(), "step");
        assert_eq!(tasks[0].wire.title, "Step");
        assert_eq!(tasks[0].wire.state, TaskState::Ready);
    }

    #[test]
    fn headline_with_stage_tag_and_no_todo_keyword_is_still_a_task() {
        // Back-compat: pre-GTD files with `:stage:` but no keyword
        // continue to be exported.
        let src = "* Step :stage:\n:PROPERTIES:\n:ID: step\n:END:\n";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].wire.id.as_str(), "step");
    }

    #[test]
    fn headline_with_neither_todo_keyword_nor_stage_tag_is_documentation() {
        // The whole point of the migration: documentation headlines
        // (no TODO keyword, no :stage:) are NOT exported.
        let src = "* Just a heading\n\nProse here.\n";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks.len(), 0);
    }

    #[test]
    fn excluded_classifications_block_task_emission_even_with_todo_keyword() {
        // A `:tool:` headline with a TODO keyword is still a tool
        // definition, not a task. The exclusion classifications win.
        let src = "* TODO run something :tool:\n:PROPERTIES:\n:ID: t\n:END:\n";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks.len(), 0);
    }

    #[test]
    fn excluded_classifications_block_task_emission_even_with_stage_tag() {
        // Same as above but for the legacy back-compat path.
        let src = "* run something :stage:tool:\n:PROPERTIES:\n:ID: t\n:END:\n";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks.len(), 0);
    }

    #[test]
    fn mixed_plan_with_documentation_and_tasks_works() {
        // A realistic case: docs intermixed with tasks.
        let src = "\
* Some context heading

This is just narrative.

* TODO Real task
:PROPERTIES:
:ID: t1
:END:

* Another doc heading

* DOING In-progress task
:PROPERTIES:
:ID: t2
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        let ids: Vec<_> = tasks.iter().map(|t| t.wire.id.as_str()).collect();
        assert_eq!(ids, vec!["t1", "t2"]);
    }

    // ── Tag inheritance (wb-0mqz.11, org-mode default) ───────────────

    #[test]
    fn child_inherits_non_excluded_tag_from_parent() {
        // Parent has :sandboxed: — a domain tag not in the default
        // exclusion list. Child should see it via inheritance.
        let src = "* TODO Iteration :sandboxed:
:PROPERTIES:
:ID: it
:END:

** TODO orient
:PROPERTIES:
:ID: orient
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        let orient = tasks.iter().find(|t| t.wire.id.as_str() == "orient").unwrap();
        assert!(
            orient.wire.tags.contains(&"sandboxed".to_string()),
            "child should inherit `sandboxed` from parent, got: {:?}",
            orient.wire.tags
        );
    }

    #[test]
    fn child_does_not_inherit_classification_tags_from_parent() {
        // Classification tags (:stage:, :tool:, :validator:, etc.) are
        // per-headline by intent. A child under a `:stage:` parent
        // shouldn't be classified as a stage just because of
        // inheritance.
        let src = "* TODO Iteration :stage:
:PROPERTIES:
:ID: it
:END:

** TODO leaf
:PROPERTIES:
:ID: leaf
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        let leaf = tasks.iter().find(|t| t.wire.id.as_str() == "leaf").unwrap();
        assert!(
            !leaf.wire.tags.contains(&"stage".to_string()),
            "child must NOT inherit classification `stage`, got: {:?}",
            leaf.wire.tags
        );
    }

    #[test]
    fn multi_level_inheritance_propagates_through_outline() {
        // :urgent: at root → grandchild inherits it.
        let src = "* TODO Grandparent :urgent:
:PROPERTIES:
:ID: gp
:END:

** TODO Parent
:PROPERTIES:
:ID: p
:END:

*** TODO Grandchild
:PROPERTIES:
:ID: gc
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        let gc = tasks.iter().find(|t| t.wire.id.as_str() == "gc").unwrap();
        assert!(
            gc.wire.tags.contains(&"urgent".to_string()),
            "grandchild should inherit `urgent` from grandparent, got: {:?}",
            gc.wire.tags
        );
    }

    #[test]
    fn tag_declared_at_multiple_levels_dedupes() {
        // Parent and child both have :sandboxed:. Should appear once.
        let src = "* TODO Iteration :sandboxed:
:PROPERTIES:
:ID: it
:END:

** TODO leaf :sandboxed:
:PROPERTIES:
:ID: leaf
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        let leaf = tasks.iter().find(|t| t.wire.id.as_str() == "leaf").unwrap();
        let count = leaf
            .wire
            .tags
            .iter()
            .filter(|t| t.as_str() == "sandboxed")
            .count();
        assert_eq!(count, 1, "tag should de-dupe, got: {:?}", leaf.wire.tags);
    }

    #[test]
    fn tag_exclude_inherit_keyword_blocks_specific_tags() {
        // Author declares #+TAG_EXCLUDE_INHERIT: secret. The
        // :secret: tag on the parent should NOT cascade to the child.
        let src = "#+TAG_EXCLUDE_INHERIT: secret

* TODO Parent :secret:
:PROPERTIES:
:ID: p
:END:

** TODO Child
:PROPERTIES:
:ID: c
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        let child = tasks.iter().find(|t| t.wire.id.as_str() == "c").unwrap();
        assert!(
            !child.wire.tags.contains(&"secret".to_string()),
            "child must NOT inherit `secret` (excluded), got: {:?}",
            child.wire.tags
        );
    }

    #[test]
    fn excluded_tag_still_present_on_own_headline() {
        // Exclusion only affects INHERITANCE — the headline that
        // declares the tag still carries it.
        let src = "#+TAG_EXCLUDE_INHERIT: secret

* TODO Parent :secret:
:PROPERTIES:
:ID: p
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        let parent = tasks.iter().find(|t| t.wire.id.as_str() == "p").unwrap();
        assert!(
            parent.wire.tags.contains(&"secret".to_string()),
            "own tag should be preserved even with exclusion, got: {:?}",
            parent.wire.tags
        );
    }

    // ── DEADLINE: timestamp (wb-0mqz.10, org-mode core) ──────────────

    #[test]
    fn deadline_date_only_maps_to_midnight_utc() {
        let src = "* TODO Ship release
DEADLINE: <2026-06-01 Mon>
:PROPERTIES:
:ID: ship
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        let due = tasks[0].wire.due.expect("due should be set");

        use time::format_description::well_known::Rfc3339;
        assert_eq!(due.format(&Rfc3339).unwrap(), "2026-06-01T00:00:00Z");
    }

    #[test]
    fn deadline_with_time_maps_to_that_time() {
        let src = "* TODO Webinar
DEADLINE: <2026-06-01 Mon 14:30>
:PROPERTIES:
:ID: webinar
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        let due = tasks[0].wire.due.expect("due should be set");

        use time::format_description::well_known::Rfc3339;
        assert_eq!(due.format(&Rfc3339).unwrap(), "2026-06-01T14:30:00Z");
    }

    #[test]
    fn no_deadline_yields_none() {
        let src = "* TODO Step
:PROPERTIES:
:ID: step
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks[0].wire.due, None);
    }

    #[test]
    fn scheduled_alone_does_not_populate_due() {
        // SCHEDULED: is informational ("ready to start on") at the
        // org-mode level. The wire `due` field is hard-deadline-only;
        // SCHEDULED: must not bleed into it.
        let src = "* TODO Step
SCHEDULED: <2026-06-01 Mon>
:PROPERTIES:
:ID: step
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks[0].wire.due, None);
    }

    #[test]
    fn scheduled_and_deadline_only_deadline_wins() {
        let src = "* TODO Step
DEADLINE: <2026-06-15 Mon> SCHEDULED: <2026-06-01 Mon>
:PROPERTIES:
:ID: step
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        let due = tasks[0].wire.due.expect("DEADLINE should populate due");

        use time::format_description::well_known::Rfc3339;
        assert_eq!(due.format(&Rfc3339).unwrap(), "2026-06-15T00:00:00Z");
    }

    // ── [#A] priority syntax (wb-0mqz.9, org-mode core) ──────────────

    #[test]
    fn priority_inline_marker_maps_letter_to_int() {
        for (letter, want) in &[("A", 1i32), ("B", 2), ("C", 3)] {
            let src = format!(
                "* TODO [#{letter}] Step
:PROPERTIES:
:ID: step
:END:
"
            );
            let doc = worg_parse::Document::parse(&src);
            let tasks = task_definitions(&doc, &test_opts());
            assert_eq!(
                tasks[0].wire.priority,
                Some(*want),
                "[#{letter}] should map to {want}"
            );
            assert_eq!(tasks[0].wire.title, "Step", "marker should not bleed into title");
        }
    }

    #[test]
    fn priority_absent_yields_none() {
        let src = "* TODO Step
:PROPERTIES:
:ID: step
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks[0].wire.priority, None);
    }

    #[test]
    fn priority_property_fallback_when_no_inline_marker() {
        // Back-compat: legacy :PRIORITY: 2 still works when no [#X]
        // marker is present on the headline.
        let src = "* TODO Step
:PROPERTIES:
:ID: step
:PRIORITY: 2
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks[0].wire.priority, Some(2));
    }

    #[test]
    fn priority_inline_marker_wins_over_property() {
        // Both declared → inline wins (the source of truth going
        // forward). The :PRIORITY: property is the legacy path.
        let src = "* TODO [#A] Step
:PROPERTIES:
:ID: step
:PRIORITY: 9
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks[0].wire.priority, Some(1));
    }

    #[test]
    fn priority_unrecognized_marker_does_not_panic() {
        // Custom #+PRIORITIES: alphabets could declare [#D] / [#E] /
        // etc. We accept only A/B/C in v1 — anything else yields
        // None rather than guessing. Authors can use :PRIORITY: for
        // a wider numeric range.
        let src = "* TODO [#D] Step
:PROPERTIES:
:ID: step
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks[0].wire.priority, None);
    }

    // ── :Effort: property (wb-0mqz.8, org-mode core) ─────────────────

    #[test]
    fn effort_hh_mm_format() {
        for (raw, want) in &[("0:30", 30u32), ("1:00", 60), ("1:30", 90), ("12:45", 12 * 60 + 45)] {
            let src = format!(
                "* TODO Step
:PROPERTIES:
:ID: step
:Effort:  {raw}
:END:
"
            );
            let doc = worg_parse::Document::parse(&src);
            let tasks = task_definitions(&doc, &test_opts());
            assert_eq!(
                tasks[0].effort_minutes,
                Some(*want),
                ":Effort: `{raw}` should be {want} minutes"
            );
        }
    }

    #[test]
    fn effort_suffix_formats() {
        for (raw, want) in &[
            ("30m", 30u32),
            ("90m", 90),
            ("1h", 60),
            ("2h", 120),
            ("1.5h", 90),
            ("1d", 8 * 60),
            ("2d", 2 * 8 * 60),
            ("0.5d", 4 * 60),
        ] {
            let src = format!(
                "* TODO Step
:PROPERTIES:
:ID: step
:Effort:  {raw}
:END:
"
            );
            let doc = worg_parse::Document::parse(&src);
            let tasks = task_definitions(&doc, &test_opts());
            assert_eq!(
                tasks[0].effort_minutes,
                Some(*want),
                ":Effort: `{raw}` should be {want} minutes"
            );
        }
    }

    #[test]
    fn effort_bare_number_treated_as_minutes() {
        let src = "* TODO Step
:PROPERTIES:
:ID: step
:Effort: 45
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks[0].effort_minutes, Some(45));
    }

    #[test]
    fn effort_absent_yields_none() {
        let src = "* TODO Step
:PROPERTIES:
:ID: step
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks[0].effort_minutes, None);
    }

    #[test]
    fn effort_unparseable_value_yields_none() {
        // Lenient parser: garbage in → None, no panic.
        for raw in &["garbage", "not:a:time", "abc:def", "h", ":15"] {
            let src = format!(
                "* TODO Step
:PROPERTIES:
:ID: step
:Effort: {raw}
:END:
"
            );
            let doc = worg_parse::Document::parse(&src);
            let tasks = task_definitions(&doc, &test_opts());
            assert_eq!(
                tasks[0].effort_minutes, None,
                ":Effort: `{raw}` should be unparseable → None"
            );
        }
    }

    // ── :TRIGGER: property (wb-0mqz.4, org-edna) ─────────────────────

    #[test]
    fn trigger_property_parses_into_definition() {
        let src = "* TODO A
:PROPERTIES:
:ID: a
:TRIGGER: [[id:b]] [[id:c]]
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks.len(), 1);
        let triggers: Vec<_> = tasks[0].trigger.iter().map(|t| t.as_str()).collect();
        assert_eq!(triggers, vec!["b", "c"]);
    }

    #[test]
    fn trigger_absent_yields_empty_vec() {
        let src = "* TODO A
:PROPERTIES:
:ID: a
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert!(tasks[0].trigger.is_empty());
    }

    #[test]
    fn trigger_independent_of_blocker() {
        // Same task may carry both; both should be extracted independently.
        let src = "* TODO A
:PROPERTIES:
:ID: a
:BLOCKER: [[id:prereq]]
:TRIGGER: [[id:dependent]]
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(
            tasks[0].blocker.iter().map(|t| t.as_str()).collect::<Vec<_>>(),
            vec!["prereq"]
        );
        assert_eq!(
            tasks[0].trigger.iter().map(|t| t.as_str()).collect::<Vec<_>>(),
            vec!["dependent"]
        );
    }

    // ── ORDERED property (wb-0mqz.5, org-mode core) ──────────────────

    #[test]
    fn ordered_parent_injects_synthetic_blocker_to_predecessor_sibling() {
        let src = "\
* TODO Iteration
:PROPERTIES:
:ID: it
:ORDERED: t
:END:

** TODO one
:PROPERTIES:
:ID: one
:END:

** TODO two
:PROPERTIES:
:ID: two
:END:

** TODO three
:PROPERTIES:
:ID: three
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());

        // Root has no predecessor sibling — empty blocker list.
        let it = tasks.iter().find(|t| t.wire.id.as_str() == "it").unwrap();
        assert_eq!(it.blocker.len(), 0);

        // First child also empty (no preceding sibling).
        let one = tasks.iter().find(|t| t.wire.id.as_str() == "one").unwrap();
        assert_eq!(one.blocker.len(), 0);

        // Second child blocked by first.
        let two = tasks.iter().find(|t| t.wire.id.as_str() == "two").unwrap();
        let two_deps: Vec<_> = two.blocker.iter().map(|d| d.as_str()).collect();
        assert_eq!(two_deps, vec!["one"]);

        // Third child blocked by SECOND (immediate predecessor), not
        // first — strict immediate-predecessor semantics.
        let three = tasks
            .iter()
            .find(|t| t.wire.id.as_str() == "three")
            .unwrap();
        let three_deps: Vec<_> = three.blocker.iter().map(|d| d.as_str()).collect();
        assert_eq!(three_deps, vec!["two"]);
    }

    #[test]
    fn non_ordered_parent_injects_no_synthetic_edges() {
        let src = "\
* TODO Iteration
:PROPERTIES:
:ID: it
:END:

** TODO one
:PROPERTIES:
:ID: one
:END:

** TODO two
:PROPERTIES:
:ID: two
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        let two = tasks.iter().find(|t| t.wire.id.as_str() == "two").unwrap();
        assert_eq!(two.blocker.len(), 0);
    }

    #[test]
    fn ordered_parent_plus_explicit_blocker_unions() {
        // ORDERED gives `two` a synthetic edge to `one`; the explicit
        // :BLOCKER: on `two` adds an edge to `extra`. Both contribute.
        let src = "\
* TODO Iteration
:PROPERTIES:
:ID: it
:ORDERED: t
:END:

** TODO one
:PROPERTIES:
:ID: one
:END:

** TODO two
:PROPERTIES:
:ID: two
:BLOCKER: [[id:extra]]
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        let two = tasks.iter().find(|t| t.wire.id.as_str() == "two").unwrap();
        let mut deps: Vec<_> = two.blocker.iter().map(|d| d.as_str().to_string()).collect();
        deps.sort();
        assert_eq!(deps, vec!["extra", "one"]);
    }

    #[test]
    fn ordered_parent_with_explicit_blocker_matching_predecessor_does_not_duplicate() {
        // Author wrote the explicit :BLOCKER: link themselves; ORDERED
        // would also infer it. Don't emit it twice.
        let src = "\
* TODO Iteration
:PROPERTIES:
:ID: it
:ORDERED: t
:END:

** TODO one
:PROPERTIES:
:ID: one
:END:

** TODO two
:PROPERTIES:
:ID: two
:BLOCKER: [[id:one]]
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        let two = tasks.iter().find(|t| t.wire.id.as_str() == "two").unwrap();
        let deps: Vec<_> = two.blocker.iter().map(|d| d.as_str()).collect();
        assert_eq!(deps, vec!["one"]);
    }

    #[test]
    fn ordered_property_lenient_truthy_parsing() {
        // All these values should be treated as true.
        for val in ["t", "true", "True", "TRUE", "yes", "1"] {
            let src = format!(
                "\
* TODO Parent
:PROPERTIES:
:ID: p
:ORDERED: {val}
:END:

** TODO a
:PROPERTIES:
:ID: a
:END:

** TODO b
:PROPERTIES:
:ID: b
:END:
"
            );
            let doc = worg_parse::Document::parse(&src);
            let tasks = task_definitions(&doc, &test_opts());
            let b = tasks.iter().find(|t| t.wire.id.as_str() == "b").unwrap();
            assert_eq!(
                b.blocker.iter().map(|d| d.as_str()).collect::<Vec<_>>(),
                vec!["a"],
                "ORDERED value `{val}` should be truthy"
            );
        }
    }

    #[test]
    fn ordered_property_falsy_values_treated_as_not_ordered() {
        // Falsy: "nil" (Emacs convention), "false", "no", "0", empty.
        for val in ["nil", "false", "no", "0"] {
            let src = format!(
                "\
* TODO Parent
:PROPERTIES:
:ID: p
:ORDERED: {val}
:END:

** TODO a
:PROPERTIES:
:ID: a
:END:

** TODO b
:PROPERTIES:
:ID: b
:END:
"
            );
            let doc = worg_parse::Document::parse(&src);
            let tasks = task_definitions(&doc, &test_opts());
            let b = tasks.iter().find(|t| t.wire.id.as_str() == "b").unwrap();
            assert!(
                b.blocker.is_empty(),
                "ORDERED value `{val}` should be falsy (no synthetic edges)"
            );
        }
    }

    #[test]
    fn ordered_skips_intermixed_documentation_sibling() {
        // A documentation headline (no TODO keyword, no :stage:) at the
        // same level between two task siblings shouldn't break the
        // predecessor chain — task `two` is still blocked by task `one`.
        let src = "\
* TODO Iteration
:PROPERTIES:
:ID: it
:ORDERED: t
:END:

** TODO one
:PROPERTIES:
:ID: one
:END:

** Just a documentation heading at the same level

Some narrative.

** TODO two
:PROPERTIES:
:ID: two
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        let two = tasks.iter().find(|t| t.wire.id.as_str() == "two").unwrap();
        assert_eq!(
            two.blocker.iter().map(|d| d.as_str()).collect::<Vec<_>>(),
            vec!["one"]
        );
    }

    #[test]
    fn nested_task_under_doc_parent_inherits_no_outline_parent() {
        // A documentation headline (no TODO keyword, no :stage:) is
        // NOT a task ancestor — the nested task should have no parent.
        let src = "\
* Doc parent

** TODO Nested task
:PROPERTIES:
:ID: child
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks.len(), 1);
        assert!(tasks[0].wire.parent.is_none());
    }

    #[test]
    fn task_parent_uses_outline_ancestry() {
        let doc = worg_parse::Document::parse(TASK_DAG_FIXTURE);
        let tasks = task_definitions(&doc, &test_opts());
        // autoloop-iteration is top-level, no parent.
        let iter = tasks.iter().find(|t| t.wire.id.as_str() == "autoloop-iteration").unwrap();
        assert!(iter.wire.parent.is_none());
        // orient is nested under autoloop-iteration.
        let orient = tasks.iter().find(|t| t.wire.id.as_str() == "orient").unwrap();
        assert_eq!(
            orient.wire.parent.as_ref().map(|p| p.as_str()),
            Some("autoloop-iteration")
        );
        // pick-issue is a sibling of orient, also nested under iteration.
        let pick = tasks.iter().find(|t| t.wire.id.as_str() == "pick-issue").unwrap();
        assert_eq!(
            pick.wire.parent.as_ref().map(|p| p.as_str()),
            Some("autoloop-iteration")
        );
    }

    #[test]
    fn blocker_captured_in_richer_field_not_parent() {
        let doc = worg_parse::Document::parse(TASK_DAG_FIXTURE);
        let tasks = task_definitions(&doc, &test_opts());
        let pick = tasks.iter().find(|t| t.wire.id.as_str() == "pick-issue").unwrap();
        // The DAG edge "pick-issue depends on orient" lives in
        // .blocker, NOT in .wire.parent (parent is outline ancestry).
        assert_eq!(
            pick.blocker.iter().map(|d| d.as_str()).collect::<Vec<_>>(),
            vec!["orient"]
        );
    }

    #[test]
    fn assigned_agent_populates_assigned_to_vec() {
        let doc = worg_parse::Document::parse(TASK_DAG_FIXTURE);
        let tasks = task_definitions(&doc, &test_opts());
        let iter = tasks
            .iter()
            .find(|t| t.wire.id.as_str() == "autoloop-iteration")
            .unwrap();
        assert_eq!(
            iter.wire.assigned_to.iter().map(|a| a.as_str()).collect::<Vec<_>>(),
            vec!["workhorse"]
        );
    }

    #[test]
    fn budget_and_retry_policy_pass_through_verbatim() {
        let doc = worg_parse::Document::parse(TASK_DAG_FIXTURE);
        let tasks = task_definitions(&doc, &test_opts());
        let iter = tasks
            .iter()
            .find(|t| t.wire.id.as_str() == "autoloop-iteration")
            .unwrap();
        assert_eq!(
            iter.budget.as_deref(),
            Some("tokens=200000 cost_usd=2.00 wallclock=600s")
        );
        assert_eq!(
            iter.retry_policy.as_deref(),
            Some("max=3 backoff=fixed fallback=mark_blocked")
        );
    }

    #[test]
    fn validators_and_tools_not_emitted_as_tasks() {
        // The fixture has a :validator: under "orient". It must not
        // appear in the task list. Also test :tool: explicitly.
        let src = "* Step :stage:\n:PROPERTIES:\n:ID: step\n:END:\n\
                   ** validator child :validator:\n:PROPERTIES:\n:KIND: cmd_zero_exit\n:END:\n\
                   ** tool child :tool:\n:PROPERTIES:\n:TOOL: fake.tool\n:END:\n";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].wire.id.as_str(), "step");
    }

    #[test]
    fn description_uses_body_prose_with_property_drawer_stripped() {
        let src = "* Step :stage:\n:PROPERTIES:\n:ID: step\n:END:\n\
                   This is the description.\n\nIt spans multiple lines.\n";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        let desc = tasks[0].wire.description.as_ref().unwrap();
        assert!(desc.contains("This is the description."));
        assert!(desc.contains("It spans multiple lines."));
        // Property drawer must be stripped from description.
        assert!(!desc.contains(":PROPERTIES:"));
        assert!(!desc.contains(":ID: step"));
        assert!(!desc.contains(":END:"));
    }

    #[test]
    fn id_falls_back_to_title_slug_when_missing() {
        let src = "* Some Cool Step :stage:\n";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks[0].wire.id.as_str(), "some-cool-step");
    }

    #[test]
    fn task_definition_by_id_finds_match() {
        let doc = worg_parse::Document::parse(TASK_DAG_FIXTURE);
        let opts = test_opts();
        assert!(task_definition_by_id(&doc, "orient", &opts).is_some());
        assert!(task_definition_by_id(&doc, "ghost", &opts).is_none());
    }

    #[test]
    fn task_state_tag_takes_precedence_over_todo_keyword() {
        // If both are present, the status tag wins. Uses TODO (which
        // orgize recognizes) + the :done: status tag to verify
        // precedence.
        let src = "* TODO Step :stage:done:\n:PROPERTIES:\n:ID: step\n:END:\n";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(tasks[0].wire.state, TaskState::Done);
    }

    #[test]
    fn tags_filtered_of_status_keywords_but_keep_classification() {
        // :stage: should survive into wire tags; :active:/:done:/etc.
        // should be filtered (they're already captured in state).
        let src = "* Step :stage:active:custom:\n:PROPERTIES:\n:ID: step\n:END:\n";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        let t = &tasks[0];
        assert!(t.wire.tags.contains(&"stage".to_string()));
        assert!(t.wire.tags.contains(&"custom".to_string()));
        assert!(!t.wire.tags.contains(&"active".to_string()));
    }

    #[test]
    fn wire_json_roundtrips_through_serde_for_task() {
        let doc = worg_parse::Document::parse(TASK_DAG_FIXTURE);
        let tasks = task_definitions(&doc, &test_opts());
        for t in &tasks {
            let json = serde_json::to_string(&t.wire).expect("serialize");
            let back: Task = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*t.wire.id.as_str(), *back.id.as_str());
            assert_eq!(t.wire.state, back.state);
        }
    }

    // wb-qwj8.3: case-insensitivity re-sweep. Every property the
    // walker reads (ID, BLOCKER, TRIGGER, EFFORT, ASSIGNED_AGENT for
    // tasks; ID, TYPE, CAPABILITIES, MODEL, TOOLS for agents) must
    // round-trip identically regardless of the case the author writes.
    // wb-0mqz.8 added the case-insensitive lookup; this test locks it
    // in across UPPER, lower, and Title cases for every read site.
    //
    // The two `get` closures in extract_task (line ~383) and
    // extract_agent (line ~119) both share the same to_ascii_uppercase
    // pattern; this test exercises both indirectly.
    #[test]
    fn property_reads_are_case_insensitive_across_all_fields() {
        // Every property in a different case variant: UPPER, lower,
        // Title, MiXeD. If any read site forgot to case-fold, the
        // corresponding field would be missing or wrong. The `:stage:`
        // tag is required — task_definitions only emits headlines that
        // wear it.
        let src = "\
* a-task :stage:
:PROPERTIES:
:ID: a-task
:Effort: 90
:blocker: [[id:other]]
:TRIGGER: [[id:downstream]]
:Assigned_Agent: workhorse
:END:
* other :stage:
:PROPERTIES:
:ID: other
:END:
* downstream :stage:
:PROPERTIES:
:ID: downstream
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        let t = tasks.iter().find(|t| t.wire.id.as_str() == "a-task").unwrap();

        assert_eq!(t.effort_minutes, Some(90), "Effort (Title case)");
        assert_eq!(
            t.blocker.iter().map(|b| b.as_str()).collect::<Vec<_>>(),
            vec!["other"],
            "blocker (lower case)"
        );
        assert_eq!(
            t.trigger.iter().map(|b| b.as_str()).collect::<Vec<_>>(),
            vec!["downstream"],
            "TRIGGER (UPPER case)"
        );
        assert_eq!(
            t.wire.assigned_to.iter().map(|a| a.as_str()).collect::<Vec<_>>(),
            vec!["workhorse"],
            "Assigned_Agent (Mixed case)"
        );
    }

    #[test]
    fn property_reads_invariant_under_three_case_variants() {
        // Three syntactically-identical files differing only in property
        // case. The extracted TaskDefinition must be equal across all
        // three.
        let upper = "* t :stage:\n:PROPERTIES:\n:ID: t\n:EFFORT: 1h\n:BLOCKER: [[id:a]]\n:END:\n* a :stage:\n:PROPERTIES:\n:ID: a\n:END:\n";
        let lower = "* t :stage:\n:PROPERTIES:\n:id: t\n:effort: 1h\n:blocker: [[id:a]]\n:END:\n* a :stage:\n:PROPERTIES:\n:id: a\n:END:\n";
        let title = "* t :stage:\n:PROPERTIES:\n:Id: t\n:Effort: 1h\n:Blocker: [[id:a]]\n:END:\n* a :stage:\n:PROPERTIES:\n:Id: a\n:END:\n";

        let extract_t = |src: &str| -> (Option<u32>, Vec<String>) {
            let doc = worg_parse::Document::parse(src);
            let tasks = task_definitions(&doc, &test_opts());
            let t = tasks.into_iter().find(|t| t.wire.id.as_str() == "t").unwrap();
            (
                t.effort_minutes,
                t.blocker.iter().map(|b| b.as_str().to_string()).collect(),
            )
        };

        let u = extract_t(upper);
        let l = extract_t(lower);
        let tc = extract_t(title);
        assert_eq!(u, l, "UPPER vs lower diverge");
        assert_eq!(u, tc, "UPPER vs Title diverge");
        assert_eq!(u.0, Some(60), "1h should parse as 60 minutes");
        assert_eq!(u.1, vec!["a"]);
    }

    // wb-6t1r: :STAGE_MODEL: property → TaskDefinition.stage_model.
    // Loop falls back to agent.model when None.
    #[test]
    fn stage_model_property_lifts_into_task_definition() {
        let src = "\
* judge-frame :stage:
:PROPERTIES:
:ID: judge-frame
:STAGE_MODEL: google/gemini-3.5-pro
:END:
* draft :stage:
:PROPERTIES:
:ID: draft
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());

        let judge = tasks.iter().find(|t| t.wire.id.as_str() == "judge-frame").unwrap();
        assert_eq!(
            judge.stage_model.as_deref(),
            Some("google/gemini-3.5-pro"),
            "judge-frame should carry STAGE_MODEL"
        );

        let draft = tasks.iter().find(|t| t.wire.id.as_str() == "draft").unwrap();
        assert_eq!(
            draft.stage_model, None,
            "draft has no STAGE_MODEL, should be None"
        );
    }

    #[test]
    fn stage_model_property_is_case_insensitive() {
        // Authors should be free to write :Stage_Model:, :stage_model:,
        // etc. — the case-fold sweep (wb-qwj8.3) means all property
        // reads through `get()` are case-insensitive.
        let src = "\
* t :stage:
:PROPERTIES:
:ID: t
:stage_model: anthropic/claude-opus-4.7
:END:
";
        let doc = worg_parse::Document::parse(src);
        let tasks = task_definitions(&doc, &test_opts());
        assert_eq!(
            tasks[0].stage_model.as_deref(),
            Some("anthropic/claude-opus-4.7"),
        );
    }
}
