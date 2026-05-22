//! worg-bench — pretraining-quality org-mode authorship benchmark for LLMs.
//!
//! Surface:
//!
//!     worg-bench run --model <slug> [--suite <dir>] [--filter <id>] [--json <path>] [--csv <path>] [-v]
//!     worg-bench list
//!     worg-bench compare --models <m1,m2,…> [--suite <dir>]

mod llm;
mod report;
mod runner;
mod spec;
mod validate;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(version, about = "LLM benchmark for raw org-mode authorship quality.", long_about = None)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Run the suite against a single model
    Run {
        #[arg(long)]
        model: String,
        #[arg(long, default_value = "specs")]
        suite: PathBuf,
        #[arg(long)]
        filter: Option<String>,
        #[arg(long)]
        json: Option<PathBuf>,
        #[arg(long)]
        csv: Option<PathBuf>,
        #[arg(short, long, default_value_t = false)]
        verbose: bool,
    },
    /// List specs in the suite (no LLM call)
    List {
        #[arg(long, default_value = "specs")]
        suite: PathBuf,
    },
    /// Run the same suite against multiple models, print a comparison table
    Compare {
        #[arg(long, value_delimiter = ',')]
        models: Vec<String>,
        #[arg(long, default_value = "specs")]
        suite: PathBuf,
        #[arg(long)]
        json: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Run {
            model,
            suite,
            filter,
            json,
            csv,
            verbose,
        } => cmd_run(&model, &suite, filter.as_deref(), json, csv, verbose).await,
        Cmd::List { suite } => cmd_list(&suite),
        Cmd::Compare {
            models,
            suite,
            json,
        } => cmd_compare(&models, &suite, json).await,
    }
}

async fn cmd_run(
    model: &str,
    suite: &std::path::Path,
    filter: Option<&str>,
    json_out: Option<PathBuf>,
    csv_out: Option<PathBuf>,
    verbose: bool,
) -> Result<()> {
    let suite_path = resolve_suite_path(suite);
    let mut specs = spec::load_dir(&suite_path)
        .with_context(|| format!("loading specs from {}", suite_path.display()))?;
    if let Some(f) = filter {
        specs.retain(|s| s.id.contains(f) || s.category.contains(f));
    }
    if specs.is_empty() {
        anyhow::bail!("no specs matched (suite={}, filter={:?})", suite_path.display(), filter);
    }
    let client = llm::Client::from_env()?;
    let results = runner::run_specs(&specs, model, &client, verbose).await?;
    let report = report::summarize(model, &results);
    report::print_human(&report);
    if let Some(p) = json_out {
        report::write_json(&p, &report)?;
        eprintln!("wrote {}", p.display());
    }
    if let Some(p) = csv_out {
        report::append_csv(&p, &report)?;
        eprintln!("appended row to {}", p.display());
    }
    Ok(())
}

fn cmd_list(suite: &std::path::Path) -> Result<()> {
    let suite_path = resolve_suite_path(suite);
    let specs = spec::load_dir(&suite_path)?;
    println!("{} specs in {}", specs.len(), suite_path.display());
    let mut current_cat = String::new();
    for s in &specs {
        if s.category != current_cat {
            println!("\n[{}]", s.category);
            current_cat = s.category.clone();
        }
        println!("  {}", s.id);
    }
    Ok(())
}

async fn cmd_compare(
    models: &[String],
    suite: &std::path::Path,
    json_out: Option<PathBuf>,
) -> Result<()> {
    let suite_path = resolve_suite_path(suite);
    let specs = spec::load_dir(&suite_path)?;
    let client = llm::Client::from_env()?;

    let mut all = Vec::new();
    for m in models {
        eprintln!("running {} …", m);
        let results = runner::run_specs(&specs, m, &client, false).await?;
        let r = report::summarize(m, &results);
        // Print individual summary inline for liveness
        println!(
            "  {} → {}/{} ({:.1}%)",
            m,
            r.pass_count,
            r.spec_count,
            if r.spec_count == 0 {
                0.0
            } else {
                100.0 * r.pass_count as f64 / r.spec_count as f64
            }
        );
        all.push((m.clone(), results));
    }

    // Cross-model table
    println!();
    println!("=== comparison ===");
    print!("{:<28}", "category/spec");
    for (m, _) in &all {
        print!(" | {:>20}", m);
    }
    println!();
    for spec in &specs {
        print!("{:<28}", format!("{} · {}", spec.category, spec.id));
        for (_, results) in &all {
            let r = results.iter().find(|r| r.id == spec.id);
            let cell = match r {
                Some(r) if r.passed => "PASS".to_string(),
                Some(_) => "FAIL".to_string(),
                None => "—".to_string(),
            };
            print!(" | {:>20}", cell);
        }
        println!();
    }

    if let Some(p) = json_out {
        let payload: Vec<_> = all
            .iter()
            .map(|(m, rs)| {
                serde_json::json!({
                    "model": m,
                    "results": rs,
                })
            })
            .collect();
        std::fs::write(&p, serde_json::to_string_pretty(&payload)?)?;
        eprintln!("wrote {}", p.display());
    }
    Ok(())
}

/// Resolve `suite` relative to the crate dir if it's a relative path that
/// doesn't exist in cwd. Lets `worg-bench run --suite specs` work from
/// anywhere.
fn resolve_suite_path(suite: &std::path::Path) -> PathBuf {
    if suite.is_absolute() || suite.exists() {
        return suite.to_path_buf();
    }
    // Try CARGO_MANIFEST_DIR / suite
    if let Ok(dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let candidate = PathBuf::from(dir).join(suite);
        if candidate.exists() {
            return candidate;
        }
    }
    // Try the canonical location: packages/worg/crates/worg-bench/specs from
    // the workbooks repo root walking up from cwd.
    let mut cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    loop {
        let candidate = cwd.join("packages/worg/crates/worg-bench").join(suite);
        if candidate.exists() {
            return candidate;
        }
        if !cwd.pop() {
            break;
        }
    }
    suite.to_path_buf()
}
