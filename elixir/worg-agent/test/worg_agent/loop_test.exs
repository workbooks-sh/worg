defmodule WorgAgent.LoopTest do
  use ExUnit.Case, async: false

  alias WorgAgent.Loop

  @fixed_now "2026-05-23T20:00:00Z"

  # ── Helpers ───────────────────────────────────────────────────────

  defp tmpdir do
    p = System.tmp_dir!() |> Path.join("worg-agent-loop-test-#{:rand.uniform(99_999_999)}")
    File.mkdir_p!(p)
    p
  end

  defp seed_board(opts \\ []) do
    board = tmpdir()
    File.mkdir_p!(Path.join(board, "tasks"))

    agents_json = %{
      "version" => 1,
      "agents" => [
        %{
          "id" => "workhorse",
          "name" => "Workhorse",
          "type" => "ai",
          "status" => "active",
          "capabilities" => ["bash", "read"]
        }
      ]
    }

    File.write!(Path.join(board, "agents.json"), Jason.encode!(agents_json))

    tasks = Keyword.get(opts, :tasks, [default_task()])

    for task <- tasks do
      File.write!(Path.join([board, "tasks", "#{task["id"]}.json"]), Jason.encode!(task))
    end

    board
  end

  defp default_task do
    %{
      "id" => "solo",
      "title" => "Say hello",
      "state" => "backlog",
      "created_by" => "worg-exporter",
      "created_at" => @fixed_now,
      "description" => "Reply with the word HELLO and stop.",
      "assigned_to" => ["workhorse"]
    }
  end

  defp llm_plug(script) do
    # `script` is a list of response bodies, returned in order.
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

  defp text_response(content, usage \\ %{}) do
    %{
      "choices" => [
        %{
          "message" => %{"role" => "assistant", "content" => content},
          "finish_reason" => "stop"
        }
      ],
      "usage" => Map.merge(%{"prompt_tokens" => 10, "completion_tokens" => 5}, usage)
    }
  end

  defp tool_call_response(name, args, usage \\ %{}) do
    %{
      "choices" => [
        %{
          "message" => %{
            "role" => "assistant",
            "content" => nil,
            "tool_calls" => [
              %{
                "id" => "call_#{name}_#{:rand.uniform(99_999)}",
                "type" => "function",
                "function" => %{"name" => name, "arguments" => Jason.encode!(args)}
              }
            ]
          },
          "finish_reason" => "tool_calls"
        }
      ],
      "usage" => Map.merge(%{"prompt_tokens" => 20, "completion_tokens" => 5}, usage)
    }
  end

  # ── Tests ─────────────────────────────────────────────────────────

  describe "run_next/2 — lifecycle telemetry (wb-jnjc)" do
    test "fires [:worg_agent, :llm, :turn] start + stop for each LLM call" do
      board = seed_board()
      handler_id = "wb-jnjc-test-llm-turn-#{:rand.uniform(1_000_000)}"
      test_pid = self()

      :telemetry.attach_many(
        handler_id,
        [
          [:worg_agent, :llm, :turn, :start],
          [:worg_agent, :llm, :turn, :stop]
        ],
        fn event, measurements, metadata, _config ->
          send(test_pid, {:telemetry, event, measurements, metadata})
        end,
        nil
      )

      try do
        plug = llm_plug([text_response("HELLO")])

        {:ok, _} =
          Loop.run_next(board,
            llm_opts: [
              api_key: "k",
              endpoint: "https://t/api/v1/chat/completions",
              req_options: [plug: plug]
            ],
            now_iso8601: @fixed_now
          )

        # Single-turn text reply → exactly 1 LLM turn → 1 start + 1 stop.
        assert_receive {:telemetry, [:worg_agent, :llm, :turn, :start], start_meas,
                        start_meta}

        assert is_map(start_meas)
        assert is_integer(start_meta.message_count)
        assert start_meta.message_count >= 2

        assert_receive {:telemetry, [:worg_agent, :llm, :turn, :stop], stop_meas,
                        stop_meta}

        # Duration measurement should be a positive native-time integer.
        assert is_integer(stop_meas.duration) and stop_meas.duration > 0
        assert stop_meta.status == :ok
        assert stop_meta.stop_reason == :end_turn
      after
        :telemetry.detach(handler_id)
        File.rm_rf!(board)
      end
    end

    test "fires [:worg_agent, :tool_call] start + stop per dispatched tool" do
      board = seed_board()
      handler_id = "wb-jnjc-test-tool-call-#{:rand.uniform(1_000_000)}"
      test_pid = self()

      :telemetry.attach_many(
        handler_id,
        [
          [:worg_agent, :tool_call, :start],
          [:worg_agent, :tool_call, :stop]
        ],
        fn event, measurements, metadata, _config ->
          send(test_pid, {:telemetry, event, measurements, metadata})
        end,
        nil
      )

      try do
        # Two-turn script: first tool_calls, then a final text reply.
        plug =
          llm_plug([
            tool_call_response("read", %{"path" => "/plan.org"}),
            text_response("done")
          ])

        {:ok, _} =
          Loop.run_next(board,
            llm_opts: [
              api_key: "k",
              endpoint: "https://t/api/v1/chat/completions",
              req_options: [plug: plug]
            ],
            now_iso8601: @fixed_now
          )

        assert_receive {:telemetry, [:worg_agent, :tool_call, :start], _meas, start_meta}
        assert start_meta.tool_name == "read"
        assert is_binary(start_meta.tool_call_id)
        assert start_meta.args == %{"path" => "/plan.org"}

        assert_receive {:telemetry, [:worg_agent, :tool_call, :stop], stop_meas, stop_meta}
        assert is_integer(stop_meas.duration) and stop_meas.duration >= 0
        assert stop_meta.tool_name == "read"
        assert stop_meta.status in [:ok, :error]
        assert is_integer(stop_meta.result_size)
      after
        :telemetry.detach(handler_id)
        File.rm_rf!(board)
      end
    end
  end

  describe "run_next/2 — :STAGE_MODEL: override (wb-6t1r)" do
    test "task stage_model wins over llm_opts[:model] (per-task LLM dispatch)" do
      task = %{
        "id" => "judge-frame",
        "title" => "Escalate to vision model",
        "state" => "backlog",
        "created_by" => "worg-exporter",
        "created_at" => @fixed_now,
        "description" => "Use the heavy vision model for this stage.",
        "assigned_to" => ["workhorse"],
        # The wire-extension field added by wb-6t1r — emitted by
        # worg-cli's TaskWithExtensions for any task that has
        # :STAGE_MODEL: in its :PROPERTIES: drawer.
        "stage_model" => "google/gemini-3.5-pro"
      }

      board = seed_board(tasks: [task])

      # Capture the model the LLM call dispatched against. The plug runs
      # for every Llm.call; if stage_model is plumbed through, the
      # body's "model" field will be the override, NOT the agent default
      # or the llm_opts[:model] fallback below.
      captured_model = :persistent_term.put({__MODULE__, :captured_model}, nil)
      _ = captured_model

      plug = fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        decoded = Jason.decode!(body)
        :persistent_term.put({__MODULE__, :captured_model}, decoded["model"])

        conn
        |> Plug.Conn.put_resp_content_type("application/json")
        |> Plug.Conn.resp(200, Jason.encode!(text_response("ok")))
      end

      {:ok, _} =
        Loop.run_next(board,
          llm_opts: [
            api_key: "k",
            endpoint: "https://t/api/v1/chat/completions",
            # This would otherwise win — but stage_model overrides.
            model: "xiaomi/mimo-v2.5-pro",
            req_options: [plug: plug]
          ],
          now_iso8601: @fixed_now
        )

      assert :persistent_term.get({__MODULE__, :captured_model}) ==
               "google/gemini-3.5-pro",
             "task.stage_model should override llm_opts[:model]"

      File.rm_rf!(board)
    end

    test "absence of stage_model falls through to llm_opts[:model]" do
      task = %{
        "id" => "draft",
        "title" => "Draft a thing",
        "state" => "backlog",
        "created_by" => "worg-exporter",
        "created_at" => @fixed_now,
        "description" => "Draft.",
        "assigned_to" => ["workhorse"]
        # NO stage_model — default model should flow through.
      }

      board = seed_board(tasks: [task])

      plug = fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        decoded = Jason.decode!(body)
        :persistent_term.put({__MODULE__, :captured_model}, decoded["model"])

        conn
        |> Plug.Conn.put_resp_content_type("application/json")
        |> Plug.Conn.resp(200, Jason.encode!(text_response("ok")))
      end

      {:ok, _} =
        Loop.run_next(board,
          llm_opts: [
            api_key: "k",
            endpoint: "https://t/api/v1/chat/completions",
            model: "xiaomi/mimo-v2.5-pro",
            req_options: [plug: plug]
          ],
          now_iso8601: @fixed_now
        )

      assert :persistent_term.get({__MODULE__, :captured_model}) ==
               "xiaomi/mimo-v2.5-pro",
             "absent stage_model should fall through to llm_opts[:model]"

      File.rm_rf!(board)
    end
  end

  describe "run_next/2 — happy path (text-only response)" do
    test "single-turn completion writes a :completed Run JSON" do
      board = seed_board()

      plug = llm_plug([text_response("HELLO")])

      {:ok, run} =
        Loop.run_next(board,
          llm_opts: [
            api_key: "k",
            endpoint: "https://t/api/v1/chat/completions",
            req_options: [plug: plug]
          ],
          now_iso8601: @fixed_now
        )

      assert run.task == "solo"
      assert run.agent == "workhorse"
      assert run.state == :completed
      assert run.attempt == 1
      assert run.result_summary == "HELLO"
      assert run.tokens == %{"input" => 10, "output" => 5}

      # Run JSON landed on disk in the wire shape.
      run_path = Path.join([board, "runs", "solo-1.json"])
      assert File.exists?(run_path)
      decoded = run_path |> File.read!() |> Jason.decode!()
      assert decoded["state"] == "completed"
      assert decoded["result_summary"] == "HELLO"

      File.rm_rf!(board)
    end

    test "honors :agent_overrides system_prompt" do
      board = seed_board()

      # Capture the request to verify the system_prompt comes from the
      # override.
      captured = :atomics.new(1, [])

      plug = fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        decoded = Jason.decode!(body)
        [system | _] = decoded["messages"]

        if String.contains?(system["content"], "OVERRIDE-PROMPT") do
          :atomics.add(captured, 1, 1)
        end

        conn
        |> Plug.Conn.put_resp_content_type("application/json")
        |> Plug.Conn.resp(200, Jason.encode!(text_response("ok")))
      end

      {:ok, _} =
        Loop.run_next(board,
          agent_overrides: %{
            "workhorse" => %{
              system_prompt: "OVERRIDE-PROMPT for tests."
            }
          },
          llm_opts: [
            api_key: "k",
            endpoint: "https://t/api/v1/chat/completions",
            req_options: [plug: plug]
          ],
          now_iso8601: @fixed_now
        )

      assert :atomics.get(captured, 1) == 1
      File.rm_rf!(board)
    end
  end

  describe "run_next/2 — multi-turn with tool calls" do
    test "dispatches a bash tool call and feeds the result back" do
      board = seed_board()

      # LLM does: tool_call(bash) → tool result fed back → final text.
      plug =
        llm_plug([
          tool_call_response("bash", %{"command" => "echo from-bash"}),
          text_response("saw bash output")
        ])

      {:ok, run} =
        Loop.run_next(board,
          llm_opts: [
            api_key: "k",
            endpoint: "https://t/api/v1/chat/completions",
            req_options: [plug: plug]
          ],
          trust_level: :sandboxed,
          working_dir: board,
          now_iso8601: @fixed_now
        )

      assert run.state == :completed
      assert run.result_summary == "saw bash output"
      assert run.tokens["input"] == 30
      assert run.tokens["output"] == 10
      File.rm_rf!(board)
    end

    test "captures tool errors as the tool message content (does not fail the run)" do
      board = seed_board()

      # bash without trust_level returns an error tuple; the loop
      # feeds the stringified error back as the tool message and
      # continues.
      plug =
        llm_plug([
          tool_call_response("bash", %{"command" => "echo x"}),
          text_response("noted the trust error")
        ])

      {:ok, run} =
        Loop.run_next(board,
          llm_opts: [
            api_key: "k",
            endpoint: "https://t/api/v1/chat/completions",
            req_options: [plug: plug]
          ],
          # No trust_level → bash refuses
          trust_level: :none,
          working_dir: board,
          now_iso8601: @fixed_now
        )

      assert run.state == :completed
      assert run.result_summary == "noted the trust error"
      File.rm_rf!(board)
    end
  end

  describe "run_next/2 — :BLOCKER: DAG semantics (wb-qk6l.3)" do
    test "task with unmet dependency is NOT picked even when outline-pickable" do
      board =
        seed_board(
          tasks: [
            %{
              "id" => "prereq",
              "title" => "Prereq",
              "state" => "backlog",
              "created_by" => "x",
              "created_at" => @fixed_now,
              "assigned_to" => ["workhorse"]
            },
            %{
              "id" => "dependent",
              "title" => "Dependent",
              "state" => "backlog",
              "created_by" => "x",
              "created_at" => @fixed_now,
              "assigned_to" => ["workhorse"],
              "blocker" => ["prereq"]
            }
          ]
        )

      plug = llm_plug([text_response("done")])

      # The picker should pick `dependent` LAST alphabetically, but
      # it's blocked by prereq → so we should get prereq.
      {:ok, run} =
        Loop.run_next(board,
          llm_opts: [
            api_key: "k",
            endpoint: "https://t/api/v1/chat/completions",
            req_options: [plug: plug]
          ],
          now_iso8601: @fixed_now
        )

      assert run.task == "prereq"
      File.rm_rf!(board)
    end

    test "dependent task unlocks once dependency reaches :done" do
      board =
        seed_board(
          tasks: [
            %{
              "id" => "prereq",
              "title" => "Prereq",
              # Simulate prereq already completed by a prior tick.
              "state" => "done",
              "created_by" => "x",
              "created_at" => @fixed_now,
              "assigned_to" => ["workhorse"]
            },
            %{
              "id" => "dependent",
              "title" => "Dependent",
              "state" => "backlog",
              "created_by" => "x",
              "created_at" => @fixed_now,
              "assigned_to" => ["workhorse"],
              "blocker" => ["prereq"]
            }
          ]
        )

      plug = llm_plug([text_response("ready now")])

      {:ok, run} =
        Loop.run_next(board,
          llm_opts: [
            api_key: "k",
            endpoint: "https://t/api/v1/chat/completions",
            req_options: [plug: plug]
          ],
          now_iso8601: @fixed_now
        )

      assert run.task == "dependent"
      File.rm_rf!(board)
    end

    test "ALL deps must be :done — a single open dep still blocks" do
      board =
        seed_board(
          tasks: [
            %{
              "id" => "a",
              "title" => "A",
              "state" => "done",
              "created_by" => "x",
              "created_at" => @fixed_now,
              "assigned_to" => ["workhorse"]
            },
            %{
              "id" => "b",
              "title" => "B",
              # Still open — should block the dependent.
              "state" => "backlog",
              "created_by" => "x",
              "created_at" => @fixed_now,
              "assigned_to" => ["workhorse"]
            },
            %{
              "id" => "z-dependent",
              "title" => "Z dependent",
              "state" => "backlog",
              "created_by" => "x",
              "created_at" => @fixed_now,
              "assigned_to" => ["workhorse"],
              "blocker" => ["a", "b"]
            }
          ]
        )

      plug = llm_plug([text_response("ok")])

      {:ok, run} =
        Loop.run_next(board,
          llm_opts: [
            api_key: "k",
            endpoint: "https://t/api/v1/chat/completions",
            req_options: [plug: plug]
          ],
          now_iso8601: @fixed_now
        )

      # Should pick `b` (the only unmet prereq), NOT z-dependent.
      assert run.task == "b"
      File.rm_rf!(board)
    end

    test "blocker referencing an unknown task blocks (fail loud, not silent skip)" do
      board =
        seed_board(
          tasks: [
            %{
              "id" => "orphan",
              "title" => "Orphan",
              "state" => "backlog",
              "created_by" => "x",
              "created_at" => @fixed_now,
              "assigned_to" => ["workhorse"],
              "blocker" => ["ghost-task"]
            }
          ]
        )

      assert {:error, :no_ready_task} =
               Loop.run_next(board,
                 llm_opts: [api_key: "k"],
                 now_iso8601: @fixed_now
               )

      File.rm_rf!(board)
    end

    test ":task_id override bypasses the blocker check (caller owns scheduling)" do
      # When a caller pins task_id explicitly, the picker is not
      # consulted at all — the caller is asserting they know what
      # they're doing. Useful for retries and integration tests.
      board =
        seed_board(
          tasks: [
            %{
              "id" => "prereq",
              "title" => "Prereq",
              "state" => "backlog",
              "created_by" => "x",
              "created_at" => @fixed_now,
              "assigned_to" => ["workhorse"]
            },
            %{
              "id" => "dependent",
              "title" => "Dependent",
              "state" => "backlog",
              "created_by" => "x",
              "created_at" => @fixed_now,
              "assigned_to" => ["workhorse"],
              "blocker" => ["prereq"]
            }
          ]
        )

      plug = llm_plug([text_response("forced")])

      {:ok, run} =
        Loop.run_next(board,
          task_id: "dependent",
          llm_opts: [
            api_key: "k",
            endpoint: "https://t/api/v1/chat/completions",
            req_options: [plug: plug]
          ],
          now_iso8601: @fixed_now
        )

      assert run.task == "dependent"
      File.rm_rf!(board)
    end
  end

  describe "run_next/2 — failure cascade (wb-0mqz.14)" do
    test "failed run cascades dependents to blocked" do
      board =
        seed_board(
          tasks: [
            %{
              "id" => "solo",
              "title" => "Solo",
              "state" => "backlog",
              "created_by" => "x",
              "created_at" => @fixed_now,
              "assigned_to" => ["workhorse"]
            },
            %{
              "id" => "downstream",
              "title" => "Downstream",
              "state" => "ready",
              "created_by" => "x",
              "created_at" => @fixed_now,
              "assigned_to" => ["workhorse"],
              "blocker" => ["solo"]
            }
          ]
        )

      # Force max_turns_exceeded → :failed.
      tool_responses =
        Stream.repeatedly(fn -> tool_call_response("bash", %{"command" => "echo y"}) end)
        |> Enum.take(3)

      plug = llm_plug(tool_responses)

      {:error, :max_turns_exceeded} =
        Loop.run_next(board,
          max_turns: 2,
          llm_opts: [
            api_key: "k",
            endpoint: "https://t/api/v1/chat/completions",
            req_options: [plug: plug]
          ],
          trust_level: :sandboxed,
          working_dir: board,
          now_iso8601: @fixed_now
        )

      downstream =
        Path.join([board, "tasks", "downstream.json"])
        |> File.read!()
        |> Jason.decode!()

      assert downstream["state"] == "blocked"
      assert downstream["blocked_reason"] == "failed dep: solo"
      File.rm_rf!(board)
    end

    test ":skip_cascade leaves dependents in their original state on failure" do
      board =
        seed_board(
          tasks: [
            %{
              "id" => "solo",
              "title" => "Solo",
              "state" => "backlog",
              "created_by" => "x",
              "created_at" => @fixed_now,
              "assigned_to" => ["workhorse"]
            },
            %{
              "id" => "downstream",
              "title" => "Downstream",
              "state" => "ready",
              "created_by" => "x",
              "created_at" => @fixed_now,
              "assigned_to" => ["workhorse"],
              "blocker" => ["solo"]
            }
          ]
        )

      tool_responses =
        Stream.repeatedly(fn -> tool_call_response("bash", %{"command" => "echo y"}) end)
        |> Enum.take(3)

      plug = llm_plug(tool_responses)

      {:error, :max_turns_exceeded} =
        Loop.run_next(board,
          max_turns: 2,
          skip_cascade: true,
          llm_opts: [
            api_key: "k",
            endpoint: "https://t/api/v1/chat/completions",
            req_options: [plug: plug]
          ],
          trust_level: :sandboxed,
          working_dir: board,
          now_iso8601: @fixed_now
        )

      downstream =
        Path.join([board, "tasks", "downstream.json"])
        |> File.read!()
        |> Jason.decode!()

      assert downstream["state"] == "ready"
      File.rm_rf!(board)
    end

    test "successful run does NOT cascade-block dependents (only failure does)" do
      board =
        seed_board(
          tasks: [
            %{
              "id" => "solo",
              "title" => "Solo",
              "state" => "backlog",
              "created_by" => "x",
              "created_at" => @fixed_now,
              "assigned_to" => ["workhorse"]
            },
            %{
              "id" => "downstream",
              "title" => "Downstream",
              "state" => "ready",
              "created_by" => "x",
              "created_at" => @fixed_now,
              "assigned_to" => ["workhorse"],
              "blocker" => ["solo"]
            }
          ]
        )

      plug = llm_plug([text_response("ok")])

      {:ok, _} =
        Loop.run_next(board,
          llm_opts: [
            api_key: "k",
            endpoint: "https://t/api/v1/chat/completions",
            req_options: [plug: plug]
          ],
          now_iso8601: @fixed_now
        )

      downstream =
        Path.join([board, "tasks", "downstream.json"])
        |> File.read!()
        |> Jason.decode!()

      # downstream's state should NOT have flipped to blocked.
      # (It may have changed to "ready" via TRIGGER if solo had any,
      # but solo declares no triggers; downstream was already ready.)
      assert downstream["state"] == "ready"
      File.rm_rf!(board)
    end
  end

  describe "run_next/2 — :TRIGGER: success cascade (wb-0mqz.4)" do
    test "successful run cascades blocked trigger targets to ready" do
      board =
        seed_board(
          tasks: [
            %{
              "id" => "solo",
              "title" => "Solo (overridden default)",
              "state" => "backlog",
              "created_by" => "x",
              "created_at" => @fixed_now,
              "assigned_to" => ["workhorse"],
              "trigger" => ["downstream"]
            },
            %{
              "id" => "downstream",
              "title" => "Downstream",
              "state" => "blocked",
              "created_by" => "x",
              "created_at" => @fixed_now,
              "assigned_to" => ["workhorse"]
            }
          ]
        )

      plug = llm_plug([text_response("ok")])

      {:ok, _} =
        Loop.run_next(board,
          llm_opts: [
            api_key: "k",
            endpoint: "https://t/api/v1/chat/completions",
            req_options: [plug: plug]
          ],
          now_iso8601: @fixed_now
        )

      downstream =
        Path.join([board, "tasks", "downstream.json"])
        |> File.read!()
        |> Jason.decode!()

      assert downstream["state"] == "ready"
      File.rm_rf!(board)
    end

    test ":skip_cascade leaves trigger targets untouched" do
      board =
        seed_board(
          tasks: [
            %{
              "id" => "solo",
              "title" => "Solo",
              "state" => "backlog",
              "created_by" => "x",
              "created_at" => @fixed_now,
              "assigned_to" => ["workhorse"],
              "trigger" => ["downstream"]
            },
            %{
              "id" => "downstream",
              "title" => "Downstream",
              "state" => "blocked",
              "created_by" => "x",
              "created_at" => @fixed_now,
              "assigned_to" => ["workhorse"]
            }
          ]
        )

      plug = llm_plug([text_response("ok")])

      {:ok, _} =
        Loop.run_next(board,
          skip_cascade: true,
          llm_opts: [
            api_key: "k",
            endpoint: "https://t/api/v1/chat/completions",
            req_options: [plug: plug]
          ],
          now_iso8601: @fixed_now
        )

      downstream =
        Path.join([board, "tasks", "downstream.json"])
        |> File.read!()
        |> Jason.decode!()

      assert downstream["state"] == "blocked"
      File.rm_rf!(board)
    end

    test "failed run does NOT cascade triggers" do
      # Triggers are a success-side concept — on failure, dependents
      # stay blocked (failure cascade is wb-0mqz.14's territory).
      board =
        seed_board(
          tasks: [
            %{
              "id" => "solo",
              "title" => "Solo",
              "state" => "backlog",
              "created_by" => "x",
              "created_at" => @fixed_now,
              "assigned_to" => ["workhorse"],
              "trigger" => ["downstream"]
            },
            %{
              "id" => "downstream",
              "title" => "Downstream",
              "state" => "blocked",
              "created_by" => "x",
              "created_at" => @fixed_now,
              "assigned_to" => ["workhorse"]
            }
          ]
        )

      # Force max_turns_exceeded → :failed.
      tool_responses =
        Stream.repeatedly(fn -> tool_call_response("bash", %{"command" => "echo y"}) end)
        |> Enum.take(3)

      plug = llm_plug(tool_responses)

      {:error, :max_turns_exceeded} =
        Loop.run_next(board,
          max_turns: 2,
          llm_opts: [
            api_key: "k",
            endpoint: "https://t/api/v1/chat/completions",
            req_options: [plug: plug]
          ],
          trust_level: :sandboxed,
          working_dir: board,
          now_iso8601: @fixed_now
        )

      downstream =
        Path.join([board, "tasks", "downstream.json"])
        |> File.read!()
        |> Jason.decode!()

      assert downstream["state"] == "blocked"
      File.rm_rf!(board)
    end
  end

  describe "run_next/2 — task-state propagation (wb-qk6l.1)" do
    test "advances tasks/<id>.json state to done on a successful run" do
      board = seed_board()
      plug = llm_plug([text_response("ok")])

      {:ok, _run} =
        Loop.run_next(board,
          llm_opts: [
            api_key: "k",
            endpoint: "https://t/api/v1/chat/completions",
            req_options: [plug: plug]
          ],
          now_iso8601: @fixed_now
        )

      task =
        Path.join([board, "tasks", "solo.json"]) |> File.read!() |> Jason.decode!()

      assert task["state"] == "done"
      File.rm_rf!(board)
    end

    test "does NOT advance tasks/<id>.json on a failed run (allows retry)" do
      board = seed_board()
      # Force max_turns_exceeded → :failed.
      tool_responses =
        Stream.repeatedly(fn -> tool_call_response("bash", %{"command" => "echo y"}) end)
        |> Enum.take(3)

      plug = llm_plug(tool_responses)

      {:error, :max_turns_exceeded} =
        Loop.run_next(board,
          max_turns: 2,
          llm_opts: [
            api_key: "k",
            endpoint: "https://t/api/v1/chat/completions",
            req_options: [plug: plug]
          ],
          trust_level: :sandboxed,
          working_dir: board,
          now_iso8601: @fixed_now
        )

      task =
        Path.join([board, "tasks", "solo.json"]) |> File.read!() |> Jason.decode!()

      # Untouched — caller may retry by running again.
      assert task["state"] == "backlog"
      File.rm_rf!(board)
    end

    test "honors :skip_task_advance — caller owns state transitions" do
      board = seed_board()
      plug = llm_plug([text_response("ok")])

      {:ok, _} =
        Loop.run_next(board,
          skip_task_advance: true,
          llm_opts: [
            api_key: "k",
            endpoint: "https://t/api/v1/chat/completions",
            req_options: [plug: plug]
          ],
          now_iso8601: @fixed_now
        )

      task =
        Path.join([board, "tasks", "solo.json"]) |> File.read!() |> Jason.decode!()

      assert task["state"] == "backlog"
      File.rm_rf!(board)
    end
  end

  describe "run_next/2 — attempt numbering" do
    test "first run is attempt=1; second attempt increments" do
      board = seed_board()
      plug = llm_plug([text_response("first"), text_response("second")])

      shared_opts = [
        # Pin task_id so the picker doesn't skip the task after the
        # first run advances its state → "done".
        task_id: "solo",
        llm_opts: [
          api_key: "k",
          endpoint: "https://t/api/v1/chat/completions",
          req_options: [plug: plug]
        ],
        now_iso8601: @fixed_now
      ]

      {:ok, run1} = Loop.run_next(board, shared_opts)
      assert run1.attempt == 1
      assert File.exists?(Path.join([board, "runs", "solo-1.json"]))

      {:ok, run2} = Loop.run_next(board, shared_opts)
      assert run2.attempt == 2
      assert File.exists?(Path.join([board, "runs", "solo-2.json"]))

      File.rm_rf!(board)
    end
  end

  describe "run_next/2 — task picking" do
    test "errors when no tasks exist" do
      board = tmpdir()
      File.write!(Path.join(board, "agents.json"), ~s({"version": 1, "agents": []}))

      assert {:error, :no_tasks} =
               Loop.run_next(board,
                 llm_opts: [api_key: "k"],
                 now_iso8601: @fixed_now
               )

      File.rm_rf!(board)
    end

    test "errors when no task is ready (all parents blocked)" do
      board =
        seed_board(
          tasks: [
            %{
              "id" => "root",
              "title" => "Root",
              "state" => "backlog",
              "created_by" => "x",
              "created_at" => @fixed_now,
              "assigned_to" => ["workhorse"]
            },
            %{
              "id" => "child",
              "title" => "Child",
              "state" => "backlog",
              "created_by" => "x",
              "created_at" => @fixed_now,
              "parent" => "root",
              "assigned_to" => ["workhorse"]
            }
          ]
        )

      # Root is pickable (no parent) → it'll be selected, not :no_ready_task.
      plug = llm_plug([text_response("done")])

      {:ok, run} =
        Loop.run_next(board,
          llm_opts: [
            api_key: "k",
            endpoint: "https://t/api/v1/chat/completions",
            req_options: [plug: plug]
          ],
          now_iso8601: @fixed_now
        )

      assert run.task == "root"
      File.rm_rf!(board)
    end

    test "honors :task_id override" do
      board =
        seed_board(
          tasks: [
            %{
              "id" => "a",
              "title" => "A",
              "state" => "backlog",
              "created_by" => "x",
              "created_at" => @fixed_now,
              "assigned_to" => ["workhorse"]
            },
            %{
              "id" => "b",
              "title" => "B",
              "state" => "backlog",
              "created_by" => "x",
              "created_at" => @fixed_now,
              "assigned_to" => ["workhorse"]
            }
          ]
        )

      plug = llm_plug([text_response("specific")])

      {:ok, run} =
        Loop.run_next(board,
          task_id: "b",
          llm_opts: [
            api_key: "k",
            endpoint: "https://t/api/v1/chat/completions",
            req_options: [plug: plug]
          ],
          now_iso8601: @fixed_now
        )

      assert run.task == "b"
      File.rm_rf!(board)
    end

    test "task_id pointing at a missing task returns :no_such_task" do
      board = seed_board()

      assert {:error, {:no_such_task, "ghost"}} =
               Loop.run_next(board,
                 task_id: "ghost",
                 llm_opts: [api_key: "k"],
                 now_iso8601: @fixed_now
               )

      File.rm_rf!(board)
    end
  end

  describe "run_next/2 — failure paths" do
    test "max_turns exceeded writes a :failed run and returns the error" do
      board = seed_board()
      # Script: tool_call forever. Loop will exhaust max_turns.
      tool_responses =
        Stream.repeatedly(fn -> tool_call_response("bash", %{"command" => "echo y"}) end)
        |> Enum.take(5)

      plug = llm_plug(tool_responses)

      assert {:error, :max_turns_exceeded} =
               Loop.run_next(board,
                 max_turns: 3,
                 llm_opts: [
                   api_key: "k",
                   endpoint: "https://t/api/v1/chat/completions",
                   req_options: [plug: plug]
                 ],
                 trust_level: :sandboxed,
                 working_dir: board,
                 now_iso8601: @fixed_now
               )

      # Failed Run is still recorded.
      run_path = Path.join([board, "runs", "solo-1.json"])
      decoded = run_path |> File.read!() |> Jason.decode!()
      assert decoded["state"] == "failed"
      assert String.contains?(decoded["error"], "max_turns_exceeded")
      File.rm_rf!(board)
    end

    test "LLM transport error writes :failed and surfaces the reason" do
      board = seed_board()
      plug = fn _conn -> raise "network down" end

      assert {:error, {:transport, _}} =
               Loop.run_next(board,
                 llm_opts: [
                   api_key: "k",
                   endpoint: "https://t/api/v1/chat/completions",
                   req_options: [plug: plug, retry: false]
                 ],
                 now_iso8601: @fixed_now
               )

      run_path = Path.join([board, "runs", "solo-1.json"])
      decoded = run_path |> File.read!() |> Jason.decode!()
      assert decoded["state"] == "failed"
      File.rm_rf!(board)
    end
  end
end
