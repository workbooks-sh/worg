defmodule WorgAgent.Run do
  @moduledoc """
  Elixir mirror of the orchestrator-protocol `Run` wire format. Matches
  `packages/worg/crates/worg-orch/src/lib.rs::Run` at the JSON level so
  `Loop` can write runs Watershed (or any other consumer) reads via
  `worg orch import runs`.

  A run is one execution attempt against a task. Append-only: once
  terminal, the file is immutable; subsequent attempts get incremented
  `attempt` and a new file.
  """

  @enforce_keys [:id, :task, :agent, :state, :attempt, :started_at]
  defstruct [
    :id,
    :task,
    :agent,
    :state,
    :attempt,
    :started_at,
    :finished_at,
    :lease_until,
    :last_heartbeat,
    :tokens,
    :cost_usd,
    :result_summary,
    :result_full,
    :error,
    commits: [],
    artifacts: []
  ]

  @type state :: :running | :completed | :failed | :cancelled

  @type t :: %__MODULE__{
          id: String.t(),
          task: String.t(),
          agent: String.t(),
          state: state(),
          attempt: pos_integer(),
          started_at: String.t(),
          finished_at: String.t() | nil,
          lease_until: String.t() | nil,
          last_heartbeat: String.t() | nil,
          tokens: %{required(String.t()) => non_neg_integer()} | nil,
          cost_usd: float() | nil,
          result_summary: String.t() | nil,
          result_full: String.t() | nil,
          error: String.t() | nil,
          commits: [String.t()],
          artifacts: [String.t()]
        }

  @doc """
  Serialize to the wire JSON shape (string keys, snake_case enum
  values, optional fields omitted when nil/empty per protocol).
  """
  @spec to_wire(t()) :: map()
  def to_wire(%__MODULE__{} = run) do
    base = %{
      "id" => run.id,
      "task" => run.task,
      "agent" => run.agent,
      "state" => encode_state(run.state),
      "attempt" => run.attempt,
      "started_at" => run.started_at
    }

    optional = [
      {"finished_at", run.finished_at},
      {"lease_until", run.lease_until},
      {"last_heartbeat", run.last_heartbeat},
      {"tokens", run.tokens},
      {"cost_usd", run.cost_usd},
      {"result_summary", run.result_summary},
      {"result_full", run.result_full},
      {"error", run.error}
    ]

    base =
      Enum.reduce(optional, base, fn
        {_k, nil}, acc -> acc
        {k, v}, acc -> Map.put(acc, k, v)
      end)

    base
    |> maybe_put_list("commits", run.commits)
    |> maybe_put_list("artifacts", run.artifacts)
  end

  defp maybe_put_list(map, _key, []), do: map
  defp maybe_put_list(map, key, list), do: Map.put(map, key, list)

  defp encode_state(:running), do: "running"
  defp encode_state(:completed), do: "completed"
  defp encode_state(:failed), do: "failed"
  defp encode_state(:cancelled), do: "cancelled"

  @doc """
  Compute the canonical run id from a task id + attempt number.
  Mirrors `run_id` in worg-orch.
  """
  @spec id_for(String.t(), pos_integer()) :: String.t()
  def id_for(task_id, attempt) when is_binary(task_id) and is_integer(attempt) do
    "#{task_id}-#{attempt}"
  end
end
