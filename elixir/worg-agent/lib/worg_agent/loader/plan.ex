defmodule WorgAgent.Loader.Plan do
  @moduledoc """
  A loaded orchestrator board: every agent + every task, keyed by id.
  Hydrated by `WorgAgent.Loader.load/1`; consumed by `WorgAgent.Loop`
  (wb-nlln.21.5) to pick the next ready task and dispatch work.

  Both maps are O(1) lookup by id. Order is not preserved — the
  loop's "next ready task" decision is based on state + parent edges,
  not document order.
  """

  alias WorgAgent.Loader.{Agent, Task}

  @enforce_keys [:agents, :tasks]
  defstruct agents: %{}, tasks: %{}

  @type t :: %__MODULE__{
          agents: %{String.t() => Agent.t()},
          tasks: %{String.t() => Task.t()}
        }
end
