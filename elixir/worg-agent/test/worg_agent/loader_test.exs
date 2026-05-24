defmodule WorgAgent.LoaderTest do
  use ExUnit.Case, async: true

  alias WorgAgent.Loader
  alias WorgAgent.Loader.{Agent, Plan, Task}

  @fixture_board Path.expand("../fixtures/wb-orch", __DIR__)

  describe "load/1 against the watershed-autoloop fixture" do
    setup do
      {:ok, plan} = Loader.load(@fixture_board)
      {:ok, plan: plan}
    end

    test "returns a Plan struct", %{plan: plan} do
      assert %Plan{} = plan
    end

    test "loads the workhorse agent with the expected wire fields", %{plan: plan} do
      assert %{"workhorse" => agent} = plan.agents
      assert %Agent{} = agent
      assert agent.id == "workhorse"
      assert agent.name == "Workhorse"
      assert agent.kind == :ai
      assert agent.status == :active
      assert agent.capabilities == ~w(bash read write lua-eval js-eval git worg network)
    end

    test "loads all 9 stages as Task structs", %{plan: plan} do
      assert map_size(plan.tasks) == 9
      assert Enum.all?(plan.tasks, fn {_id, t} -> match?(%Task{}, t) end)

      expected_ids = ~w(
        autoloop-iteration orient pick-issue claim-issue
        implement deploy verify commit-push close-issue
      )

      for id <- expected_ids do
        assert Map.has_key?(plan.tasks, id), "missing task #{id} in #{inspect(Map.keys(plan.tasks))}"
      end
    end

    test "preserves parent edges from outline ancestry", %{plan: plan} do
      # The top-level Iteration has no parent.
      assert plan.tasks["autoloop-iteration"].parent == nil

      # Every other stage is a direct child of autoloop-iteration —
      # they're all level-2 under that headline in the source org file.
      for id <- ~w(orient pick-issue claim-issue implement deploy verify commit-push close-issue) do
        task = plan.tasks[id]
        assert task.parent == "autoloop-iteration", "task #{id} has parent #{inspect(task.parent)}, expected autoloop-iteration"
      end
    end

    test "Workhorse is recorded as :ASSIGNED_AGENT: on the root iteration", %{plan: plan} do
      iter = plan.tasks["autoloop-iteration"]
      assert iter.assigned_to == ["workhorse"]
    end

    test "state defaults to :backlog for a fresh export", %{plan: plan} do
      # The template skill.org has no status tags + no TODO keywords,
      # so the walker emits all tasks as backlog. Loader preserves it.
      assert Enum.all?(Map.values(plan.tasks), &(&1.state == :backlog))
    end

    test "tags survive the round-trip", %{plan: plan} do
      iter = plan.tasks["autoloop-iteration"]
      # The :stage: classification tag + the :sandboxed: trust tag.
      assert "stage" in iter.tags
      assert "sandboxed" in iter.tags
    end

    test "title is the headline title from the org file", %{plan: plan} do
      assert plan.tasks["autoloop-iteration"].title == "Iteration"
      assert plan.tasks["orient"].title == "orient"
      assert plan.tasks["pick-issue"].title == "pick issue"
    end
  end

  describe "load/1 error paths" do
    test "missing board dir returns an error" do
      assert {:error, {:missing, "agents.json"}} = Loader.load("/nonexistent/dir")
    end

    test "missing tasks/ subdir loads cleanly with zero tasks" do
      tmp = create_minimal_board()
      assert {:ok, %Plan{tasks: tasks}} = Loader.load(tmp)
      assert tasks == %{}
      File.rm_rf!(tmp)
    end

    test "malformed agents.json returns an invalid_json error" do
      tmp = System.tmp_dir!() |> Path.join("worg-agent-loader-test-#{:rand.uniform(99_999_999)}")
      File.mkdir_p!(tmp)
      File.write!(Path.join(tmp, "agents.json"), "{ not valid json")
      assert {:error, {:invalid_json, "agents.json", _}} = Loader.load(tmp)
      File.rm_rf!(tmp)
    end

    test "agents.json missing the agents array fails shape validation" do
      tmp = System.tmp_dir!() |> Path.join("worg-agent-loader-test-#{:rand.uniform(99_999_999)}")
      File.mkdir_p!(tmp)
      File.write!(Path.join(tmp, "agents.json"), ~s({"version": 1}))
      assert {:error, {:invalid_shape, "agents.json", _}} = Loader.load(tmp)
      File.rm_rf!(tmp)
    end
  end

  describe "Task.from_wire/1 — blocker extension field (wb-qk6l.3)" do
    test "absent blocker defaults to []" do
      task =
        Task.from_wire(%{
          "id" => "t",
          "title" => "T",
          "state" => "backlog",
          "created_by" => "x",
          "created_at" => "2026-05-23T20:00:00Z"
        })

      assert task.blocker == []
    end

    test "present blocker round-trips as a list" do
      task =
        Task.from_wire(%{
          "id" => "t",
          "title" => "T",
          "state" => "backlog",
          "created_by" => "x",
          "created_at" => "2026-05-23T20:00:00Z",
          "blocker" => ["a", "b", "c"]
        })

      assert task.blocker == ["a", "b", "c"]
    end

    test "empty blocker list is preserved" do
      task =
        Task.from_wire(%{
          "id" => "t",
          "title" => "T",
          "state" => "backlog",
          "created_by" => "x",
          "created_at" => "2026-05-23T20:00:00Z",
          "blocker" => []
        })

      assert task.blocker == []
    end

    test "absent trigger defaults to []" do
      task =
        Task.from_wire(%{
          "id" => "t",
          "title" => "T",
          "state" => "backlog",
          "created_by" => "x",
          "created_at" => "2026-05-23T20:00:00Z"
        })

      assert task.trigger == []
    end

    test "present trigger round-trips as a list" do
      task =
        Task.from_wire(%{
          "id" => "t",
          "title" => "T",
          "state" => "backlog",
          "created_by" => "x",
          "created_at" => "2026-05-23T20:00:00Z",
          "trigger" => ["a", "b"]
        })

      assert task.trigger == ["a", "b"]
    end

    test "absent effort_minutes defaults to nil" do
      task =
        Task.from_wire(%{
          "id" => "t",
          "title" => "T",
          "state" => "backlog",
          "created_by" => "x",
          "created_at" => "2026-05-23T20:00:00Z"
        })

      assert task.effort_minutes == nil
    end

    test "present effort_minutes round-trips as an integer" do
      task =
        Task.from_wire(%{
          "id" => "t",
          "title" => "T",
          "state" => "backlog",
          "created_by" => "x",
          "created_at" => "2026-05-23T20:00:00Z",
          "effort_minutes" => 90
        })

      assert task.effort_minutes == 90
    end
  end

  describe "ready_tasks/1 (wb-0mqz.12)" do
    defp mk_task(id, fields \\ %{}) do
      base = %Task{
        id: id,
        title: id,
        state: :backlog,
        created_by: "test",
        created_at: "2026-05-23T00:00:00Z"
      }

      Enum.reduce(fields, base, fn {k, v}, t -> Map.put(t, k, v) end)
    end

    defp mk_plan(tasks) do
      %Plan{
        agents: %{},
        tasks: Map.new(tasks, fn t -> {t.id, t} end)
      }
    end

    test "empty plan yields []" do
      assert Loader.ready_tasks(%Plan{agents: %{}, tasks: %{}}) == []
    end

    test "single ready task is returned" do
      plan = mk_plan([mk_task("solo")])
      [t] = Loader.ready_tasks(plan)
      assert t.id == "solo"
    end

    test "blocked/in_progress/done/cancelled tasks are excluded" do
      plan =
        mk_plan([
          mk_task("ready", %{state: :ready}),
          mk_task("done", %{state: :done}),
          mk_task("doing", %{state: :in_progress}),
          mk_task("waiting", %{state: :blocked}),
          mk_task("gone", %{state: :cancelled}),
          mk_task("review", %{state: :review}),
          mk_task("input", %{state: :input_required})
        ])

      ids = Loader.ready_tasks(plan) |> Enum.map(& &1.id)
      assert ids == ["ready"]
    end

    test "priority orders results: [#A] < [#B] < [#C] < none" do
      plan =
        mk_plan([
          mk_task("none1"),
          mk_task("a", %{priority: 1}),
          mk_task("c", %{priority: 3}),
          mk_task("b", %{priority: 2}),
          mk_task("none2")
        ])

      ids = Loader.ready_tasks(plan) |> Enum.map(& &1.id)
      # Priority asc, id asc within priority bucket; nil last.
      assert ids == ["a", "b", "c", "none1", "none2"]
    end

    test "same priority breaks ties by id alphabetical" do
      plan =
        mk_plan([
          mk_task("z", %{priority: 1}),
          mk_task("a", %{priority: 1}),
          mk_task("m", %{priority: 1})
        ])

      ids = Loader.ready_tasks(plan) |> Enum.map(& &1.id)
      assert ids == ["a", "m", "z"]
    end

    test "diamond DAG: with A done, returns [B, C]; both done, returns [D]" do
      # A → B, A → C, (B+C) → D
      a_done = mk_task("a", %{state: :done})
      b = mk_task("b", %{blocker: ["a"]})
      c = mk_task("c", %{blocker: ["a"]})
      d = mk_task("d", %{blocker: ["b", "c"]})

      stage1 = mk_plan([a_done, b, c, d])
      ids1 = Loader.ready_tasks(stage1) |> Enum.map(& &1.id)
      assert ids1 == ["b", "c"]

      b_done = mk_task("b", %{state: :done, blocker: ["a"]})
      c_done = mk_task("c", %{state: :done, blocker: ["a"]})
      stage2 = mk_plan([a_done, b_done, c_done, d])
      ids2 = Loader.ready_tasks(stage2) |> Enum.map(& &1.id)
      assert ids2 == ["d"]
    end

    test "task with unmet blocker is excluded" do
      plan =
        mk_plan([
          mk_task("prereq"),
          mk_task("dependent", %{blocker: ["prereq"]})
        ])

      ids = Loader.ready_tasks(plan) |> Enum.map(& &1.id)
      assert ids == ["prereq"]
    end

    test "task with dangling blocker is excluded (fail loud)" do
      plan = mk_plan([mk_task("orphan", %{blocker: ["ghost"]})])
      assert Loader.ready_tasks(plan) == []
    end

    # wb-qwj8.5: explicit design decision. A task whose only blocker is
    # CANCELED stays gated. Rationale: a canceled task didn't produce
    # its output, so any dependent that needed that output cannot
    # proceed. The dependent must be explicitly unblocked (re-author
    # the plan, remove the :BLOCKER:, or mark the canceled task DONE)
    # if the cancellation actually means "skip and continue downstream."
    # See docs/ADR-worg-blocker-from-cancelled.md.
    test "task whose blocker is :cancelled is NOT unlocked" do
      plan =
        mk_plan([
          mk_task("cancelled_dep", %{state: :cancelled}),
          mk_task("dependent", %{blocker: ["cancelled_dep"]})
        ])

      # cancelled_dep itself isn't pickable; dependent stays gated.
      ids = Loader.ready_tasks(plan) |> Enum.map(& &1.id)
      assert ids == []
    end

    test "outline-parent gate: child unpickable while parent in :backlog" do
      plan =
        mk_plan([
          mk_task("parent"),
          mk_task("child", %{parent: "parent"})
        ])

      ids = Loader.ready_tasks(plan) |> Enum.map(& &1.id)
      # Parent is pickable (no parent of its own); child blocked
      # because its parent is :backlog (needs :in_progress or :done).
      assert ids == ["parent"]
    end
  end

  describe "pickable?/2 (wb-0mqz.12)" do
    test "returns false for terminal/blocked/in-progress/etc states" do
      for state <- [:done, :cancelled, :in_progress, :blocked, :input_required, :review] do
        task = %Task{
          id: "t",
          title: "t",
          state: state,
          created_by: "x",
          created_at: "2026-05-23T00:00:00Z"
        }

        refute Loader.pickable?(task, %{"t" => task}),
               "state #{inspect(state)} should NOT be pickable"
      end
    end

    test "returns true for backlog/ready with no parent or blocker" do
      for state <- [:backlog, :ready] do
        task = %Task{
          id: "t",
          title: "t",
          state: state,
          created_by: "x",
          created_at: "2026-05-23T00:00:00Z"
        }

        assert Loader.pickable?(task, %{"t" => task}),
               "state #{inspect(state)} should be pickable"
      end
    end
  end

  describe "Task.terminal?/1" do
    test "done and cancelled are terminal" do
      assert Task.terminal?(:done)
      assert Task.terminal?(:cancelled)
    end

    test "active states are not terminal" do
      refute Task.terminal?(:backlog)
      refute Task.terminal?(:ready)
      refute Task.terminal?(:in_progress)
      refute Task.terminal?(:blocked)
      refute Task.terminal?(:input_required)
      refute Task.terminal?(:review)
    end
  end

  defp create_minimal_board do
    tmp = System.tmp_dir!() |> Path.join("worg-agent-loader-test-#{:rand.uniform(99_999_999)}")
    File.mkdir_p!(tmp)

    File.write!(
      Path.join(tmp, "agents.json"),
      ~s({"version": 1, "agents": [{"id": "x", "name": "X", "type": "ai", "status": "active"}]})
    )

    tmp
  end
end
