defmodule WorgAgent.Sync do
  @moduledoc """
  Persists Run state to the orchestrator board and folds it back into
  the source `.org` plan file. Two operations:

  1. `persist_run/2` — write a `%Run{}` to
     `<board>/runs/<task-id>-<attempt>.json`. Idempotent at the file
     level: a run with the same id overwrites in place (terminal runs
     SHOULD be immutable per protocol, so callers should bump
     `attempt` rather than re-writing). Used by `Loop.run_next/2` at
     the end of every run.

  2. `fold_into_org/3` — invoke `worg orch import runs <plan.org>`
     as a subprocess, which reads runs from the board and appends
     `:LOGBOOK:` entries + transitions TODO keywords in the source
     `.org` file. Idempotent: re-running detects already-imported
     run ids via the `run=<id>` marker and skips them. The CLI path
     works today; the NIF path lands once `wb-6irl.33` (Wasmex
     bridge) + a `worg-nif` import binding both exist.
  """

  alias WorgAgent.Run

  # sync.ex lives at packages/worg/elixir/worg-agent/lib/worg_agent/sync.ex
  # The Rust target binary is at packages/worg/target/debug/worg —
  # four directory hops up from __DIR__.
  @default_worg_bin Path.expand("../../../../target/debug/worg", __DIR__)

  @doc """
  Write a `%Run{}` to `<board_dir>/runs/<run-id>.json`. Creates the
  runs directory if missing. Returns `{:ok, %Run{}}` on success,
  `{:error, term}` on filesystem failure.

  Per protocol, terminal runs are append-only — callers should never
  re-write an existing run with the same id. We don't enforce that
  here (no `exists?` check) because `Loop` always increments
  `attempt` before calling us; the caller owns that invariant.
  """
  @spec persist_run(Path.t(), Run.t()) :: {:ok, Run.t()} | {:error, term}
  def persist_run(board_dir, %Run{} = run) do
    runs_dir = Path.join(board_dir, "runs")

    with :ok <- File.mkdir_p(runs_dir) do
      path = Path.join(runs_dir, "#{run.id}.json")
      json = run |> Run.to_wire() |> Jason.encode!(pretty: true)

      case File.write(path, json) do
        :ok -> {:ok, run}
        {:error, reason} -> {:error, {:write_run_failed, path, reason}}
      end
    else
      {:error, reason} -> {:error, {:mkdir_runs_failed, runs_dir, reason}}
    end
  end

  @doc """
  Update `<board_dir>/tasks/<task_id>.json` to a new `state`. Preserves
  every other field in the task JSON byte-for-byte (only the `state`
  key is rewritten). Returns `{:ok, decoded_task}` on success or
  `{:error, term}` on filesystem / JSON parse failure.

  Used by `Loop.run_next/2` after a successful run to advance the
  task from `:ready` (or whatever the export wrote) to `:done`, so
  that `Sync.fold_into_org/3`'s subsequent `worg orch import runs`
  invocation has something to transition the TODO keyword from.

  Accepts the new state as either a snake_case string ("done",
  "in_progress", "failed", etc.) or an atom of the same name.
  Doesn't validate against the protocol's enum — the worg CLI will
  reject unknown states at the next export, and over-validation
  here would couple us to the schema's current shape.
  """
  @spec advance_task(Path.t(), String.t(), atom | String.t()) ::
          {:ok, map} | {:error, term}
  def advance_task(board_dir, task_id, new_state) do
    path = Path.join([board_dir, "tasks", "#{task_id}.json"])
    state_str = if is_atom(new_state), do: Atom.to_string(new_state), else: new_state

    with {:ok, raw} <- File.read(path),
         {:ok, task} <- Jason.decode(raw) do
      updated = Map.put(task, "state", state_str)
      encoded = Jason.encode!(updated, pretty: true)

      case File.write(path, encoded) do
        :ok -> {:ok, updated}
        {:error, reason} -> {:error, {:write_task_failed, path, reason}}
      end
    else
      {:error, %Jason.DecodeError{} = e} ->
        {:error, {:task_json_decode_failed, path, e}}

      {:error, :enoent} ->
        {:error, {:task_not_found, path}}

      {:error, reason} ->
        {:error, {:read_task_failed, path, reason}}
    end
  end

  @doc """
  Failure-side cascade (wb-0mqz.14).

  When the task identified by `failed_task_id` has transitioned to
  `:failed` (or its Run finished `:failed`), walk every dependent
  whose `:blocker` includes the failed task and promote them to
  `:blocked` with a `blocked_reason` of `"failed dep: <id>"`. Then
  recurse — a task newly blocked by the cascade may have its own
  dependents, which are now stuck too.

  Mirror of `cascade_success/2` (wb-0mqz.4) — same graph walk in the
  opposite direction. Both are idempotent and best-effort.

  Recursion stops at:
  - terminal targets (`:done`, `:cancelled`) — never regress those.
  - targets already in `:blocked` — already-cascaded; idempotent.
  - cycles — visited set prevents infinite descent. The cycle-
    detection lint (E007 in wb-0mqz.6) should catch cycles at
    author time; this is defense in depth.

  Returns `{:ok, [promoted_id]}` — the ids actually transitioned to
  `:blocked` during this call (empty list if no dependents needed
  blocking). `{:error, _}` only on filesystem failures.
  """
  @spec cascade_failure(Path.t(), String.t()) ::
          {:ok, [String.t()]} | {:error, term}
  def cascade_failure(board_dir, failed_task_id) do
    tasks_dir = Path.join(board_dir, "tasks")

    # Load once; walk the in-memory graph; write each promotion exactly
    # once. wb-qwj8.4 (was O(depth × N) reads, now O(N) reads).
    with {:ok, all_tasks} <- read_all_tasks(tasks_dir) do
      promoted = cascade_block(all_tasks, failed_task_id, MapSet.new(), [])
      write_promotions(promoted, all_tasks, tasks_dir, failed_task_id)
    end
  end

  defp read_all_tasks(tasks_dir) do
    case File.ls(tasks_dir) do
      {:ok, files} ->
        tasks =
          files
          |> Enum.filter(&String.ends_with?(&1, ".json"))
          |> Enum.map(fn name ->
            path = Path.join(tasks_dir, name)

            case File.read(path) do
              {:ok, raw} ->
                case Jason.decode(raw) do
                  {:ok, task} -> {task["id"], task}
                  {:error, _} -> nil
                end

              {:error, _} ->
                nil
            end
          end)
          |> Enum.reject(&is_nil/1)
          |> Map.new()

        {:ok, tasks}

      {:error, :enoent} ->
        {:ok, %{}}

      {:error, reason} ->
        {:error, {:read_tasks_dir_failed, tasks_dir, reason}}
    end
  end

  # In-memory DFS: returns the list of task ids to promote to :blocked,
  # in reverse-discovery order. Does NOT write anything — that's the
  # caller's job. The visited set prevents cycle loops; the all_tasks
  # map is the read source of truth.
  defp cascade_block(all_tasks, failed_id, visited, promoted) do
    if MapSet.member?(visited, failed_id) do
      promoted
    else
      visited = MapSet.put(visited, failed_id)

      all_tasks
      |> Enum.reduce(promoted, fn {id, task}, acc ->
        if depends_on?(task, failed_id) and non_terminal_non_blocked?(task) and
             id not in acc do
          # This task transitions to :blocked. Recurse with `id` as the
          # next "failed" — the cascade chains through its dependents.
          new_acc = [id | acc]
          cascade_block(all_tasks, id, visited, new_acc)
        else
          acc
        end
      end)
    end
  end

  # Write each promotion exactly once. The original failed_task_id is
  # used to attribute the immediate-dependents' blocked_reason; for
  # cascaded dependents (B blocked by A blocked by failed), the
  # blocked_reason references A — the proximate cause — which matches
  # the original implementation's behavior.
  defp write_promotions([], _all_tasks, _tasks_dir, _failed_id), do: {:ok, []}

  defp write_promotions(promoted_rev, all_tasks, tasks_dir, root_failed_id) do
    # promoted_rev was [last_discovered, ..., first_discovered]. The
    # original cascade walked breadth-first along each level; we walked
    # depth-first in-memory but produced the same set. Reverse to
    # restore the breadth-first discovery order callers expect.
    promoted = Enum.reverse(promoted_rev)

    Enum.each(promoted, fn id ->
      task = Map.fetch!(all_tasks, id)

      # Find the immediate trigger: the first blocker of this task that's
      # already promoted OR is the root failed id. This preserves the
      # "blocked_reason: failed dep: <proximate-id>" semantics from the
      # original O(N²) implementation.
      proximate = proximate_trigger(task, promoted, root_failed_id)

      updated =
        task
        |> Map.put("state", "blocked")
        |> Map.put("blocked_reason", "failed dep: #{proximate}")

      path = Path.join(tasks_dir, "#{id}.json")
      _ = File.write(path, Jason.encode!(updated, pretty: true))
    end)

    {:ok, promoted}
  end

  # The proximate trigger for a cascaded block is any of the task's
  # blockers that's either the root failed id or itself in the promoted
  # set. First match wins for stability.
  defp proximate_trigger(%{"blocker" => deps}, promoted, root_id) when is_list(deps) do
    Enum.find(deps, root_id, fn dep -> dep == root_id or dep in promoted end)
  end

  defp proximate_trigger(_task, _promoted, root_id), do: root_id

  defp depends_on?(%{"blocker" => deps}, id) when is_list(deps), do: id in deps
  defp depends_on?(_, _), do: false

  defp non_terminal_non_blocked?(%{"state" => state}) do
    state not in ["done", "cancelled", "blocked"]
  end

  defp non_terminal_non_blocked?(_), do: true

  @doc """
  Success-side cascade (wb-0mqz.4 / org-edna `:TRIGGER:` semantic).

  When the task identified by `done_task_id` has transitioned to
  `:done`, advance every `:trigger` target whose current state is
  `:blocked` to `:ready`. Tasks already past `:blocked` (`:in_progress`,
  `:done`, `:cancelled`, `:input_required`, `:review`) are left
  untouched — `:trigger` only unblocks; it doesn't regress state.

  Reads `<board>/tasks/<done_task_id>.json` to get the trigger list,
  then rewrites each affected target's `<board>/tasks/<id>.json`.
  Returns `{:ok, [advanced_target_id]}` listing the ids that were
  actually advanced (the empty list is a successful no-op — no
  targets needed unblocking).

  Mirror of `cascade_failure/2` (wb-0mqz.14, blocked-side cascade).
  Both walk the dependency graph in opposite directions; both are
  best-effort and idempotent.

  Missing `<done_task_id>.json` returns `{:error, {:task_not_found,
  path}}`. Missing trigger targets are silently skipped — they
  may have been deleted by the time the cascade runs; the
  unresolved-trigger lint (wb-0mqz.7) catches that at author time.
  """
  @spec cascade_success(Path.t(), String.t()) ::
          {:ok, [String.t()]} | {:error, term}
  def cascade_success(board_dir, done_task_id) do
    done_path = Path.join([board_dir, "tasks", "#{done_task_id}.json"])

    with {:ok, raw} <- File.read(done_path),
         {:ok, done_task} <- Jason.decode(raw) do
      triggers = Map.get(done_task, "trigger", [])
      advanced = Enum.flat_map(triggers, &maybe_advance_trigger_target(board_dir, &1))
      {:ok, advanced}
    else
      {:error, :enoent} -> {:error, {:task_not_found, done_path}}
      {:error, %Jason.DecodeError{} = e} -> {:error, {:task_json_decode_failed, done_path, e}}
      {:error, reason} -> {:error, {:read_task_failed, done_path, reason}}
    end
  end

  defp maybe_advance_trigger_target(board_dir, target_id) do
    target_path = Path.join([board_dir, "tasks", "#{target_id}.json"])

    case File.read(target_path) do
      {:ok, raw} ->
        case Jason.decode(raw) do
          {:ok, %{"state" => "blocked"} = target} ->
            updated = Map.put(target, "state", "ready")

            case File.write(target_path, Jason.encode!(updated, pretty: true)) do
              :ok -> [target_id]
              {:error, _reason} -> []
            end

          {:ok, _other_state} ->
            # Target is :ready, :in_progress, :done, etc. — :trigger
            # only unblocks; it doesn't regress state. Skip silently.
            []

          {:error, _decode} ->
            []
        end

      {:error, :enoent} ->
        # Trigger pointed at a task that's not (or no longer) on the
        # board. The unresolved-:TRIGGER: lint (wb-0mqz.7) flags this
        # at author time; at runtime we silently skip rather than
        # crash a successful Loop iteration.
        []

      {:error, _reason} ->
        []
    end
  end

  @doc """
  Fold orchestrator runs back into the source `.org` plan file by
  invoking `worg orch import runs <plan.org> --from <board_dir>`.

  Options:
  - `:worg_bin` — path to the `worg` CLI binary. Defaults to
    `<worg-agent>/../../target/debug/worg`, i.e. the binary built by
    `cargo build --bin worg` in the worg crate workspace.
  - `:dry_run` — pass `--dry-run` through to the CLI; the org file
    is not modified.

  Returns `{:ok, stderr_summary}` on a zero-exit subprocess (the CLI
  emits its summary to stderr), or `{:error, {:nonzero, exit_code,
  stderr}}` otherwise. Filesystem errors (binary missing, plan file
  unreadable) surface as `{:error, {:invocation_failed, reason}}`.
  """
  @spec fold_into_org(Path.t(), Path.t(), keyword) ::
          {:ok, String.t()} | {:error, term}
  def fold_into_org(board_dir, plan_org, opts \\ []) do
    bin = Keyword.get(opts, :worg_bin, @default_worg_bin)
    dry_run? = Keyword.get(opts, :dry_run, false)

    args =
      ["orch", "import", "runs", plan_org, "--from", board_dir] ++
        if dry_run?, do: ["--dry-run"], else: []

    cond do
      not File.exists?(bin) ->
        {:error,
         {:invocation_failed,
          "worg binary not found at #{bin} — run `cargo build --bin worg` in packages/worg/"}}

      not File.exists?(plan_org) ->
        {:error, {:invocation_failed, "plan .org file not found at #{plan_org}"}}

      true ->
        # System.cmd merges stderr→stdout when configured; the CLI
        # writes its summary to stderr, so we want that captured.
        case System.cmd(bin, args, stderr_to_stdout: true) do
          {output, 0} ->
            {:ok, output}

          {output, code} ->
            {:error, {:nonzero, code, output}}
        end
    end
  end
end
