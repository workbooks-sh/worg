defmodule Worg.SmokeTest do
  @moduledoc """
  wb-4vhr.24 — end-to-end integration smoke for the Elixir library
  surface. Exercises the full read → mutate → execute → write-back →
  reparse loop a real consumer would run.

  Steps:
    1. Author a small fixture document (round-trip-clean).
    2. Worg.parse/1 — verify the parse handle.
    3. Worg.transition_todo/3 — TODO → DOING.
    4. Worg.Exec.shell/2 on an embedded block — capture stdout.
    5. Worg.write_results/3 — embed the captured output as #+RESULTS:.
    6. Worg.AtomicFile.write/2 — persist to a temp path.
    7. Re-parse the on-disk file — assert it equals the in-memory
       post-execution document byte-for-byte.

  The Node-WASM half of the bd ticket (packages/worg/bindings/node/)
  is intentionally out of scope here — that needs a separate JS test
  harness + wasm-bindgen target builds.
  """

  use ExUnit.Case, async: true

  alias Worg.AtomicFile

  test "parse → transition → exec → write_results → atomic write → reparse" do
    fixture = """
    * TODO Compute the answer
    :PROPERTIES:
    :ID: compute
    :END:

    #+begin_src bash
    echo 42
    #+end_src
    """

    # 1 + 2: parse
    assert {:ok, src1} = Worg.parse(fixture)
    assert src1 == fixture

    # 3: transition
    assert {:ok, src2} = Worg.transition_todo(src1, "compute", "DOING")
    assert src2 =~ "* DOING Compute the answer"
    refute src2 =~ "* TODO Compute"

    # 4: execute the embedded bash block. The block body is just
    # `echo 42` — Worg.Exec.shell/2 takes a %{body: ...} map; we
    # construct it inline (a real consumer would pull source blocks
    # via Worg.Parser.source_blocks_json/2, but the surface here is
    # the executor, not the discovery — and discovery is covered
    # under the worg-cli `result` subcommand path already).
    assert {:ok, output} = Worg.Exec.shell(%{body: "echo 42"})
    assert String.trim(output) == "42"

    # 5: write_results — embed the captured output under the
    # transitioned headline's first source block.
    assert {:ok, src3} = Worg.write_results(src2, "compute", String.trim(output))
    assert src3 =~ "#+RESULTS:"
    assert src3 =~ "42"

    # 6: atomic write to disk.
    tmp =
      Path.join(
        System.tmp_dir!(),
        "worg-smoke-#{:erlang.unique_integer([:positive])}.org"
      )

    try do
      assert :ok = AtomicFile.write(tmp, src3)
      assert File.exists?(tmp)

      # 7: re-parse from disk; the on-disk text must equal the
      # in-memory post-execution document byte-for-byte (the parser is
      # round-trip stable + AtomicFile is just bytes-on-disk).
      from_disk = File.read!(tmp)
      assert from_disk == src3, "on-disk text drifted from in-memory"

      # And re-parsing it through Worg.parse must succeed (no drift
      # introduced by the write+read cycle).
      assert {:ok, ^src3} = Worg.parse(from_disk)
    after
      File.rm(tmp)
    end
  end

  test "tangle round-trip: write blocks to disk, re-read identical" do
    base = Path.join(System.tmp_dir!(), "worg-smoke-tangle-#{:erlang.unique_integer([:positive])}")
    File.mkdir_p!(base)

    try do
      src = """
      * Build outputs
      #+begin_src bash :tangle scripts/build.sh
      #!/usr/bin/env bash
      echo building
      #+end_src

      #+begin_src css :tangle assets/styles.css :mkdirp yes
      body { background: black; }
      #+end_src
      """

      File.mkdir_p!(Path.join(base, "scripts"))

      assert {:ok, [build_sh, styles_css]} = Worg.Tangle.tangle(src, base)
      assert String.ends_with?(build_sh, "scripts/build.sh")
      assert String.ends_with?(styles_css, "assets/styles.css")
      assert File.read!(build_sh) =~ "echo building"
      assert File.read!(styles_css) =~ "background: black"
    after
      File.rm_rf!(base)
    end
  end
end
