defmodule WorgAgent.Llm.ResponseTest do
  use ExUnit.Case, async: true

  alias WorgAgent.Llm.Response

  test "parses a text-only response" do
    body = %{
      "choices" => [
        %{
          "message" => %{"role" => "assistant", "content" => "hello"},
          "finish_reason" => "stop"
        }
      ],
      "usage" => %{"prompt_tokens" => 10, "completion_tokens" => 2}
    }

    assert {:ok, %Response{} = r} = Response.from_wire(body)
    assert r.content == "hello"
    assert r.tool_calls == []
    assert r.stop_reason == :end_turn
    assert r.usage == %{"prompt_tokens" => 10, "completion_tokens" => 2}
  end

  test "parses a response with tool_calls" do
    body = %{
      "choices" => [
        %{
          "message" => %{
            "role" => "assistant",
            "content" => nil,
            "tool_calls" => [
              %{
                "id" => "call_1",
                "type" => "function",
                "function" => %{"name" => "bash", "arguments" => ~s({"command": "pwd"})}
              },
              %{
                "id" => "call_2",
                "type" => "function",
                "function" => %{"name" => "read", "arguments" => ~s({"path": "x"})}
              }
            ]
          },
          "finish_reason" => "tool_calls"
        }
      ]
    }

    assert {:ok, %Response{tool_calls: [c1, c2], stop_reason: :tool_calls, content: nil}} =
             Response.from_wire(body)

    assert c1.name == "bash"
    assert c1.arguments == %{"command" => "pwd"}
    assert c2.name == "read"
    assert c2.arguments == %{"path" => "x"}
  end

  test "preserves tool_call order" do
    calls =
      Enum.map(1..5, fn i ->
        %{
          "id" => "call_#{i}",
          "type" => "function",
          "function" => %{"name" => "t#{i}", "arguments" => "{}"}
        }
      end)

    body = %{"choices" => [%{"message" => %{"tool_calls" => calls}, "finish_reason" => "tool_calls"}]}
    {:ok, %Response{tool_calls: parsed}} = Response.from_wire(body)
    assert Enum.map(parsed, & &1.id) == ["call_1", "call_2", "call_3", "call_4", "call_5"]
  end

  test "maps finish_reason to stop_reason atom" do
    for {wire, expected} <- [
          {"stop", :end_turn},
          {"tool_calls", :tool_calls},
          {"length", :max_tokens},
          {"content_filter", :other},
          {nil, :other}
        ] do
      body = %{"choices" => [%{"message" => %{"content" => "x"}, "finish_reason" => wire}]}
      assert {:ok, %Response{stop_reason: ^expected}} = Response.from_wire(body)
    end
  end

  test "preserves the full raw body for callers that need extras" do
    body = %{"choices" => [%{"message" => %{}, "finish_reason" => "stop"}], "id" => "resp_abc"}
    assert {:ok, %Response{raw: ^body}} = Response.from_wire(body)
  end

  test "missing choices returns :no_choices" do
    assert {:error, {:no_choices, _}} = Response.from_wire(%{})
    assert {:error, {:no_choices, _}} = Response.from_wire(%{"error" => "x"})
  end

  test "malformed tool_call propagates the error" do
    body = %{
      "choices" => [
        %{
          "message" => %{
            "tool_calls" => [%{"id" => "x", "function" => %{"name" => "y", "arguments" => "{ bad"}}]
          },
          "finish_reason" => "tool_calls"
        }
      ]
    }

    assert {:error, {:invalid_arguments_json, _}} = Response.from_wire(body)
  end
end
