//! Output formatters: human-readable to stdout, JSON to a file, optional CSV
//! append for time-series tracking.

use crate::runner::{OutcomeRecord, SpecResult};
use anyhow::Result;
use colored::Colorize;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

#[derive(Debug, Serialize)]
pub struct RunReport<'a> {
    pub model: &'a str,
    pub spec_count: usize,
    pub pass_count: usize,
    pub fail_count: usize,
    /// Specs that errored at the LLM call (timeout, 5xx, network). These
    /// don't reflect the model's authoring ability — they reflect
    /// transient reliability. Reported separately from validation
    /// failures so a model with stalls doesn't look worse than it is.
    pub error_count: usize,
    pub per_category: BTreeMap<String, CategoryStat>,
    pub latency_ms_p50: u128,
    pub latency_ms_p95: u128,
    pub results: &'a [SpecResult],
}

#[derive(Debug, Serialize, Default, Clone)]
pub struct CategoryStat {
    pub total: usize,
    pub pass: usize,
}

pub fn summarize<'a>(model: &'a str, results: &'a [SpecResult]) -> RunReport<'a> {
    let mut per_category: BTreeMap<String, CategoryStat> = BTreeMap::new();
    let mut latencies: Vec<u128> = Vec::with_capacity(results.len());
    let mut pass = 0;
    let mut errors = 0;
    for r in results {
        let entry = per_category.entry(r.category.clone()).or_default();
        entry.total += 1;
        if r.passed {
            entry.pass += 1;
            pass += 1;
        } else if is_errored(r) {
            errors += 1;
        }
        latencies.push(r.latency_ms);
    }
    latencies.sort_unstable();
    let p = |q: f64| -> u128 {
        if latencies.is_empty() {
            0
        } else {
            let idx = ((latencies.len() as f64) * q).floor() as usize;
            latencies[idx.min(latencies.len() - 1)]
        }
    };
    let total = results.len();
    let fail = total.saturating_sub(pass).saturating_sub(errors);
    RunReport {
        model,
        spec_count: total,
        pass_count: pass,
        fail_count: fail,
        error_count: errors,
        per_category,
        latency_ms_p50: p(0.50),
        latency_ms_p95: p(0.95),
        results,
    }
}

/// A spec is "errored" if it didn't pass AND its only outcome is an
/// `Error` variant from the LLM call — i.e. the call itself failed
/// (timeout, 5xx, network), not the validators. Validation failures
/// (`Fail(_)`) count as `fail_count` instead.
fn is_errored(r: &SpecResult) -> bool {
    if r.passed {
        return false;
    }
    r.validator_outcomes
        .iter()
        .any(|(_, o)| matches!(o, OutcomeRecord::Error(_)))
}

pub fn print_human(report: &RunReport) {
    println!();
    println!(
        "worg-bench · model={} · {} specs",
        report.model.bold(),
        report.spec_count
    );
    println!();
    for (cat, stat) in &report.per_category {
        let pct = if stat.total == 0 {
            0.0
        } else {
            100.0 * stat.pass as f64 / stat.total as f64
        };
        let line = format!(
            "  {:<28}  {}/{}   {:>5.1}%",
            cat, stat.pass, stat.total, pct
        );
        println!("{}", if stat.pass == stat.total { line.green() } else if stat.pass == 0 { line.red() } else { line.yellow() });
    }
    println!();
    let reliability_pct = if report.spec_count == 0 {
        0.0
    } else {
        100.0 * report.pass_count as f64 / report.spec_count as f64
    };
    // Capability = pass / (pass + real_fail), excluding errored specs.
    // Tells us how the model does on specs that actually completed,
    // separated from transient OpenRouter / network issues.
    let scored = report.pass_count + report.fail_count;
    let capability_pct = if scored == 0 {
        0.0
    } else {
        100.0 * report.pass_count as f64 / scored as f64
    };
    println!(
        "  total                         {}/{}   reliability {:>5.1}%",
        report.pass_count, report.spec_count, reliability_pct
    );
    if report.error_count > 0 {
        println!(
            "  capability (errors excluded)  {}/{}   {:>5.1}%   ({} errored)",
            report.pass_count, scored, capability_pct, report.error_count
        );
    }
    println!(
        "  latency                       p50={}ms  p95={}ms",
        report.latency_ms_p50, report.latency_ms_p95
    );
    println!();

    // Failure detail
    let failures: Vec<&SpecResult> = report.results.iter().filter(|r| !r.passed).collect();
    if !failures.is_empty() {
        println!("Failures:");
        for r in failures {
            println!("  {} · {}", r.category.dimmed(), r.id.bold());
            for (label, outcome) in &r.validator_outcomes {
                let line = format!("    {} · {}", outcome_marker(outcome), label);
                println!("{}", color_outcome(outcome, &line));
                if let OutcomeRecord::Fail(msg) = outcome {
                    println!("        {}", msg.dimmed());
                } else if let OutcomeRecord::Error(msg) = outcome {
                    println!("        {}", msg.red().dimmed());
                }
            }
        }
        println!();
    }
}

fn outcome_marker(o: &OutcomeRecord) -> &'static str {
    match o {
        OutcomeRecord::Pass => "✓",
        OutcomeRecord::Fail(_) => "✗",
        OutcomeRecord::Gated => "·",
        OutcomeRecord::Error(_) => "!",
    }
}

fn color_outcome(o: &OutcomeRecord, line: &str) -> colored::ColoredString {
    match o {
        OutcomeRecord::Pass => line.green(),
        OutcomeRecord::Fail(_) => line.red(),
        OutcomeRecord::Gated => line.normal().dimmed(),
        OutcomeRecord::Error(_) => line.red().bold(),
    }
}

pub fn write_json(path: &Path, report: &RunReport) -> Result<()> {
    let f = std::fs::File::create(path)?;
    serde_json::to_writer_pretty(f, report)?;
    Ok(())
}

pub fn append_csv(path: &Path, report: &RunReport) -> Result<()> {
    let exists = path.exists();
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    if !exists {
        writeln!(
            f,
            "timestamp,model,spec_count,pass_count,fail_count,pct,latency_p50_ms,latency_p95_ms"
        )?;
    }
    let pct = if report.spec_count == 0 {
        0.0
    } else {
        100.0 * report.pass_count as f64 / report.spec_count as f64
    };
    writeln!(
        f,
        "{},{},{},{},{},{:.2},{},{}",
        chrono_now(),
        report.model,
        report.spec_count,
        report.pass_count,
        report.fail_count,
        pct,
        report.latency_ms_p50,
        report.latency_ms_p95
    )?;
    Ok(())
}

// Inline ISO-8601-ish UTC timestamp without pulling in chrono.
fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // YYYY-MM-DDTHH:MM:SSZ
    let (y, m, d, hh, mm, ss) = epoch_to_components(secs as i64);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

fn epoch_to_components(mut secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let ss = (secs % 60) as u32;
    secs /= 60;
    let mm = (secs % 60) as u32;
    secs /= 60;
    let hh = (secs % 24) as u32;
    let mut days = secs / 24;
    let mut y: i64 = 1970;
    loop {
        let in_year = if is_leap(y) { 366 } else { 365 };
        if days >= in_year {
            days -= in_year;
            y += 1;
        } else {
            break;
        }
    }
    let months_len: [u32; 12] = if is_leap(y) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut m: u32 = 1;
    for ml in months_len {
        if days >= ml as i64 {
            days -= ml as i64;
            m += 1;
        } else {
            break;
        }
    }
    let d = (days as u32) + 1;
    (y, m, d, hh, mm, ss)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}
