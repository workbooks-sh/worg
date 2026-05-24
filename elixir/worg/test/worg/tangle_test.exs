defmodule Worg.TangleTest do
  use ExUnit.Case, async: true

  alias Worg.Tangle

  setup do
    base = Path.join(System.tmp_dir!(), "worg-tangle-test-#{:erlang.unique_integer([:positive])}")
    File.mkdir_p!(base)
    on_exit(fn -> File.rm_rf!(base) end)
    {:ok, base: base}
  end

  describe "tangle/2" do
    test "writes a single :tangle block to the resolved path", %{base: base} do
      src = """
      * Title
      #+begin_src bash :tangle scripts/run.sh
      #!/usr/bin/env bash
      echo hello
      #+end_src
      """

      File.mkdir_p!(Path.join(base, "scripts"))
      assert {:ok, [written]} = Tangle.tangle(src, base)
      assert written == Path.expand("scripts/run.sh", base)
      assert File.read!(written) =~ "echo hello"
    end

    test "multiple :tangle blocks are written in source order", %{base: base} do
      src = """
      #+begin_src css :tangle one.css
      a { color: red; }
      #+end_src

      #+begin_src css :tangle two.css
      b { color: blue; }
      #+end_src
      """

      assert {:ok, [first, second]} = Tangle.tangle(src, base)
      assert String.ends_with?(first, "one.css")
      assert String.ends_with?(second, "two.css")
      assert File.read!(first) =~ "color: red"
      assert File.read!(second) =~ "color: blue"
    end

    test "no :tangle blocks → empty result", %{base: base} do
      src = """
      * Title
      #+begin_src bash
      echo not tangled
      #+end_src
      """

      assert {:ok, []} = Tangle.tangle(src, base)
    end

    test ":mkdirp yes creates parent dirs", %{base: base} do
      src = """
      #+begin_src css :tangle deep/nested/styles.css :mkdirp yes
      body { background: black; }
      #+end_src
      """

      assert {:ok, [written]} = Tangle.tangle(src, base)
      assert File.exists?(written)
      assert File.read!(written) =~ "background: black"
    end

    test "no :mkdirp + missing parent dir returns {:write_failed, ...}", %{base: base} do
      src = """
      #+begin_src css :tangle deep/missing/styles.css
      body { }
      #+end_src
      """

      assert {:error, {:write_failed, _path, _reason}} = Tangle.tangle(src, base)
    end

    test "case-insensitive on #+begin_src and #+end_src", %{base: base} do
      src = """
      #+BEGIN_SRC bash :tangle out.sh
      echo upper
      #+END_SRC
      """

      assert {:ok, [written]} = Tangle.tangle(src, base)
      assert File.read!(written) =~ "echo upper"
    end

    test "blocks WITHOUT :tangle are skipped even when others have it", %{base: base} do
      src = """
      #+begin_src bash
      echo not-tangled
      #+end_src

      #+begin_src bash :tangle tangled.sh
      echo yes-tangled
      #+end_src
      """

      assert {:ok, [written]} = Tangle.tangle(src, base)
      assert String.ends_with?(written, "tangled.sh")
      assert File.read!(written) =~ "yes-tangled"
      refute File.read!(written) =~ "not-tangled"
    end
  end
end
