defmodule WorgTest do
  use ExUnit.Case, async: true

  @sample """
  * TODO First task
  :PROPERTIES:
  :ID: task-1
  :END:
  body
  ** DONE Subtask
  """

  describe "parse/1" do
    test "round-trip-clean source returns {:ok, src}" do
      assert {:ok, @sample} = Worg.parse(@sample)
    end

    test "empty document is clean" do
      assert {:ok, ""} = Worg.parse("")
    end
  end

  describe "round_trip_ok?/1" do
    test "true for well-formed source" do
      assert Worg.round_trip_ok?(@sample) == true
    end
  end

  describe "transition_todo/3" do
    test "moves TODO → DONE" do
      assert {:ok, updated} = Worg.transition_todo(@sample, "task-1", "DONE")
      assert updated =~ "* DONE First task"
    end

    test "unknown id returns error" do
      assert {:error, _} = Worg.transition_todo(@sample, "no-such-id", "DONE")
    end
  end

  describe "append_logbook/3" do
    test "appends to :LOGBOOK: drawer (creates if absent)" do
      assert {:ok, updated} = Worg.append_logbook(@sample, "task-1", "spike done")
      assert updated =~ ":LOGBOOK:"
      assert updated =~ "spike done"
    end
  end

  describe "append_drawer/4" do
    test "appends to a named drawer" do
      assert {:ok, updated} = Worg.append_drawer(@sample, "task-1", "NOTES", "context")
      assert updated =~ ":NOTES:"
      assert updated =~ "context"
    end
  end

  describe "set_property/4" do
    test "sets a property in :PROPERTIES:" do
      assert {:ok, updated} = Worg.set_property(@sample, "task-1", "BLOCKER", "ids(other)")
      assert updated =~ ":BLOCKER: ids(other)"
    end

    test "refuses to set :ID:" do
      assert {:error, _} = Worg.set_property(@sample, "task-1", "ID", "task-2")
    end
  end

  describe "add_child/5" do
    test "inserts a child headline with state + :ID:" do
      assert {:ok, updated} =
               Worg.add_child(@sample, "task-1", "Sub-thing", "NEXT", "task-1.1")

      assert updated =~ "NEXT Sub-thing"
      assert updated =~ ":ID: task-1.1"
    end

    test "state nil yields a plain headline" do
      assert {:ok, updated} =
               Worg.add_child(@sample, "task-1", "Plain", nil, "task-1.2")

      assert updated =~ "Plain"
      assert updated =~ ":ID: task-1.2"
    end
  end

  describe "write_results/3" do
    setup do
      src = """
      * TODO Compute
      :PROPERTIES:
      :ID: compute
      :END:
      #+begin_src bash
      echo hi
      #+end_src
      """

      {:ok, src: src}
    end

    test "writes a #+RESULTS: block under the first source block", %{src: src} do
      assert {:ok, updated} = Worg.write_results(src, "compute", "hi")
      assert updated =~ "#+RESULTS:"
      assert updated =~ "hi"
    end
  end

  describe "query/2" do
    test "returns headlines matching the predicate" do
      assert {:ok, results} = Worg.query(@sample, %{"kind" => "state", "state" => "TODO"})
      assert is_list(results)
      titles = Enum.map(results, & &1["title"])
      assert "First task" in titles
      refute "Subtask" in titles
    end
  end
end
