defmodule WorgAgent.Integration.FullPipelineTest do
  @moduledoc """
  End-to-end pipeline proof for wb-nlln.21.7. Exercises:

      definition (plan.org + workhorse.org)
        → `worg orch export` (org → tasks/*.json + agents.json)
        → `WorgAgent.Loop.run_next/2` for each stage (LLM mocked)
        → `WorgAgent.Sync.fold_into_org/3` (worg orch import runs)
        → `worg lint` on the mutated plan.org
        → `worg orch export tasks` re-export shows updated state

  Lives in `test/integration/` rather than `test/worg_agent/` to keep
  the integration scope visible: this test requires the worg CLI
  binary built (`cargo build --bin worg` from `packages/worg/`).
  The autoloop's verify step builds it; CI does the same.
  """
  use ExUnit.Case, async: false

  alias WorgAgent.{Loop, Sync}

  @worg_pkg Path.expand("../../../..", __DIR__)
  @worg_bin Path.join(@worg_pkg, "target/debug/worg")
  @workhorse_org Path.join(@worg_pkg, "proposed/agents/workhorse.org")
  @w_org Path.join(@worg_pkg, "w.org")
  @fixed_now "2026-05-23T20:00:00Z"

  setup do
    unless File.exists?(@worg_bin) do
      flunk("worg binary missing — run `cargo build --bin worg` in packages/worg/")
    end

    unless File.exists?(@workhorse_org) do
      flunk("workhorse.org fixture missing at #{@workhorse_org}")
    end

    :ok
  end

  defp tmpdir do
    p = System.tmp_dir!() |> Path.join("worg-agent-integration-#{:rand.uniform(99_999_999)}")
    File.mkdir_p!(p)
    p
  end

  # Shared LLM mock — both Loop.run_next/2 calls hit the same plug.
  # Counter advances on each call so the responses are deterministic
  # per stage.
  defp llm_plug(script) do
    counter = :counters.new(1, [])

    fn conn ->
      idx = :counters.get(counter, 1)
      :counters.add(counter, 1, 1)
      body = Enum.at(script, idx) || raise "LLM mock called more times than scripted"

      conn
      |> Plug.Conn.put_resp_content_type("application/json")
      |> Plug.Conn.resp(200, Jason.encode!(body))
    end
  end

  defp text_response(content) do
    %{
      "choices" => [
        %{
          "message" => %{"role" => "assistant", "content" => content},
          "finish_reason" => "stop"
        }
      ],
      "usage" => %{"prompt_tokens" => 12, "completion_tokens" => 6}
    }
  end

  defp run_cli!(args) do
    case System.cmd(@worg_bin, args, stderr_to_stdout: true) do
      {output, 0} ->
        output

      {output, code} ->
        flunk("worg #{Enum.join(args, " ")} exited #{code}\n#{output}")
    end
  end

  test "definition → export → run → import → re-export round-trip" do
    board = tmpdir()
    plan_path = Path.join(board, "plan.org")

    # ── 1. Author a 2-stage plan.org with TODO keywords so state
    #    transitions are observable in the re-export.
    File.write!(plan_path, """
    #+TITLE: integration plan
    #+GLOSSARY: #{@w_org}
    #+TODO: TODO DOING | DONE BLOCKED

    * TODO stage one                                               :stage:
    :PROPERTIES:
    :ID: stage-one
    :ASSIGNED_AGENT: workhorse
    :END:

    Reply with "stage one ack" and stop.

    * TODO stage two                                               :stage:
    :PROPERTIES:
    :ID: stage-two
    :ASSIGNED_AGENT: workhorse
    :END:

    Reply with "stage two ack" and stop.
    """)

    # ── 2. Export tasks + agents to the orchestrator board.
    File.mkdir_p!(Path.join(board, "tasks"))

    run_cli!([
      "orch",
      "export",
      "tasks",
      plan_path,
      "--to",
      Path.join(board, "tasks"),
      "--created-at",
      @fixed_now
    ])

    run_cli!(["orch", "export", "agents", @workhorse_org, "--to", board])

    # Sanity: the orchestrator board has what we expect.
    assert File.exists?(Path.join(board, "agents.json"))
    assert File.exists?(Path.join([board, "tasks", "stage-one.json"]))
    assert File.exists?(Path.join([board, "tasks", "stage-two.json"]))

    initial_one =
      Path.join([board, "tasks", "stage-one.json"]) |> File.read!() |> Jason.decode!()

    assert initial_one["state"] == "ready"
    assert initial_one["assigned_to"] == ["workhorse"]

    # ── 3. Run both stages through Loop with a scripted LLM.
    plug = llm_plug([text_response("stage one ack"), text_response("stage two ack")])

    llm_opts = [
      api_key: "test-key",
      endpoint: "https://test/api/v1/chat/completions",
      req_options: [plug: plug]
    ]

    {:ok, run1} =
      Loop.run_next(board,
        task_id: "stage-one",
        llm_opts: llm_opts,
        trust_level: :sandboxed,
        working_dir: board,
        now_iso8601: @fixed_now
      )

    {:ok, run2} =
      Loop.run_next(board,
        task_id: "stage-two",
        llm_opts: llm_opts,
        trust_level: :sandboxed,
        working_dir: board,
        now_iso8601: @fixed_now
      )

    assert run1.state == :completed
    assert run1.task == "stage-one"
    assert run1.agent == "workhorse"
    assert run1.result_summary == "stage one ack"

    assert run2.state == :completed
    assert run2.task == "stage-two"

    # ── 4. Both Run JSONs landed on disk in the wire shape.
    run1_path = Path.join([board, "runs", "stage-one-1.json"])
    run2_path = Path.join([board, "runs", "stage-two-1.json"])
    assert File.exists?(run1_path)
    assert File.exists?(run2_path)

    decoded1 = run1_path |> File.read!() |> Jason.decode!()
    assert decoded1["state"] == "completed"
    assert decoded1["task"] == "stage-one"
    assert decoded1["agent"] == "workhorse"

    # ── 5. Loop.run_next/2 already advanced tasks/*.json state to
    #    "done" on each successful run (wb-qk6l.1). Verify before
    #    folding so a regression here surfaces as a clear failure
    #    rather than a downstream TODO-transition assertion.
    for task_id <- ["stage-one", "stage-two"] do
      decoded =
        Path.join([board, "tasks", "#{task_id}.json"])
        |> File.read!()
        |> Jason.decode!()

      assert decoded["state"] == "done"
    end

    # ── 6. Fold runs back into the source .org file.
    {:ok, import_output} = Sync.fold_into_org(board, plan_path)
    assert String.contains?(import_output, "imported 2 logbook")
    assert String.contains?(import_output, "transitioned 2 TODO")

    mutated = File.read!(plan_path)
    assert String.contains?(mutated, ":LOGBOOK:")
    assert String.contains?(mutated, "run=stage-one-1")
    assert String.contains?(mutated, "run=stage-two-1")
    assert String.contains?(mutated, "state=completed")
    assert String.contains?(mutated, "agent=workhorse")
    # Both stage headlines should have transitioned TODO → DONE.
    assert String.contains?(mutated, "* DONE stage one")
    assert String.contains?(mutated, "* DONE stage two")

    # ── 7. worg lint on the mutated plan.org still passes clean.
    lint_output = run_cli!(["lint", plan_path])
    assert String.contains?(lint_output, "clean")
    refute String.contains?(lint_output, "error ")

    # ── 8. Re-exporting tasks from the mutated plan yields updated
    #    state. This is the closing assertion — the round-trip
    #    survives a definition→export→run→import→re-export cycle.
    re_export_dir = Path.join(board, "tasks-reexport")
    File.mkdir_p!(re_export_dir)

    run_cli!([
      "orch",
      "export",
      "tasks",
      plan_path,
      "--to",
      re_export_dir,
      "--created-at",
      @fixed_now
    ])

    reexported_one =
      re_export_dir |> Path.join("stage-one.json") |> File.read!() |> Jason.decode!()

    reexported_two =
      re_export_dir |> Path.join("stage-two.json") |> File.read!() |> Jason.decode!()

    assert reexported_one["state"] == "done"
    assert reexported_two["state"] == "done"
    assert reexported_one["id"] == "stage-one"
    assert reexported_two["id"] == "stage-two"

    File.rm_rf!(board)
  end

  test "Sync.fold_into_org is idempotent across the full pipeline" do
    # Smaller scope: confirm the import-side idempotency holds when
    # invoked twice on a real Loop-produced board. Distinct from the
    # SyncTest unit test in that it uses an actual Loop run (not a
    # hand-rolled Run struct).
    board = tmpdir()
    plan_path = Path.join(board, "plan.org")

    File.write!(plan_path, """
    #+TITLE: idempotency
    #+GLOSSARY: #{@w_org}
    #+TODO: TODO DOING | DONE BLOCKED

    * TODO solo                                                    :stage:
    :PROPERTIES:
    :ID: solo
    :ASSIGNED_AGENT: workhorse
    :END:

    Reply with "ok" and stop.
    """)

    File.mkdir_p!(Path.join(board, "tasks"))

    run_cli!([
      "orch",
      "export",
      "tasks",
      plan_path,
      "--to",
      Path.join(board, "tasks"),
      "--created-at",
      @fixed_now
    ])

    run_cli!(["orch", "export", "agents", @workhorse_org, "--to", board])

    plug = llm_plug([text_response("ok")])

    {:ok, _run} =
      Loop.run_next(board,
        task_id: "solo",
        llm_opts: [
          api_key: "k",
          endpoint: "https://test/api/v1/chat/completions",
          req_options: [plug: plug]
        ],
        trust_level: :sandboxed,
        working_dir: board,
        now_iso8601: @fixed_now
      )

    {:ok, first_output} = Sync.fold_into_org(board, plan_path)
    assert String.contains?(first_output, "imported 1 logbook")
    size_after_first = File.stat!(plan_path).size

    {:ok, second_output} = Sync.fold_into_org(board, plan_path)
    assert String.contains?(second_output, "skipped 1 already-imported")
    size_after_second = File.stat!(plan_path).size

    assert size_after_first == size_after_second

    mutated = File.read!(plan_path)
    # Marker appears exactly once.
    assert mutated |> String.split("run=solo-1") |> length() == 2

    File.rm_rf!(board)
  end
end
