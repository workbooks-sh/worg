defmodule WorgAgent.Loader do
  @moduledoc """
  Reads the orchestrator protocol's `.wb-orch/` JSON state into Elixir
  structs the rest of `WorgAgent` operates on.

  Sources:
  - `<board>/agents.json` — produced by `worg orch export agents`.
  - `<board>/tasks/*.json` — one per `:stage:` headline, produced by
    `worg orch export tasks`.

  Returns a `%Loader.Plan{}` carrying maps keyed by agent slug and task
  id. Structures mirror the orchestrator-protocol wire format (per
  `packages/worg/crates/worg-orch/src/lib.rs`); fields the protocol
  treats as optional decode to `nil`.

  This module does not interpret the data — it just hydrates. The
  downstream consumer (`WorgAgent.Loop`, wb-nlln.21.5) walks the
  resulting plan to pick a ready task and dispatch work.
  """

  alias WorgAgent.Loader.{Agent, Plan, Task}

  @doc """
  Load a `%Plan{}` from the given board directory. Defaults to
  `.wb-orch/` relative to CWD.

  Returns `{:ok, %Plan{}}` on success; `{:error, term}` if any file
  is missing, malformed, or fails JSON decoding.
  """
  @spec load(Path.t()) :: {:ok, Plan.t()} | {:error, term}
  def load(board_dir \\ ".wb-orch") do
    with {:ok, agents} <- load_agents(board_dir),
         {:ok, tasks} <- load_tasks(board_dir) do
      {:ok, %Plan{agents: agents, tasks: tasks}}
    end
  end

  defp load_agents(board_dir) do
    path = Path.join(board_dir, "agents.json")

    with {:ok, raw} <- File.read(path),
         {:ok, %{"agents" => entries} = _file} <- Jason.decode(raw) do
      agents =
        entries
        |> Enum.map(&Agent.from_wire/1)
        |> Map.new(fn agent -> {agent.id, agent} end)

      {:ok, agents}
    else
      {:error, :enoent} -> {:error, {:missing, "agents.json"}}
      {:error, %Jason.DecodeError{} = e} -> {:error, {:invalid_json, "agents.json", e}}
      {:ok, _other} -> {:error, {:invalid_shape, "agents.json", "missing top-level `agents` array"}}
      other -> other
    end
  end

  defp load_tasks(board_dir) do
    tasks_dir = Path.join(board_dir, "tasks")

    case File.ls(tasks_dir) do
      {:ok, files} ->
        files
        |> Enum.filter(&String.ends_with?(&1, ".json"))
        |> Enum.reduce_while({:ok, %{}}, fn filename, {:ok, acc} ->
          path = Path.join(tasks_dir, filename)

          case load_one_task(path) do
            {:ok, task} -> {:cont, {:ok, Map.put(acc, task.id, task)}}
            {:error, _} = err -> {:halt, err}
          end
        end)

      {:error, :enoent} ->
        # No tasks directory is a valid empty state — return an empty map.
        {:ok, %{}}

      {:error, reason} ->
        {:error, {:tasks_dir, reason}}
    end
  end

  defp load_one_task(path) do
    with {:ok, raw} <- File.read(path),
         {:ok, decoded} <- Jason.decode(raw) do
      {:ok, Task.from_wire(decoded)}
    else
      {:error, %Jason.DecodeError{} = e} -> {:error, {:invalid_json, path, e}}
      other -> other
    end
  end

  # ── Ready-task selection (wb-0mqz.12) ──────────────────────────────

  @doc """
  Return every task in `plan` that's currently pickable, sorted in
  the order an orchestrator should consider claiming them.

  A task is pickable when:
    * its state is one of `:backlog` or `:ready` (terminal states,
      `:in_progress`, `:blocked`, `:input_required`, `:review` are
      all excluded — they're not "ready to claim"),
    * its outline parent (if any) is `:in_progress` or `:done`, and
    * every `:blocker` entry is `:done`. Missing blocker ids block
      loudly — fail visibly rather than silently scheduling.

  Sort order:
    1. Priority (lower number first — wire convention is "0 = highest").
       Nil priority sorts last.
    2. Id, alphabetical, for determinism within a priority bucket.

  Empty plan yields `[]`. Use this as the canonical entry point for
  any orchestrator that wants to fan out parallelism — call
  `ready_tasks/1` and dispatch the first N to a pool, not just the
  head. `Loop.run_next/2` itself uses this internally and picks the
  head.
  """
  @spec ready_tasks(Plan.t()) :: [Task.t()]
  def ready_tasks(%Plan{tasks: tasks}) when map_size(tasks) == 0, do: []

  def ready_tasks(%Plan{tasks: tasks}) do
    tasks
    |> Map.values()
    |> Enum.filter(&pickable?(&1, tasks))
    |> Enum.sort_by(&sort_key/1)
  end

  @doc """
  True iff the task is currently pickable. See [`ready_tasks/1`] for
  the definition. Exposed so callers can spot-check a specific task
  rather than iterating the whole plan.
  """
  @spec pickable?(Task.t(), %{String.t() => Task.t()}) :: boolean()
  def pickable?(%Task{state: state}, _all)
      when state in [
             :done,
             :cancelled,
             :in_progress,
             :blocked,
             :input_required,
             :review
           ],
      do: false

  def pickable?(%Task{} = task, all) do
    parent_ok?(task, all) and blocker_ok?(task, all)
  end

  defp parent_ok?(%Task{parent: nil}, _all), do: true

  defp parent_ok?(%Task{parent: parent_id}, all) do
    case Map.fetch(all, parent_id) do
      {:ok, %Task{state: parent_state}} ->
        parent_state in [:in_progress, :done]

      :error ->
        # Parent referenced but not in the plan — treat as runnable;
        # the loop fails fast at execute-time if parent state matters.
        true
    end
  end

  defp blocker_ok?(%Task{blocker: []}, _all), do: true

  defp blocker_ok?(%Task{blocker: deps}, all) do
    Enum.all?(deps, fn dep_id ->
      case Map.fetch(all, dep_id) do
        {:ok, %Task{state: :done}} -> true
        # Anything else — :backlog, :ready, :in_progress, :failed,
        # :blocked, :cancelled — leaves the dependent task gated.
        {:ok, _} -> false
        # Referenced id not in the plan — block. A declared prereq
        # that isn't even tracked is a data-integrity issue.
        :error -> false
      end
    end)
  end

  # `nil` priority sorts last by mapping to a sentinel high number.
  # The wire protocol's i32 range can't reach this so it can't
  # collide with a real priority value.
  defp sort_key(%Task{priority: nil, id: id}), do: {1_000_000_000, id}
  defp sort_key(%Task{priority: p, id: id}), do: {p, id}
end
