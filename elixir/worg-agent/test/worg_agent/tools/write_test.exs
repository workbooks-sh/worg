defmodule WorgAgent.Tools.WriteTest do
  use ExUnit.Case, async: true

  alias WorgAgent.Tools.Write

  setup do
    tmp = System.tmp_dir!() |> Path.join("worg-agent-write-test-#{:rand.uniform(99_999_999)}")
    File.mkdir_p!(tmp)
    on_exit(fn -> File.rm_rf!(tmp) end)
    {:ok, tmp: tmp, ctx: %{trust_level: :sandboxed, working_dir: tmp}}
  end

  test "metadata fields are well-formed" do
    assert Write.name() == "write"
    schema = Write.input_schema()
    assert "path" in schema["required"]
    assert "content" in schema["required"]
  end

  test "writes a file to a relative path", %{ctx: ctx, tmp: tmp} do
    {:ok, msg} = Write.execute(%{"path" => "out.txt", "content" => "hello"}, ctx)
    assert msg =~ "wrote 5 bytes"
    assert File.read!(Path.join(tmp, "out.txt")) == "hello"
  end

  test "creates parent directories if missing", %{ctx: ctx, tmp: tmp} do
    {:ok, _} = Write.execute(%{"path" => "a/b/c/deep.txt", "content" => "x"}, ctx)
    assert File.read!(Path.join(tmp, "a/b/c/deep.txt")) == "x"
  end

  test "overwrites existing files", %{ctx: ctx, tmp: tmp} do
    File.write!(Path.join(tmp, "exists.txt"), "old")
    {:ok, _} = Write.execute(%{"path" => "exists.txt", "content" => "new"}, ctx)
    assert File.read!(Path.join(tmp, "exists.txt")) == "new"
  end

  test "refuses without trust gate" do
    assert {:error, {:trust, _}} = Write.execute(%{"path" => "x", "content" => "y"}, %{})
  end

  test "bad args returns :bad_args", %{ctx: ctx} do
    assert {:error, {:bad_args, _}} = Write.execute(%{"path" => "x"}, ctx)
    assert {:error, {:bad_args, _}} = Write.execute(%{"content" => "y"}, ctx)
    assert {:error, {:bad_args, _}} = Write.execute(%{}, ctx)
  end
end
