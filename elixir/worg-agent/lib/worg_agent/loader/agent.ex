defmodule WorgAgent.Loader.Agent do
  @moduledoc """
  Elixir mirror of the orchestrator-protocol `Agent` wire format.
  Fields match `packages/worg/crates/worg-orch/src/lib.rs::Agent`
  byte-for-byte at the JSON level.

  Application-layer fields the protocol doesn't transmit (`model`,
  `tools`, `system_prompt`) come from reading the WORG source file
  directly, not from `agents.json`. They land in a separate struct
  when wb-nlln.21.5 wires that path; for now this struct carries only
  the wire fields.
  """

  @enforce_keys [:id, :name, :kind, :status]
  defstruct [
    :id,
    :name,
    :kind,
    :status,
    :runtime,
    :role,
    :reports_to,
    :heartbeat_sec,
    capabilities: []
  ]

  @type kind :: :ai | :human
  @type status :: :active | :paused | :terminated

  @type t :: %__MODULE__{
          id: String.t(),
          name: String.t(),
          kind: kind(),
          status: status(),
          runtime: String.t() | nil,
          role: String.t() | nil,
          reports_to: String.t() | nil,
          heartbeat_sec: non_neg_integer() | nil,
          capabilities: [String.t()]
        }

  @doc """
  Construct an `%Agent{}` from a wire-decoded JSON map.
  Unknown fields are silently dropped; missing optional fields default.
  """
  @spec from_wire(map) :: t()
  def from_wire(%{} = wire) do
    %__MODULE__{
      id: Map.fetch!(wire, "id"),
      name: Map.fetch!(wire, "name"),
      kind: decode_kind(Map.fetch!(wire, "type")),
      status: decode_status(Map.fetch!(wire, "status")),
      runtime: Map.get(wire, "runtime"),
      role: Map.get(wire, "role"),
      reports_to: Map.get(wire, "reports_to"),
      heartbeat_sec: Map.get(wire, "heartbeat_sec"),
      capabilities: Map.get(wire, "capabilities", [])
    }
  end

  defp decode_kind("ai"), do: :ai
  defp decode_kind("human"), do: :human
  defp decode_kind(other), do: raise(ArgumentError, "unknown agent type: #{inspect(other)}")

  defp decode_status("active"), do: :active
  defp decode_status("paused"), do: :paused
  defp decode_status("terminated"), do: :terminated
  defp decode_status(other), do: raise(ArgumentError, "unknown agent status: #{inspect(other)}")
end
