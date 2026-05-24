defmodule WorgAgent.Loader.Task do
  @moduledoc """
  Elixir mirror of the orchestrator-protocol `Task` wire format.
  Fields match `packages/worg/crates/worg-orch/src/lib.rs::Task`
  at the JSON level.

  The `parent` field is the outline-tree edge (one parent per task —
  protocol-level invariant). Cross-tree dependencies (`:BLOCKER:`
  in the original org file) live in `blocker`. This started as
  an extension field — orchestrator-core's wire Task doesn't define
  it, but the schema isn't `deny_unknown_fields`, so the worg
  exporter is free to surface it under that key. Consumers that
  don't care (the canonical orchestrator) ignore it; the worg-agent
  Loop honors it in scheduling.
  """

  @enforce_keys [:id, :title, :state, :created_by, :created_at]
  defstruct [
    :id,
    :title,
    :state,
    :created_by,
    :created_at,
    :description,
    :parent,
    :priority,
    :due,
    :reviewer,
    :acceptance,
    :result_summary,
    :result_full,
    :error,
    :blocked_reason,
    :input_required_prompt,
    :updated_at,
    assigned_to: [],
    capabilities: [],
    tags: [],
    comments: [],
    blocker: [],
    trigger: [],
    effort_minutes: nil,
    # wb-6t1r: per-stage LLM model override. When non-nil, the Loop
    # dispatches this task's LLM call to `stage_model` instead of the
    # agent's default :MODEL:. Use case: wavelet's cheap orchestrator
    # loop escalates specific stages to Gemini / Claude Opus.
    stage_model: nil
  ]

  @type state ::
          :backlog
          | :ready
          | :in_progress
          | :input_required
          | :review
          | :done
          | :blocked
          | :cancelled

  @type t :: %__MODULE__{
          id: String.t(),
          title: String.t(),
          state: state(),
          created_by: String.t(),
          created_at: String.t(),
          description: String.t() | nil,
          parent: String.t() | nil,
          priority: integer() | nil,
          due: String.t() | nil,
          reviewer: String.t() | nil,
          acceptance: String.t() | nil,
          result_summary: String.t() | nil,
          result_full: String.t() | nil,
          error: String.t() | nil,
          blocked_reason: String.t() | nil,
          input_required_prompt: String.t() | nil,
          updated_at: String.t() | nil,
          assigned_to: [String.t()],
          capabilities: [String.t()],
          tags: [String.t()],
          comments: list(),
          blocker: [String.t()],
          trigger: [String.t()],
          effort_minutes: non_neg_integer() | nil,
          stage_model: String.t() | nil
        }

  @doc """
  Construct a `%Task{}` from a wire-decoded JSON map.
  """
  @spec from_wire(map) :: t()
  def from_wire(%{} = wire) do
    %__MODULE__{
      id: Map.fetch!(wire, "id"),
      title: Map.fetch!(wire, "title"),
      state: decode_state(Map.fetch!(wire, "state")),
      created_by: Map.fetch!(wire, "created_by"),
      created_at: Map.fetch!(wire, "created_at"),
      description: Map.get(wire, "description"),
      parent: Map.get(wire, "parent"),
      priority: Map.get(wire, "priority"),
      due: Map.get(wire, "due"),
      reviewer: Map.get(wire, "reviewer"),
      acceptance: Map.get(wire, "acceptance"),
      result_summary: Map.get(wire, "result_summary"),
      result_full: Map.get(wire, "result_full"),
      error: Map.get(wire, "error"),
      blocked_reason: Map.get(wire, "blocked_reason"),
      input_required_prompt: Map.get(wire, "input_required_prompt"),
      updated_at: Map.get(wire, "updated_at"),
      assigned_to: Map.get(wire, "assigned_to", []),
      capabilities: Map.get(wire, "capabilities", []),
      tags: Map.get(wire, "tags", []),
      comments: Map.get(wire, "comments", []),
      blocker: Map.get(wire, "blocker", []),
      trigger: Map.get(wire, "trigger", []),
      effort_minutes: Map.get(wire, "effort_minutes"),
      stage_model: Map.get(wire, "stage_model")
    }
  end

  defp decode_state("backlog"), do: :backlog
  defp decode_state("ready"), do: :ready
  defp decode_state("in_progress"), do: :in_progress
  defp decode_state("input_required"), do: :input_required
  defp decode_state("review"), do: :review
  defp decode_state("done"), do: :done
  defp decode_state("blocked"), do: :blocked
  defp decode_state("cancelled"), do: :cancelled
  defp decode_state(other), do: raise(ArgumentError, "unknown task state: #{inspect(other)}")

  @doc """
  True if the state is terminal (no transitions allowed out).
  Mirrors `TaskState::is_terminal` in worg-orch.
  """
  @spec terminal?(state()) :: boolean()
  def terminal?(:done), do: true
  def terminal?(:cancelled), do: true
  def terminal?(_), do: false
end
