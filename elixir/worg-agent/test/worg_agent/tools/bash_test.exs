defmodule WorgAgent.Tools.BashTest do
  use ExUnit.Case, async: true

  alias WorgAgent.Tools.Bash

  describe "behaviour metadata" do
    test "name + description + input_schema all defined" do
      assert Bash.name() == "bash"
      assert is_binary(Bash.description())
      assert String.length(Bash.description()) > 20

      schema = Bash.input_schema()
      assert schema["type"] == "object"
      assert schema["properties"]["command"]["type"] == "string"
      assert "command" in schema["required"]
    end
  end

  describe "execute/2 happy path" do
    setup do
      tmp = sandbox()
      ctx = %{trust_level: :sandboxed, working_dir: tmp}
      on_exit(fn -> File.rm_rf!(tmp) end)
      {:ok, ctx: ctx, tmp: tmp}
    end

    test "runs a simple command and returns stdout with exit=0", %{ctx: ctx} do
      {:ok, output} = Bash.execute(%{"command" => "echo hello"}, ctx)
      assert output =~ "exit=0"
      assert output =~ "hello"
    end

    test "non-zero exit is captured in the marker line", %{ctx: ctx} do
      {:ok, output} = Bash.execute(%{"command" => "exit 7"}, ctx)
      assert output =~ "exit=7"
    end

    test "stderr is merged into stdout", %{ctx: ctx} do
      {:ok, output} = Bash.execute(%{"command" => "echo oops 1>&2"}, ctx)
      assert output =~ "oops"
    end

    test "runs in the configured working_dir", %{ctx: ctx, tmp: tmp} do
      {:ok, output} = Bash.execute(%{"command" => "pwd"}, ctx)
      assert output =~ Path.expand(tmp)
    end
  end

  describe "execute/2 trust + arg errors" do
    test "refuses when trust_level is absent" do
      assert {:error, {:trust, _}} = Bash.execute(%{"command" => "echo x"}, %{})
    end

    test "refuses when trust_level is something else" do
      assert {:error, {:trust, _}} =
               Bash.execute(%{"command" => "echo x"}, %{trust_level: :none})
    end

    test ":full trust is accepted" do
      {:ok, output} = Bash.execute(%{"command" => "echo ok"}, %{trust_level: :full})
      assert output =~ "ok"
    end

    test "missing command argument returns :bad_args" do
      assert {:error, {:bad_args, _}} =
               Bash.execute(%{}, %{trust_level: :sandboxed})
    end

    test "wrong-type command returns :bad_args" do
      assert {:error, {:bad_args, _}} =
               Bash.execute(%{"command" => 42}, %{trust_level: :sandboxed})
    end
  end

  defp sandbox do
    tmp = System.tmp_dir!() |> Path.join("worg-agent-bash-test-#{:rand.uniform(99_999_999)}")
    File.mkdir_p!(tmp)
    tmp
  end
end
