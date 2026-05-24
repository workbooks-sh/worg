defmodule WorgAgentTest do
  use ExUnit.Case
  doctest WorgAgent

  test "module is defined and has documentation" do
    {:docs_v1, _, _, _, %{"en" => doc}, _, _} = Code.fetch_docs(WorgAgent)
    assert is_binary(doc)
    assert String.contains?(doc, "Elixir runtime for WORG-defined agents")
  end

  test "OTP application boots with the configured supervision tree" do
    # The :worg_agent app is started automatically by ExUnit via the
    # `application` callback in mix.exs. Reaching the assertion at all
    # means start/2 returned :ok and the supervisor is alive.
    assert Process.whereis(WorgAgent.Supervisor) |> is_pid()
  end

  test "supervisor starts with no children at scaffold time" do
    # Children land alongside the modules that need them in
    # wb-nlln.21.3 / .4 / .5. Scaffold-time invariant: zero children.
    assert Supervisor.which_children(WorgAgent.Supervisor) == []
  end
end
