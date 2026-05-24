defmodule WorgAgent.Tools.VideoJudgeTest do
  use ExUnit.Case, async: false

  alias WorgAgent.Tools.VideoJudge

  setup do
    case System.find_executable("ffmpeg") do
      nil ->
        {:skip, "ffmpeg not on PATH; skipping video_judge integration tests"}

      _ ->
        tmp = Path.join(System.tmp_dir!(), "wa_vj_test_#{:rand.uniform(1_000_000)}")
        File.mkdir_p!(tmp)
        mp4 = Path.join(tmp, "fixture.mp4")

        # 6-second clip so the 5%-trim leaves a non-degenerate sampling window.
        {_out, 0} =
          System.cmd("ffmpeg", [
            "-y",
            "-f", "lavfi",
            "-i", "testsrc=duration=6:size=320x240:rate=4",
            "-pix_fmt", "yuv420p",
            mp4
          ], stderr_to_stdout: true)

        on_exit(fn -> File.rm_rf!(tmp) end)
        {:ok, mp4: mp4, tmp: tmp}
    end
  end

  describe "behaviour metadata" do
    test "name + description + input_schema all defined" do
      assert VideoJudge.name() == "video_judge"
      assert is_binary(VideoJudge.description())

      schema = VideoJudge.input_schema()
      assert schema["type"] == "object"
      assert schema["properties"]["mp4_path"]["type"] == "string"
      assert "mp4_path" in schema["required"]
      # prompt is optional — uses default 8-dim rubric when omitted
      refute "prompt" in schema["required"]
    end
  end

  describe "execute/2 happy path" do
    test "samples 4 frames, asks VLM, returns scorecard + image blocks", %{mp4: mp4} do
      plug = fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        decoded = Jason.decode!(body)

        [msg] = decoded["messages"]
        assert msg["role"] == "user"

        # 4 sampled frames + 1 text block
        content = msg["content"]
        image_blocks = Enum.filter(content, &(&1["type"] == "image_url"))
        assert length(image_blocks) == 4

        # Default rubric prompt mentions key dimensions
        text_block = Enum.find(content, &(&1["type"] == "text"))
        assert text_block["text"] =~ "character_consistency"
        assert text_block["text"] =~ "product_visible"

        conn
        |> Plug.Conn.put_resp_content_type("application/json")
        |> Plug.Conn.resp(200, Jason.encode!(%{
          "choices" => [
            %{
              "message" => %{
                "role" => "assistant",
                "content" => ~s({"overall": "PASS", "dimensions": {}})
              },
              "finish_reason" => "stop"
            }
          ],
          "usage" => %{}
        }))
      end

      Application.put_env(:worg_agent, :video_judge_req_options, [plug: plug])
      Application.put_env(:worg_agent, :video_judge_api_key, "test-key")
      on_exit(fn ->
        Application.delete_env(:worg_agent, :video_judge_req_options)
        Application.delete_env(:worg_agent, :video_judge_api_key)
      end)

      assert {:ok, blocks} = VideoJudge.execute(%{"mp4_path" => mp4}, %{})

      # 1 verdict + 4 frames
      assert length(blocks) == 5
      [verdict | image_blocks] = blocks
      assert verdict["type"] == "text"
      assert verdict["text"] =~ "PASS"
      assert length(image_blocks) == 4
      Enum.each(image_blocks, fn block ->
        assert block["type"] == "image"
      end)
    end

    test "custom prompt overrides the default rubric", %{mp4: mp4} do
      plug = fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        decoded = Jason.decode!(body)
        [msg] = decoded["messages"]
        text_block = Enum.find(msg["content"], &(&1["type"] == "text"))
        # Custom prompt visible, default-rubric vocab absent
        assert text_block["text"] == "Custom rubric please"
        refute text_block["text"] =~ "character_consistency"

        conn
        |> Plug.Conn.put_resp_content_type("application/json")
        |> Plug.Conn.resp(200, Jason.encode!(%{
          "choices" => [%{"message" => %{"role" => "assistant", "content" => "ok"}, "finish_reason" => "stop"}],
          "usage" => %{}
        }))
      end

      Application.put_env(:worg_agent, :video_judge_req_options, [plug: plug])
      Application.put_env(:worg_agent, :video_judge_api_key, "test-key")
      on_exit(fn ->
        Application.delete_env(:worg_agent, :video_judge_req_options)
        Application.delete_env(:worg_agent, :video_judge_api_key)
      end)

      assert {:ok, _} =
               VideoJudge.execute(%{"mp4_path" => mp4, "prompt" => "Custom rubric please"}, %{})
    end
  end

  describe "execute/2 input validation" do
    test "rejects missing mp4_path" do
      assert {:error, _} = VideoJudge.execute(%{"prompt" => "x"}, %{})
    end

    test "rejects nonexistent mp4_path" do
      assert {:error, msg} = VideoJudge.execute(%{"mp4_path" => "/nonexistent/x.mp4"}, %{})
      assert msg =~ "not found"
    end
  end
end
