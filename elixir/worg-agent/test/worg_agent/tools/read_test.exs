defmodule WorgAgent.Tools.ReadTest do
  use ExUnit.Case, async: true

  alias WorgAgent.Tools.Read

  setup do
    tmp = System.tmp_dir!() |> Path.join("worg-agent-read-test-#{:rand.uniform(99_999_999)}")
    File.mkdir_p!(tmp)
    on_exit(fn -> File.rm_rf!(tmp) end)
    {:ok, tmp: tmp}
  end

  test "metadata fields are well-formed" do
    assert Read.name() == "read"
    assert is_binary(Read.description())

    schema = Read.input_schema()
    assert schema["type"] == "object"
    assert "path" in schema["required"]
  end

  test "reads a file via relative path against working_dir", %{tmp: tmp} do
    File.write!(Path.join(tmp, "hello.txt"), "hi there\n")
    {:ok, contents} = Read.execute(%{"path" => "hello.txt"}, %{working_dir: tmp})
    assert contents == "hi there\n"
  end

  test "reads via absolute path", %{tmp: tmp} do
    abs = Path.join(tmp, "abs.txt")
    File.write!(abs, "absolute")
    {:ok, contents} = Read.execute(%{"path" => abs}, %{working_dir: "/elsewhere"})
    assert contents == "absolute"
  end

  test "missing file returns :read_failed", %{tmp: tmp} do
    assert {:error, {:read_failed, _, :enoent}} =
             Read.execute(%{"path" => "nope.txt"}, %{working_dir: tmp})
  end

  test "missing path arg returns :bad_args" do
    assert {:error, {:bad_args, _}} = Read.execute(%{}, %{})
  end

  test "no trust gate — read works without trust_level" do
    File.write!(Path.join(System.tmp_dir!(), "worg-read-untrusted.txt"), "x")

    assert {:ok, "x"} =
             Read.execute(
               %{"path" => Path.join(System.tmp_dir!(), "worg-read-untrusted.txt")},
               %{}
             )

    File.rm!(Path.join(System.tmp_dir!(), "worg-read-untrusted.txt"))
  end
end
