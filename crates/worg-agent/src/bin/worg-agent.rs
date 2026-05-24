//! `worg-agent` CLI — single command surface for running a WORG agent
//! against a prompt. Designed to slot into eval runners + the Phase 8
//! npm wrapper without changes.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use worg_agent::llm::OpenRouterClient;
use worg_agent::loader;
use worg_agent::loop_::{execute_turn, LoopError, TurnConfig};
use worg_agent::tool_registry::ToolRegistry;
use worg_agent::tools;

/// WORG agent runtime — load an agent.org file, run a single turn
/// against an LLM with tool dispatch, exit with a JSON summary.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Run a single agent turn end-to-end.
    Run(RunArgs),
}

#[derive(Parser, Debug)]
struct RunArgs {
    /// Path to the agent .org file.
    #[arg(long)]
    agent: PathBuf,

    /// Agent :ID: to pick when the file declares multiple. Defaults
    /// to the first :agent:-tagged headline in the file.
    #[arg(long)]
    agent_id: Option<String>,

    /// Working directory the agent operates in. File tools resolve
    /// relative paths here; bash runs with this as cwd.
    #[arg(long)]
    workdir: PathBuf,

    /// Initial user prompt. Pass `-` to read from stdin.
    #[arg(long)]
    prompt: String,

    /// Override the model declared in agent.org (e.g. for evals that
    /// want to pin a specific cheap model).
    #[arg(long)]
    model: Option<String>,

    /// Cap on LLM rounds within a single turn. Defaults to 10.
    #[arg(long, default_value_t = worg_agent::loop_::DEFAULT_MAX_TOOL_ROUNDS)]
    max_tool_rounds: u32,

    /// Optional transcript path. Each LLM round + tool result appends
    /// one JSON line for external observers (eval runners).
    #[arg(long)]
    transcript: Option<PathBuf>,

    /// JSON-only output on stdout (default is a human-readable summary
    /// plus the JSON on the last line).
    #[arg(long)]
    json: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    // Default to info-level so the loop's own tracing prints during
    // local dev; eval runners can quiet this with RUST_LOG=warn.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(std::io::stderr)
        .try_init();

    let cli = Cli::parse();
    match cli.command {
        Cmd::Run(args) => match run(args).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("worg-agent: {e:#}");
                ExitCode::from(1)
            }
        },
    }
}

async fn run(args: RunArgs) -> anyhow::Result<()> {
    let mut spec = loader::load_one(&args.agent, args.agent_id.as_deref())?;
    if let Some(model) = args.model {
        spec.model = model;
    }

    let api_key = std::env::var("OPENROUTER_API_KEY")
        .map_err(|_| anyhow::anyhow!("OPENROUTER_API_KEY not set"))?;
    let client = OpenRouterClient::new(api_key);

    let mut registry = ToolRegistry::new();
    tools::register_wavelet_director(&mut registry);

    let prompt = if args.prompt == "-" {
        let mut buf = String::new();
        use std::io::Read as _;
        std::io::stdin().read_to_string(&mut buf)?;
        buf
    } else {
        args.prompt
    };

    let mut cfg = TurnConfig::new(&args.workdir).with_max_tool_rounds(args.max_tool_rounds);
    if let Some(t) = args.transcript {
        cfg = cfg.with_transcript(t);
    }

    let outcome = match execute_turn(&spec, &client, &registry, &[], &prompt, &cfg).await {
        Ok(o) => o,
        Err(LoopError::RoundsExhausted {
            rounds_budget,
            partial_messages,
        }) => {
            // Treat as a non-zero exit but still emit a structured
            // summary so eval runners can inspect the partial trace.
            let summary = serde_json::json!({
                "status": "rounds_exhausted",
                "rounds_budget": rounds_budget,
                "message_count": partial_messages.len(),
                "messages": partial_messages,
            });
            println!("{}", serde_json::to_string_pretty(&summary)?);
            anyhow::bail!("agent loop exhausted tool round budget ({rounds_budget})");
        }
        Err(e) => return Err(e.into()),
    };

    let final_text = outcome
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "assistant")
        .and_then(|m| m.content.as_ref())
        .and_then(|c| c.as_str())
        .map(String::from)
        .unwrap_or_default();

    if !args.json {
        eprintln!(
            "\n--- {} round{} · {} prompt tok · {} completion tok ---",
            outcome.rounds,
            if outcome.rounds == 1 { "" } else { "s" },
            outcome.usage.prompt_tokens,
            outcome.usage.completion_tokens
        );
        if !final_text.is_empty() {
            println!("{final_text}");
        }
    }

    let summary = serde_json::json!({
        "status": "ok",
        "rounds": outcome.rounds,
        "usage": outcome.usage,
        "final_text": final_text,
        "message_count": outcome.messages.len(),
    });

    if args.json {
        println!("{}", serde_json::to_string(&summary)?);
    } else {
        eprintln!("{}", serde_json::to_string(&summary)?);
    }

    Ok(())
}
