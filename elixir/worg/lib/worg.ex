defmodule Worg do
  @moduledoc """
  Worg — canonical org-mode planning library.

  Top-level facade. Pure functions over .org source text:

    * **Parse / serialize** — Worg.parse/1, Worg.round_trip_ok?/1
    * **Mutators** — Worg.transition_todo/3, Worg.append_logbook/3,
      Worg.append_drawer/4, Worg.set_property/4, Worg.add_child/5,
      Worg.write_results/3
    * **Query** — Worg.query/2, Worg.ready/1
    * **Execute** — `Worg.Exec.{shell, lua, elixir}/2` (run source blocks)
    * **Tangle** — `Worg.Tangle.tangle/2` (materialize :tangle blocks to disk)
    * **Persist** — `Worg.AtomicFile.write/2` (write-and-rename helper)

  Delegates to `Worg.Parser` (Rustler NIF over worg-nif) for parse / mutate
  / query. The facade is the stable public API; `Worg.Parser` is the
  implementation seam and may move when the Wasmex bridge replaces
  Rustler for some consumers (wb-6irl.33).

  The "parsed handle" returned by `parse/1` is the source string itself —
  every mutator is a pure (src, args) -> {:ok, new_src} | {:error, term}
  function. There is no in-memory document state to manage; that's the
  worg ethos. Callers persist new_src however they want.
  """

  alias Worg.Parser

  # ───── Parse / serialize ─────

  @doc """
  Parse a worg document. Returns the source string wrapped in `{:ok, _}`
  if it parses cleanly + round-trips (every byte we read can be written
  back), `{:error, :round_trip_failed}` otherwise.

  The returned value is the source string itself — every subsequent
  mutator takes the string and returns a new string.
  """
  @spec parse(String.t()) :: {:ok, String.t()} | {:error, term}
  def parse(src) when is_binary(src) do
    if Parser.round_trip_ok(src) do
      {:ok, src}
    else
      {:error, :round_trip_failed}
    end
  end

  @doc "Round-trip invariant check. True iff parse(src) |> serialize == src."
  @spec round_trip_ok?(String.t()) :: boolean
  def round_trip_ok?(src) when is_binary(src), do: Parser.round_trip_ok(src)

  # ───── Mutators (return updated document text) ─────

  @doc "Transition a headline's TODO keyword. Returns {:ok, new_src} or {:error, reason}."
  @spec transition_todo(String.t(), String.t(), String.t()) ::
          {:ok, String.t()} | {:error, term}
  def transition_todo(src, id, new_state)
      when is_binary(src) and is_binary(id) and is_binary(new_state),
      do: Parser.transition_todo(src, id, new_state)

  @doc "Append an entry to a headline's :LOGBOOK: drawer."
  @spec append_logbook(String.t(), String.t(), String.t()) ::
          {:ok, String.t()} | {:error, term}
  def append_logbook(src, id, entry)
      when is_binary(src) and is_binary(id) and is_binary(entry),
      do: Parser.append_logbook(src, id, entry)

  @doc "Append to ANY named drawer (NOTES, CONSTRAINTS, custom)."
  @spec append_drawer(String.t(), String.t(), String.t(), String.t()) ::
          {:ok, String.t()} | {:error, term}
  def append_drawer(src, id, drawer_name, entry)
      when is_binary(src) and is_binary(id) and is_binary(drawer_name) and is_binary(entry),
      do: Parser.append_drawer(src, id, drawer_name, entry)

  @doc "Set/update a :PROPERTIES: key. :ID: is reserved."
  @spec set_property(String.t(), String.t(), String.t(), String.t()) ::
          {:ok, String.t()} | {:error, term}
  def set_property(src, id, name, value)
      when is_binary(src) and is_binary(id) and is_binary(name) and is_binary(value),
      do: Parser.set_property(src, id, name, value)

  @doc """
  Add a child headline under `parent_id`. `state` is optional (`nil` for a
  plain headline). `child_id` becomes the new headline's `:ID:` property.
  """
  @spec add_child(String.t(), String.t(), String.t(), String.t() | nil, String.t()) ::
          {:ok, String.t()} | {:error, term}
  def add_child(src, parent_id, title, state, child_id)
      when is_binary(src) and is_binary(parent_id) and is_binary(title) and
             (is_binary(state) or is_nil(state)) and is_binary(child_id),
      do: Parser.add_child(src, parent_id, title, state, child_id)

  @doc "Write or replace #+RESULTS: under the first source block of a headline."
  @spec write_results(String.t(), String.t(), String.t()) ::
          {:ok, String.t()} | {:error, term}
  def write_results(src, id, results)
      when is_binary(src) and is_binary(id) and is_binary(results),
      do: Parser.write_results(src, id, results)

  # ───── Query ─────

  @doc """
  Run a predicate over the document. Predicate is a map matching
  `worg_query::Predicate`'s serde shape, e.g.:

      %{kind: "and", of: [
        %{kind: "state", state: "TODO"},
        %{kind: "ready"}
      ]}

  Returns `{:ok, [headline_summary]}` (list of maps from
  parse_headlines_json), or `{:error, term}`.
  """
  @spec query(String.t(), map) :: {:ok, [map]} | {:error, term}
  def query(src, predicate) when is_binary(src) and is_map(predicate) do
    with {:ok, pred_json} <- Jason.encode(predicate),
         {:ok, json} <- Parser.query_json(src, pred_json),
         {:ok, results} <- Jason.decode(json) do
      {:ok, results}
    else
      {:error, _} = err -> err
      other -> {:error, {:query_failed, other}}
    end
  end

  @doc """
  Convenience: list NEXT/TODO headlines whose :BLOCKER: is satisfied.
  Same shape as Worg.query/2 returns.
  """
  @spec ready(String.t()) :: {:ok, [map]} | {:error, term}
  def ready(src) when is_binary(src) do
    query(src, %{
      "kind" => "and",
      "of" => [
        %{"kind" => "state", "state" => "TODO"},
        %{"kind" => "ready"}
      ]
    })
  end
end
