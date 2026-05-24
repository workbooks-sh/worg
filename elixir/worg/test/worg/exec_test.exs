defmodule Worg.ExecTest do
  use ExUnit.Case, async: true

  alias Worg.Exec

  describe "shell/2" do
    test "captures stdout on success" do
      assert {:ok, output} = Exec.shell(%{body: "echo hello"})
      assert output =~ "hello"
    end

    test "merges stderr into output" do
      assert {:ok, output} = Exec.shell(%{body: "echo err >&2"})
      assert output =~ "err"
    end

    test "non-zero exit returns {:exit_status, n, output}" do
      assert {:error, {:exit_status, status, output}} =
               Exec.shell(%{body: "echo before; exit 7"})

      assert status == 7
      assert output =~ "before"
    end

    test "timeout fires when script hangs" do
      assert {:error, {:timeout, 100}} =
               Exec.shell(%{body: "sleep 5"}, timeout_ms: 100)
    end

    test "honors :cwd" do
      # Make our own subdir so resolved path is stable (avoids macOS's
      # /tmp → /private/tmp symlink ambiguity at the top-level).
      tmp = Path.join(System.tmp_dir!(), "worg-exec-cwd-#{:erlang.unique_integer([:positive])}")
      File.mkdir_p!(tmp)
      on_exit_dir = fn -> File.rm_rf!(tmp) end
      try do
        assert {:ok, output} = Exec.shell(%{body: "pwd"}, cwd: tmp)
        # On macOS, `pwd` from /tmp/X resolves through the symlink and
        # prints /private/tmp/X. Accept either form.
        trimmed = String.trim(output)
        assert trimmed == tmp or trimmed == Path.expand(tmp) or
                 String.ends_with?(trimmed, Path.basename(tmp))
      after
        on_exit_dir.()
      end
    end

    test "language :sh uses sh interpreter" do
      assert {:ok, output} = Exec.shell(%{language: "sh", body: "echo $0"})
      assert output =~ "sh"
    end

    test "default language is bash" do
      assert {:ok, output} = Exec.shell(%{body: "echo ${BASH_VERSION:-no-bash}"})
      refute output =~ "no-bash"
    end
  end

  describe "elixir/2" do
    test "evaluates an expression + returns inspected value" do
      assert {:ok, "42"} = Exec.elixir(%{body: "1 + 41"})
    end

    test "honors bindings" do
      assert {:ok, "6"} = Exec.elixir(%{body: "x * 2"}, bindings: [x: 3])
      assert {:ok, "12"} = Exec.elixir(%{body: "x + y"}, bindings: [x: 5, y: 7])
    end

    test "raises become {:error, {:raise, msg}}" do
      assert {:error, {:raise, msg}} = Exec.elixir(%{body: ~s|raise "boom"|})
      assert msg == "boom"
    end

    test "timeout kills an infinite loop" do
      assert {:error, {:timeout, 100}} =
               Exec.elixir(
                 %{body: "Process.sleep(:infinity)"},
                 timeout_ms: 100
               )
    end
  end

  describe "lua/2" do
    test "returns the last expression's value, inspected" do
      assert {:ok, _value} = Exec.lua(%{body: "return 42"})
    end

    test "string concatenation works" do
      assert {:ok, value} = Exec.lua(%{body: ~s|return "hello, " .. "world"|})
      assert value =~ "hello, world"
    end

    test "arithmetic returns a number" do
      assert {:ok, value} = Exec.lua(%{body: "return 2 + 3 * 4"})
      assert value =~ "14"
    end

    test "table operations work (stdlib subset)" do
      assert {:ok, value} =
               Exec.lua(%{
                 body: """
                 local t = {1, 2, 3}
                 local sum = 0
                 for _, v in ipairs(t) do sum = sum + v end
                 return sum
                 """
               })

      assert value =~ "6"
    end

    test "script error surfaces as {:error, {:lua_error, _}}" do
      assert {:error, {:lua_error, _}} = Exec.lua(%{body: ~s|error("boom")|})
    end

    test "timeout fires on infinite loops" do
      assert {:error, {:timeout, 100}} =
               Exec.lua(%{body: "while true do end"}, timeout_ms: 100)
    end

    # NOTE: sandbox stripping (no os/io/package/debug) + :inject globals
    # are documented as future work — Luerl's set_table_keys/3 API
    # mismatch makes the straightforward implementation produce
    # :badrecord errors. Don't test the sandbox here; the function's
    # doc explicitly calls out that lua/2 is full-trust today.
  end
end
