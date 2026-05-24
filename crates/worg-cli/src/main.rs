//! `worg` — command-line tool. Four subcommands:
//!
//!   - `worg parse <file>` → JSON AST sketch
//!   - `worg query <file> <predicate-json>` → matching headline IDs/titles
//!   - `worg lint  <file>` → diagnostics
//!   - `worg render <file> --format=html` → static export
//!
//! All output goes to stdout. Errors and diagnostics summary go to stderr.
//! Exit code: 0 on clean lint or other commands' success, 1 on errors.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use std::fs;
use std::path::{Path, PathBuf};
use worg_lint::{Glossary, Severity};
use worg_parse::Document;
use worg_query::Predicate;

#[derive(Parser)]
#[command(name = "worg", version, about = "worg — canonical org-mode for multi-agent orchestration")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Parse a worg file and emit a JSON sketch of the document.
    Parse {
        /// Path to the `.org` file.
        file: PathBuf,
    },
    /// Run a JSON-encoded predicate over the document; list matching headlines.
    Query {
        file: PathBuf,
        /// JSON predicate. See `worg-query::Predicate` for shape.
        predicate: String,
    },
    /// Lint per WORG.md rules.
    Lint {
        file: PathBuf,
    },
    /// Render to a static format (html only for now).
    Render {
        file: PathBuf,
        #[arg(long, value_enum, default_value_t = RenderFormat::Html)]
        format: RenderFormat,
    },
    /// Bridge worg files to the Workbooks Orchestrator Protocol's
    /// `.wb-orch/` JSON state.
    Orch {
        #[command(subcommand)]
        cmd: OrchCmd,
    },
    /// List pickable (ready) tasks in the file — headlines whose state
    /// is NEXT or TODO (org-mode "actionable" states). Optionally filter
    /// by `--agent=<slug>` to list only tasks whose :ASSIGNED_AGENT:
    /// matches. Does NOT resolve :BLOCKER: dependencies — that's a
    /// higher-level concern handled by worg-orch / worg-agent.
    Ready {
        file: PathBuf,
        #[arg(long = "agent")]
        agent: Option<String>,
    },
    /// Claim a task by its :ID: — transitions to DOING and stamps
    /// :ASSIGNED_AGENT: with the given slug. Atomic file write.
    Claim {
        file: PathBuf,
        id: String,
        #[arg(long = "agent")]
        agent: Option<String>,
    },
    /// Transition a task's TODO keyword to `<state>`. State must be one
    /// of the GTD vocabulary recognized by worg-parse (TODO, NEXT,
    /// WAITING, DOING, SOMEDAY, DONE, CANCELED, FAILED). Atomic write.
    Transition {
        file: PathBuf,
        id: String,
        state: String,
    },
    /// Append a `- <entry>` line to the task's :LOGBOOK: drawer (creates
    /// the drawer if absent). Atomic write.
    Log {
        file: PathBuf,
        id: String,
        entry: String,
    },
    /// Write a `#+RESULTS:` block under the task's first source block,
    /// replacing any existing one. Use for surfacing tool output back
    /// into the org file. Atomic write.
    #[command(name = "result")]
    Result_ {
        file: PathBuf,
        id: String,
        content: String,
    },
    /// Transition the task to DONE and optionally append a `:LOGBOOK:`
    /// entry with `--reason=<text>` documenting why. Atomic write.
    Close {
        file: PathBuf,
        id: String,
        #[arg(long = "reason")]
        reason: Option<String>,
    },
}

#[derive(Subcommand)]
enum OrchCmd {
    /// Export entities from a worg file to orchestrator-protocol JSON.
    Export {
        #[command(subcommand)]
        cmd: ExportCmd,
    },
    /// Import orchestrator-protocol state back into a worg file.
    Import {
        #[command(subcommand)]
        cmd: ImportCmd,
    },
    /// wb-4vhr.21 Phase A — single-call snapshot of the whole board
    /// as one JSON blob on stdout. Replaces the `.wb-orch/{agents.json,
    /// tasks/*.json}` directory pattern for consumers that want a live
    /// read instead of a filesystem export. Shape:
    ///
    ///   { "version": 1,
    ///     "agents": [ ...wire-strict Agent... ],
    ///     "tasks":  [ ...wire-strict Task...  ] }
    Board {
        /// Path to the worg `.org` file.
        file: PathBuf,
        /// Agent slug recorded as each task's `created_by`. Same
        /// semantics as `worg orch export tasks --created-by`.
        #[arg(long = "created-by", default_value = "worg-exporter")]
        created_by: String,
        /// RFC 3339 timestamp recorded as each task's `created_at`.
        #[arg(long = "created-at")]
        created_at: Option<String>,
    },
}

#[derive(Subcommand)]
enum ImportCmd {
    /// Walk `<from>/runs/*.json`, append a `:LOGBOOK:` entry for each
    /// run to the matching org headline (by `:ID:` = `run.task`), and
    /// transition the headline's TODO keyword to match the orchestrator
    /// task state in `<from>/tasks/<id>.json` (if present). Edits the
    /// plan file in place. Idempotent: re-runs detect already-imported
    /// run ids and skip them.
    Runs {
        /// Path to the worg `.org` file to update in place.
        file: PathBuf,
        /// Orchestrator board directory. Defaults to `.wb-orch/`.
        #[arg(long = "from", default_value = ".wb-orch")]
        from: PathBuf,
        /// Print what would change without writing back.
        #[arg(long = "dry-run")]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum ExportCmd {
    /// Walk every `:agent:`-tagged level-1 headline in `<input.org>`
    /// and emit `<output-dir>/agents.json` matching the orchestrator
    /// protocol's wire format. Application-layer fields (`model`,
    /// `tools`, `system_prompt`) live in the WORG file and are NOT
    /// part of the wire export — Watershed reads those directly via
    /// the Wasmex bridge (see wb-6irl.33).
    Agents {
        /// Path to the worg `.org` file containing the agent definitions.
        file: PathBuf,
        /// Output directory. `agents.json` will be written under it
        /// (created if it doesn't exist). Typically `.wb-orch/`.
        #[arg(long = "to")]
        to: PathBuf,
    },
    /// Walk every `:stage:`-tagged headline in `<input.org>` and emit
    /// one `<output-dir>/<id>.json` per task. Validators and tools
    /// nested inside stages are NOT emitted (they're application-layer
    /// gating). Outline ancestry → `parent`; `:BLOCKER:` lives in
    /// the richer side of the export and is dropped from the wire JSON.
    Tasks {
        /// Path to the worg `.org` file containing the task DAG.
        file: PathBuf,
        /// Output directory. Each task lands at `<output-dir>/<id>.json`.
        /// Typically `.wb-orch/tasks/`.
        #[arg(long = "to")]
        to: PathBuf,
        /// Agent slug recorded as each task's `created_by`. Defaults
        /// to `worg-exporter` — override for traceability in CI.
        #[arg(long = "created-by", default_value = "worg-exporter")]
        created_by: String,
        /// RFC 3339 timestamp recorded as each task's `created_at`.
        /// Defaults to the current UTC time. Set explicitly to make
        /// exports reproducible (fixture-diff tests, deterministic CI).
        #[arg(long = "created-at")]
        created_at: Option<String>,
    },
}

#[derive(ValueEnum, Clone)]
enum RenderFormat {
    Html,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Parse { file } => parse_cmd(file),
        Cmd::Query { file, predicate } => query_cmd(file, predicate),
        Cmd::Lint { file } => lint_cmd(file),
        Cmd::Render { file, format } => render_cmd(file, format),
        Cmd::Orch {
            cmd: OrchCmd::Export {
                cmd: ExportCmd::Agents { file, to },
            },
        } => orch_export_agents_cmd(file, to),
        Cmd::Orch {
            cmd: OrchCmd::Export {
                cmd: ExportCmd::Tasks {
                    file,
                    to,
                    created_by,
                    created_at,
                },
            },
        } => orch_export_tasks_cmd(file, to, created_by, created_at),
        Cmd::Orch {
            cmd: OrchCmd::Import {
                cmd: ImportCmd::Runs { file, from, dry_run },
            },
        } => orch_import_runs_cmd(file, from, dry_run),
        Cmd::Orch {
            cmd: OrchCmd::Board {
                file,
                created_by,
                created_at,
            },
        } => orch_board_cmd(file, created_by, created_at),
        Cmd::Ready { file, agent } => ready_cmd(file, agent),
        Cmd::Claim { file, id, agent } => claim_cmd(file, id, agent),
        Cmd::Transition { file, id, state } => transition_cmd(file, id, state),
        Cmd::Log { file, id, entry } => log_cmd(file, id, entry),
        Cmd::Result_ { file, id, content } => result_cmd(file, id, content),
        Cmd::Close { file, id, reason } => close_cmd(file, id, reason),
    }
}

fn parse_cmd(file: PathBuf) -> Result<()> {
    let src = fs::read_to_string(&file)
        .with_context(|| format!("reading {}", file.display()))?;
    let doc = Document::parse(&src);
    let headlines = doc.headlines();
    let summary: Vec<_> = headlines
        .iter()
        .map(|h| {
            serde_json::json!({
                "level": h.level(),
                "title": h.title_raw().trim(),
                "state": h.todo_keyword().map(|t| t.to_string()),
                "id": h.properties().and_then(|p| p.get("ID")).map(|t| t.to_string()),
                "tags": h.tags().map(|t| t.to_string()).collect::<Vec<_>>(),
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

fn query_cmd(file: PathBuf, predicate: String) -> Result<()> {
    let src = fs::read_to_string(&file)
        .with_context(|| format!("reading {}", file.display()))?;
    let doc = Document::parse(&src);
    let pred: Predicate = serde_json::from_str(&predicate)
        .context("parsing predicate JSON — see Predicate enum in worg-query")?;
    let matches = worg_query::query(&doc, &pred);
    let summary: Vec<_> = matches
        .iter()
        .map(|h| {
            serde_json::json!({
                "id": h.properties().and_then(|p| p.get("ID")).map(|t| t.to_string()),
                "title": h.title_raw().trim(),
                "state": h.todo_keyword().map(|t| t.to_string()),
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

fn lint_cmd(file: PathBuf) -> Result<()> {
    let src = fs::read_to_string(&file)
        .with_context(|| format!("reading {}", file.display()))?;
    let doc = Document::parse(&src);

    let mut glossary = match Glossary::discover(&file) {
        Some(path) => {
            eprintln!("glossary: {}", path.display());
            Glossary::from_file(&path)
                .with_context(|| format!("loading glossary from {}", path.display()))?
        }
        None => {
            eprintln!("glossary: built-in default (no w.org discovered)");
            Glossary::default()
        }
    };

    // Layer in any glossaries the target file itself declares.
    let target_dir = file.parent().unwrap_or_else(|| std::path::Path::new("."));
    for line in src.lines() {
        if let Some(rest) = line.strip_prefix("#+GLOSSARY:") {
            for token in rest.split_whitespace() {
                let g_path = target_dir.join(token);
                match Glossary::from_file(&g_path) {
                    Ok(extra) => {
                        eprintln!("glossary: + {} (from target file)", g_path.display());
                        glossary.merge(extra);
                    }
                    Err(e) => {
                        eprintln!(
                            "warn: could not load glossary `{}` declared in target file: {}",
                            g_path.display(),
                            e
                        );
                    }
                }
            }
        }
    }

    // Setup-level diagnostics from glossary internal consistency (E006/W011).
    let setup_diags = glossary.validate();
    let file_diags = worg_lint::lint_with_glossary(&doc, &glossary);
    let diags: Vec<_> = setup_diags.into_iter().chain(file_diags).collect();
    let mut errors = 0;
    for d in &diags {
        let sev = match d.severity {
            Severity::Error => {
                errors += 1;
                "error"
            }
            Severity::Warn => "warn",
        };
        let where_ = d.headline_id.as_deref().unwrap_or("?");
        eprintln!("{} [{}] [{}] {}", sev, d.code, where_, d.message);
    }
    if diags.is_empty() {
        eprintln!("clean ({} headlines checked)", doc.headlines().len());
    } else {
        eprintln!(
            "summary: {} diagnostic(s) — {} error(s), {} warning(s)",
            diags.len(),
            errors,
            diags.len() - errors
        );
    }
    if errors > 0 {
        std::process::exit(1);
    }
    Ok(())
}

fn render_cmd(file: PathBuf, format: RenderFormat) -> Result<()> {
    let src = fs::read_to_string(&file)
        .with_context(|| format!("reading {}", file.display()))?;
    match format {
        RenderFormat::Html => {
            let org = orgize::Org::parse(&src);
            println!("{}", org.to_html());
        }
    }
    Ok(())
}

fn orch_export_agents_cmd(file: PathBuf, to: PathBuf) -> Result<()> {
    let src = fs::read_to_string(&file)
        .with_context(|| format!("reading {}", file.display()))?;
    let doc = worg_parse::Document::parse(&src);
    let agents_file = worg_orch::agents_file(&doc);
    let n = agents_file.agents.len();

    fs::create_dir_all(&to)
        .with_context(|| format!("creating output directory {}", to.display()))?;
    let out_path = to.join("agents.json");
    let json = serde_json::to_string_pretty(&agents_file)?;
    fs::write(&out_path, &json)
        .with_context(|| format!("writing {}", out_path.display()))?;

    eprintln!("wrote {} agent(s) to {}", n, out_path.display());
    Ok(())
}

fn orch_import_runs_cmd(file: PathBuf, from: PathBuf, dry_run: bool) -> Result<()> {
    use worg_orch::{Run, Task};

    let src = fs::read_to_string(&file)
        .with_context(|| format!("reading {}", file.display()))?;

    let runs_dir = from.join("runs");
    let tasks_dir = from.join("tasks");

    // Collect all runs grouped by task id.
    let mut runs_by_task: std::collections::BTreeMap<String, Vec<Run>> =
        std::collections::BTreeMap::new();
    if runs_dir.is_dir() {
        for entry in fs::read_dir(&runs_dir)
            .with_context(|| format!("reading {}", runs_dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let run: Run = serde_json::from_str(&raw)
                .with_context(|| format!("parsing run {}", path.display()))?;
            runs_by_task
                .entry(run.task.as_str().to_string())
                .or_default()
                .push(run);
        }
    }

    // For each task with runs, fold updates into the document.
    let mut doc = worg_parse::Document::parse(&src);
    let mut appended = 0usize;
    let mut transitioned = 0usize;
    let mut skipped_idempotent = 0usize;

    for (task_id, runs) in &runs_by_task {
        // Locate the org headline. If absent, skip silently — the
        // orchestrator may have tasks the plan file doesn't mirror.
        if doc.find_by_id(task_id).is_none() {
            continue;
        }

        // Append LOGBOOK entries for runs not yet in the document.
        // Idempotency: scan the whole serialized doc for `run=<id>`;
        // if present, skip. Coarse but reliable since run ids are
        // protocol-unique slugs.
        for run in runs {
            // Reserialize each iteration to see prior appends.
            let current = doc.serialize();
            let marker = format!("run={}", run.id.as_str());
            if current.contains(&marker) {
                skipped_idempotent += 1;
                continue;
            }
            let entry = format_run_logbook_entry(run);
            doc.append_logbook(task_id, &entry)
                .with_context(|| format!("appending logbook to {task_id}"))?;
            appended += 1;
        }

        // Optionally transition TODO keyword to match the
        // orchestrator task state.
        let task_json = tasks_dir.join(format!("{task_id}.json"));
        if task_json.is_file() {
            let raw = fs::read_to_string(&task_json)?;
            let task: Task = serde_json::from_str(&raw)
                .with_context(|| format!("parsing task {}", task_json.display()))?;
            if let Some(kw) = task_state_to_todo_keyword(task.state) {
                // transition_todo silently no-ops if the headline has
                // no current TODO keyword to replace, which is the
                // right behavior — a template without #+TODO:
                // declarations shouldn't suddenly gain TODO keywords.
                if doc.transition_todo(task_id, kw).is_ok() {
                    transitioned += 1;
                }
            }
        }
    }

    let new_src = doc.serialize();
    if dry_run {
        eprintln!(
            "dry-run: would append {appended} logbook entry(ies), \
             transition {transitioned} TODO keyword(s), \
             skip {skipped_idempotent} already-imported run(s)"
        );
        return Ok(());
    }
    if new_src != src {
        fs::write(&file, &new_src)
            .with_context(|| format!("writing {}", file.display()))?;
    }
    eprintln!(
        "imported {appended} logbook entry(ies); \
         transitioned {transitioned} TODO keyword(s); \
         skipped {skipped_idempotent} already-imported run(s)"
    );
    Ok(())
}

/// Format a single Run as a LOGBOOK entry. WORG.md convention:
/// the line starts with `Attempt N`, includes a timestamp in brackets,
/// and embeds `run=<id>` as the machine-readable idempotency anchor.
///
/// When the Run has both `started_at` and `finished_at`, also append
/// a native org-mode `CLOCK:` line on a separate line of the same
/// entry (wb-0mqz.13). The CLOCK line is recognized by org-clock-report
/// and any other org-mode time-tracking consumer — including LLMs
/// reading the file — without needing to know about our custom
/// `dur=Ns` field.
fn format_run_logbook_entry(run: &worg_orch::Run) -> String {
    let custom = format_run_custom_entry(run);
    match format_run_clock_line(run) {
        Some(clock) => format!("{custom}\n{clock}"),
        None => custom,
    }
}

fn format_run_custom_entry(run: &worg_orch::Run) -> String {
    use time::format_description::well_known::Rfc3339;
    let ts = run
        .finished_at
        .unwrap_or(run.started_at)
        .format(&Rfc3339)
        .unwrap_or_else(|_| "?".into());
    let mut parts = vec![
        format!("Attempt {}", run.attempt),
        format!("[{ts}]"),
        format!("run={}", run.id),
        format!("state={}", run_state_str(run.state)),
        format!("agent={}", run.agent),
    ];
    if let Some(cost) = run.cost_usd {
        parts.push(format!("cost=${cost:.4}"));
    }
    if let Some(t) = &run.tokens {
        parts.push(format!("tokens_in={}", t.input));
        parts.push(format!("tokens_out={}", t.output));
    }
    if let (Some(start), Some(end)) = (Some(run.started_at), run.finished_at) {
        let dur = end - start;
        parts.push(format!("dur={}s", dur.whole_seconds()));
    }
    if let Some(err) = &run.error {
        parts.push(format!("error={:?}", err));
    }
    if let Some(summary) = &run.result_summary {
        parts.push(format!("summary={:?}", summary));
    }
    parts.join(" ")
}

/// Native org-mode CLOCK line for a Run, when both timestamps are
/// present. In-progress runs (no `finished_at`) yield None — an open
/// CLOCK isn't appropriate from a one-shot importer.
///
/// Format: `CLOCK: [start]--[end] =>  H:MM`, where timestamps use
/// org-mode's inactive bracket form with 3-letter day-of-week
/// abbreviations (e.g. `[2026-05-23 Sat 20:00]`).
fn format_run_clock_line(run: &worg_orch::Run) -> Option<String> {
    let end = run.finished_at?;
    let start = run.started_at;
    let secs = (end - start).whole_seconds().max(0);
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    Some(format!(
        "CLOCK: {}--{} =>  {}:{:02}",
        format_org_inactive_ts(start),
        format_org_inactive_ts(end),
        hours,
        minutes,
    ))
}

fn format_org_inactive_ts(dt: time::OffsetDateTime) -> String {
    let dow = match dt.weekday() {
        time::Weekday::Monday => "Mon",
        time::Weekday::Tuesday => "Tue",
        time::Weekday::Wednesday => "Wed",
        time::Weekday::Thursday => "Thu",
        time::Weekday::Friday => "Fri",
        time::Weekday::Saturday => "Sat",
        time::Weekday::Sunday => "Sun",
    };
    format!(
        "[{:04}-{:02}-{:02} {} {:02}:{:02}]",
        dt.year(),
        dt.month() as u8,
        dt.day(),
        dow,
        dt.hour(),
        dt.minute(),
    )
}

fn run_state_str(state: worg_orch::RunState) -> &'static str {
    match state {
        worg_orch::RunState::Running => "running",
        worg_orch::RunState::Completed => "completed",
        worg_orch::RunState::Failed => "failed",
        worg_orch::RunState::Cancelled => "cancelled",
    }
}

/// Map orchestrator TaskState → org TODO keyword from the default w.org
/// set. Returns None for states that don't have an obvious keyword
/// equivalent (e.g. Backlog vs Ready both → TODO; InputRequired and
/// Review have no canonical org keyword in the default set).
fn task_state_to_todo_keyword(state: worg_orch::TaskState) -> Option<&'static str> {
    use worg_orch::TaskState;
    match state {
        TaskState::Backlog | TaskState::Ready => Some("TODO"),
        TaskState::InProgress => Some("DOING"),
        TaskState::Done => Some("DONE"),
        TaskState::Blocked | TaskState::InputRequired | TaskState::Review => Some("BLOCKED"),
        TaskState::Cancelled => Some("ABANDONED"),
    }
}

/// wb-4vhr.21 Phase A — emit the whole board (agents + tasks) as a
/// single JSON blob on stdout. Replaces the directory-walking
/// pattern that consumers (Pi, Studio HTTP controller, third-party
/// tools) had to perform against `.wb-orch/`.
///
/// Lint-cycle check matches `orch export tasks` — :BLOCKER: cycles
/// fail loudly here too, since a downstream consumer can't recover.
fn orch_board_cmd(
    file: PathBuf,
    created_by: String,
    created_at: Option<String>,
) -> Result<()> {
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;
    use worg_orch::{AgentId, ExportOpts};

    let exported_at = match &created_at {
        Some(s) => OffsetDateTime::parse(s, &Rfc3339)
            .with_context(|| format!("parsing --created-at `{s}` as RFC 3339"))?,
        None => OffsetDateTime::now_utc(),
    };
    let opts = ExportOpts {
        created_by: AgentId::new(created_by),
        exported_at,
    };

    let src = fs::read_to_string(&file)
        .with_context(|| format!("reading {}", file.display()))?;
    let doc = worg_parse::Document::parse(&src);

    let cycle_diags: Vec<_> = worg_lint::lint(&doc)
        .into_iter()
        .filter(|d| d.code == "E007")
        .collect();
    if !cycle_diags.is_empty() {
        for d in &cycle_diags {
            eprintln!("error [{}] {}", d.code, d.message);
        }
        anyhow::bail!(
            "refusing to emit board — {} :BLOCKER: cycle(s) detected. Fix the source and re-run.",
            cycle_diags.len()
        );
    }

    let snap = worg_orch::board_snapshot(&doc, &opts);

    // Project to wire types (Agent + Task; the application-layer
    // fields on TaskDefinition stay in-process for richer consumers
    // like Watershed's Wasmex bridge). Same shape as concatenating
    // agents.json + a tasks array.
    let payload = serde_json::json!({
        "version": snap.version.0,
        "agents": snap.agents,
        "tasks": snap.tasks.iter().map(|t| &t.wire).collect::<Vec<_>>(),
    });
    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}

fn orch_export_tasks_cmd(
    file: PathBuf,
    to: PathBuf,
    created_by: String,
    created_at: Option<String>,
) -> Result<()> {
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;
    use worg_orch::{AgentId, ExportOpts};

    let exported_at = match &created_at {
        Some(s) => OffsetDateTime::parse(s, &Rfc3339)
            .with_context(|| format!("parsing --created-at `{s}` as RFC 3339"))?,
        None => OffsetDateTime::now_utc(),
    };
    let opts = ExportOpts {
        created_by: AgentId::new(created_by),
        exported_at,
    };

    let src = fs::read_to_string(&file)
        .with_context(|| format!("reading {}", file.display()))?;
    let doc = worg_parse::Document::parse(&src);

    // Refuse to export a plan with :BLOCKER: cycles (wb-0mqz.6).
    // The cycle would mean the orchestrator can never schedule any
    // participant — failing here at export time is much more useful
    // than letting the runtime stall later.
    let cycle_diags: Vec<_> = worg_lint::lint(&doc)
        .into_iter()
        .filter(|d| d.code == "E007")
        .collect();
    if !cycle_diags.is_empty() {
        for d in &cycle_diags {
            eprintln!("error [{}] {}", d.code, d.message);
        }
        anyhow::bail!(
            "refusing to export tasks — {} :BLOCKER: cycle(s) detected. Fix the source and re-run.",
            cycle_diags.len()
        );
    }

    let tasks = worg_orch::task_definitions(&doc, &opts);

    fs::create_dir_all(&to)
        .with_context(|| format!("creating output directory {}", to.display()))?;
    let mut written = 0usize;
    for t in &tasks {
        let out_path = to.join(format!("{}.json", t.wire.id));
        // For tasks without :BLOCKER:, write the canonical wire
        // shape verbatim — preserves the wire struct's field
        // ordering for clean fixture diffs.
        //
        // For tasks WITH :BLOCKER:, wrap in a flattened struct
        // that surfaces `blocker` as a trailing extension field
        // (wb-qk6l.3). orchestrator-core's Task doesn't define this
        // key, but its deserializer isn't `deny_unknown_fields`,
        // so the field is forward-compatible — canonical
        // orchestrator ignores it; worg-agent's Loop reads it for
        // Wire fields first (via #[serde(flatten)]), then any
        // declared extension fields appended in struct-declared
        // order. `skip_serializing_if` keeps the output byte-
        // identical to the canonical wire shape when no extensions
        // are present.
        //
        // Extension fields, all forward-compatible additions
        // tolerated by orchestrator-core's wire deserializer (no
        // deny_unknown_fields):
        //   - `blocker` (wb-qk6l.3, renamed to org-edna in wb-0mqz.3)
        //   - `trigger` (wb-0mqz.4, org-edna)
        //   - `effort_minutes` (wb-0mqz.8, org-mode :Effort:)
        //   - `stage_model` (wb-6t1r, per-stage LLM model override)
        #[derive(serde::Serialize)]
        struct TaskWithExtensions<'a> {
            #[serde(flatten)]
            wire: &'a worg_orch::Task,
            #[serde(skip_serializing_if = "Vec::is_empty")]
            blocker: Vec<&'a str>,
            #[serde(skip_serializing_if = "Vec::is_empty")]
            trigger: Vec<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            effort_minutes: Option<u32>,
            #[serde(skip_serializing_if = "Option::is_none")]
            stage_model: Option<&'a str>,
        }
        let json = serde_json::to_string_pretty(&TaskWithExtensions {
            wire: &t.wire,
            blocker: t.blocker.iter().map(|d| d.as_str()).collect(),
            trigger: t.trigger.iter().map(|d| d.as_str()).collect(),
            effort_minutes: t.effort_minutes,
            stage_model: t.stage_model.as_deref(),
        })?;
        fs::write(&out_path, &json)
            .with_context(|| format!("writing {}", out_path.display()))?;
        written += 1;
    }
    eprintln!("wrote {} task(s) to {}", written, to.display());
    Ok(())
}

// ─── wb-4vhr.26: standalone mutation subcommands ────────────────────
//
// All six commands share two invariants:
//   1. The org file is loaded fresh per invocation (no in-memory state
//      shared with other CLI calls).
//   2. Writes go through atomic_write — temp-file + rename on the same
//      filesystem (atomic on POSIX, atomic-equivalent on NTFS).
//
// Exit-code contract (matches Parse/Lint/Render):
//   0 — success
//   non-zero — anyhow::Error bubbles out of main, prints to stderr.
//
// Out of scope for this command surface (intentional):
//   - :BLOCKER: dependency resolution. `worg ready` lists tasks by
//     state only; full blocker-aware scheduling lives in worg-agent's
//     Loader.ready_tasks/1 + worg-orch's task graph walk.

fn ready_cmd(file: PathBuf, agent: Option<String>) -> Result<()> {
    let src = fs::read_to_string(&file)
        .with_context(|| format!("reading {}", file.display()))?;
    let doc = Document::parse(&src);
    let ready: Vec<_> = doc
        .headlines()
        .iter()
        .filter(|h| {
            matches!(
                h.todo_keyword().map(|t| t.to_string()).as_deref(),
                Some("NEXT") | Some("TODO")
            )
        })
        .filter(|h| match &agent {
            None => true,
            Some(slug) => h
                .properties()
                .and_then(|p| p.get("ASSIGNED_AGENT"))
                .map(|t| t.to_string() == *slug)
                .unwrap_or(false),
        })
        .map(|h| {
            serde_json::json!({
                "id": h.properties().and_then(|p| p.get("ID")).map(|t| t.to_string()),
                "title": h.title_raw().trim(),
                "state": h.todo_keyword().map(|t| t.to_string()),
                "assigned_agent": h
                    .properties()
                    .and_then(|p| p.get("ASSIGNED_AGENT"))
                    .map(|t| t.to_string()),
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&ready)?);
    Ok(())
}

fn claim_cmd(file: PathBuf, id: String, agent: Option<String>) -> Result<()> {
    with_locked_mutation(&file, |doc| {
        // wb-nlln.18: CAS — claim only succeeds when the task is in
        // TODO or NEXT. A concurrent second claim (under the same
        // file lock or just racing on disk) sees DOING and is
        // rejected with a clear error.
        doc.transition_todo_cas(&id, &["TODO", "NEXT"], "DOING")
            .map_err(|e| anyhow::anyhow!("claim {id} failed: {e}"))?;
        if let Some(slug) = &agent {
            doc.set_property(&id, "ASSIGNED_AGENT", slug)
                .map_err(|e| anyhow::anyhow!("set :ASSIGNED_AGENT: failed: {e:?}"))?;
        }
        Ok(())
    })
}

fn transition_cmd(file: PathBuf, id: String, state: String) -> Result<()> {
    with_locked_mutation(&file, |doc| {
        doc.transition_todo(&id, &state)
            .map_err(|e| anyhow::anyhow!("transition {id} → {state} failed: {e:?}"))
    })
}

fn log_cmd(file: PathBuf, id: String, entry: String) -> Result<()> {
    with_locked_mutation(&file, |doc| {
        doc.append_logbook(&id, &entry)
            .map_err(|e| anyhow::anyhow!("append_logbook {id} failed: {e:?}"))
    })
}

fn result_cmd(file: PathBuf, id: String, content: String) -> Result<()> {
    with_locked_mutation(&file, |doc| {
        doc.write_results(&id, &content)
            .map_err(|e| anyhow::anyhow!("write_results {id} failed: {e:?}"))
    })
}

fn close_cmd(file: PathBuf, id: String, reason: Option<String>) -> Result<()> {
    with_locked_mutation(&file, |doc| {
        // CAS: don't allow close-from-already-DONE/CANCELED/FAILED.
        // Accept any non-terminal state as a valid pre-close state.
        doc.transition_todo_cas(
            &id,
            &["TODO", "NEXT", "DOING", "WAITING", "SOMEDAY"],
            "DONE",
        )
        .map_err(|e| anyhow::anyhow!("close {id} failed: {e}"))?;
        if let Some(text) = &reason {
            doc.append_logbook(&id, text)
                .map_err(|e| anyhow::anyhow!("append close reason failed: {e:?}"))?;
        }
        Ok(())
    })
}

/// wb-nlln.18: serialize concurrent mutations to the same .org file.
///
/// Acquires an exclusive advisory lock on `<file>.lock`, then runs the
/// closure on a freshly-parsed `Document`, then atomic-writes the
/// serialized result back. The lock is held across the full
/// read-modify-write window so two concurrent invocations cannot both
/// see the same pre-mutation state.
///
/// Lock file persists between runs (cheap and avoids a TOCTOU on
/// deletion). The first invocation creates it; subsequent runs reuse.
fn with_locked_mutation<F>(file: &Path, mutate: F) -> Result<()>
where
    F: FnOnce(&mut Document) -> Result<()>,
{
    // Rust 1.89 stabilized inherent File::lock/unlock for advisory
    // file locking — no external crate needed. flock(2) on Unix,
    // LockFileEx on Windows.
    let lock_path = file.with_extension(format!(
        "{}.lock",
        file.extension()
            .and_then(|e: &std::ffi::OsStr| e.to_str())
            .unwrap_or("org")
    ));
    let lock_file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .read(true)
        .open(&lock_path)
        .with_context(|| format!("opening lock {}", lock_path.display()))?;
    lock_file
        .lock()
        .with_context(|| format!("acquiring exclusive lock on {}", lock_path.display()))?;

    // Block to ensure the lock is released even if mutate or the write
    // panics — Rust unwinds and drops `lock_file`, releasing the lock.
    let result = (|| -> Result<()> {
        let src = fs::read_to_string(file)
            .with_context(|| format!("reading {}", file.display()))?;
        let mut doc = Document::parse(&src);
        mutate(&mut doc)?;
        atomic_write(file, &doc.serialize())?;
        Ok(())
    })();

    // fs4 calls unlock on Drop, but be explicit for readability. If the
    // unlock itself errors (very unusual), prefer surfacing the
    // mutation result so users see the actual failure cause.
    let _ = lock_file.unlock();
    result
}

/// Atomic file write: write to `<file>.tmp` then rename over `<file>`.
/// `std::fs::rename` is atomic when both paths are on the same
/// filesystem (POSIX guarantee, NTFS provides equivalent semantics
/// for ReplaceFile). Same convention as the existing orch_export_*
/// path, hoisted for reuse across mutation handlers.
fn atomic_write(file: &Path, contents: &str) -> Result<()> {
    let tmp = file.with_extension(format!(
        "{}.tmp",
        file.extension()
            .and_then(|e: &std::ffi::OsStr| e.to_str())
            .unwrap_or("org")
    ));
    fs::write(&tmp, contents)
        .with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, file)
        .with_context(|| format!("renaming {} → {}", tmp.display(), file.display()))?;
    Ok(())
}
