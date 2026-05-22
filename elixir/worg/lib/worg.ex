defmodule Worg do
  @moduledoc """
  Worg — canonical org-mode planning library for multi-agent orchestration.

  Library API. Not a service. Not supervised. Pure functions over `.org`
  document strings, backed by a Rust NIF (`Worg.Parser`) that wraps
  `worg-parse` + `worg-query` + `worg-lint`.

  Three concept boundaries:

    1. **Parse / serialize / mutate** — `parse/1`, `serialize/1`, `transition_todo/3`,
       `append_logbook/3`, `write_results/3`.
    2. **Query** — `query/2`, `ready/1`, `ready/2`.
    3. **Execute source blocks** — `Worg.Exec.{shell, lua, elixir}/2`,
       `Worg.tangle/2`.

  The caller decides when to invoke any of these. There is no scheduler ticking
  in the background. If an agent process is following a plan, it calls
  `ready/1`, picks a headline, calls a mutator + executor, writes results back,
  and the loop is its own — Worg has no opinion about it.

  ## Atomic writes

  All mutators take a *document string* and return a new document string. The
  caller is responsible for writing that string to disk atomically (temp file
  + rename). `Worg.AtomicFile.write/2` provides a convenience wrapper. For
  Studio: the agent writes the string to R2 + upserts derived Ecto rows in a
  single `Ecto.Multi`. Worg has no R2 or Ecto dependency.

  This module is a stub at the time of handoff. Real implementation lands as
  part of wb-4vhr.15 (executor primitives) and a new issue for the library
  surface itself.
  """

  # ───── Parse / serialize ─────

  @doc """
  Parse a worg document. Returns an opaque parsed handle that subsequent
  functions take. Internally this is the source string — every call to a
  mutator hands the string to the NIF, which parses + mutates + re-emits.
  """
  @spec parse(String.t()) :: {:ok, String.t()} | {:error, term}
  def parse(_src), do: raise("not yet implemented — wb-4vhr handoff stub")

  @doc "Round-trip invariant check. Returns true iff parse(s) |> serialize == s."
  @spec round_trip_ok?(String.t()) :: boolean
  def round_trip_ok?(_src), do: raise("not yet implemented")

  # ───── Mutators (return updated document text) ─────

  @doc "Transition a headline's TODO keyword. Returns {:ok, new_src} or {:error, reason}."
  @spec transition_todo(String.t(), String.t(), String.t()) :: {:ok, String.t()} | {:error, term}
  def transition_todo(_src, _id, _new_state), do: raise("not yet implemented")

  @doc "Append an entry to a headline's :LOGBOOK: drawer."
  @spec append_logbook(String.t(), String.t(), String.t()) :: {:ok, String.t()} | {:error, term}
  def append_logbook(_src, _id, _entry), do: raise("not yet implemented")

  @doc "Write or replace #+RESULTS: under the first source block of a headline."
  @spec write_results(String.t(), String.t(), String.t()) :: {:ok, String.t()} | {:error, term}
  def write_results(_src, _id, _results), do: raise("not yet implemented")

  # ───── Query ─────

  @doc """
  Run a predicate over the document. Predicate is a map matching
  `worg_query::Predicate`'s serde shape, e.g.:

      %{kind: "and", of: [
        %{kind: "state", state: "TODO"},
        %{kind: "ready"}
      ]}
  """
  @spec query(String.t(), map) :: {:ok, [map]} | {:error, term}
  def query(_src, _predicate), do: raise("not yet implemented")

  @doc "Convenience: headlines whose state is in TODO/DOING and whose :DEPENDS_ON: is satisfied."
  @spec ready(String.t()) :: {:ok, [map]} | {:error, term}
  def ready(src), do: query(src, %{kind: "and", of: [%{kind: "state", state: "TODO"}, %{kind: "ready"}]})
end
