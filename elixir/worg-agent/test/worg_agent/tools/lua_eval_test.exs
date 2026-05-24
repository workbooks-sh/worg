defmodule WorgAgent.Tools.LuaEvalTest do
  use ExUnit.Case, async: true

  alias WorgAgent.Tools.LuaEval

  describe "metadata" do
    test "name + description + input_schema are well-formed" do
      assert LuaEval.name() == "lua_eval"
      assert is_binary(LuaEval.description())
      assert "code" in LuaEval.input_schema()["required"]
    end
  end

  describe "execute/2 — happy path" do
    test "evaluates an arithmetic expression" do
      assert {:ok, "3"} = LuaEval.execute(%{"code" => "return 1+2"}, %{})
    end

    test "evaluates a string return" do
      assert {:ok, "hello"} = LuaEval.execute(%{"code" => "return \"hello\""}, %{})
    end

    test "evaluates a float" do
      assert {:ok, "3.14"} = LuaEval.execute(%{"code" => "return 3.14"}, %{})
    end

    test "evaluates boolean return values" do
      assert {:ok, "true"} = LuaEval.execute(%{"code" => "return true"}, %{})
      assert {:ok, "false"} = LuaEval.execute(%{"code" => "return false"}, %{})
    end

    test "void return is the empty string" do
      assert {:ok, ""} = LuaEval.execute(%{"code" => "local x = 5"}, %{})
    end

    test "multi-statement source with locals" do
      assert {:ok, "15"} =
               LuaEval.execute(
                 %{"code" => "local x = 5; local y = 3; return x*y"},
                 %{}
               )
    end

    test "multiple return values are tab-separated" do
      assert {:ok, "1\t2\t3"} = LuaEval.execute(%{"code" => "return 1, 2, 3"}, %{})
    end

    test "string concatenation via .." do
      assert {:ok, "ab"} = LuaEval.execute(%{"code" => "return \"a\" .. \"b\""}, %{})
    end
  end

  describe "execute/2 — error paths" do
    test "syntax error surfaces as {:lua_parse_error, _}" do
      assert {:error, {:lua_parse_error, msg}} =
               LuaEval.execute(%{"code" => "this @ is not valid lua"}, %{})

      assert is_binary(msg)
      assert byte_size(msg) > 0
    end

    test "runtime error from error() surfaces as {:lua_runtime_error, _}" do
      assert {:error, {:lua_runtime_error, msg}} =
               LuaEval.execute(%{"code" => "error(\"boom\")"}, %{})

      assert msg =~ "boom"
    end

    test "type mismatch at runtime surfaces as :lua_runtime_error" do
      assert {:error, {:lua_runtime_error, _}} =
               LuaEval.execute(%{"code" => "return nil + 1"}, %{})
    end
  end

  describe "execute/2 — sandbox semantics" do
    test "print is suppressed — no stdout pollution" do
      # If print weren't suppressed, ExUnit's captured stdio would
      # show "noisy" during the test. We capture and assert it stays
      # empty.
      output =
        ExUnit.CaptureIO.capture_io(fn ->
          {:ok, "42"} = LuaEval.execute(%{"code" => "print(\"noisy\"); return 42"}, %{})
        end)

      refute output =~ "noisy"
    end

    test "tables are returned as an opaque <lua table ...> string" do
      assert {:ok, "<lua table " <> _} =
               LuaEval.execute(%{"code" => "return {1, 2, 3}"}, %{})
    end

    test "each invocation gets a fresh state — no cross-call leakage" do
      {:ok, ""} = LuaEval.execute(%{"code" => "_G.SECRET = 42"}, %{})
      # If state leaked, this would return "42". A fresh init means
      # SECRET is nil.
      assert {:ok, "nil"} = LuaEval.execute(%{"code" => "return _G.SECRET"}, %{})
    end
  end

  describe "execute/2 — argument validation" do
    test "missing code key returns :bad_args" do
      assert {:error, {:bad_args, _}} = LuaEval.execute(%{}, %{})
    end

    test "non-string code returns :bad_args" do
      assert {:error, {:bad_args, _}} = LuaEval.execute(%{"code" => 123}, %{})
    end
  end
end
