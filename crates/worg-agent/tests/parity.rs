//! Parity test runner. Walks every fixture directory under
//! `packages/worg/parity-fixtures/`, drives `execute_turn` against a
//! queue-stubbed LLM that pops responses from `llm-script.json`, and
//! asserts the resulting conversation matches `expected.json`.
//!
//! When the Elixir runtime grows its sibling parity runner, both
//! suites point at the same fixture dir — drift in either runtime
//! shows up as a fail in this test.
//!
//! Bless flag: `WORG_AGENT_PARITY_BLESS=1 cargo test parity` rewrites
//! every fixture's `expected.json` from the current run. Use after a
//! deliberate semantic change you've reviewed.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::Value;
use worg_agent::llm::{ChatRequest, LlmClient, LlmError, LlmResponse};
use worg_agent::loader;
use worg_agent::loop_::{execute_turn, TurnConfig};
use worg_agent::tool_registry::ToolRegistry;
use worg_agent::tools;

fn fixtures_root() -> PathBuf {
    [env!("CARGO_MANIFEST_DIR"), "..", "..", "parity-fixtures"]
        .iter()
        .collect()
}

fn fixture_dirs() -> Vec<PathBuf> {
    let root = fixtures_root();
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&root).unwrap_or_else(|e| {
        panic!("read parity-fixtures dir {}: {e}", root.display())
    }) {
        let entry = entry.unwrap();
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.join("agent.org").exists() && path.join("llm-script.json").exists() {
            out.push(path);
        }
    }
    out.sort();
    out
}

/// Queue-backed LLM. Pops one canned response per call. Order in the
/// JSON file is order of consumption.
struct ScriptedLlm {
    responses: Mutex<Vec<LlmResponse>>,
}

impl ScriptedLlm {
    fn from_file(path: &Path) -> Self {
        let raw = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let mut responses: Vec<LlmResponse> = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
        responses.reverse(); // so we can pop from the back
        Self {
            responses: Mutex::new(responses),
        }
    }
}

#[async_trait]
impl LlmClient for ScriptedLlm {
    async fn chat(&self, _req: ChatRequest<'_>) -> Result<LlmResponse, LlmError> {
        self.responses
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| LlmError::Malformed("llm-script.json exhausted".into()))
    }
}

#[tokio::test]
async fn every_fixture_matches_expected_or_is_blessed() {
    let bless = std::env::var("WORG_AGENT_PARITY_BLESS")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false);

    let fixtures = fixture_dirs();
    assert!(
        !fixtures.is_empty(),
        "no fixtures found under {}",
        fixtures_root().display()
    );

    for fixture in fixtures {
        let name = fixture.file_name().unwrap().to_string_lossy().into_owned();
        eprintln!("\n=== parity fixture: {name} ===");

        let agent = loader::load_one(fixture.join("agent.org"), None)
            .unwrap_or_else(|e| panic!("[{name}] load agent.org: {e}"));
        let brief = std::fs::read_to_string(fixture.join("brief.txt"))
            .unwrap_or_else(|e| panic!("[{name}] read brief.txt: {e}"));

        let llm = ScriptedLlm::from_file(&fixture.join("llm-script.json"));
        let mut registry = ToolRegistry::new();
        tools::register_default_tools(&mut registry);

        let workdir = tempfile::tempdir().unwrap();
        let cfg = TurnConfig::new(workdir.path());

        let outcome = execute_turn(&agent, &llm, &registry, &[], &brief, &cfg)
            .await
            .unwrap_or_else(|e| panic!("[{name}] execute_turn: {e:?}"));

        let actual = serde_json::json!({
            "rounds": outcome.rounds,
            "message_count": outcome.messages.len(),
            "messages": project_messages(&outcome.messages),
        });

        let expected_path = fixture.join("expected.json");

        if bless {
            let pretty = serde_json::to_string_pretty(&actual).unwrap() + "\n";
            std::fs::write(&expected_path, pretty).unwrap();
            eprintln!("[{name}] blessed → {}", expected_path.display());
            continue;
        }

        let expected_raw = std::fs::read_to_string(&expected_path)
            .unwrap_or_else(|e| {
                panic!(
                    "[{name}] read expected.json: {e}\n\
                     run with WORG_AGENT_PARITY_BLESS=1 to capture"
                )
            });
        let expected: Value = serde_json::from_str(&expected_raw)
            .unwrap_or_else(|e| panic!("[{name}] parse expected.json: {e}"));

        if actual != expected {
            let actual_pretty = serde_json::to_string_pretty(&actual).unwrap();
            let expected_pretty = serde_json::to_string_pretty(&expected).unwrap();
            panic!(
                "[{name}] parity drift\n\n--- expected ---\n{expected_pretty}\n\n--- actual ---\n{actual_pretty}\n"
            );
        }
        eprintln!("[{name}] ok");
    }
}

/// Project messages down to the parity-normative subset: role +
/// content + tool_calls (id, name, arguments) + tool_call_id + name.
/// Drops anything provider-specific.
fn project_messages(msgs: &[worg_agent::llm::Message]) -> Vec<Value> {
    msgs.iter()
        .map(|m| {
            let mut obj = serde_json::Map::new();
            obj.insert("role".into(), Value::String(m.role.clone()));
            if let Some(c) = &m.content {
                obj.insert("content".into(), c.clone());
            }
            if let Some(calls) = &m.tool_calls {
                let projected: Vec<Value> = calls
                    .iter()
                    .map(|c| {
                        serde_json::json!({
                            "id": c.id,
                            "type": "function",
                            "function": {
                                "name": c.function.name,
                                "arguments": c.function.arguments,
                            }
                        })
                    })
                    .collect();
                obj.insert("tool_calls".into(), Value::Array(projected));
            }
            if let Some(id) = &m.tool_call_id {
                obj.insert("tool_call_id".into(), Value::String(id.clone()));
            }
            if let Some(n) = &m.name {
                obj.insert("name".into(), Value::String(n.clone()));
            }
            Value::Object(obj)
        })
        .collect()
}
