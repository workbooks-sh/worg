defmodule WorgAgent.Scheduler do
  @moduledoc """
  Topological wave-planner for task DAGs. Given a list of tasks with
  declared dependencies, produces a sequence of waves where every
  task in a wave can run concurrently (all its dependencies are
  satisfied by earlier waves). Pure data transformation — no
  concurrency primitives, no side effects, no Oban.

  Pairs with the worg-agent Loop, which today executes tasks
  serially. The Scheduler's role is the *plan*; the integration that
  *dispatches* a wave via Task.async_stream / Oban / equivalent is
  the next ticket (see follow-up filed under wb-lrul). This module
  ships the foundation: deterministic wave assignment, cycle
  detection, and a `next_ready/2` predicate the Loop can use once
  integration lands.

  wb-lrul (primitive only — Loop integration deferred).

  ## Use

      iex> tasks = [
      ...>   {"research", []},
      ...>   {"script", ["research"]},
      ...>   {"velocity", ["research"]},
      ...>   {"storyboard", ["script", "velocity"]},
      ...>   {"shots", ["storyboard"]}
      ...> ]
      iex> WorgAgent.Scheduler.topological_waves(tasks)
      {:ok, [["research"], ["script", "velocity"], ["storyboard"], ["shots"]]}

  Wave 2 (`script` + `velocity`) demonstrates the speedup: both can
  fan out concurrently once research lands.

  ## Cycle detection

  If the task list contains a cycle (`a → b → a`), returns
  `{:error, {:cycle, [task_ids_in_cycle]}}`. No partial result.

  ## Unknown dependency detection

  If a task lists a `depends_on` id that isn't in the task list,
  returns `{:error, {:unknown_dep, task_id, dep_id}}`. Forces the
  caller to either include the dep task or remove the reference —
  silently treating unknown deps as satisfied would mask real bugs.
  """

  @type task_id :: String.t() | atom()
  @type task_spec :: {task_id(), [task_id()]}
  @type wave :: [task_id()]

  @doc """
  Plan execution waves for a list of `{task_id, depends_on}` pairs.

  Returns `{:ok, [wave1, wave2, ...]}` on success or
  `{:error, term}` on detected cycles / unknown deps.

  Task ordering within each wave is deterministic: tasks are
  emitted in the same order they appeared in the input list. This
  is important for reproducibility — a downstream dispatcher might
  prefer to schedule expensive tasks first, but that's a policy
  decision above this layer.
  """
  @spec topological_waves([task_spec()]) ::
          {:ok, [wave()]} | {:error, {:cycle, [task_id()]} | {:unknown_dep, task_id(), task_id()}}
  def topological_waves(tasks) when is_list(tasks) do
    with :ok <- check_unknown_deps(tasks) do
      ids = Enum.map(tasks, &elem(&1, 0))
      deps_map = Map.new(tasks)
      build_waves(ids, deps_map, MapSet.new(), [])
    end
  end

  @doc """
  Given a task list and a set of already-completed task ids, return
  the list of task ids that are ready to dispatch NOW: their
  dependencies are all in `completed`, and they themselves are not
  in `completed`. Order matches input order.

  Use to drive incremental dispatch: after each wave's tasks
  complete, merge them into the completed set and call again for
  the next batch.
  """
  @spec next_ready([task_spec()], MapSet.t(task_id())) :: [task_id()]
  def next_ready(tasks, completed) when is_list(tasks) do
    Enum.flat_map(tasks, fn {id, deps} ->
      cond do
        MapSet.member?(completed, id) -> []
        Enum.all?(deps, &MapSet.member?(completed, &1)) -> [id]
        true -> []
      end
    end)
  end

  # ── Internals ─────────────────────────────────────────────────────

  defp check_unknown_deps(tasks) do
    known = MapSet.new(tasks, &elem(&1, 0))

    Enum.reduce_while(tasks, :ok, fn {id, deps}, _acc ->
      case Enum.find(deps, fn d -> not MapSet.member?(known, d) end) do
        nil -> {:cont, :ok}
        bad -> {:halt, {:error, {:unknown_dep, id, bad}}}
      end
    end)
  end

  # Build waves greedily: each iteration picks every task whose deps
  # are all in `completed`. If a pass yields zero new tasks while
  # uncompleted tasks remain, we have a cycle.
  defp build_waves(remaining, deps_map, completed, acc) do
    {ready, still} = partition_ready(remaining, deps_map, completed)

    cond do
      ready == [] and still == [] ->
        {:ok, Enum.reverse(acc)}

      ready == [] and still != [] ->
        # No progress AND uncompleted tasks → cycle among `still`
        {:error, {:cycle, still}}

      true ->
        new_completed = Enum.reduce(ready, completed, &MapSet.put(&2, &1))
        build_waves(still, deps_map, new_completed, [ready | acc])
    end
  end

  defp partition_ready(ids, deps_map, completed) do
    Enum.split_with(ids, fn id ->
      deps = Map.fetch!(deps_map, id)
      Enum.all?(deps, &MapSet.member?(completed, &1))
    end)
  end
end
