defmodule WorgAgent.Llm.StreamTest do
  use ExUnit.Case, async: true

  alias WorgAgent.Llm.{Response, Stream, ToolCall}

  describe "parse_frames/2" do
    test "extracts a single data event" do
      chunk = "data: {\"x\":1}\n\n"
      {events, rest} = Stream.parse_frames(chunk)
      assert events == [{:event, %{"x" => 1}}]
      assert rest == ""
    end

    test "handles the [DONE] sentinel" do
      {events, _} = Stream.parse_frames("data: [DONE]\n\n")
      assert events == [:done]
    end

    test "buffers partial frames across chunks" do
      {events1, buf1} = Stream.parse_frames("data: {\"a")
      assert events1 == []
      assert buf1 != ""

      {events2, buf2} = Stream.parse_frames("\":1}\n\n", buf1)
      assert events2 == [{:event, %{"a" => 1}}]
      assert buf2 == ""
    end

    test "splits multiple frames in one chunk" do
      chunk = "data: {\"i\":1}\n\ndata: {\"i\":2}\n\ndata: [DONE]\n\n"
      {events, rest} = Stream.parse_frames(chunk)
      assert events == [{:event, %{"i" => 1}}, {:event, %{"i" => 2}}, :done]
      assert rest == ""
    end

    test "tolerates \\r\\n\\r\\n separator (some proxies rewrite)" do
      chunk = "data: {\"a\":1}\r\n\r\ndata: {\"a\":2}\r\n\r\n"
      {events, _} = Stream.parse_frames(chunk)
      assert events == [{:event, %{"a" => 1}}, {:event, %{"a" => 2}}]
    end

    test "marks unparseable data as :invalid (does not crash)" do
      {events, _} = Stream.parse_frames("data: this-is-not-json\n\n")
      assert [{:invalid, "this-is-not-json"}] = events
    end

    test "ignores comment lines (lines not starting with data:)" do
      chunk = ": keepalive\ndata: {\"x\":1}\n\n"
      {events, _} = Stream.parse_frames(chunk)
      assert events == [{:event, %{"x" => 1}}]
    end
  end

  describe "apply_event/2 + finalize/1 — content accumulation" do
    test "concatenates content fragments in order" do
      acc = Stream.new_acc()

      acc =
        ["Hello, ", "world", "!"]
        |> Enum.reduce(acc, fn frag, a ->
          Stream.apply_event(
            a,
            {:event, %{"choices" => [%{"delta" => %{"content" => frag}}]}}
          )
        end)

      acc =
        Stream.apply_event(
          acc,
          {:event, %{"choices" => [%{"delta" => %{}, "finish_reason" => "stop"}]}}
        )

      response = Stream.finalize(acc)
      assert %Response{content: "Hello, world!", tool_calls: [], stop_reason: :end_turn} = response
    end

    test "handles empty content fragments without choking" do
      acc =
        Stream.new_acc()
        |> Stream.apply_event(
          {:event, %{"choices" => [%{"delta" => %{"content" => ""}}]}}
        )
        |> Stream.apply_event(
          {:event, %{"choices" => [%{"delta" => %{"content" => "ok"}, "finish_reason" => "stop"}]}}
        )

      assert %Response{content: "ok"} = Stream.finalize(acc)
    end

    test "captures usage when emitted with the terminal frame" do
      acc =
        Stream.new_acc()
        |> Stream.apply_event(
          {:event,
           %{
             "choices" => [%{"delta" => %{}, "finish_reason" => "stop"}],
             "usage" => %{"prompt_tokens" => 5, "completion_tokens" => 3}
           }}
        )

      response = Stream.finalize(acc)
      assert response.usage == %{"prompt_tokens" => 5, "completion_tokens" => 3}
    end
  end

  describe "apply_event/2 — tool-call merging" do
    test "merges id+name then concatenates arguments fragments by index" do
      frames = [
        # First frame: id + name set, args starts empty
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
        # Args fragment #1
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
        # Args fragment #2
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
        # Terminal
        %{"choices" => [%{"delta" => %{}, "finish_reason" => "tool_calls"}]}
      ]

      acc =
        Enum.reduce(frames, Stream.new_acc(), fn frame, a ->
          Stream.apply_event(a, {:event, frame})
        end)

      response = Stream.finalize(acc)
      assert [%ToolCall{id: "call_abc", name: "bash", arguments: %{"command" => "ls"}}] =
               response.tool_calls

      assert response.stop_reason == :tool_calls
    end

    test "preserves index order when tool_calls arrive out-of-order" do
      frames = [
        %{
          "choices" => [
            %{
              "delta" => %{
                "tool_calls" => [
                  %{
                    "index" => 1,
                    "id" => "call_2",
                    "function" => %{"name" => "second", "arguments" => "{}"}
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
                  %{
                    "index" => 0,
                    "id" => "call_1",
                    "function" => %{"name" => "first", "arguments" => "{}"}
                  }
                ]
              }
            }
          ]
        }
      ]

      response =
        frames
        |> Enum.reduce(Stream.new_acc(), fn f, a -> Stream.apply_event(a, {:event, f}) end)
        |> Stream.finalize()

      assert [
               %ToolCall{name: "first"},
               %ToolCall{name: "second"}
             ] = response.tool_calls
    end
  end

  describe "finalize/1 stop_reason inference" do
    test "no explicit stop_reason + no tool calls → :end_turn" do
      response = Stream.finalize(Stream.new_acc())
      assert response.stop_reason == :end_turn
    end

    test "no explicit stop_reason but tool calls present → :tool_calls" do
      acc =
        Stream.new_acc()
        |> Stream.apply_event(
          {:event,
           %{
             "choices" => [
               %{
                 "delta" => %{
                   "tool_calls" => [
                     %{"index" => 0, "id" => "x", "function" => %{"name" => "n", "arguments" => "{}"}}
                   ]
                 }
               }
             ]
           }}
        )

      assert Stream.finalize(acc).stop_reason == :tool_calls
    end

    test "maps length → :max_tokens" do
      acc =
        Stream.new_acc()
        |> Stream.apply_event(
          {:event, %{"choices" => [%{"delta" => %{}, "finish_reason" => "length"}]}}
        )

      assert Stream.finalize(acc).stop_reason == :max_tokens
    end
  end
end
