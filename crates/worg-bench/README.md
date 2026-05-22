# worg-bench

Pretraining-quality benchmark for org-mode authorship. Given a tightly-scoped
authoring prompt, run any LLM (via OpenRouter), parse the response with
`worg-parse`, and check it against structural assertions. Scores a model's
**innate** org-mode competency — the kind we expect to come from training
data (Emacs docs, Reddit, the orgmode.org manual) rather than from any
Workbooks-specific skill teaching.

Distinct from `packages/workbooks/packages/workbench/evals/worg/`, which tests
the BEAM agent loop using worg tools end-to-end. This is the upstream
question: can the model write valid org-mode at all?

## Run

```bash
export OPENROUTER_API_KEY=sk-or-...

# Single model
cargo run -p worg-bench -- run --model xiaomi/mimo-v2.5-pro

# A subset by id substring or category
cargo run -p worg-bench -- run --model qwen/qwen3.7-max --filter 02-composition

# Append a row to a CSV for time-series tracking
cargo run -p worg-bench -- run --model xiaomi/mimo-v2.5-pro --csv ~/.worg-bench.csv

# Compare several models on the same suite
cargo run -p worg-bench -- compare \
  --models anthropic/claude-sonnet-4.6,openai/gpt-5,xiaomi/mimo-v2.5-pro

# List specs without calling any model
cargo run -p worg-bench -- list
```

## Spec format

One TOML per spec, in `specs/<category>/<id>.toml`:

```toml
id = "todo-headline"
category = "01-basic"
prompt = """
Write a single org-mode headline at level 1 with TODO state and the title
"Build login flow". Output ONLY the org-mode text, no fences, no commentary.
"""

[[validate]]
kind = "parses"

[[validate]]
kind = "headline_count"
count = 1

[[validate]]
kind = "state_match"
headline_index = 0
state = "TODO"
```

### Validators

| `kind` | Args | What it checks |
|---|---|---|
| `parses` | — | worg-parse accepts the text AND `serialize(parse(t)) == t` |
| `headline_count` | `count` | exactly N headlines in document order |
| `state_match` | `headline_index`, `state` | TODO/NEXT/DONE/WAITING/… at index |
| `has_property` | `headline_index`, `name`, optional `value` | :PROPERTIES: contains the key |
| `has_drawer` | `headline_index`, `name` | drawer with name exists (LOGBOOK, NOTES, custom) |
| `tags_contain` | `headline_index`, `tags[]` | headline includes all listed tags |
| `priority_match` | `headline_index`, `priority` | `[#A]` / `[#B]` etc. |
| `level_match` | `headline_index`, `level` | 1 = `*`, 2 = `**`, etc. |
| `regex` | `pattern` | free-form regex over full output |
| `contains` | `substring` | substring match anywhere |
| `equals_normalized` | `expected` | exact match after whitespace normalization |

All structural validators are gated on `parses == true` — if the output is
unparsable, downstream checks short-circuit as `gated` rather than failing.
This makes failure logs read cleanly: one root cause per spec, not a cascade.

### Categories (33 specs total)

- `01-basic/` (6) — single-element syntax: TODO headline, property drawer, tags+priority, multi-marker inline markup with escape rules, canonical-order headline (state→priority→title→tags), COMMENT keyword + #+BEGIN_COMMENT.
- `02-composition/` (8) — combining primitives: nested children, LOGBOOK, source block, CLOCK time-tracking, repeating SCHEDULED, multi-drawer order (PROPERTIES→CLOCK→LOGBOOK→NOTES), 3-level nested list, affiliated keywords (NAME/CAPTION/ATTR_HTML).
- `03-planning/` (7) — multi-headline plans: three-level DAG, constraints drawer, orchestrator board with statistics cookies + BLOCKED_BY, decision log with REVOKED + temporal layering, cross-referenced DAG via :DEPENDS_ON:, **incident postmortem from intent** (no construct names), **sprint board from PM brief** (no construct names).
- `04-mutation/` (5) — given input, return transformed: update CLOCK total, transition + cookie update, revoke decision additively, extract deadline from prose, merge two tasks preserving all properties.
- `05-blocks/` (5) — real source-block competence: defensive bash (pipefail/heredoc/quoted expansion), bash pipeline with retry + pg_dump|gzip + env-var guards, Lua object with metatable behavior, Lua coroutine-based lazy line reader, org-babel noweb composition from named blocks.
- `06-tables/` (3) — table shapes: hline groupings + TOTAL row, two-formula #+TBLFM with ;f1 format spec, table nested in QUOTE block.

### Prompt style: conceptual where possible

Two prompt styles in the suite:

- **Syntax prompts** name the org-mode construct ("write a `:PROPERTIES:` drawer with `:ID:`"). Measure whether the model knows the syntax.
- **Conceptual prompts** describe a USER NEED ("attach a stable identifier so other tasks can reference this one"). Measure whether the model reaches for the right construct unprompted — the real test of pretraining-quality org-mode competency.

The conceptual specs are intentionally fewer but harder. They're concentrated in `03-planning/` (incident-postmortem, sprint-board-from-brief), `05-blocks/` (the bash and lua specs that describe BEHAVIOR not constructs), and the `04-mutation/` set (which describes the desired TRANSFORMATION, never the syntax).

Add categories by creating a new subdirectory and dropping TOML specs in it.
`worg-bench` discovers everything under `specs/` recursively.

## Why this matters

Workbooks treats `.org` files as the canonical agent planning substrate. If
a model can't write valid org-mode without elaborate teaching, every skill
file in our system is fighting uphill against the model's intrinsic
knowledge. This benchmark surfaces that gap directly, per-model, so we know
which models need scaffolding and which don't.

Sister suite (integration): `packages/workbooks/packages/workbench/evals/worg/`
— tests agent BEHAVIOR using worg tools. `worg-bench` tests model SYNTAX
without tools.
