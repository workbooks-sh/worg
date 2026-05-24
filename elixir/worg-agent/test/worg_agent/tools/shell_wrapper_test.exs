defmodule WorgAgent.Tools.ShellWrapperTest do
  use ExUnit.Case, async: true

  alias WorgAgent.Tools.ShellWrapper

  describe "build_args/2" do
    test "positional arg appends value as plain argv element" do
      assert ShellWrapper.build_args([{"path", :positional}], %{"path" => "foo.html"}) ==
               ["foo.html"]
    end

    test "positional arg missing → skipped" do
      assert ShellWrapper.build_args([{"path", :positional}], %{}) == []
    end

    test "positional_list arg expands a list into multiple positionals" do
      assert ShellWrapper.build_args(
               [{"refs", :positional_list}],
               %{"refs" => ["a.jpg", "b.jpg", "c.jpg"]}
             ) == ["a.jpg", "b.jpg", "c.jpg"]
    end

    test "positional_list with scalar value wraps to one element" do
      assert ShellWrapper.build_args(
               [{"refs", :positional_list}],
               %{"refs" => "a.jpg"}
             ) == ["a.jpg"]
    end

    test "string flag emits --flag value pair" do
      assert ShellWrapper.build_args(
               [{"platform", "--platform"}],
               %{"platform" => "instagram_reels"}
             ) == ["--platform", "instagram_reels"]
    end

    test "boolean flag emits the flag only when true" do
      assert ShellWrapper.build_args(
               [{"pretty", {"--pretty", :boolean}}],
               %{"pretty" => true}
             ) == ["--pretty"]

      assert ShellWrapper.build_args(
               [{"pretty", {"--pretty", :boolean}}],
               %{"pretty" => false}
             ) == []

      assert ShellWrapper.build_args(
               [{"pretty", {"--pretty", :boolean}}],
               %{}
             ) == []
    end

    test "repeat flag emits --flag value per list element" do
      assert ShellWrapper.build_args(
               [{"reference", {"--reference", :repeat}}],
               %{"reference" => ["a.jpg", "b.jpg"]}
             ) == ["--reference", "a.jpg", "--reference", "b.jpg"]
    end

    test "repeat flag with scalar wraps to single emission" do
      assert ShellWrapper.build_args(
               [{"reference", {"--reference", :repeat}}],
               %{"reference" => "a.jpg"}
             ) == ["--reference", "a.jpg"]
    end

    test "absent flag entries are skipped (no defaults)" do
      assert ShellWrapper.build_args(
               [
                 {"path", :positional},
                 {"platform", "--platform"},
                 {"mp4", "--mp4"}
               ],
               %{"path" => "x.html"}
             ) == ["x.html"]
    end

    test "ordering of arg_map is preserved in argv" do
      assert ShellWrapper.build_args(
               [
                 {"a", "--a"},
                 {"b", "--b"},
                 {"c", "--c"}
               ],
               %{"c" => "3", "a" => "1", "b" => "2"}
             ) == ["--a", "1", "--b", "2", "--c", "3"]
    end

    test "numbers stringify via to_string/1" do
      assert ShellWrapper.build_args(
               [{"duration", "--duration"}],
               %{"duration" => 18.5}
             ) == ["--duration", "18.5"]

      assert ShellWrapper.build_args(
               [{"width", "--width"}],
               %{"width" => 1080}
             ) == ["--width", "1080"]
    end
  end

  describe "run/6 — System.cmd integration via /bin/echo" do
    @argv_prefix ["just-echo-this:"]

    test "successful command returns exit=0 marker with stdout" do
      assert {:ok, output} =
               ShellWrapper.run(
                 "echo",
                 @argv_prefix,
                 [{"value", :positional}],
                 [],
                 %{"value" => "hello"},
                 %{working_dir: System.tmp_dir!()}
               )

      assert output =~ "exit=0"
      assert output =~ "just-echo-this: hello"
    end

    test "nonzero exit code is captured in the marker line" do
      assert {:ok, output} =
               ShellWrapper.run(
                 "sh",
                 ["-c"],
                 [{"cmd", :positional}],
                 [],
                 %{"cmd" => "exit 7"},
                 %{working_dir: System.tmp_dir!()}
               )

      assert output =~ "exit=7"
    end

    test "missing binary returns {:error, _}" do
      assert {:error, msg} =
               ShellWrapper.run(
                 "nonexistent-binary-zzzzz",
                 [],
                 [],
                 [],
                 %{},
                 %{working_dir: System.tmp_dir!()}
               )

      assert msg =~ "not found" or match?({:cmd_failed, _}, msg)
    end

    test "env injection is forwarded to System.cmd" do
      # Use `sh -c` so we can interpolate the env var into the output.
      assert {:ok, output} =
               ShellWrapper.run(
                 "sh",
                 ["-c"],
                 [{"cmd", :positional}],
                 [{"TEST_VAR_X", "smoke"}],
                 %{"cmd" => "echo $TEST_VAR_X"},
                 %{working_dir: System.tmp_dir!()}
               )

      assert output =~ "smoke"
    end

    test "env entry with {:env, NAME} resolves at call time" do
      System.put_env("WA_SHELL_WRAPPER_TEST_ENV", "from-process")

      try do
        assert {:ok, output} =
                 ShellWrapper.run(
                   "sh",
                   ["-c"],
                   [{"cmd", :positional}],
                   [{"FORWARDED", {:env, "WA_SHELL_WRAPPER_TEST_ENV"}}],
                   %{"cmd" => "echo $FORWARDED"},
                   %{working_dir: System.tmp_dir!()}
                 )

        assert output =~ "from-process"
      after
        System.delete_env("WA_SHELL_WRAPPER_TEST_ENV")
      end
    end

    test "env entry resolving to empty is dropped (not injected as empty)" do
      System.delete_env("WA_SHELL_WRAPPER_TEST_UNSET")

      assert {:ok, output} =
               ShellWrapper.run(
                 "sh",
                 ["-c"],
                 [{"cmd", :positional}],
                 [{"DROPPED", {:env, "WA_SHELL_WRAPPER_TEST_UNSET"}}],
                 %{"cmd" => "echo ${DROPPED:-empty}"},
                 %{working_dir: System.tmp_dir!()}
               )

      assert output =~ "empty"
    end
  end

  describe "concrete wrappers compile + carry the right shape" do
    test "WaveletLint declares the expected Tool behaviour surface" do
      assert WorgAgent.Tools.WaveletLint.name() == "wavelet_lint"
      assert is_binary(WorgAgent.Tools.WaveletLint.description())
      schema = WorgAgent.Tools.WaveletLint.input_schema()
      assert schema["properties"]["path"]["type"] == "string"
      assert "path" in schema["required"]
    end

    test "WaveletRender declares the expected Tool behaviour surface" do
      assert WorgAgent.Tools.WaveletRender.name() == "wavelet_render"
      schema = WorgAgent.Tools.WaveletRender.input_schema()
      assert schema["properties"]["comp"]["type"] == "string"
      assert schema["properties"]["no_audio"]["type"] == "boolean"
    end

    test "WaveletScreenplayValidate declares the expected Tool behaviour surface" do
      assert WorgAgent.Tools.WaveletScreenplayValidate.name() == "wavelet_screenplay_validate"
      schema = WorgAgent.Tools.WaveletScreenplayValidate.input_schema()
      assert schema["properties"]["duration"]["type"] == "number"
      assert Enum.sort(schema["required"]) == ["duration", "path"]
    end

    test "WaveletCharacterDefine declares the expected Tool behaviour surface" do
      assert WorgAgent.Tools.WaveletCharacterDefine.name() == "wavelet_character_define"
      schema = WorgAgent.Tools.WaveletCharacterDefine.input_schema()
      assert schema["properties"]["reference"]["type"] == "array"
      assert schema["properties"]["character_type"]["enum"] ==
               ["full-body", "hands", "product-hands"]
    end

    test "BrandworkBrief declares the expected Tool behaviour surface" do
      assert WorgAgent.Tools.BrandworkBrief.name() == "brandwork_brief"
      schema = WorgAgent.Tools.BrandworkBrief.input_schema()
      assert "domain" in schema["required"]
    end

    test "BrandworkResolve declares the expected Tool behaviour surface" do
      assert WorgAgent.Tools.BrandworkResolve.name() == "brandwork_resolve"
      schema = WorgAgent.Tools.BrandworkResolve.input_schema()
      assert "query" in schema["required"]
    end

    test "all six wrappers expose Tool callbacks" do
      wrappers = [
        WorgAgent.Tools.WaveletLint,
        WorgAgent.Tools.WaveletRender,
        WorgAgent.Tools.WaveletScreenplayValidate,
        WorgAgent.Tools.WaveletCharacterDefine,
        WorgAgent.Tools.BrandworkBrief,
        WorgAgent.Tools.BrandworkResolve
      ]

      Enum.each(wrappers, fn mod ->
        assert is_binary(mod.name())
        assert is_binary(mod.description())
        assert is_map(mod.input_schema())
        # execute/2 callback present (we don't actually invoke it —
        # would shell out to a binary that may not be on test PATH)
        assert function_exported?(mod, :execute, 2)
      end)
    end
  end
end
