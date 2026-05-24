defmodule WorgAgent.LlmStreamTest do
  @moduledoc """
  End-to-end coverage of `Llm.stream/3` against a chunked-SSE Plug stub.
  The stub emits OpenRouter-shaped frames (text + tool_call deltas).
  """

  use ExUnit.Case, async: true

  alias WorgAgent.Llm
  alias WorgAgent.Llm.{Response, ToolCall}

  @messages [%{"role" => "user", "content" => "say hi"}]
  @tools []

  # Build a chunked-response plug that emits `frames` (each a JSON map),
  # followed by a final `data: [DONE]\n\n` sentinel. Each frame is
  # sent in its own chunk so the SSE parser exercises its
  # cross-chunk-buffer path.
  defp sse_plug(frames) do
    fn conn ->
      conn =
        conn
        |> Plug.Conn.put_resp_content_type("text/event-stream")
        |> Plug.Conn.send_chunked(200)

      Enum.reduce(frames, conn, fn frame, c ->
        {:ok, c} = Plug.Conn.chunk(c, "data: " <> Jason.encode!(frame) <> "\n\n")
        c
      end)
      |> then(fn c ->
        {:ok, c} = Plug.Conn.chunk(c, "data: [DONE]\n\n")
        c
      end)
    end
  end

  describe "stream/3 text-only" do
    test "aggregates content fragments into a single %Response{}" do
      plug =
        sse_plug([
          %{"choices" => [%{"delta" => %{"content" => "Hello, "}, "finish_reason" => nil}]},
          %{"choices" => [%{"delta" => %{"content" => "world"}, "finish_reason" => nil}]},
          %{"choices" => [%{"delta" => %{"content" => "!"}, "finish_reason" => nil}]},
          %{
            "choices" => [%{"delta" => %{}, "finish_reason" => "stop"}],
            "usage" => %{"prompt_tokens" => 4, "completion_tokens" => 3}
          }
        ])

      assert {:ok, %Response{} = resp} =
               Llm.stream(@messages, @tools,
                 api_key: "k",
                 endpoint: "https://openrouter.test/api/v1/chat/completions",
                 req_options: [plug: plug]
               )

      assert resp.content == "Hello, world!"
      assert resp.tool_calls == []
      assert resp.stop_reason == :end_turn
      assert resp.usage["prompt_tokens"] == 4
    end

    test "fires on_delta callback per content chunk" do
      plug =
        sse_plug([
          %{"choices" => [%{"delta" => %{"content" => "a"}, "finish_reason" => nil}]},
          %{"choices" => [%{"delta" => %{"content" => "b"}, "finish_reason" => nil}]},
          %{"choices" => [%{"delta" => %{}, "finish_reason" => "stop"}]}
        ])

      test_pid = self()

      cb = fn
        {:content, text} -> send(test_pid, {:delta, text})
        {:done, %Response{} = r} -> send(test_pid, {:final, r})
        _ -> :ok
      end

      assert {:ok, %Response{content: "ab"}} =
               Llm.stream(@messages, @tools,
                 api_key: "k",
                 endpoint: "https://openrouter.test/api/v1/chat/completions",
                 req_options: [plug: plug],
                 on_delta: cb
               )

      assert_received {:delta, "a"}
      assert_received {:delta, "b"}
      assert_received {:final, %Response{content: "ab"}}
    end
  end

  describe "stream/3 tool calls" do
    test "merges streamed tool-call deltas and decodes arguments JSON" do
      plug =
        sse_plug([
          %{
            "choices" => [
              %{
                "delta" => %{
                  "tool_calls" => [
                    %{
                      "index" => 0,
                      "id" => "call_abc",
                      "function" => %{"name" => "bash", "arguments" => ""}
                    }
                  ]
                }
              }
            ]
          },
          %{
            "choices" => [
              %{
                "delta" => %{
                  "tool_calls" => [
                    %{"index" => 0, "function" => %{"arguments" => "{\"command\":"}}
                  ]
                }
              }
            ]
          },
          %{
            "choices" => [
              %{
                "delta" => %{
                  "tool_calls" => [
                    %{"index" => 0, "function" => %{"arguments" => "\"ls\"}"}}
                  ]
                }
              }
            ]
          },
          %{"choices" => [%{"delta" => %{}, "finish_reason" => "tool_calls"}]}
        ])

      assert {:ok, %Response{tool_calls: [tc], stop_reason: :tool_calls}} =
               Llm.stream(@messages, @tools,
                 api_key: "k",
                 endpoint: "https://openrouter.test/api/v1/chat/completions",
                 req_options: [plug: plug]
               )

      assert %ToolCall{id: "call_abc", name: "bash", arguments: %{"command" => "ls"}} = tc
    end
  end

  describe "stream/3 telemetry" do
    test "emits [:worg_agent, :llm, :delta] with session_id per chunk" do
      handler_id = :"stream_delta_#{System.unique_integer([:positive])}"
      session_id = "sid-" <> Integer.to_string(System.unique_integer([:positive]))
      test_pid = self()

      :telemetry.attach(
        handler_id,
        [:worg_agent, :llm, :delta],
        fn name, measurements, metadata, _ ->
          send(test_pid, {:tele, name, measurements, metadata})
        end,
        nil
      )

      on_exit(fn -> :telemetry.detach(handler_id) end)

      plug =
        sse_plug([
          %{"choices" => [%{"delta" => %{"content" => "x"}, "finish_reason" => nil}]},
          %{"choices" => [%{"delta" => %{"content" => "yz"}, "finish_reason" => nil}]},
          %{"choices" => [%{"delta" => %{}, "finish_reason" => "stop"}]}
        ])

      assert {:ok, _} =
               Llm.stream(@messages, @tools,
                 api_key: "k",
                 endpoint: "https://openrouter.test/api/v1/chat/completions",
                 req_options: [plug: plug],
                 session_id: session_id
               )

      assert_receive {:tele, [:worg_agent, :llm, :delta], _,
                      %{session_id: ^session_id, kind: :content, byte_count: 1}}

      assert_receive {:tele, [:worg_agent, :llm, :delta], _,
                      %{session_id: ^session_id, kind: :content, byte_count: 2}}
    end
  end

  describe "stream/3 error paths" do
    test "missing api_key returns :missing_api_key (does NOT hit the network)" do
      # Pass api_key: nil AND clear the env var for this call so the
      # OPENROUTER_API_KEY shell default doesn't mask the test.
      prior = System.get_env("OPENROUTER_API_KEY")
      System.delete_env("OPENROUTER_API_KEY")
      on_exit(fn -> if prior, do: System.put_env("OPENROUTER_API_KEY", prior) end)

      assert {:error, :missing_api_key} = Llm.stream(@messages, @tools, api_key: nil)
    end

    # NOTE: an HTTP-error path test (e.g. assert {:error, {:http, 401, _}}
    # via a Plug returning a non-chunked 401) would belong here, but Req's
    # :plug adapter does not currently honor `into:` for stream collection
    # — the plug is bypassed and the request leaks to the real OpenRouter
    # endpoint. The non-streaming Llm.call tests already cover the
    # `{:http, status, body}` error shape, and the production code path
    # for that pattern is shared. Revisit if Req's plug adapter gains
    # streaming support.
  end
end
