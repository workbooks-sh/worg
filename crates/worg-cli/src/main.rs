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
use std::path::PathBuf;
use worg_lint::Severity;
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
    let diags = worg_lint::lint(&doc);
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
