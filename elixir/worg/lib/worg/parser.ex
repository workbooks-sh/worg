defmodule Worg.Parser do
  @moduledoc """
  Rustler NIF wrapper over `worg-nif`.

  Loaded as `Elixir.Worg.Parser` (matches the `rustler::init!` call in
  `crates/worg-nif/src/lib.rs`). All NIFs run on dirty CPU schedulers since
  parsing is CPU-bound.

  This module is the thin Elixir face of the Rust parser. Public callers
  (agent processes, CLI subcommands, tests) should use `Worg` — not
  `Worg.Parser` directly — so that ergonomics stay consistent.
  """

  use Rustler,
    otp_app: :worg,
    crate: "worg-nif",
    path: "../../crates/worg-nif",
    load_from: {:worg, "priv/native/libworg_nif"}

  # All of these are replaced by the NIF at module load time. The function
  # bodies below are only reached if the NIF fails to load.

  @doc "Returns true iff the round-trip is byte-identical."
  def round_trip_ok(_src), do: nif_error()

  @doc "Returns a JSON string: array of headline summaries."
  def parse_headlines_json(_src), do: nif_error()

  @doc "Returns {:ok, new_src_string} | {:error, atom}."
  def transition_todo(_src, _id, _new_state), do: nif_error()

  @doc "Returns {:ok, new_src_string} | {:error, atom}."
  def append_logbook(_src, _id, _entry), do: nif_error()

  @doc "Append `entry` to the named drawer under `id`. Generic over LOGBOOK/NOTES/CONSTRAINTS/custom. Returns {:ok, new_src} | {:error, atom}."
  def append_drawer(_src, _id, _drawer_name, _entry), do: nif_error()

  @doc "Set or update the `name` property in the :PROPERTIES: drawer under `id`. :ID: is reserved. Returns {:ok, new_src} | {:error, atom}."
  def set_property(_src, _id, _name, _value), do: nif_error()

  @doc "Add a child headline under `parent_id`. State is optional. Returns {:ok, new_src} | {:error, atom}."
  def add_child(_src, _parent_id, _title, _state, _child_id), do: nif_error()

  @doc "Returns {:ok, new_src_string} | {:error, atom}."
  def write_results(_src, _id, _results), do: nif_error()

  @doc "Return every source block under `target_id` as a JSON array of {language, body, index}. Returns {:ok, json_string} | {:error, atom}."
  def source_blocks_json(_src, _target_id), do: nif_error()

  @doc "Returns {:ok, json_string} | {:error, atom}."
  def query_json(_src, _predicate_json), do: nif_error()

  @doc "Returns a JSON string: array of diagnostics."
  def lint_json(_src), do: nif_error()

  defp nif_error, do: :erlang.nif_error(:nif_not_loaded)
end
