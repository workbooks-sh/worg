defmodule WorgAgent.Loop do
  @moduledoc """
  The agent execution loop. Reads `.wb-orch/` state, picks the next
  ready task, builds an LLM prompt from the assigned agent +
  task description, dispatches tool calls, and writes a Run JSON on
  completion.

  ## Public API

      WorgAgent.Loop.run_next(board_dir, opts \\\\ [])

  Options:
  - `:agent_overrides` — map `%{agent_slug => %{system_prompt:
    String.t(), tools: [String.t()]}}`. The wire format
    (`agents.json`) does NOT carry system_prompt or per-agent tool
    catalogs — that data lives in the WORG source `.org` file.
    Callers extract it themselves (via `worg orch export`'s richer
    output, the Wasmex bridge once wb-6irl.33 lands, or any
    consumer-specific path) and pass it through here. Without an
    override, the loop falls back to a generic system prompt + the
    full `WorgAgent.ToolRegistry.catalog/0`.
  - `:llm_opts` — keyword forwarded to `WorgAgent.Llm.call/3`
    (`:api_key`, `:model`, `:endpoint`, `:cache`, `:req_options` for
    test plug injection).
  - `:working_dir` — passed to tool ctx. Defaults to the board_dir's
    parent.
  - `:trust_level` — `:sandboxed | :full` for tool ctx. Defaults to
    `:sandboxed`.
  - `:max_turns` — cap on LLM ↔ tool round-trips (default 30). Hard
    fail if exceeded — surfaces as `{:error, :max_turns_exceeded}`.
  - `:task_id` — pick this specific task by id, skipping the
    ready-task heuristic. Useful for tests and the integration story
    in wb-nlln.21.7.
  - `:now_iso8601` — override the wall-clock for timestamps in tests.
  - `:skip_task_advance` — set true to skip the `tasks/<id>.json`
    state update after a successful run. Default `false`. Useful
    when a caller (e.g. a richer orchestrator) wants to own task
    state transitions itself.
  - `:skip_cascade` — set true to skip the success-side `:TRIGGER:`
    cascade (and the failure-side `:BLOCKER:` cascade once
    wb-0mqz.14 lands). Default `false`. Same opt-out reasoning as
    `:skip_task_advance` — defers state propagation to the caller.

  Returns `{:ok, %Run{}}` on terminal completion, or `{:error, term}`.

  ## DAG semantics

  Two axes of dependency:

  1. **Outline parent** — `Task.parent` from the wire JSON. A task's
     outline parent must be `:in_progress` or `:done` for the child
     to be pickable. (Each task has at most one outline parent;
     this is a protocol-level invariant.)

  2. **`:BLOCKER:` list** — cross-tree dependencies from the org
     source, surfaced as the `blocker` extension field on the
     exported task JSON (added by `worg orch export tasks`).
     A task with non-empty `blocker` is only pickable once every
     listed task is in `:done` state. Missing dependencies (referenced
     id not in the plan) are treated as unresolved and block — fail
     loudly is preferable to silently picking a task whose declared
     prerequisite isn't even tracked.

  Pick order: first non-terminal, non-in_progress task whose parent
  AND every blocker entry satisfy the rules above, in deterministic
  id-sorted order.
  """

  alias WorgAgent.{Loader, Llm, Run, Sync, ToolRegistry}
  alias WorgAgent.Loader.{Plan, Task}
  alias WorgAgent.Llm.{Response, ToolCall}

  @default_max_turns 30

  @doc """
  Run the next ready task and return its Run.
  """
  @spec run_next(Path.t(), keyword) :: {:ok, Run.t()} | {:error, term}
  def run_next(board_dir, opts \\ []) do
    with {:ok, plan} <- Loader.load(board_dir),
         {:ok, task} <- pick_task(plan, opts),
         {:ok, agent} <- find_assigned_agent(plan, task),
         {:ok, run} <- execute(board_dir, plan, agent, task, opts) do
      {:ok, run}
    end
  end

  # ── Task selection ─────────────────────────────────────────────────
  #
  # The picker now delegates to `WorgAgent.Loader.ready_tasks/1`
  # (wb-0mqz.12) — a public API any orchestrator can call to enumerate
  # currently-pickable tasks. Loop just takes the head; richer
  # consumers can fan out to a pool.

  defp pick_task(%Plan{tasks: tasks} = plan, opts) do
    case Keyword.get(opts, :task_id) do
      nil -> first_ready(plan, tasks)
      id -> task_by_id(tasks, id)
    end
  end

  defp task_by_id(tasks, id) do
    case Map.fetch(tasks, id) do
      {:ok, task} -> {:ok, task}
      :error -> {:error, {:no_such_task, id}}
    end
  end

  defp first_ready(_plan, tasks) when map_size(tasks) == 0, do: {:error, :no_tasks}

  defp first_ready(plan, _tasks) do
    case Loader.ready_tasks(plan) do
      [task | _] -> {:ok, task}
      [] -> {:error, :no_ready_task}
    end
  end

  defp find_assigned_agent(%Plan{agents: agents}, %Task{assigned_to: [slug | _]}) do
    case Map.fetch(agents, slug) do
      {:ok, agent} -> {:ok, agent}
      :error -> {:error, {:unknown_agent, slug}}
    end
  end

  defp find_assigned_agent(_plan, %Task{assigned_to: []}) do
    {:error, :no_assigned_agent}
  end

  # ── Execution ──────────────────────────────────────────────────────

  defp execute(board_dir, _plan, agent, task, opts) do
    overrides = Keyword.get(opts, :agent_overrides, %{})
    enrichment = Map.get(overrides, agent.id, %{})
    system_prompt = enrichment[:system_prompt] || default_system_prompt(agent)
    tools = build_tool_catalog(enrichment[:tools])
    ctx = build_tool_ctx(board_dir, opts)
    max_turns = Keyword.get(opts, :max_turns, @default_max_turns)
    now = Keyword.get(opts, :now_iso8601, now_iso8601())
    attempt = next_attempt(board_dir, task.id)

    # wb-6t1r: task.stage_model overrides any model in llm_opts, which
    # in turn overrides Llm.call's default. The wavelet use case:
    # default agent runs on a cheap model, individual judging stages
    # escalate to Gemini / Claude Opus via :STAGE_MODEL: on the task
    # node. Falls through cleanly when stage_model is nil.
    opts = override_model_from_task(opts, task)

    initial_messages = [
      %{"role" => "system", "content" => system_prompt},
      %{"role" => "user", "content" => task_prompt(task)}
    ]

    case iterate(initial_messages, tools, ctx, opts, max_turns, %{tokens_in: 0, tokens_out: 0}) do
      {:ok, summary, usage} ->
        run = %Run{
          id: Run.id_for(task.id, attempt),
          task: task.id,
          agent: agent.id,
          state: :completed,
          attempt: attempt,
          started_at: now,
          finished_at: Keyword.get(opts, :now_iso8601, now_iso8601()),
          tokens: %{"input" => usage.tokens_in, "output" => usage.tokens_out},
          result_summary: summary
        }

        with {:ok, _} <- write_run(board_dir, run),
             :ok <- maybe_advance_task(board_dir, task.id, opts),
             :ok <- maybe_cascade_success(board_dir, task.id, opts) do
          {:ok, run}
        end

      {:error, reason, partial_usage} ->
        run = %Run{
          id: Run.id_for(task.id, attempt),
          task: task.id,
          agent: agent.id,
          state: :failed,
          attempt: attempt,
          started_at: now,
          finished_at: Keyword.get(opts, :now_iso8601, now_iso8601()),
          tokens: %{
            "input" => partial_usage.tokens_in,
            "output" => partial_usage.tokens_out
          },
          error: inspect(reason)
        }

        with {:ok, _} <- write_run(board_dir, run),
             :ok <- maybe_cascade_failure(board_dir, task.id, opts) do
          {:error, reason}
        end
    end
  end

  defp iterate(_messages, _tools, _ctx, _opts, 0, usage) do
    {:error, :max_turns_exceeded, usage}
  end

  defp iterate(messages, tools, ctx, opts, turns_remaining, usage) do
    llm_opts = Keyword.get(opts, :llm_opts, [])

    # wb-jnjc: lifecycle telemetry for live UIs. Spans wrap each LLM
    # turn and each tool call. Subscribers (Phoenix channel, IEx
    # debug printer, OpenTelemetry exporter) attach handlers without
    # the Loop knowing about them. See moduledoc § Telemetry events.
    :telemetry.span([:worg_agent, :llm, :turn], llm_turn_metadata(messages, ctx), fn ->
      result = Llm.call(messages, tools, llm_opts)
      {result, llm_turn_result_metadata(result)}
    end)
    |> handle_llm_result(messages, tools, ctx, opts, turns_remaining, usage)
  end

  # Pattern-match on the LLM result + continue the iterate loop.
  # Pulled out so the telemetry.span wrapper above stays a single
  # expression.
  defp handle_llm_result(result, messages, tools, ctx, opts, turns_remaining, usage) do
    case result do
      {:ok, %Response{stop_reason: :tool_calls, tool_calls: calls, content: content} = resp} ->
        # Dispatch each tool call, append results to the conversation.
        usage = accumulate(usage, resp.usage)

        assistant_msg = build_assistant_message(content, calls)
        tool_msgs = Enum.map(calls, &execute_one_tool(&1, ctx))

        iterate(
          messages ++ [assistant_msg | tool_msgs],
          tools,
          ctx,
          opts,
          turns_remaining - 1,
          usage
        )

      {:ok, %Response{stop_reason: stop, content: content} = resp}
      when stop in [:end_turn, :max_tokens, :other] ->
        usage = accumulate(usage, resp.usage)
        summary = content || "(no content)"
        {:ok, summary, usage}

      {:error, reason} ->
        {:error, reason, usage}
    end
  end

  defp execute_one_tool(%ToolCall{id: id, name: name, arguments: args}, ctx) do
    # wb-jnjc: telemetry.span around the dispatch so live UIs can
    # show tool start / stop with timings. Args are passed in
    # :start metadata; result size + ok/error in :stop measurements.
    :telemetry.span(
      [:worg_agent, :tool_call],
      %{tool_name: name, tool_call_id: id, args: args},
      fn ->
        dispatch_result = ToolRegistry.dispatch(name, args, ctx)

        result_string =
          case dispatch_result do
            {:ok, output} -> output
            {:error, reason} -> "error: #{inspect(reason)}"
          end

        tool_msg = %{
          "role" => "tool",
          "tool_call_id" => id,
          "content" => result_string
        }

        meta = %{
          tool_name: name,
          tool_call_id: id,
          status:
            case dispatch_result do
              {:ok, _} -> :ok
              {:error, _} -> :error
            end,
          result_size: byte_size(result_string)
        }

        {tool_msg, meta}
      end
    )
  end

  defp build_assistant_message(content, calls) do
    %{
      "role" => "assistant",
      "content" => content,
      "tool_calls" =>
        Enum.map(calls, fn %ToolCall{id: id, name: name, arguments: args} ->
          %{
            "id" => id,
            "type" => "function",
            "function" => %{
              "name" => name,
              "arguments" => Jason.encode!(args)
            }
          }
        end)
    }
  end

  defp accumulate(usage, provider_usage) when is_map(provider_usage) do
    %{
      tokens_in: usage.tokens_in + Map.get(provider_usage, "prompt_tokens", 0),
      tokens_out: usage.tokens_out + Map.get(provider_usage, "completion_tokens", 0)
    }
  end

  defp accumulate(usage, _), do: usage

  # ── Tool catalog + prompt building ─────────────────────────────────

  defp build_tool_catalog(nil) do
    ToolRegistry.catalog()
    |> wrap_for_openai()
  end

  defp build_tool_catalog(tool_names) when is_list(tool_names) do
    ToolRegistry.catalog()
    |> Enum.filter(&(&1["name"] in tool_names))
    |> wrap_for_openai()
  end

  defp wrap_for_openai(catalog) do
    Enum.map(catalog, fn entry ->
      %{
        "type" => "function",
        "function" => %{
          "name" => entry["name"],
          "description" => entry["description"],
          "parameters" => entry["input_schema"]
        }
      }
    end)
  end

  defp build_tool_ctx(board_dir, opts) do
    %{
      working_dir: Keyword.get(opts, :working_dir, Path.dirname(board_dir)),
      trust_level: Keyword.get(opts, :trust_level, :sandboxed),
      task_id: nil
    }
  end

  defp task_prompt(%Task{} = task) do
    """
    # Task: #{task.title}

    ID: #{task.id}
    #{if task.description, do: "\nDescription:\n#{task.description}", else: ""}
    #{if task.acceptance, do: "\nAcceptance criteria:\n#{task.acceptance}", else: ""}

    Use the available tools to complete this task. When the task is
    complete, respond with a short summary and stop calling tools.
    """
  end

  defp default_system_prompt(agent) do
    """
    You are #{agent.name}, an agent in the WORG runtime.
    Capabilities: #{Enum.join(agent.capabilities, ", ")}.

    Use the provided tools to make progress on the task. When you
    believe the task is complete, reply with a brief summary and
    stop calling tools.
    """
  end

  # ── Run persistence ────────────────────────────────────────────────

  defp next_attempt(board_dir, task_id) do
    runs_dir = Path.join(board_dir, "runs")

    case File.ls(runs_dir) do
      {:ok, files} ->
        prefix = "#{task_id}-"

        files
        |> Enum.filter(&String.starts_with?(&1, prefix))
        |> Enum.map(fn name ->
          name
          |> String.replace_suffix(".json", "")
          |> String.replace_prefix(prefix, "")
          |> Integer.parse()
          |> case do
            {n, _} -> n
            _ -> 0
          end
        end)
        |> Enum.max(fn -> 0 end)
        |> Kernel.+(1)

      {:error, _} ->
        1
    end
  end

  defp write_run(board_dir, %Run{} = run), do: Sync.persist_run(board_dir, run)

  defp maybe_advance_task(board_dir, task_id, opts) do
    if Keyword.get(opts, :skip_task_advance, false) do
      :ok
    else
      # A missing tasks/<id>.json is fine — some test boards seed tasks
      # only via Loader and never use the CLI exporter. Advancement is
      # best-effort; failures here don't fail the Run.
      case Sync.advance_task(board_dir, task_id, :done) do
        {:ok, _} -> :ok
        {:error, {:task_not_found, _}} -> :ok
        {:error, reason} -> {:error, {:task_advance_failed, reason}}
      end
    end
  end

  # Success-side cascade (wb-0mqz.4 / org-edna :TRIGGER:). After a
  # task transitions to :done, advance every :trigger target whose
  # current state is :blocked to :ready. Behind opts[:skip_cascade]
  # so callers that own their own state machine (richer orchestrator)
  # can opt out. Missing task JSONs are silently no-op'd (some test
  # boards skip the exporter step).
  defp maybe_cascade_success(board_dir, task_id, opts) do
    if Keyword.get(opts, :skip_cascade, false) do
      :ok
    else
      case Sync.cascade_success(board_dir, task_id) do
        {:ok, _advanced} -> :ok
        {:error, {:task_not_found, _}} -> :ok
        {:error, reason} -> {:error, {:cascade_success_failed, reason}}
      end
    end
  end

  # Failure-side cascade (wb-0mqz.14). After a Run terminates as
  # :failed, walk dependents whose :blocker includes the failed task
  # and promote them to :blocked with a "failed dep: <id>" reason.
  # Same opt-out (opts[:skip_cascade]) as the success path — both
  # cascades are part of the same propagation policy. Errors here
  # don't override the original failure reason returned to the
  # caller; we surface them via the new error tuple but the failure
  # path always returns {:error, original_reason} for back-compat.
  defp maybe_cascade_failure(board_dir, task_id, opts) do
    if Keyword.get(opts, :skip_cascade, false) do
      :ok
    else
      case Sync.cascade_failure(board_dir, task_id) do
        {:ok, _blocked} -> :ok
        # Filesystem errors during cascade are best-effort — don't
        # mask the original Run failure. Returning :ok here is
        # intentional; if cascade can't complete the orchestrator
        # will discover stale dependents on the next tick and a
        # re-cascade (or human intervention) sorts it.
        {:error, _reason} -> :ok
      end
    end
  end

  defp now_iso8601 do
    DateTime.utc_now() |> DateTime.to_iso8601()
  end

  # ── wb-jnjc telemetry helpers ──────────────────────────────────────
  #
  # Events emitted (each as a :telemetry.span pair — `:start` + `:stop`
  # or `:exception` on raise):
  #
  #   [:worg_agent, :llm, :turn] — wraps one Llm.call. Metadata:
  #     :start  → %{message_count, tools_present, board_dir}
  #     :stop   → %{status: :ok | :error, stop_reason, tokens_in,
  #                 tokens_out, content_size, tool_call_count}
  #
  #   [:worg_agent, :tool_call] — wraps one tool dispatch. Metadata:
  #     :start  → %{tool_name, tool_call_id, args}
  #     :stop   → %{tool_name, tool_call_id, status: :ok | :error,
  #                 result_size}
  #
  # Subscribers (live-UI WebSocket channel, OTel exporter, debug
  # printer) attach handlers without the Loop knowing about them.
  # `:telemetry` is a transitive dep through Req — no new package.

  defp llm_turn_metadata(messages, _ctx) do
    %{
      message_count: length(messages),
      # tools_present is captured here as a hint — actual tool list
      # lives in the closure and would bloat the metadata payload.
      board_dir: nil
    }
  end

  defp llm_turn_result_metadata({:ok, %Response{} = resp}) do
    %{
      status: :ok,
      stop_reason: resp.stop_reason,
      tokens_in: get_in(resp.usage, [:tokens_in]) || 0,
      tokens_out: get_in(resp.usage, [:tokens_out]) || 0,
      content_size: byte_size(resp.content || ""),
      tool_call_count: length(resp.tool_calls || [])
    }
  end

  defp llm_turn_result_metadata({:error, reason}) do
    %{status: :error, error: reason}
  end

  # wb-6t1r: if the task carries a :STAGE_MODEL: override, merge it
  # into opts[:llm_opts][:model] so the existing Llm.call dispatch
  # picks it up unchanged. Falls through (returns opts unchanged) when
  # the task has no stage_model set.
  defp override_model_from_task(opts, %Task{stage_model: nil}), do: opts

  defp override_model_from_task(opts, %Task{stage_model: model}) when is_binary(model) do
    llm_opts =
      opts
      |> Keyword.get(:llm_opts, [])
      |> Keyword.put(:model, model)

    Keyword.put(opts, :llm_opts, llm_opts)
  end
end
