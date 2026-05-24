defmodule WorgAgent.Application do
  @moduledoc """
  OTP application entry point. Boots an empty supervisor at scaffold
  time; later issues register their processes here:

  - `WorgAgent.Llm` (HTTP client GenServer or task supervisor) — wb-nlln.21.4
  - Tool execution supervisors — wb-nlln.21.3
  - The agent loop runner — wb-nlln.21.5

  Children are added as they land. Empty is the correct shape for a
  library that's still being built up; OTP doesn't require any children
  to start cleanly.
  """

  use Application

  @impl true
  def start(_type, _args) do
    children = []
    opts = [strategy: :one_for_one, name: WorgAgent.Supervisor]
    Supervisor.start_link(children, opts)
  end
end
