defmodule WorgAgent.ToolRegistryTest do
  use ExUnit.Case, async: true

  alias WorgAgent.ToolRegistry

  test "all/0 returns the default tool set when no config is set" do
    # config/config.exs may set this; the default-fallback path is
    # tested by clearing the env var inside the test.
    saved = Application.get_env(:worg_agent, :tools)
    Application.delete_env(:worg_agent, :tools)

    on_exit(fn ->
      if saved do
        Application.put_env(:worg_agent, :tools, saved)
      end
    end)

    tools = ToolRegistry.all()
    assert WorgAgent.Tools.Bash in tools
    assert WorgAgent.Tools.Read in tools
    assert WorgAgent.Tools.Write in tools
    assert WorgAgent.Tools.LuaEval in tools
  end

  test "lookup/1 finds a tool by its name/0 value" do
    assert ToolRegistry.lookup("bash") == WorgAgent.Tools.Bash
    assert ToolRegistry.lookup("read") == WorgAgent.Tools.Read
    assert ToolRegistry.lookup("write") == WorgAgent.Tools.Write
    assert ToolRegistry.lookup("lua_eval") == WorgAgent.Tools.LuaEval
  end

  test "lookup/1 returns nil for unknown names" do
    assert ToolRegistry.lookup("ghost") == nil
  end

  test "catalog/0 returns LLM-shaped tool descriptors" do
    catalog = ToolRegistry.catalog()
    assert is_list(catalog)
    assert length(catalog) >= 4

    bash_entry = Enum.find(catalog, &(&1["name"] == "bash"))
    assert is_binary(bash_entry["description"])
    assert is_map(bash_entry["input_schema"])
    assert bash_entry["input_schema"]["properties"]["command"]
  end

  test "dispatch/3 routes to the named tool" do
    {:ok, output} =
      ToolRegistry.dispatch("bash", %{"command" => "echo dispatched"}, %{trust_level: :sandboxed})

    assert output =~ "dispatched"
  end

  test "dispatch/3 returns :unknown_tool for unregistered names" do
    assert {:error, {:unknown_tool, "ghost"}} = ToolRegistry.dispatch("ghost", %{}, %{})
  end
end
