defmodule WorgAgent.Tools.LuaEval do
  @moduledoc """
  Evaluate Lua source in a sandboxed Luerl VM. Sandbox is enforced
  by Luerl running entirely inside BEAM — Lua's `os.execute`,
  `io.open`, and network primitives don't reach the host OS unless
  explicitly bridged.

  Each call gets a fresh Luerl state; there's no cross-call memory.
  `print` is overridden to a no-op so Lua's stdout side-effects
  don't pollute the calling process.

  Return value: a tab-separated string of the Lua expression's
  return values, formatted via `inspect/1` for tables and as-is
  for primitives. An empty return list yields `""`.
  """

  @behaviour WorgAgent.Tool

  # Prepended to every user invocation. Overrides `print` so Lua's
  # stdout writes don't bleed into the calling process's output.
  # Users can still override print themselves AFTER this; their
  # override wins. Line numbers in errors shift by 1 — acceptable
  # for v1.1.
  @sandbox_preamble "_G.print = function() end\n"

  @impl true
  def name, do: "lua_eval"

  @impl true
  def description do
    """
    Evaluate Lua source in the sandboxed Luerl runtime. Returns
    return values formatted as a tab-separated string. Useful for:
    arithmetic, string manipulation, structured data transforms
    without dropping to bash. Sandbox is enforced by Luerl — no
    os.execute, no file I/O, no network from Lua.
    """
  end

  @impl true
  def input_schema do
    %{
      "type" => "object",
      "properties" => %{
        "code" => %{
          "type" => "string",
          "description" =>
            "Lua source. The last `return` statement's values are returned. Standard Lua libraries are available; the OS-touching subset is inert in Luerl."
        }
      },
      "required" => ["code"]
    }
  end

  @impl true
  def execute(%{"code" => code}, _ctx) when is_binary(code) do
    full_source = @sandbox_preamble <> code

    try do
      case :luerl.do(full_source, :luerl.init()) do
        {:ok, vals, _state} ->
          {:ok, format_values(vals)}

        {:error, errs, _state} ->
          {:error, {:lua_parse_error, format_errors(errs)}}

        {:lua_error, details, _state} ->
          {:error, {:lua_runtime_error, format_runtime_error(details)}}
      end
    rescue
      e -> {:error, {:lua_eval_crashed, Exception.message(e)}}
    end
  end

  def execute(args, _ctx),
    do: {:error, {:bad_args, "expected %{\"code\" => string}; got #{inspect(args)}"}}

  defp format_values([]), do: ""
  defp format_values([v]), do: format_value(v)
  defp format_values(vs), do: Enum.map_join(vs, "\t", &format_value/1)

  defp format_value(v) when is_binary(v), do: v
  defp format_value(v) when is_integer(v), do: Integer.to_string(v)
  defp format_value(v) when is_float(v), do: Float.to_string(v)
  defp format_value(true), do: "true"
  defp format_value(false), do: "false"
  defp format_value(nil), do: "nil"
  defp format_value({:tref, _} = ref), do: "<lua table #{inspect(ref)}>"
  defp format_value(other), do: inspect(other)

  defp format_errors(errs) when is_list(errs) do
    errs
    |> Enum.map_join("\n", fn
      {line, _scanner, {:user, msg}} when is_list(msg) -> "line #{line}: #{List.to_string(msg)}"
      {line, _scanner, msg} when is_list(msg) -> "line #{line}: #{List.to_string(msg)}"
      other -> inspect(other)
    end)
  end

  defp format_errors(other), do: inspect(other)

  defp format_runtime_error({:error_call, [msg]}) when is_binary(msg), do: msg
  defp format_runtime_error({:error_call, [msg]}) when is_list(msg), do: List.to_string(msg)
  defp format_runtime_error(other), do: inspect(other)
end
