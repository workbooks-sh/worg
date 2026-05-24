defmodule WorgAgent.Llm.ToolCallTest do
  use ExUnit.Case, async: true

  alias WorgAgent.Llm.ToolCall

  test "decodes a well-formed tool_call object" do
    wire = %{
      "id" => "call_abc",
      "type" => "function",
      "function" => %{
        "name" => "bash",
        "arguments" => ~s({"command": "ls -la"})
      }
    }

    assert {:ok, %ToolCall{id: "call_abc", name: "bash", arguments: %{"command" => "ls -la"}}} =
             ToolCall.from_wire(wire)
  end

  test "arguments are decoded as a map" do
    wire = %{
      "id" => "x",
      "type" => "function",
      "function" => %{"name" => "y", "arguments" => ~s({"a": 1, "b": [2, 3]})}
    }

    assert {:ok, %ToolCall{arguments: %{"a" => 1, "b" => [2, 3]}}} = ToolCall.from_wire(wire)
  end

  test "non-object arguments string returns :arguments_not_object" do
    wire = %{
      "id" => "x",
      "type" => "function",
      "function" => %{"name" => "y", "arguments" => "[1, 2, 3]"}
    }

    assert {:error, {:arguments_not_object, _}} = ToolCall.from_wire(wire)
  end

  test "malformed arguments JSON returns :invalid_arguments_json" do
    wire = %{
      "id" => "x",
      "type" => "function",
      "function" => %{"name" => "y", "arguments" => "{ broken"}
    }

    assert {:error, {:invalid_arguments_json, _}} = ToolCall.from_wire(wire)
  end

  test "missing fields return :invalid_tool_call_shape" do
    assert {:error, {:invalid_tool_call_shape, _}} = ToolCall.from_wire(%{})
    assert {:error, {:invalid_tool_call_shape, _}} = ToolCall.from_wire(%{"id" => "x"})
    assert {:error, {:invalid_tool_call_shape, _}} = ToolCall.from_wire(%{"function" => %{}})
  end
end
