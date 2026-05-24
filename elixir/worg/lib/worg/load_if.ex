defmodule Worg.LoadIf do
  @moduledoc """
  Conditional subtree loading via the `:LOAD_IF:` property (wb-q4dk).

  Strips headlines (and their descendants) whose `:LOAD_IF:` property
  doesn't match a runtime variable map. The use case: a wavelet-director
  plan declares four creative-concept playbooks at the same outline
  level, each guarded by `:LOAD_IF: concept=cinematic` (or `organic`,
  etc.). At dispatch time, the caller already picked a concept; loading
  all four playbooks into the system prompt wastes ~1500 tokens of
  context.

      ┌─────────────────────────────────────────┐
      │ * Cinematic playbook                    │
      │   :PROPERTIES:                          │
      │   :LOAD_IF: concept=cinematic           │
      │   :END:                                 │
      │   ...300 tokens of cinematography rules │
      └─────────────────────────────────────────┘

  Given `vars = %{"concept" => "cinematic"}`, this headline + its
  descendants pass through; the `organic` / `direct-response` /
  `animated` siblings are stripped.

  Implementation: pure-Elixir line scan. Avoids adding a NIF because:

    - The :LOAD_IF: filter is structural, not requiring full re-parse.
    - The .org line format is unambiguous for the headline / property
      shapes we care about (`* headline`, `:LOAD_IF: <expr>`, `:END:`).
    - Keeping it in Elixir means the filter is hot-reloadable and easy
      to extend with new expression shapes without rebuilding the NIF.

  ## Expression shapes

  Initial release supports a single shape: `KEY=VALUE` (no spaces). A
  headline survives iff `vars[KEY] == VALUE`. Anything more elaborate
  (boolean ops, multi-key matches) is deferred until a real use case
  demands it.

  Headlines with NO `:LOAD_IF:` always pass through (no filter ==
  "always load").
  """

  @doc """
  Filter the org source, removing every headline (and its descendants)
  whose `:LOAD_IF:` doesn't match `vars`. Returns the filtered source.

  Top-level keywords (`#+TITLE:`, `#+TODO:`, etc) and content before
  the first headline are preserved verbatim.
  """
  @spec filter(String.t(), %{optional(String.t()) => String.t()}) :: String.t()
  def filter(src, vars) when is_binary(src) and is_map(vars) do
    src
    |> String.split("\n", trim: false)
    |> walk(vars, [], nil)
    |> Enum.reverse()
    |> Enum.join("\n")
  end

  # State: acc (kept lines, reversed), skip_until_level (when non-nil,
  # we're inside a stripped subtree; resume when we hit a headline at
  # level <= skip_until_level).
  defp walk([], _vars, acc, _skip), do: acc

  defp walk([line | rest], vars, acc, skip) when not is_nil(skip) do
    case headline_level(line) do
      nil ->
        # In a stripped subtree; drop the line.
        walk(rest, vars, acc, skip)

      level when level <= skip ->
        # Sibling-or-uncle headline — stripped subtree is over, this
        # line is a fresh headline that needs its own evaluation.
        walk([line | rest], vars, acc, nil)

      _ ->
        # Deeper headline still inside the stripped subtree.
        walk(rest, vars, acc, skip)
    end
  end

  defp walk([line | rest], vars, acc, nil) do
    case headline_level(line) do
      nil ->
        # Non-headline line OUTSIDE a stripped subtree — keep.
        walk(rest, vars, [line | acc], nil)

      level ->
        case lookahead_load_if(rest) do
          nil ->
            # No :LOAD_IF: under this headline — always load.
            walk(rest, vars, [line | acc], nil)

          load_if ->
            if matches?(load_if, vars) do
              walk(rest, vars, [line | acc], nil)
            else
              # Strip THIS headline + everything until next same-or-
              # shallower-level headline. Don't add the current line.
              walk(rest, vars, acc, level)
            end
        end
    end
  end

  # ───── helpers ────────────────────────────────────────────────────

  # Returns the asterisk count (1, 2, 3, ...) for an org headline
  # line, or nil if the line isn't a headline. Headlines must start
  # with N>=1 asterisks followed by a space.
  defp headline_level(line) do
    case Regex.run(~r/^(\*+)\s/, line, capture: :all_but_first) do
      [stars] -> String.length(stars)
      _ -> nil
    end
  end

  # Scan forward for `:LOAD_IF: <expr>` inside the headline's
  # :PROPERTIES: drawer. Returns the expr string or nil. Stops at the
  # next headline (whether the property exists or not).
  defp lookahead_load_if(lines) do
    Enum.reduce_while(lines, nil, fn line, acc ->
      cond do
        # Hit next headline — stop, return whatever we found (or nil).
        Regex.match?(~r/^\*+\s/, line) ->
          {:halt, acc}

        # Match :LOAD_IF: <expr> case-insensitively (org property
        # lookups are case-insensitive per wb-qwj8.3).
        match = Regex.run(~r/^\s*:LOAD_IF:\s*(.+?)\s*$/i, line) ->
          [_, expr] = match
          {:halt, expr}

        true ->
          {:cont, acc}
      end
    end)
  end

  # Single shape supported in this release: KEY=VALUE. Whitespace
  # around the `=` is tolerated. Returns true iff vars[KEY] == VALUE.
  defp matches?(expr, vars) do
    case String.split(expr, "=", parts: 2) do
      [key, value] ->
        Map.get(vars, String.trim(key)) == String.trim(value)

      _ ->
        # Unparseable expression — fail-safe to "don't load" so a
        # malformed property doesn't accidentally leak content.
        false
    end
  end
end
