defmodule Worg.AtomicFile do
  @moduledoc """
  Atomic write-back for `.org` document strings.

  Worg mutators return a new document *string*. Callers persist it however
  they want — local disk, R2, in-memory. For local disk, this helper writes
  `<path>.tmp` and then renames into place, surviving crashes mid-write.

  For R2 (Studio): callers use the broker's R2 client + Ecto.Multi to commit
  the .org write and derived rows in one transaction. Worg has no R2 / Ecto
  dep — this module covers the local-disk case used by the workbook CLI and
  the standalone `worg` binary.
  """

  @doc """
  Write `contents` to `path` atomically: writes `<path>.tmp`, then renames.
  Returns `:ok` or `{:error, reason}`.
  """
  @spec write(Path.t(), String.t()) :: :ok | {:error, term}
  def write(path, contents) do
    tmp = path <> ".tmp"
    with :ok <- File.write(tmp, contents),
         :ok <- File.rename(tmp, path) do
      :ok
    end
  end
end
