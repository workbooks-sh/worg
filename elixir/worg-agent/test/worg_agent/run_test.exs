defmodule WorgAgent.RunTest do
  use ExUnit.Case, async: true

  alias WorgAgent.Run

  test "id_for/2 follows the protocol convention" do
    assert Run.id_for("orient", 1) == "orient-1"
    assert Run.id_for("wb-nlln.21.5", 3) == "wb-nlln.21.5-3"
  end

  test "to_wire/1 emits the wire shape with state encoded as snake_case" do
    run = %Run{
      id: "x-1",
      task: "x",
      agent: "workhorse",
      state: :completed,
      attempt: 1,
      started_at: "2026-05-23T20:00:00Z",
      finished_at: "2026-05-23T20:00:14Z",
      tokens: %{"input" => 100, "output" => 50},
      cost_usd: 0.0043,
      result_summary: "ok"
    }

    wire = Run.to_wire(run)
    assert wire["state"] == "completed"
    assert wire["id"] == "x-1"
    assert wire["tokens"] == %{"input" => 100, "output" => 50}
    assert wire["finished_at"] == "2026-05-23T20:00:14Z"
    assert wire["result_summary"] == "ok"
  end

  test "to_wire/1 omits nil optional fields per protocol" do
    run = %Run{
      id: "x-1",
      task: "x",
      agent: "workhorse",
      state: :running,
      attempt: 1,
      started_at: "2026-05-23T20:00:00Z"
    }

    wire = Run.to_wire(run)
    refute Map.has_key?(wire, "finished_at")
    refute Map.has_key?(wire, "cost_usd")
    refute Map.has_key?(wire, "tokens")
    refute Map.has_key?(wire, "result_summary")
    refute Map.has_key?(wire, "error")
    refute Map.has_key?(wire, "commits")
    refute Map.has_key?(wire, "artifacts")
  end

  test "to_wire/1 omits empty lists per protocol" do
    run = %Run{
      id: "x-1",
      task: "x",
      agent: "workhorse",
      state: :completed,
      attempt: 1,
      started_at: "2026-05-23T20:00:00Z",
      commits: [],
      artifacts: []
    }

    wire = Run.to_wire(run)
    refute Map.has_key?(wire, "commits")
    refute Map.has_key?(wire, "artifacts")
  end

  test "to_wire/1 includes non-empty lists" do
    run = %Run{
      id: "x-1",
      task: "x",
      agent: "workhorse",
      state: :completed,
      attempt: 1,
      started_at: "2026-05-23T20:00:00Z",
      commits: ["abc123"],
      artifacts: ["out.json"]
    }

    wire = Run.to_wire(run)
    assert wire["commits"] == ["abc123"]
    assert wire["artifacts"] == ["out.json"]
  end

  test "all 4 states encode correctly" do
    for {state, encoded} <- [
          {:running, "running"},
          {:completed, "completed"},
          {:failed, "failed"},
          {:cancelled, "cancelled"}
        ] do
      wire =
        Run.to_wire(%Run{
          id: "x",
          task: "x",
          agent: "a",
          state: state,
          attempt: 1,
          started_at: "t"
        })

      assert wire["state"] == encoded
    end
  end
end
