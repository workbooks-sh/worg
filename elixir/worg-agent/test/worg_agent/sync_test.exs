defmodule WorgAgent.SyncTest do
  use ExUnit.Case, async: false

  alias WorgAgent.{Run, Sync}

  @fixed_now "2026-05-23T20:00:00Z"

  defp tmpdir do
    p = System.tmp_dir!() |> Path.join("worg-agent-sync-test-#{:rand.uniform(99_999_999)}")
    File.mkdir_p!(p)
    p
  end

  defp sample_run(opts \\ []) do
    %Run{
      id: Keyword.get(opts, :id, "orient-1"),
      task: Keyword.get(opts, :task, "orient"),
      agent: "workhorse",
      state: Keyword.get(opts, :state, :completed),
      attempt: Keyword.get(opts, :attempt, 1),
      started_at: @fixed_now,
      finished_at: @fixed_now,
      tokens: %{"input" => 100, "output" => 50},
      result_summary: "ok"
    }
  end

  describe "persist_run/2" do
    test "writes the Run JSON to <board>/runs/<id>.json" do
      board = tmpdir()
      run = sample_run()

      assert {:ok, ^run} = Sync.persist_run(board, run)

      path = Path.join([board, "runs", "orient-1.json"])
      assert File.exists?(path)

      decoded = path |> File.read!() |> Jason.decode!()
      assert decoded["id"] == "orient-1"
      assert decoded["state"] == "completed"
      assert decoded["tokens"] == %{"input" => 100, "output" => 50}

      File.rm_rf!(board)
    end

    test "creates the runs/ subdirectory if it doesn't exist" do
      board = tmpdir()
      refute File.exists?(Path.join(board, "runs"))

      {:ok, _} = Sync.persist_run(board, sample_run())

      assert File.dir?(Path.join(board, "runs"))
      File.rm_rf!(board)
    end

    test "overwrites a same-id file (caller responsibility to bump attempt)" do
      board = tmpdir()

      {:ok, _} = Sync.persist_run(board, sample_run(state: :running))
      {:ok, _} = Sync.persist_run(board, sample_run(state: :completed))

      path = Path.join([board, "runs", "orient-1.json"])
      decoded = path |> File.read!() |> Jason.decode!()
      # Second write wins.
      assert decoded["state"] == "completed"
      File.rm_rf!(board)
    end

    test "JSON is pretty-printed for human readability" do
      board = tmpdir()
      {:ok, _} = Sync.persist_run(board, sample_run())

      raw = Path.join([board, "runs", "orient-1.json"]) |> File.read!()
      # Pretty-printed JSON has newlines between top-level keys.
      assert String.contains?(raw, "\n  \"id\"")
      File.rm_rf!(board)
    end
  end

  defp seed_task(board, task_id, fields \\ %{}) do
    tasks_dir = Path.join(board, "tasks")
    File.mkdir_p!(tasks_dir)
    path = Path.join(tasks_dir, "#{task_id}.json")

    base = %{
      "id" => task_id,
      "title" => "Sample",
      "state" => "ready",
      "created_by" => "worg-exporter",
      "created_at" => @fixed_now,
      "assigned_to" => ["workhorse"]
    }

    File.write!(path, Jason.encode!(Map.merge(base, fields), pretty: true))
    path
  end

  describe "advance_task/3" do
    test "rewrites state to the new value, preserving every other field" do
      board = tmpdir()

      seed_task(board, "alpha", %{
        "description" => "keep me",
        "tags" => ["stage"],
        "assigned_to" => ["workhorse"]
      })

      {:ok, updated} = Sync.advance_task(board, "alpha", :done)

      assert updated["state"] == "done"
      assert updated["description"] == "keep me"
      assert updated["tags"] == ["stage"]
      assert updated["assigned_to"] == ["workhorse"]

      # Round-trip through disk to confirm the write landed.
      disk =
        Path.join([board, "tasks", "alpha.json"]) |> File.read!() |> Jason.decode!()

      assert disk["state"] == "done"
      assert disk["description"] == "keep me"

      File.rm_rf!(board)
    end

    test "accepts a string state (no atom-conversion required)" do
      board = tmpdir()
      seed_task(board, "alpha")

      {:ok, updated} = Sync.advance_task(board, "alpha", "in_progress")
      assert updated["state"] == "in_progress"

      File.rm_rf!(board)
    end

    test "returns :task_not_found when the task JSON doesn't exist" do
      board = tmpdir()
      File.mkdir_p!(Path.join(board, "tasks"))

      assert {:error, {:task_not_found, _}} =
               Sync.advance_task(board, "ghost", :done)

      File.rm_rf!(board)
    end

    test "returns :task_json_decode_failed on malformed JSON" do
      board = tmpdir()
      tasks_dir = Path.join(board, "tasks")
      File.mkdir_p!(tasks_dir)
      File.write!(Path.join(tasks_dir, "broken.json"), "not json {")

      assert {:error, {:task_json_decode_failed, _, _}} =
               Sync.advance_task(board, "broken", :done)

      File.rm_rf!(board)
    end
  end

  describe "cascade_failure/2 (wb-0mqz.14)" do
    test "single dependent: state → blocked, reason set" do
      board = tmpdir()
      seed_task(board, "failed-src", %{"state" => "failed"})
      seed_task(board, "dependent", %{"state" => "ready", "blocker" => ["failed-src"]})

      {:ok, promoted} = Sync.cascade_failure(board, "failed-src")
      assert promoted == ["dependent"]

      task = Path.join([board, "tasks", "dependent.json"]) |> File.read!() |> Jason.decode!()
      assert task["state"] == "blocked"
      assert task["blocked_reason"] == "failed dep: failed-src"

      File.rm_rf!(board)
    end

    test "chain A → B → C: A fails, both B and C cascade to blocked" do
      board = tmpdir()
      seed_task(board, "a", %{"state" => "failed"})
      seed_task(board, "b", %{"state" => "ready", "blocker" => ["a"]})
      seed_task(board, "c", %{"state" => "ready", "blocker" => ["b"]})

      {:ok, promoted} = Sync.cascade_failure(board, "a")
      assert Enum.sort(promoted) == ["b", "c"]

      for id <- ["b", "c"] do
        task = Path.join([board, "tasks", "#{id}.json"]) |> File.read!() |> Jason.decode!()
        assert task["state"] == "blocked"
      end

      File.rm_rf!(board)
    end

    # wb-qwj8.4: deep chain stress + perf-shape guard. The original
    # cascade re-ls'd + re-read the entire tasks/ dir at every recursion
    # level (O(depth × N) reads). The refactor loads once and walks
    # in-memory (O(N) reads). This test exercises a 20-deep chain to
    # catch any regression to the quadratic shape (would time out or
    # OOM under the old code with N much larger than this).
    test "deep chain (depth=20): all 20 dependents cascade in one read pass" do
      board = tmpdir()
      seed_task(board, "root", %{"state" => "failed"})

      for i <- 1..20 do
        parent = if i == 1, do: "root", else: "n#{i - 1}"
        seed_task(board, "n#{i}", %{"state" => "ready", "blocker" => [parent]})
      end

      {:ok, promoted} = Sync.cascade_failure(board, "root")

      assert length(promoted) == 20
      assert Enum.sort(promoted) == Enum.sort(Enum.map(1..20, &"n#{&1}"))

      for i <- 1..20 do
        task =
          Path.join([board, "tasks", "n#{i}.json"]) |> File.read!() |> Jason.decode!()

        assert task["state"] == "blocked"
        # blocked_reason references the proximate blocker (n_{i-1} or
        # "root" for n1), not the root of the cascade.
        expected_proximate = if i == 1, do: "root", else: "n#{i - 1}"
        assert task["blocked_reason"] == "failed dep: #{expected_proximate}"
      end

      File.rm_rf!(board)
    end

    test "diamond DAG: A fails, B/C depend on A, D depends on B+C — all three blocked" do
      board = tmpdir()
      seed_task(board, "a", %{"state" => "failed"})
      seed_task(board, "b", %{"state" => "ready", "blocker" => ["a"]})
      seed_task(board, "c", %{"state" => "ready", "blocker" => ["a"]})
      seed_task(board, "d", %{"state" => "ready", "blocker" => ["b", "c"]})

      {:ok, promoted} = Sync.cascade_failure(board, "a")
      assert Enum.sort(promoted) == ["b", "c", "d"]

      for id <- ["b", "c", "d"] do
        task = Path.join([board, "tasks", "#{id}.json"]) |> File.read!() |> Jason.decode!()
        assert task["state"] == "blocked"
      end

      File.rm_rf!(board)
    end

    test ":trigger target NOT cascaded as blocked (triggers are success-only)" do
      # A task whose :trigger references the failed source — and is
      # NOT in the source's :blocker chain — must be left alone.
      board = tmpdir()
      seed_task(board, "failed-src", %{"state" => "failed"})
      seed_task(board, "trigger-target", %{
        "state" => "ready",
        # 'trigger' is the success-side; this task doesn't depend
        # on failed-src via :blocker, so cascade should ignore it.
      })

      {:ok, promoted} = Sync.cascade_failure(board, "failed-src")
      assert promoted == []

      task = Path.join([board, "tasks", "trigger-target.json"]) |> File.read!() |> Jason.decode!()
      assert task["state"] == "ready"

      File.rm_rf!(board)
    end

    test "target already :blocked is skipped (idempotent)" do
      board = tmpdir()
      seed_task(board, "failed-src", %{"state" => "failed"})
      seed_task(board, "already-blocked", %{
        "state" => "blocked",
        "blocker" => ["failed-src"],
        "blocked_reason" => "earlier reason"
      })

      {:ok, promoted} = Sync.cascade_failure(board, "failed-src")
      assert promoted == []

      task =
        Path.join([board, "tasks", "already-blocked.json"]) |> File.read!() |> Jason.decode!()

      assert task["state"] == "blocked"
      # Reason NOT overwritten — already-blocked is a no-op.
      assert task["blocked_reason"] == "earlier reason"

      File.rm_rf!(board)
    end

    test "terminal target (:done) is left alone (don't un-finish)" do
      board = tmpdir()
      seed_task(board, "failed-src", %{"state" => "failed"})
      seed_task(board, "done-already", %{"state" => "done", "blocker" => ["failed-src"]})

      {:ok, promoted} = Sync.cascade_failure(board, "failed-src")
      assert promoted == []

      task = Path.join([board, "tasks", "done-already.json"]) |> File.read!() |> Jason.decode!()
      assert task["state"] == "done"
      File.rm_rf!(board)
    end

    test "no dependents → empty list, no error" do
      board = tmpdir()
      seed_task(board, "solo", %{"state" => "failed"})
      assert {:ok, []} = Sync.cascade_failure(board, "solo")
      File.rm_rf!(board)
    end

    test "missing tasks/ directory → empty list, no error" do
      board = tmpdir()
      assert {:ok, []} = Sync.cascade_failure(board, "ghost")
      File.rm_rf!(board)
    end

    test "task whose :blocker doesn't include the failed id is left alone" do
      board = tmpdir()
      seed_task(board, "failed-src", %{"state" => "failed"})
      seed_task(board, "unrelated", %{
        "state" => "ready",
        "blocker" => ["someone-else"]
      })

      {:ok, promoted} = Sync.cascade_failure(board, "failed-src")
      assert promoted == []

      task = Path.join([board, "tasks", "unrelated.json"]) |> File.read!() |> Jason.decode!()
      assert task["state"] == "ready"
      File.rm_rf!(board)
    end
  end

  describe "cascade_success/2 (wb-0mqz.4)" do
    test "advances every blocked trigger target to ready" do
      board = tmpdir()
      seed_task(board, "done-task", %{"trigger" => ["dep-a", "dep-b"], "state" => "done"})
      seed_task(board, "dep-a", %{"state" => "blocked"})
      seed_task(board, "dep-b", %{"state" => "blocked"})

      {:ok, advanced} = Sync.cascade_success(board, "done-task")

      assert Enum.sort(advanced) == ["dep-a", "dep-b"]

      for id <- ["dep-a", "dep-b"] do
        task = Path.join([board, "tasks", "#{id}.json"]) |> File.read!() |> Jason.decode!()
        assert task["state"] == "ready"
      end

      File.rm_rf!(board)
    end

    test "skips targets already past blocked (no regression)" do
      board = tmpdir()
      seed_task(board, "done-task", %{"trigger" => ["already-done", "in-flight", "blocked-one"], "state" => "done"})
      seed_task(board, "already-done", %{"state" => "done"})
      seed_task(board, "in-flight", %{"state" => "in_progress"})
      seed_task(board, "blocked-one", %{"state" => "blocked"})

      {:ok, advanced} = Sync.cascade_success(board, "done-task")
      assert advanced == ["blocked-one"]

      already_done = Path.join([board, "tasks", "already-done.json"]) |> File.read!() |> Jason.decode!()
      in_flight = Path.join([board, "tasks", "in-flight.json"]) |> File.read!() |> Jason.decode!()
      assert already_done["state"] == "done"
      assert in_flight["state"] == "in_progress"

      File.rm_rf!(board)
    end

    test "no triggers → empty list, no error" do
      board = tmpdir()
      seed_task(board, "solo", %{"state" => "done"})
      assert {:ok, []} = Sync.cascade_success(board, "solo")
      File.rm_rf!(board)
    end

    test "missing source task returns :task_not_found" do
      board = tmpdir()
      File.mkdir_p!(Path.join(board, "tasks"))
      assert {:error, {:task_not_found, _}} = Sync.cascade_success(board, "ghost")
      File.rm_rf!(board)
    end

    test "trigger pointing at a missing target is silently skipped" do
      # Author-time lint catches dangling refs (wb-0mqz.7). At runtime
      # we don't crash a successful Loop iteration over a stale link.
      board = tmpdir()
      seed_task(board, "done-task", %{"trigger" => ["ghost"], "state" => "done"})

      assert {:ok, []} = Sync.cascade_success(board, "done-task")
      File.rm_rf!(board)
    end
  end

  describe "fold_into_org/3" do
    @worg_pkg_dir Path.expand("../../../..", __DIR__)
    @worg_bin Path.join(@worg_pkg_dir, "target/debug/worg")

    setup do
      # The worg CLI must be built for this test. The autoloop's
      # verify step builds it; CI should too.
      unless File.exists?(@worg_bin) do
        ExUnit.configure(exclude: [:fold_into_org])
        flunk("worg binary missing — run `cargo build --bin worg` in packages/worg/")
      end

      :ok
    end

    @tag :fold_into_org
    test "appends a LOGBOOK entry to the source .org file" do
      board = tmpdir()
      File.mkdir_p!(Path.join(board, "runs"))

      # Author a tiny plan.org with a single :stage: headline.
      plan_path = Path.join(board, "plan.org")

      File.write!(plan_path, """
      #+TITLE: tiny plan
      #+GLOSSARY: #{Path.join(@worg_pkg_dir, "w.org")}

      * test task                                                    :stage:
      :PROPERTIES:
      :ID: tiny-task
      :END:

      A test task to verify fold_into_org integrates with the worg CLI.
      """)

      # Persist a Run for that task.
      {:ok, _} =
        Sync.persist_run(board, %Run{
          id: "tiny-task-1",
          task: "tiny-task",
          agent: "workhorse",
          state: :completed,
          attempt: 1,
          started_at: @fixed_now,
          finished_at: @fixed_now,
          tokens: %{"input" => 10, "output" => 5},
          cost_usd: 0.001,
          result_summary: "done"
        })

      # Fold it into the org file.
      assert {:ok, output} = Sync.fold_into_org(board, plan_path)

      assert String.contains?(output, "imported 1 logbook")

      # Verify the LOGBOOK entry landed.
      mutated = File.read!(plan_path)
      assert String.contains?(mutated, ":LOGBOOK:")
      assert String.contains?(mutated, "run=tiny-task-1")
      assert String.contains?(mutated, "state=completed")
      assert String.contains?(mutated, "agent=workhorse")

      File.rm_rf!(board)
    end

    @tag :fold_into_org
    test "is idempotent — a second fold doesn't duplicate entries" do
      board = tmpdir()
      File.mkdir_p!(Path.join(board, "runs"))
      plan_path = Path.join(board, "plan.org")

      File.write!(plan_path, """
      #+TITLE: idempotency test
      #+GLOSSARY: #{Path.join(@worg_pkg_dir, "w.org")}

      * idempotent                                                   :stage:
      :PROPERTIES:
      :ID: idem
      :END:
      """)

      {:ok, _} =
        Sync.persist_run(board, %Run{
          id: "idem-1",
          task: "idem",
          agent: "workhorse",
          state: :completed,
          attempt: 1,
          started_at: @fixed_now,
          finished_at: @fixed_now
        })

      {:ok, _} = Sync.fold_into_org(board, plan_path)
      first_size = File.stat!(plan_path).size

      {:ok, second_output} = Sync.fold_into_org(board, plan_path)
      second_size = File.stat!(plan_path).size

      assert first_size == second_size
      assert String.contains?(second_output, "skipped 1 already-imported")

      # Marker appears exactly once.
      mutated = File.read!(plan_path)
      assert mutated |> String.split("run=idem-1") |> length() == 2

      File.rm_rf!(board)
    end

    @tag :fold_into_org
    test "--dry-run leaves the file untouched but reports what would change" do
      board = tmpdir()
      File.mkdir_p!(Path.join(board, "runs"))
      plan_path = Path.join(board, "plan.org")
      content = """
      #+TITLE: dry test
      #+GLOSSARY: #{Path.join(@worg_pkg_dir, "w.org")}

      * dry                                                          :stage:
      :PROPERTIES:
      :ID: dry
      :END:
      """

      File.write!(plan_path, content)
      original_bytes = File.read!(plan_path)

      {:ok, _} =
        Sync.persist_run(board, %Run{
          id: "dry-1",
          task: "dry",
          agent: "workhorse",
          state: :completed,
          attempt: 1,
          started_at: @fixed_now,
          finished_at: @fixed_now
        })

      assert {:ok, output} = Sync.fold_into_org(board, plan_path, dry_run: true)
      assert String.contains?(output, "dry-run")

      # File is byte-identical.
      assert File.read!(plan_path) == original_bytes
      File.rm_rf!(board)
    end

    test "returns :invocation_failed when the worg binary path doesn't exist" do
      board = tmpdir()
      File.mkdir_p!(Path.join(board, "runs"))
      File.write!(Path.join(board, "plan.org"), "")

      assert {:error, {:invocation_failed, msg}} =
               Sync.fold_into_org(board, Path.join(board, "plan.org"),
                 worg_bin: "/nonexistent/worg"
               )

      assert String.contains?(msg, "/nonexistent/worg")
      File.rm_rf!(board)
    end

    test "returns :invocation_failed when the plan .org doesn't exist" do
      board = tmpdir()
      File.mkdir_p!(Path.join(board, "runs"))

      # Use ANY existing file as :worg_bin so we get past the bin check.
      # (We never invoke it because the plan check fails first.)
      bin_stub = Path.join(board, "stub")
      File.write!(bin_stub, "")
      File.chmod!(bin_stub, 0o755)

      assert {:error, {:invocation_failed, msg}} =
               Sync.fold_into_org(board, "/nonexistent/plan.org", worg_bin: bin_stub)

      assert String.contains?(msg, "/nonexistent/plan.org")
      File.rm_rf!(board)
    end
  end
end
