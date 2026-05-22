//! Run a list of specs against a model, scoring each.

use crate::llm::{strip_fences, CallResult, Client, Usage};
use crate::spec::Spec;
use crate::validate::{check, parses_ok, Outcome};
use anyhow::Result;
use serde::Serialize;
use std::time::Instant;

#[derive(Debug, Serialize, Clone)]
pub struct SpecResult {
    pub id: String,
    pub category: String,
    pub passed: bool,
    pub validator_outcomes: Vec<(String, OutcomeRecord)>,
    pub output: String,
    pub latency_ms: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    pub model: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeRecord {
    Pass,
    Fail(String),
    Gated,
    Error(String),
}

impl From<Outcome> for OutcomeRecord {
    fn from(o: Outcome) -> Self {
        match o {
            Outcome::Pass => OutcomeRecord::Pass,
            Outcome::Fail(s) => OutcomeRecord::Fail(s),
            Outcome::Gated => OutcomeRecord::Gated,
        }
    }
}

pub async fn run_specs(
    specs: &[Spec],
    model: &str,
    client: &Client,
    verbose: bool,
) -> Result<Vec<SpecResult>> {
    let mut results = Vec::with_capacity(specs.len());
    for (i, spec) in specs.iter().enumerate() {
        if verbose {
            println!("[{}/{}] {} ...", i + 1, specs.len(), spec.id);
        }
        results.push(run_one(spec, model, client).await);
    }
    Ok(results)
}

pub async fn run_one(spec: &Spec, model: &str, client: &Client) -> SpecResult {
    let started = Instant::now();
    let call = client
        .complete(model, spec.system.as_deref(), &spec.prompt)
        .await;

    let (output_raw, latency_ms, usage, llm_err) = match call {
        Ok(CallResult {
            text,
            latency_ms,
            usage,
        }) => (text, latency_ms, usage, None),
        Err(e) => (
            String::new(),
            started.elapsed().as_millis(),
            None,
            Some(e.to_string()),
        ),
    };

    // Most models wrap org-mode output in ```org fences even when asked not to.
    // Strip them unless the spec opts out (e.g. when the spec wants to test
    // raw-output discipline).
    let output = if spec.strip_fences {
        output_raw.clone()
    } else {
        strip_fences(&output_raw).to_string()
    };

    let parses = parses_ok(&output);
    let mut outcomes: Vec<(String, OutcomeRecord)> = Vec::with_capacity(spec.validate.len());
    let mut all_pass = true;

    if let Some(err) = llm_err {
        outcomes.push(("llm_call".into(), OutcomeRecord::Error(err)));
        all_pass = false;
    } else {
        for v in &spec.validate {
            let outcome = check(v, &output, parses);
            let pass = matches!(outcome, Outcome::Pass);
            if !pass {
                all_pass = false;
            }
            outcomes.push((validator_label(v), outcome.into()));
        }
    }

    SpecResult {
        id: spec.id.clone(),
        category: spec.category.clone(),
        passed: all_pass,
        validator_outcomes: outcomes,
        output,
        latency_ms,
        usage,
        model: model.to_string(),
    }
}

fn validator_label(v: &crate::spec::ValidatorSpec) -> String {
    use crate::spec::ValidatorSpec::*;
    match v {
        Parses => "parses".into(),
        HeadlineCount { count } => format!("headline_count={count}"),
        StateMatch {
            headline_index,
            state,
        } => format!("state_match[{headline_index}]={state}"),
        HasProperty {
            headline_index,
            name,
            value,
        } => {
            let v = value.as_deref().unwrap_or("*");
            format!("has_property[{headline_index}]:{name}={v}")
        }
        HasDrawer {
            headline_index,
            name,
        } => format!("has_drawer[{headline_index}]:{name}"),
        TagsContain {
            headline_index,
            tags,
        } => format!("tags_contain[{headline_index}]={}", tags.join(",")),
        PriorityMatch {
            headline_index,
            priority,
        } => format!("priority[{headline_index}]={priority}"),
        LevelMatch {
            headline_index,
            level,
        } => format!("level[{headline_index}]={level}"),
        Regex { pattern } => format!("regex={pattern}"),
        EqualsNormalized { .. } => "equals_normalized".into(),
        Contains { substring } => {
            let s = substring.chars().take(24).collect::<String>();
            format!("contains={s:?}")
        }
    }
}
