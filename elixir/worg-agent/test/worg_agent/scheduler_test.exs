defmodule WorgAgent.SchedulerTest do
  use ExUnit.Case, async: true

  alias WorgAgent.Scheduler

  describe "topological_waves/1 — basic shapes" do
    test "empty input → empty waves" do
      assert {:ok, []} = Scheduler.topological_waves([])
    end

    test "single task with no deps → one wave of one" do
      assert {:ok, [["only"]]} = Scheduler.topological_waves([{"only", []}])
    end

    test "linear chain → one task per wave" do
      tasks = [
        {"a", []},
        {"b", ["a"]},
        {"c", ["b"]},
        {"d", ["c"]}
      ]

      assert {:ok, [["a"], ["b"], ["c"], ["d"]]} = Scheduler.topological_waves(tasks)
    end

    test "wide fan-out → single wave with multiple tasks" do
      tasks = [
        {"root", []},
        {"a", ["root"]},
        {"b", ["root"]},
        {"c", ["root"]}
      ]

      assert {:ok, [["root"], waves2]} = Scheduler.topological_waves(tasks)
      assert Enum.sort(waves2) == ["a", "b", "c"]
    end

    test "fan-in → joined task waits for all branches" do
      tasks = [
        {"a", []},
        {"b", []},
        {"c", []},
        {"join", ["a", "b", "c"]}
      ]

      assert {:ok, [wave1, ["join"]]} = Scheduler.topological_waves(tasks)
      assert Enum.sort(wave1) == ["a", "b", "c"]
    end

    test "diamond DAG → 3 waves, middle two fan out" do
      # a → b, c → d
      tasks = [
        {"a", []},
        {"b", ["a"]},
        {"c", ["a"]},
        {"d", ["b", "c"]}
      ]

      assert {:ok, [["a"], middle, ["d"]]} = Scheduler.topological_waves(tasks)
      assert Enum.sort(middle) == ["b", "c"]
    end

    test "the wavelet pipeline shape (research → script+velocity → storyboard → ship)" do
      tasks = [
        {"research", []},
        {"script", ["research"]},
        {"velocity", ["research"]},
        {"storyboard", ["script", "velocity"]},
        {"shots", ["storyboard"]},
        {"compose", ["shots"]},
        {"render", ["compose"]}
      ]

      assert {:ok, waves} = Scheduler.topological_waves(tasks)
      assert length(waves) == 6

      assert Enum.at(waves, 0) == ["research"]
      assert Enum.sort(Enum.at(waves, 1)) == ["script", "velocity"]
      assert Enum.at(waves, 2) == ["storyboard"]
      assert Enum.at(waves, 3) == ["shots"]
      assert Enum.at(waves, 4) == ["compose"]
      assert Enum.at(waves, 5) == ["render"]
    end
  end

  describe "topological_waves/1 — error paths" do
    test "self-loop → cycle error" do
      assert {:error, {:cycle, ["a"]}} = Scheduler.topological_waves([{"a", ["a"]}])
    end

    test "two-task cycle → cycle error" do
      assert {:error, {:cycle, _}} =
               Scheduler.topological_waves([{"a", ["b"]}, {"b", ["a"]}])
    end

    test "three-task cycle → cycle error names all participants" do
      tasks = [
        {"a", ["c"]},
        {"b", ["a"]},
        {"c", ["b"]}
      ]

      assert {:error, {:cycle, cycle_ids}} = Scheduler.topological_waves(tasks)
      assert Enum.sort(cycle_ids) == ["a", "b", "c"]
    end

    test "partial DAG with a cycle stops cleanly (no partial waves emitted)" do
      tasks = [
        {"clean", []},
        {"a", ["b"]},
        {"b", ["a"]}
      ]

      assert {:error, {:cycle, _}} = Scheduler.topological_waves(tasks)
    end

    test "unknown dep → explicit error, not silent skip" do
      tasks = [
        {"a", ["ghost"]}
      ]

      assert {:error, {:unknown_dep, "a", "ghost"}} = Scheduler.topological_waves(tasks)
    end

    test "unknown dep on second task fails the whole plan" do
      tasks = [
        {"a", []},
        {"b", ["a", "nope"]}
      ]

      assert {:error, {:unknown_dep, "b", "nope"}} = Scheduler.topological_waves(tasks)
    end
  end

  describe "topological_waves/1 — determinism" do
    test "wave ordering matches input order within each wave" do
      tasks = [
        {"root", []},
        {"z", ["root"]},
        {"a", ["root"]},
        {"m", ["root"]}
      ]

      # Within wave 2, tasks appear in input order (z, a, m) — NOT
      # sorted. The caller can sort if they want; this layer
      # preserves the source ordering.
      assert {:ok, [["root"], ["z", "a", "m"]]} = Scheduler.topological_waves(tasks)
    end

    test "same input → same output every run" do
      tasks = [{"a", []}, {"b", ["a"]}, {"c", ["a"]}, {"d", ["b", "c"]}]

      assert Scheduler.topological_waves(tasks) ==
               Scheduler.topological_waves(tasks)
    end
  end

  describe "next_ready/2" do
    test "empty completed set → only zero-dep tasks are ready" do
      tasks = [{"a", []}, {"b", ["a"]}, {"c", []}]
      assert Scheduler.next_ready(tasks, MapSet.new()) == ["a", "c"]
    end

    test "after completing some tasks, the next layer becomes ready" do
      tasks = [{"a", []}, {"b", ["a"]}, {"c", ["a"]}, {"d", ["b", "c"]}]
      completed = MapSet.new(["a"])
      assert Scheduler.next_ready(tasks, completed) == ["b", "c"]
    end

    test "completed tasks are excluded from the ready list" do
      tasks = [{"a", []}, {"b", []}]
      completed = MapSet.new(["a"])
      assert Scheduler.next_ready(tasks, completed) == ["b"]
    end

    test "task with partially unsatisfied deps is not ready" do
      tasks = [{"d", ["b", "c"]}]
      completed = MapSet.new(["b"])
      assert Scheduler.next_ready(tasks, completed) == []
    end

    test "all done → empty ready list" do
      tasks = [{"a", []}, {"b", ["a"]}]
      completed = MapSet.new(["a", "b"])
      assert Scheduler.next_ready(tasks, completed) == []
    end
  end
end
