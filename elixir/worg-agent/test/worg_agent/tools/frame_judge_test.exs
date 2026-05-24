defmodule WorgAgent.Tools.FrameJudgeTest do
  use ExUnit.Case, async: false

  alias WorgAgent.Tools.FrameJudge

  # The frame_judge / video_judge tools shell out to ffmpeg, so we
  # need a real MP4 fixture. We synthesize one in setup using
  # ffmpeg's testsrc generator. If ffmpeg isn't on PATH the
  # synthesis fails and we skip — these tests are integration-shaped.
  setup do
    case System.find_executable("ffmpeg") do
      nil ->
        {:skip, "ffmpeg not on PATH; skipping frame_judge integration tests"}

      _ ->
        tmp = Path.join(System.tmp_dir!(), "wa_fj_test_#{:rand.uniform(1_000_000)}")
        File.mkdir_p!(tmp)
        mp4 = Path.join(tmp, "fixture.mp4")

        # 4-second 320x240 testsrc clip. Cheap to generate.
        {_out, 0} =
          System.cmd("ffmpeg", [
            "-y",
            "-f", "lavfi",
            "-i", "testsrc=duration=4:size=320x240:rate=4",
            "-pix_fmt", "yuv420p",
            mp4
          ], stderr_to_stdout: true)

        on_exit(fn -> File.rm_rf!(tmp) end)
        {:ok, mp4: mp4, tmp: tmp}
    end
  end

  describe "behaviour metadata" do
    test "name + description + input_schema all defined" do
      assert FrameJudge.name() == "frame_judge"
      assert is_binary(FrameJudge.description())
      assert String.length(FrameJudge.description()) > 50

      schema = FrameJudge.input_schema()
      assert schema["type"] == "object"
      assert schema["properties"]["mp4_path"]["type"] == "string"
      assert schema["properties"]["timestamps_sec"]["type"] == "array"
      assert schema["properties"]["prompt"]["type"] == "string"
      assert Enum.sort(schema["required"]) == ["mp4_path", "prompt", "timestamps_sec"]
    end
  end

  describe "execute/2 happy path" do
    test "extracts frames, calls VLM, returns verdict text + image blocks", %{mp4: mp4} do
      # Stub the VLM endpoint to return a canned verdict.
      plug = fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        decoded = Jason.decode!(body)

        # Verify the request shape: one user message, content list,
        # text + image_url blocks.
        [msg] = decoded["messages"]
        assert msg["role"] == "user"
        assert is_list(msg["content"])

        [text_block | image_blocks] = msg["content"]
        assert text_block["type"] == "text"
        assert text_block["text"] =~ "Score these"

        # Two timestamps requested → two image_url blocks
        assert length(image_blocks) == 2
        Enum.each(image_blocks, fn block ->
          assert block["type"] == "image_url"
          assert String.starts_with?(block["image_url"]["url"], "data:image/jpeg;base64,")
        end)

        conn
        |> Plug.Conn.put_resp_content_type("application/json")
        |> Plug.Conn.resp(200, Jason.encode!(%{
          "choices" => [
            %{
              "message" => %{"role" => "assistant", "content" => "frame 1: pass\nframe 2: pass"},
              "finish_reason" => "stop"
            }
          ],
          "usage" => %{"prompt_tokens" => 100, "completion_tokens" => 10}
        }))
      end

      Application.put_env(:worg_agent, :video_judge_req_options, [plug: plug])
      Application.put_env(:worg_agent, :video_judge_api_key, "test-key")
      on_exit(fn ->
        Application.delete_env(:worg_agent, :video_judge_req_options)
        Application.delete_env(:worg_agent, :video_judge_api_key)
      end)

      args = %{
        "mp4_path" => mp4,
        "timestamps_sec" => [1.0, 2.0],
        "prompt" => "Score these test frames"
      }

      assert {:ok, blocks} = FrameJudge.execute(args, %{})
      assert is_list(blocks)
      assert length(blocks) == 3

      [verdict | image_blocks] = blocks
      assert verdict["type"] == "text"
      assert verdict["text"] =~ "frame 1: pass"

      assert length(image_blocks) == 2
      Enum.each(image_blocks, fn block ->
        assert block["type"] == "image"
        assert block["source"]["type"] == "base64"
        assert block["source"]["media_type"] == "image/jpeg"
        assert byte_size(block["source"]["data"]) > 100
      end)
    end
  end

  describe "execute/2 input validation" do
    test "rejects missing mp4_path" do
      assert {:error, _} = FrameJudge.execute(%{"timestamps_sec" => [1.0], "prompt" => "x"}, %{})
    end

    test "rejects nonexistent mp4_path" do
      assert {:error, msg} = FrameJudge.execute(
        %{"mp4_path" => "/nonexistent/x.mp4", "timestamps_sec" => [1.0], "prompt" => "x"},
        %{}
      )
      assert msg =~ "not found"
    end

    test "rejects empty timestamps list", %{mp4: mp4} do
      assert {:error, msg} =
               FrameJudge.execute(%{"mp4_path" => mp4, "timestamps_sec" => [], "prompt" => "x"}, %{})
      assert msg =~ "at least one"
    end

    test "rejects non-number in timestamps", %{mp4: mp4} do
      assert {:error, msg} =
               FrameJudge.execute(
                 %{"mp4_path" => mp4, "timestamps_sec" => [1.0, "two"], "prompt" => "x"},
                 %{}
               )
      assert msg =~ "non-number"
    end
  end

end
