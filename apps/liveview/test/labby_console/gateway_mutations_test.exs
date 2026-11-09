defmodule LabbyConsole.GatewayMutationsTest do
  use ExUnit.Case, async: true
  alias LabbyConsole.GatewayMutations, as: Mutations

  defp draft(params), do: Mutations.change(Mutations.new(), params)

  defp view(config \\ %{}) do
    %{
      "config" =>
        Map.merge(
          %{
            "name" => "saved",
            "url" => "https://saved.example/mcp",
            "command" => nil,
            "args" => [],
            "enabled" => true,
            "proxy_resources" => true,
            "proxy_prompts" => true,
            "proxy_skills" => false,
            "oauth_enabled" => false
          },
          config
        ),
      "runtime" => %{"name" => "saved", "connected" => false}
    }
  end

  test "add preserves argv boundaries, enabled false and explicit proxy defaults" do
    state =
      draft(%{
        "name" => "My.server_1",
        "transport" => "stdio",
        "command" => "node",
        "args" => Jason.encode!(["a b", "", "\"quoted\""]),
        "enabled" => "false"
      })

    assert {:ok, "gateway.add", %{"spec" => spec}} = Mutations.build_request(state, :save)
    assert spec["args"] == ["a b", "", "\"quoted\""]
    assert spec["enabled"] == false
    assert spec["proxy_skills"] == false
    refute Map.has_key?(spec, "url")
  end

  test "strict get decoder binds exact name and rejects malformed config" do
    assert {:ok, _} = Mutations.draft_from_view(view(), "saved")
    assert {:error, _} = Mutations.draft_from_view(view(), "other")
    assert {:error, _} = Mutations.draft_from_view(view(%{"enabled" => "yes"}), "saved")
    assert {:error, _} = Mutations.draft_from_view(view(%{"args" => [1]}), "saved")
  end

  test "edit omits untouched and hidden fields, rename targets original name" do
    {:ok, state} =
      Mutations.draft_from_view(
        view(%{"env" => %{"SECRET" => "never-project"}, "socket_path" => "/private/socket"}),
        "saved"
      )

    refute inspect(state) =~ "never-project"
    state = Mutations.change(state, %{"name" => "new", "enabled" => "false"})

    assert {:ok, "gateway.update",
            %{"name" => "saved", "patch" => %{"name" => "new", "enabled" => false}}} =
             Mutations.build_request(state, :save)
  end

  test "private maps have explicit preserve replace clear intent" do
    {:ok, state} = Mutations.draft_from_view(view(), "saved")
    assert {:ok, _, %{"patch" => patch}} = Mutations.build_request(state, :save)
    refute Map.has_key?(patch, "env")

    state =
      Mutations.change(state, %{
        "env_intent" => "replace",
        "env" => ~S({"KEY":"a=b\nvalue"}),
        "headers_intent" => "clear",
        "headers_env_intent" => "replace",
        "headers_env" => ~s({"X-Key":"TOKEN_ENV"})
      })

    assert {:ok, _, %{"patch" => patch}} = Mutations.build_request(state, :save)
    assert patch["env"] == %{"KEY" => "a=b\nvalue"}
    assert patch["headers"] == %{}
    assert patch["headers_env"] == %{"X-Key" => "TOKEN_ENV"}
  end

  test "private malformed JSON errors never quote input" do
    state =
      draft(%{
        "name" => "test",
        "url" => "https://example.test/mcp",
        "env_intent" => "replace",
        "env" => ~s({"PRIVATE":"sensitive-sentinel",)
      })

    assert {:error, errors} = Mutations.build_request(state, :save)
    refute inspect(errors) =~ "sensitive-sentinel"
  end

  test "raw bearer is write only and never a draft test argument" do
    state =
      draft(%{
        "name" => "test",
        "url" => "https://example.test/mcp",
        "bearer_token_value" => "private-token"
      })

    assert {:ok, "gateway.add", %{"bearer_token_value" => "private-token"}} =
             Mutations.build_request(state, :save)

    assert {:error, _} = Mutations.build_request(state, :test)
    {:ok, saved} = Mutations.draft_from_view(view(%{"bearer_token_env" => "SAVED_ENV"}), "saved")

    saved =
      Mutations.change(saved, %{
        "url" => "https://changed.test/mcp",
        "bearer_token_value" => "private-token"
      })

    assert {:ok, "gateway.test", %{"name" => "saved"}} = Mutations.build_request(saved, :test)
  end

  test "edit transport switching is unavailable when explicit transport is unprojected" do
    {:ok, state} = Mutations.draft_from_view(view(), "saved")

    assert {:error, _} =
             state
             |> Mutations.change(%{"transport" => "stdio", "command" => "node"})
             |> Mutations.build_request(:save)
  end

  test "JSON supports one wrapper, bare map and single config with exact arrays" do
    for json <- [
          ~s({"mcpServers":{"one":{"command":"node","args":["a b",""]}}}),
          ~s({"one":{"command":"node","args":["a b",""]}}),
          ~s({"name":"one","command":"node","args":["a b",""]})
        ] do
      assert {:ok, state} = Mutations.apply_json(Mutations.new(), json)

      assert {:ok, "gateway.add", %{"spec" => %{"name" => "one", "args" => ["a b", ""]}}} =
               Mutations.build_request(state, :save)
    end
  end

  test "JSON rejects unknown meaningful fields, ambiguous servers and scalar private maps" do
    for json <- [
          ~s({"one":{"url":"https://example.test","unknown":"important"}}),
          ~s({"one":{},"two":{}}),
          ~s({"name":"one","command":"node","args":[1]}),
          ~s({"name":"one","command":"node","env":{"A":3}}),
          ~s({"name":"one","url":"https://example.test","oauth":{"enabled":true}})
        ] do
      assert {:error, errors} = Mutations.apply_json(Mutations.new(), json)
      refute inspect(errors) =~ "important"
    end
  end

  test "oversized private JSON and request are rejected before admission" do
    assert {:error, _} = Mutations.apply_json(Mutations.new(), String.duplicate(" ", 32_769))

    state =
      draft(%{
        "name" => "test",
        "url" => "https://example.test",
        "env_intent" => "replace",
        "env" => Jason.encode!(%{"KEY" => String.duplicate("x", 32_000)}),
        "headers_intent" => "replace",
        "headers" => Jason.encode!(%{"X-Key" => String.duplicate("y", 1000)})
      })

    assert {:error, _} = Mutations.build_request(state, :save)
  end

  test "URL credentials, Authorization headers and invalid environment references are rejected" do
    for params <- [
          %{"url" => "https://user:secret@example.test/mcp"},
          %{"headers_intent" => "replace", "headers" => ~s({"Authorization":"private"})},
          %{"headers_env_intent" => "replace", "headers_env" => ~s({"X-Key":"bad env"})}
        ] do
      state = draft(Map.merge(%{"name" => "test", "url" => "https://example.test/mcp"}, params))
      assert {:error, _} = Mutations.build_request(state, :save)
    end
  end

  test "oversized changed field cannot silently submit an earlier valid value" do
    state =
      draft(%{
        "name" => "test",
        "url" => "https://example.test",
        "env_intent" => "replace",
        "env" => "{}"
      })

    state = Mutations.change(state, %{"env" => String.duplicate("x", 32_769)})
    assert {:error, _} = Mutations.build_request(state, :save)
    state = Mutations.change(state, %{"env" => "{}"})
    assert {:ok, _, _} = Mutations.build_request(state, :save)
  end

  test "network JSON cannot discard nonempty command arguments" do
    assert {:error, _} =
             Mutations.apply_json(
               Mutations.new(),
               ~s({"name":"test","url":"https://example.test","args":["important"]})
             )
  end

  test "draft roots are removed by Phoenix log parameter filtering" do
    filtered =
      Phoenix.Logger.filter_values(%{
        "server" => %{
          "env" => "env-sentinel",
          "headers" => "header-sentinel",
          "bearer_token_value" => "bearer-sentinel"
        },
        "config" => %{"json" => "json-sentinel"},
        "credential" => "credential-sentinel"
      })

    for sentinel <-
          ~w(env-sentinel header-sentinel bearer-sentinel json-sentinel credential-sentinel),
        do: refute(inspect(filtered) =~ sentinel)
  end

  test "draft inspection excludes private transient values" do
    state =
      draft(%{
        "name" => "test",
        "bearer_token_value" => "inspection-sentinel",
        "env" => "inspection-sentinel"
      })

    refute inspect(state) =~ "inspection-sentinel"
  end

  test "Agent Skills use the backend origin grammar and reserved origin" do
    for name <- ~w(lab upstream--two) do
      assert {:ok, _, _} =
               draft(%{"name" => name, "url" => "https://example.test", "proxy_skills" => "true"})
               |> Mutations.build_request(:save)
    end

    assert {:error, _} =
             draft(%{
               "name" => "labby",
               "url" => "https://example.test",
               "proxy_skills" => "true"
             })
             |> Mutations.build_request(:save)
  end

  test "environment maps retain string keys supported by the backend without shell parsing" do
    state =
      draft(%{
        "name" => "test",
        "transport" => "stdio",
        "command" => "node",
        "env_intent" => "replace",
        "env" => ~s({"HTTP-Proxy":"value","EMPTY":""})
      })

    assert {:ok, _, %{"spec" => %{"env" => %{"HTTP-Proxy" => "value", "EMPTY" => ""}}}} =
             Mutations.build_request(state, :save)
  end

  test "readback accepts a bounded endpoint that fits the admitted request budget" do
    endpoint = "https://example.test/" <> String.duplicate("a", 5000)
    assert {:ok, state} = Mutations.draft_from_view(view(%{"url" => endpoint}), "saved")
    assert state.draft["url"] == endpoint
  end

  test "runtime success needs connection, clean error and capability observation" do
    assert {:ok, %{connected: true}} =
             Mutations.test_result(
               %{
                 "name" => "test",
                 "connected" => true,
                 "last_error" => nil,
                 "capability_observation" => %{
                   "tools" => %{"state" => "known", "discovered" => 0, "exposed" => 0}
                 }
               },
               "test"
             )

    for data <- [
          %{},
          %{"name" => "test", "connected" => true},
          %{"name" => "test", "connected" => false},
          %{"name" => "wrong", "connected" => true},
          %{"name" => "test", "connected" => true, "last_error" => "private"}
        ] do
      assert {:error, _} = Mutations.test_result(data, "test")
    end
  end
end
