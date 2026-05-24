defmodule WorgAgent.LlmTest do
  use ExUnit.Case, async: true

  alias WorgAgent.Llm
  alias WorgAgent.Llm.Response

  @messages [
    %{"role" => "system", "content" => "You are a test agent."},
    %{"role" => "user", "content" => "say hi"}
  ]

  @tools [
    %{
      "type" => "function",
      "function" => %{
        "name" => "bash",
        "description" => "run a command",
        "parameters" => %{"type" => "object", "properties" => %{"command" => %{"type" => "string"}}}
      }
    }
  ]

  @fake_response_body %{
    "choices" => [
      %{
        "message" => %{"role" => "assistant", "content" => "hi"},
        "finish_reason" => "stop"
      }
    ],
    "usage" => %{"prompt_tokens" => 12, "completion_tokens" => 1}
  }

  describe "call/3 happy path" do
    test "POSTs to the configured endpoint and parses the response" do
      plug = fn conn ->
        # Verify the request shape before responding.
        assert conn.method == "POST"
        assert conn.request_path == "/api/v1/chat/completions"
        assert {"authorization", "Bearer test-key"} in conn.req_headers
        assert {"content-type", "application/json"} in conn.req_headers

        {:ok, body, conn} = Plug.Conn.read_body(conn)
        decoded = Jason.decode!(body)
        assert decoded["model"] == "xiaomi/mimo-v2.5-pro"
        assert decoded["messages"] == @messages
        assert is_list(decoded["tools"])

        conn
        |> Plug.Conn.put_resp_content_type("application/json")
        |> Plug.Conn.resp(200, Jason.encode!(@fake_response_body))
      end

      assert {:ok, %Response{content: "hi", stop_reason: :end_turn}} =
               Llm.call(@messages, @tools,
                 api_key: "test-key",
                 endpoint: "https://openrouter.test/api/v1/chat/completions",
                 req_options: [plug: plug]
               )
    end

    test "omits the tools field when the catalog is empty (some providers reject [])" do
      plug = fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        decoded = Jason.decode!(body)
        refute Map.has_key?(decoded, "tools")

        conn
        |> Plug.Conn.put_resp_content_type("application/json")
        |> Plug.Conn.resp(200, Jason.encode!(@fake_response_body))
      end

      assert {:ok, _} =
               Llm.call(@messages, [],
                 api_key: "k",
                 endpoint: "https://openrouter.test/api/v1/chat/completions",
                 req_options: [plug: plug]
               )
    end

    test "honors :model override" do
      plug = fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        decoded = Jason.decode!(body)
        assert decoded["model"] == "anthropic/claude-3-5-sonnet"

        conn
        |> Plug.Conn.put_resp_content_type("application/json")
        |> Plug.Conn.resp(200, Jason.encode!(@fake_response_body))
      end

      assert {:ok, _} =
               Llm.call(@messages, @tools,
                 api_key: "k",
                 model: "anthropic/claude-3-5-sonnet",
                 endpoint: "https://openrouter.test/api/v1/chat/completions",
                 req_options: [plug: plug]
               )
    end
  end

  describe "prompt caching" do
    test "without :cache, system message is plain string + tools have no cache_control" do
      plug = fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        decoded = Jason.decode!(body)
        [sys | _] = decoded["messages"]
        assert sys["content"] == "You are a test agent."
        refute Enum.any?(decoded["tools"], &Map.has_key?(&1, "cache_control"))

        conn
        |> Plug.Conn.put_resp_content_type("application/json")
        |> Plug.Conn.resp(200, Jason.encode!(@fake_response_body))
      end

      assert {:ok, _} =
               Llm.call(@messages, @tools,
                 api_key: "k",
                 endpoint: "https://openrouter.test/api/v1/chat/completions",
                 req_options: [plug: plug]
               )
    end

    test "with :cache true, system message wraps content in cache_control block + last tool gets cache_control" do
      plug = fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        decoded = Jason.decode!(body)
        [sys | _] = decoded["messages"]

        assert is_list(sys["content"])
        [block] = sys["content"]
        assert block["type"] == "text"
        assert block["cache_control"] == %{"type" => "ephemeral"}

        last_tool = List.last(decoded["tools"])
        assert last_tool["cache_control"] == %{"type" => "ephemeral"}

        conn
        |> Plug.Conn.put_resp_content_type("application/json")
        |> Plug.Conn.resp(200, Jason.encode!(@fake_response_body))
      end

      assert {:ok, _} =
               Llm.call(@messages, @tools,
                 api_key: "k",
                 cache: true,
                 endpoint: "https://openrouter.test/api/v1/chat/completions",
                 req_options: [plug: plug]
               )
    end
  end

  describe "error paths" do
    test "missing api_key returns :missing_api_key without making a request" do
      # No plug — would crash if Req actually tried to call out.
      saved = System.get_env("OPENROUTER_API_KEY")
      System.delete_env("OPENROUTER_API_KEY")

      try do
        assert {:error, :missing_api_key} = Llm.call(@messages, @tools)
      after
        if saved, do: System.put_env("OPENROUTER_API_KEY", saved)
      end
    end

    test "non-2xx response returns {:error, {:http, status, body}}" do
      plug = fn conn ->
        conn
        |> Plug.Conn.put_resp_content_type("application/json")
        |> Plug.Conn.resp(429, Jason.encode!(%{"error" => "rate limit"}))
      end

      assert {:error, {:http, 429, %{"error" => "rate limit"}}} =
               Llm.call(@messages, @tools,
                 api_key: "k",
                 endpoint: "https://openrouter.test/api/v1/chat/completions",
                 req_options: [plug: plug]
               )
    end

    test "transport error surfaces as {:error, {:transport, _}}" do
      plug = fn _conn -> raise RuntimeError, "network down" end

      assert {:error, {:transport, _}} =
               Llm.call(@messages, @tools,
                 api_key: "k",
                 endpoint: "https://openrouter.test/api/v1/chat/completions",
                 # disable retries so the test is fast
                 req_options: [plug: plug, retry: false]
               )
    end

    test "unparseable response body returns the from_wire error" do
      plug = fn conn ->
        conn
        |> Plug.Conn.put_resp_content_type("application/json")
        |> Plug.Conn.resp(200, Jason.encode!(%{"choices" => []}))
      end

      assert {:error, {:no_choices, _}} =
               Llm.call(@messages, @tools,
                 api_key: "k",
                 endpoint: "https://openrouter.test/api/v1/chat/completions",
                 req_options: [plug: plug]
               )
    end
  end

  describe "call_with_default_tools/2" do
    test "wraps ToolRegistry.catalog into the OpenAI tool descriptor shape" do
      plug = fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        decoded = Jason.decode!(body)

        bash_tool = Enum.find(decoded["tools"], &(&1["function"]["name"] == "bash"))
        assert bash_tool["type"] == "function"
        assert is_binary(bash_tool["function"]["description"])
        assert bash_tool["function"]["parameters"]["type"] == "object"

        conn
        |> Plug.Conn.put_resp_content_type("application/json")
        |> Plug.Conn.resp(200, Jason.encode!(@fake_response_body))
      end

      assert {:ok, _} =
               Llm.call_with_default_tools(@messages,
                 api_key: "k",
                 endpoint: "https://openrouter.test/api/v1/chat/completions",
                 req_options: [plug: plug]
               )
    end
  end

  describe "image-bearing tool results (wb-t274)" do
    @sample_b64 "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII="

    test "tool message with string content passes through unchanged" do
      messages = [
        %{"role" => "user", "content" => "look at this"},
        %{"role" => "assistant", "content" => nil, "tool_calls" => [%{"id" => "c1", "function" => %{"name" => "x"}}]},
        %{"role" => "tool", "tool_call_id" => "c1", "content" => "plain text result"}
      ]

      plug = fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        decoded = Jason.decode!(body)
        # Three messages, unchanged shape
        assert length(decoded["messages"]) == 3
        tool_msg = Enum.find(decoded["messages"], &(&1["role"] == "tool"))
        assert tool_msg["content"] == "plain text result"

        conn |> Plug.Conn.put_resp_content_type("application/json") |> Plug.Conn.resp(200, Jason.encode!(@fake_response_body))
      end

      assert {:ok, _} =
               Llm.call(messages, [],
                 api_key: "k",
                 endpoint: "https://openrouter.test/api/v1/chat/completions",
                 req_options: [plug: plug]
               )
    end

    test "tool message with list-of-text content collapses to a string" do
      messages = [
        %{"role" => "tool", "tool_call_id" => "c1",
          "content" => [
            %{"type" => "text", "text" => "verdict: ok"},
            %{"type" => "text", "text" => "score: 0.91"}
          ]}
      ]

      plug = fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        decoded = Jason.decode!(body)
        assert length(decoded["messages"]) == 1
        tool_msg = Enum.at(decoded["messages"], 0)
        assert tool_msg["role"] == "tool"
        assert tool_msg["content"] == "verdict: ok\nscore: 0.91"

        conn |> Plug.Conn.put_resp_content_type("application/json") |> Plug.Conn.resp(200, Jason.encode!(@fake_response_body))
      end

      assert {:ok, _} =
               Llm.call(messages, [],
                 api_key: "k",
                 endpoint: "https://openrouter.test/api/v1/chat/completions",
                 req_options: [plug: plug]
               )
    end

    test "anthropic-shaped image (base64) splits into text-only tool msg + user msg with image_url data URI" do
      messages = [
        %{"role" => "user", "content" => "judge this frame"},
        %{"role" => "tool", "tool_call_id" => "frame_judge_1",
          "content" => [
            %{"type" => "text", "text" => "verdict: pass"},
            %{"type" => "image", "source" => %{
              "type" => "base64",
              "media_type" => "image/png",
              "data" => @sample_b64
            }}
          ]}
      ]

      plug = fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        decoded = Jason.decode!(body)
        # Original 2 messages + 1 synthetic user message carrying the image
        assert length(decoded["messages"]) == 3

        [_user, tool_msg, image_user] = decoded["messages"]
        assert tool_msg["role"] == "tool"
        assert tool_msg["content"] == "verdict: pass"

        assert image_user["role"] == "user"
        [label, image_part] = image_user["content"]
        assert label["type"] == "text"
        assert String.contains?(label["text"], "frame_judge_1")
        assert image_part["type"] == "image_url"
        assert image_part["image_url"]["url"] == "data:image/png;base64,#{@sample_b64}"

        conn |> Plug.Conn.put_resp_content_type("application/json") |> Plug.Conn.resp(200, Jason.encode!(@fake_response_body))
      end

      assert {:ok, _} =
               Llm.call(messages, [],
                 api_key: "k",
                 endpoint: "https://openrouter.test/api/v1/chat/completions",
                 req_options: [plug: plug]
               )
    end

    test "anthropic-shaped image (url source) keeps the URL on the image_url part" do
      messages = [
        %{"role" => "tool", "tool_call_id" => "c1",
          "content" => [
            %{"type" => "image", "source" => %{
              "type" => "url",
              "url" => "https://example.test/frame.jpg"
            }}
          ]}
      ]

      plug = fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        decoded = Jason.decode!(body)
        assert length(decoded["messages"]) == 2

        tool_msg = Enum.find(decoded["messages"], &(&1["role"] == "tool"))
        assert tool_msg["content"] == "[image content attached in next message]"

        image_user = Enum.find(decoded["messages"], &(&1["role"] == "user"))
        image_part = Enum.find(image_user["content"], &(&1["type"] == "image_url"))
        assert image_part["image_url"]["url"] == "https://example.test/frame.jpg"

        conn |> Plug.Conn.put_resp_content_type("application/json") |> Plug.Conn.resp(200, Jason.encode!(@fake_response_body))
      end

      assert {:ok, _} =
               Llm.call(messages, [],
                 api_key: "k",
                 endpoint: "https://openrouter.test/api/v1/chat/completions",
                 req_options: [plug: plug]
               )
    end

    test "already-OpenAI image_url block passes through unchanged" do
      messages = [
        %{"role" => "tool", "tool_call_id" => "c1",
          "content" => [
            %{"type" => "text", "text" => "result"},
            %{"type" => "image_url", "image_url" => %{"url" => "https://example.test/x.png"}}
          ]}
      ]

      plug = fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        decoded = Jason.decode!(body)
        assert length(decoded["messages"]) == 2

        image_user = Enum.find(decoded["messages"], &(&1["role"] == "user"))
        image_part = Enum.find(image_user["content"], &(&1["type"] == "image_url"))
        assert image_part["image_url"]["url"] == "https://example.test/x.png"

        conn |> Plug.Conn.put_resp_content_type("application/json") |> Plug.Conn.resp(200, Jason.encode!(@fake_response_body))
      end

      assert {:ok, _} =
               Llm.call(messages, [],
                 api_key: "k",
                 endpoint: "https://openrouter.test/api/v1/chat/completions",
                 req_options: [plug: plug]
               )
    end

    test "multiple images in one tool result all surface in the synthetic user message" do
      messages = [
        %{"role" => "tool", "tool_call_id" => "c1",
          "content" => [
            %{"type" => "text", "text" => "two frames sampled"},
            %{"type" => "image", "source" => %{
              "type" => "base64", "media_type" => "image/jpeg", "data" => @sample_b64
            }},
            %{"type" => "image", "source" => %{
              "type" => "base64", "media_type" => "image/jpeg", "data" => @sample_b64
            }}
          ]}
      ]

      plug = fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        decoded = Jason.decode!(body)
        image_user = Enum.find(decoded["messages"], &(&1["role"] == "user"))
        image_parts = Enum.filter(image_user["content"], &(&1["type"] == "image_url"))
        assert length(image_parts) == 2

        conn |> Plug.Conn.put_resp_content_type("application/json") |> Plug.Conn.resp(200, Jason.encode!(@fake_response_body))
      end

      assert {:ok, _} =
               Llm.call(messages, [],
                 api_key: "k",
                 endpoint: "https://openrouter.test/api/v1/chat/completions",
                 req_options: [plug: plug]
               )
    end

    test "non-tool messages with list content are not split" do
      # User messages already carrying images should NOT trigger the
      # split — only tool messages do, because OpenAI-compat tool
      # messages can't carry images.
      messages = [
        %{"role" => "user", "content" => [
          %{"type" => "text", "text" => "look"},
          %{"type" => "image_url", "image_url" => %{"url" => "https://example.test/u.png"}}
        ]}
      ]

      plug = fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        decoded = Jason.decode!(body)
        # Single message, list content preserved
        assert length(decoded["messages"]) == 1
        msg = Enum.at(decoded["messages"], 0)
        assert msg["role"] == "user"
        assert is_list(msg["content"])
        assert length(msg["content"]) == 2

        conn |> Plug.Conn.put_resp_content_type("application/json") |> Plug.Conn.resp(200, Jason.encode!(@fake_response_body))
      end

      assert {:ok, _} =
               Llm.call(messages, [],
                 api_key: "k",
                 endpoint: "https://openrouter.test/api/v1/chat/completions",
                 req_options: [plug: plug]
               )
    end
  end
end
