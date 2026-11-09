defmodule LabbyConsole.GatewayMutationFixture do
  @moduledoc false
  use Plug.Router
  plug Plug.Parsers, parsers: [:json], json_decoder: Jason
  plug :match
  plug :dispatch

  post "/v1/gateway" do
    action = conn.body_params["action"]
    params = conn.body_params["params"]
    state = Agent.get(conn.private.fixture, & &1)
    send(state.owner, {:gateway_mutation_wire, action, params})

    if state[:hold] == action do
      send(state.owner, {:gateway_mutation_hold, self()})
      receive do: (:release_gateway -> :ok), after: (3_000 -> :ok)
    end

    {status, body} =
      if state[:reject] == action do
        {403, %{"message" => "private-provider-sentinel"}}
      else
        dispatch(conn.private.fixture, action, params)
      end

    send_resp(conn, status, Jason.encode!(body))
  end

  match _ do
    LabbyConsole.ActionFixture.call(conn, state: conn.private.fixture)
  end

  defp dispatch(agent, action, params) do
    rows = Agent.get(agent, & &1.rows)
    name = params["name"]
    spec_name = get_in(params, ["spec", "name"])

    case action do
      "gateway.list" ->
        {200,
         Enum.map(rows, fn {name, config} ->
           %{
             id: name,
             name: name,
             display_name: config["display_name"],
             source: "custom_gateway",
             enabled: config["enabled"],
             connected: false
           }
         end)}

      "gateway.get" ->
        if rows[name],
          do:
            {200,
             view(Map.merge(rows[name], Agent.get(agent, &(&1[:read_config_override] || %{}))))},
          else:
            {404,
             %{
               contract_version: 1,
               service: "gateway",
               action: "gateway.get",
               kind: "not_found",
               side_effects: "none_expected"
             }}

      "gateway.test" ->
        {200,
         %{
           name: name || params["spec"]["name"],
           connected: true,
           last_error: nil,
           capability_observation: %{tools: %{state: "known", discovered: 1, exposed: 1}}
         }}

      "gateway.add" when is_map_key(rows, spec_name) ->
        {409,
         %{
           contract_version: 1,
           service: "gateway",
           action: action,
           kind: "conflict",
           side_effects: "none_expected"
         }}

      "gateway.add" ->
        config = params["spec"]
        reference = Agent.get(agent, & &1[:normalized_reference])

        config =
          if params["bearer_token_value"] && reference,
            do: Map.put(config, "bearer_token_env", reference),
            else: config

        Agent.update(agent, &put_in(&1, [:rows, config["name"]], config))
        result(agent, action, config)

      "gateway.update" ->
        config = Map.merge(rows[name], params["patch"])

        Agent.update(
          agent,
          &%{&1 | rows: &1.rows |> Map.delete(name) |> Map.put(config["name"], config)}
        )

        result(agent, action, config)

      "gateway.remove" ->
        Agent.update(agent, &%{&1 | rows: Map.delete(&1.rows, name)})
        result(agent, action, rows[name])
    end
  end

  defp result(agent, action, config) do
    if Agent.get(agent, &(&1[:malformed] == action)), do: {200, %{}}, else: {200, view(config)}
  end

  def view(config),
    do: %{
      config:
        Map.merge(
          %{
            "url" => nil,
            "command" => nil,
            "args" => [],
            "enabled" => true,
            "proxy_resources" => true,
            "proxy_prompts" => true,
            "proxy_skills" => false,
            "oauth_enabled" => false
          },
          Map.drop(config, ~w(env headers headers_env display_name transport socket_path))
        ),
      runtime: %{name: config["name"], connected: false}
    }

  def init(opts), do: opts
  def call(conn, opts), do: super(put_private(conn, :fixture, opts[:state]), opts)
end

defmodule LabbyConsoleWeb.GatewayMutationsLiveTest do
  use LabbyConsoleWeb.ConnCase
  import Phoenix.LiveViewTest

  setup do
    owner = self()

    config = %{
      "name" => "saved",
      "enabled" => true,
      "url" => "https://saved.example/mcp",
      "command" => nil,
      "args" => [],
      "proxy_resources" => true,
      "proxy_prompts" => true,
      "proxy_skills" => false,
      "oauth_enabled" => false,
      "env" => %{"PRIVATE" => "retained-secret"},
      "priority" => 2.0
    }

    state =
      start_supervised!(
        {Agent,
         fn ->
           %{owner: owner, session_status: 200, response: {200, "{}"}, rows: %{"saved" => config}}
         end}
      )

    server =
      start_supervised!(
        {Bandit,
         plug: {LabbyConsole.GatewayMutationFixture, state: state}, port: 0, ip: {127, 0, 0, 1}}
      )

    {:ok, {_, port}} = ThousandIsland.listener_info(server)
    old = Application.get_env(:labby_console, :gateway_url)
    Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:#{port}")
    on_exit(fn -> Application.put_env(:labby_console, :gateway_url, old) end)
    %{state: state}
  end

  test "normal Gateway entry opens a native add form and invalid draft cannot dispatch", %{
    conn: conn
  } do
    {view, _} = open(conn)
    assert has_element?(view, "#gateway-add")
    view |> element("#gateway-add") |> render_click()
    assert has_element?(view, "#gm-dialog[role=dialog][aria-modal=true]", "Add server")

    view
    |> form("#gm-server-form", server: %{name: "bad name", url: "invalid"})
    |> render_submit()

    assert has_element?(view, "#gm-errors")
    refute_receive {:gateway_mutation_wire, "gateway.add", _}
  end

  test "add saves without test, preserves exact argv, and refreshes real fleet", %{
    conn: conn,
    state: state
  } do
    {view, _} = open(conn)
    view |> element("#gateway-add") |> render_click()
    render_change(view, "gateway_form_change", %{"server" => %{"transport" => "stdio"}})

    view
    |> form("#gm-server-form",
      server: %{name: "added", command: "node", args: ~s(["a b","","c\\\\d"]), enabled: "false"}
    )
    |> render_submit()

    assert_receive {:gateway_mutation_wire, "gateway.add", %{"spec" => spec}}, 2_000
    assert spec["args"] == ["a b", "", "c\\d"]
    assert spec["enabled"] == false
    wait_idle(view)
    assert has_element?(view, "#gm-notice", "accepted")
    refute has_element?(view, "#gm-dialog")
    assert Agent.get(state, & &1.rows["added"]["args"]) == ["a b", "", "c\\d"]
    refute_receive {:gateway_mutation_wire, "gateway.test", _}
  end

  test "fresh edit produces only changed patch, rename preserves hidden fields", %{
    conn: conn,
    state: state
  } do
    {view, _} = open(conn)
    render_click(view, "gateway_edit", %{"name" => "saved"})
    assert_receive {:gateway_mutation_wire, "gateway.get", %{"name" => "saved"}}, 2_000
    wait_idle(view)

    view
    |> form("#gm-server-form", server: %{name: "renamed", enabled: "false"})
    |> render_submit()

    assert_receive {:gateway_mutation_wire, "gateway.update",
                    %{"name" => "saved", "patch" => patch}},
                   2_000

    assert patch == %{"name" => "renamed", "enabled" => false}
    wait_idle(view)
    assert Agent.get(state, & &1.rows["renamed"]["env"]) == %{"PRIVATE" => "retained-secret"}
    assert Agent.get(state, & &1.rows["renamed"]["priority"]) == 2.0
    refute render(view) =~ "retained-secret"
  end

  test "JSON applies locally, draft test leaves inventory unchanged, draft changes invalidate result",
       %{conn: conn, state: state} do
    {view, _} = open(conn)
    view |> element("#gateway-add") |> render_click()
    view |> element("#gm-json") |> render_click()

    view
    |> form("#gm-json-form",
      config: %{json: ~s({"mcpServers":{"draft":{"url":"https://draft.example/mcp","args":[]}}})}
    )
    |> render_submit()

    refute_receive {:gateway_mutation_wire, "gateway.add", _}
    view |> element("#gm-test") |> render_click()

    assert_receive {:gateway_mutation_wire, "gateway.test", %{"spec" => %{"name" => "draft"}}},
                   2_000

    wait_idle(view)
    assert has_element?(view, "#gm-test-result", "connected")
    assert Agent.get(state, &Map.keys(&1.rows)) == ["saved"]
    view |> form("#gm-server-form", server: %{name: "changed"}) |> render_change()
    refute has_element?(view, "#gm-test-result")
  end

  test "saved test explicitly ignores unsaved changes and removal requires named confirmation", %{
    conn: conn,
    state: state
  } do
    {view, _} = open(conn)
    render_click(view, "gateway_edit", %{"name" => "saved"})
    wait_idle(view)

    view
    |> form("#gm-server-form", server: %{url: "https://unsaved.example/mcp"})
    |> render_change()

    view |> element("#gm-test") |> render_click()
    assert_receive {:gateway_mutation_wire, "gateway.test", %{"name" => "saved"}}, 2_000
    wait_idle(view)
    render_click(view, "gateway_close", %{})
    render_click(view, "gateway_remove", %{"name" => "saved"})
    wait_idle(view)
    view |> form("#gm-remove-form", confirmation: %{name: "wrong"}) |> render_submit()
    refute_receive {:gateway_mutation_wire, "gateway.remove", _}
    view |> form("#gm-remove-form", confirmation: %{name: "saved"}) |> render_submit()
    assert_receive {:gateway_mutation_wire, "gateway.remove", %{"name" => "saved"}}, 2_000
    wait_idle(view)
    assert Agent.get(state, & &1.rows) == %{}
  end

  test "forbidden failure retains draft, redacts server details and keeps session valid", %{
    conn: conn,
    state: state
  } do
    {view, signed} = open(conn)
    Agent.update(state, &Map.put(&1, :reject, "gateway.add"))
    view |> element("#gateway-add") |> render_click()

    view
    |> form("#gm-server-form",
      server: %{
        name: "denied",
        url: "https://denied.example/mcp",
        bearer_token_value: "typed-secret"
      }
    )
    |> render_submit()

    assert_receive {:gateway_mutation_wire, "gateway.add", _}, 2_000
    wait_idle(view)
    assert has_element?(view, "#gm-name[value=denied]")
    assert has_element?(view, "#gm-errors", "forbidden")
    refute render(view) =~ "private-provider-sentinel"
    refute render(view) =~ "typed-secret"
    assert {:ok, _} = LabbyConsole.Auth.current(get_session(signed, :console_session))
    refute inspect(:sys.get_status(view.pid)) =~ "typed-secret"
  end

  test "duplicate submit is fenced, closing cancels local wait and uncertain write needs affected reads",
       %{conn: conn, state: state} do
    {view, _} = open(conn)
    Agent.update(state, &Map.put(&1, :hold, "gateway.add"))
    view |> element("#gateway-add") |> render_click()
    params = %{"server" => %{"name" => "once", "url" => "https://once.example/mcp"}}
    render_submit(view, "gateway_save", params)
    assert_receive {:gateway_mutation_wire, "gateway.add", _}, 2_000
    assert_receive {:gateway_mutation_hold, request}, 2_000
    render_submit(view, "gateway_save", params)
    assert has_element?(view, "#gm-save[disabled]")
    render_click(view, "gateway_close", %{})
    refute has_element?(view, "#gm-dialog")
    wait_idle(view)
    refute_receive {:gateway_mutation_wire, "gateway.add", _}, 100
    send(request, :release_gateway)
    assert has_element?(view, "#gm-reconcile")
    assert has_element?(view, "#gateway-add[disabled]")
    Agent.update(state, &Map.delete(&1, :hold))
    view |> element("#gm-reconcile") |> render_click()
    assert_receive {:gateway_mutation_wire, "gateway.get", %{"name" => "once"}}, 2_000
    wait_idle(view)
    refute has_element?(view, "#gateway-add[disabled]")
  end

  test "closed drafts ignore late JSON input and logout clears private state", %{conn: conn} do
    {view, signed} = open(conn)
    view |> element("#gateway-add") |> render_click()
    view |> element("#gm-json") |> render_click()
    render_change(view, "gateway_json_change", %{"config" => %{"json" => "private-paste"}})
    render_click(view, "gateway_close", %{})
    render_change(view, "gateway_json_change", %{"config" => %{"json" => "late-private-paste"}})
    assert :sys.get_state(view.pid).socket.assigns.mutation.json == ""

    render_submit(view, "gateway_apply_json", %{
      "config" => %{
        "json" => ~s({"name":"late","command":"node","env":{"KEY":"late-private-paste"}})
      }
    })

    refute inspect(:sys.get_state(view.pid).socket.assigns.mutation) =~ "late-private-paste"
    view |> element("#gateway-add") |> render_click()

    render_change(view, "gateway_form_change", %{
      "server" => %{"bearer_token_value" => "private-before-logout"}
    })

    assert :ok = LabbyConsole.Auth.sign_out(get_session(signed, :console_session))
    assert_redirect(view, "/sign-in", 2_000)
  end

  test "duplicate ID reports typed conflict while preserving its valid draft", %{
    conn: conn,
    state: state
  } do
    {view, _} = open(conn)
    view |> element("#gateway-add") |> render_click()

    view
    |> form("#gm-server-form", server: %{name: "saved", url: "https://duplicate.example/mcp"})
    |> render_submit()

    wait_idle(view)
    assert has_element?(view, "#gm-errors", "conflict")
    assert has_element?(view, "#gm-url[value='https://duplicate.example/mcp']")
    assert Agent.get(state, & &1.rows["saved"]["url"]) == "https://saved.example/mcp"
  end

  test "accepted write with failed affected readback stays accepted and cannot be replayed", %{
    conn: conn,
    state: state
  } do
    {view, _} = open(conn)
    Agent.update(state, &Map.put(&1, :reject, "gateway.get"))
    view |> element("#gateway-add") |> render_click()

    view
    |> form("#gm-server-form", server: %{name: "accepted", url: "https://accepted.example/mcp"})
    |> render_submit()

    assert_receive {:gateway_mutation_wire, "gateway.add", _}, 2_000
    wait_idle(view)
    assert has_element?(view, "#gm-receipt", "accepted")
    assert has_element?(view, "#gateway-add[disabled]")
    view |> element("#servers-refresh") |> render_click()
    render_async(view)
    assert has_element?(view, "#gm-receipt", "accepted")
    refute_receive {:gateway_mutation_wire, "gateway.add", _}
    Agent.update(state, &Map.delete(&1, :reject))
    view |> element("#gm-reconcile") |> render_click()
    wait_idle(view)
    refute has_element?(view, "#gm-receipt")
  end

  test "uncertain rename inspects both affected names and unrelated refresh cannot clear hold", %{
    conn: conn,
    state: state
  } do
    {view, _} = open(conn)
    render_click(view, "gateway_edit", %{"name" => "saved"})
    wait_idle(view)
    Agent.update(state, &Map.put(&1, :malformed, "gateway.update"))
    view |> form("#gm-server-form", server: %{name: "uncertain-rename"}) |> render_submit()
    wait_idle(view)
    assert has_element?(view, "#gm-receipt", "uncertain")
    assert has_element?(view, "#gm-save[disabled]")
    render_click(view, "refresh", %{})
    render_async(view)
    assert has_element?(view, "#gm-receipt")
    view |> element("#gm-reconcile") |> render_click()
    wait_idle(view)
    assert_receive {:gateway_mutation_wire, "gateway.get", %{"name" => "saved"}}
    assert_receive {:gateway_mutation_wire, "gateway.get", %{"name" => "uncertain-rename"}}
    refute has_element?(view, "#gm-receipt")
  end

  test "private map intent reaches wire and successful save clears private form input", %{
    conn: conn,
    state: state
  } do
    {view, _} = open(conn)
    render_click(view, "gateway_edit", %{"name" => "saved"})
    wait_idle(view)

    render_change(view, "gateway_form_change", %{
      "server" => %{
        "env_intent" => "replace",
        "env" => ~s({"KEY":"ephemeral-map-value"}),
        "headers_intent" => "clear"
      }
    })

    view |> form("#gm-server-form") |> render_submit()

    assert_receive {:gateway_mutation_wire, "gateway.update",
                    %{"patch" => %{"env" => %{"KEY" => "ephemeral-map-value"}, "headers" => %{}}}},
                   2_000

    wait_idle(view)
    refute render(view) =~ "ephemeral-map-value"
    assert Agent.get(state, & &1.rows["saved"]["env"]) == %{"KEY" => "ephemeral-map-value"}
  end

  test "dialog restoration targets a connected control when its row is filtered out" do
    state = %{
      mutation: %LabbyConsole.GatewayMutations.Dialog{
        dialog: :form,
        model: LabbyConsole.GatewayMutations.new(),
        opener: "#gateway-edit-c2F2ZWQ"
      },
      mutation_request: nil,
      mutation_receipt: nil,
      rows: [
        %{"id" => "saved", "name" => "saved", "enabled" => true, "source" => "custom_gateway"}
      ],
      selected: nil,
      preferences: LabbyConsole.GatewayFleet.preferences(%{"query" => "other"})
    }

    html = render_component(&LabbyConsoleWeb.GatewayMutationComponents.dialog/1, state: state)

    [commands] =
      html
      |> LazyHTML.from_fragment()
      |> LazyHTML.query("#gm-backdrop")
      |> LazyHTML.attribute("phx-remove")

    assert [["focus", %{"to" => "#gateway-add"}]] = Jason.decode!(commands)
  end

  test "accepted raw bearer add reconciles the backend-derived reference only after matching readback",
       %{conn: conn, state: state} do
    {view, _} = open(conn)

    Agent.update(
      state,
      &Map.merge(&1, %{
        normalized_reference: "LABBY_GW_RAW_BEARER_AUTH_HEADER",
        read_config_override: %{"bearer_token_env" => "UNRELATED_REFERENCE"}
      })
    )

    view |> element("#gateway-add") |> render_click()

    view
    |> form("#gm-server-form",
      server: %{
        name: "raw-bearer",
        url: "https://raw.example/mcp",
        bearer_token_value: "transient-bearer-sentinel"
      }
    )
    |> render_submit()

    assert_receive {:gateway_mutation_wire, "gateway.add",
                    %{
                      "spec" => %{"bearer_token_env" => nil},
                      "bearer_token_value" => "transient-bearer-sentinel"
                    }},
                   2_000

    wait_idle(view)
    assert has_element?(view, "#gm-receipt", "accepted")
    assert has_element?(view, "#gateway-add[disabled]")
    refute render(view) =~ "transient-bearer-sentinel"
    render_click(view, "refresh", %{})
    render_async(view)
    assert has_element?(view, "#gm-receipt", "accepted")

    render_submit(view, "gateway_save", %{
      "server" => %{"name" => "repeat", "url" => "https://repeat.example/mcp"}
    })

    refute_receive {:gateway_mutation_wire, "gateway.add", _}
    Agent.update(state, &Map.delete(&1, :read_config_override))
    view |> element("#gm-reconcile") |> render_click()
    wait_idle(view)
    refute has_element?(view, "#gm-receipt")
    refute has_element?(view, "#gateway-add[disabled]")

    assert Agent.get(state, & &1.rows["raw-bearer"]["bearer_token_env"]) ==
             "LABBY_GW_RAW_BEARER_AUTH_HEADER"
  end

  test "unknown raw bearer add retains its private hold despite a normalized affected read", %{
    conn: conn,
    state: state
  } do
    {view, _} = open(conn)

    Agent.update(
      state,
      &Map.merge(&1, %{
        normalized_reference: "LABBY_GW_UNKNOWN_BEARER_AUTH_HEADER",
        malformed: "gateway.add"
      })
    )

    view |> element("#gateway-add") |> render_click()

    view
    |> form("#gm-server-form",
      server: %{
        name: "unknown-bearer",
        url: "https://raw.example/mcp",
        bearer_token_value: "transient-bearer-sentinel"
      }
    )
    |> render_submit()

    assert_receive {:gateway_mutation_wire, "gateway.add", _}, 2_000
    wait_idle(view)
    view |> element("#gm-reconcile") |> render_click()
    wait_idle(view)
    assert has_element?(view, "#gm-receipt", "uncertain")
    assert has_element?(view, "#gateway-add[disabled]")

    render_submit(view, "gateway_save", %{
      "server" => %{"name" => "unknown-bearer", "url" => "https://raw.example/mcp"}
    })

    refute_receive {:gateway_mutation_wire, "gateway.add", _}
  end

  for intent <- ~w(clear replace) do
    test "uncertain #{intent} label update remains held after projected and unrelated reads", %{
      conn: conn,
      state: state
    } do
      intent = unquote(intent)
      Agent.update(state, &put_in(&1, [:rows, "saved", "display_name"], "Existing label"))
      {view, _} = open(conn)
      render_click(view, "gateway_edit", %{"name" => "saved"})
      wait_idle(view)
      Agent.update(state, &Map.put(&1, :malformed, "gateway.update"))

      render_submit(view, "gateway_save", %{
        "server" => %{"display_name_intent" => intent, "display_name" => "New label"}
      })

      expected = if intent == "clear", do: nil, else: "New label"

      assert_receive {:gateway_mutation_wire, "gateway.update",
                      %{"patch" => %{"display_name" => ^expected}}},
                     2_000

      wait_idle(view)
      assert has_element?(view, "#gm-receipt", "uncertain")
      render_click(view, "refresh", %{})
      render_async(view)
      assert has_element?(view, "#gm-receipt", "uncertain")
      view |> element("#gm-reconcile") |> render_click()
      wait_idle(view)
      assert has_element?(view, "#gm-receipt", "uncertain")
      assert has_element?(view, "#gm-save[disabled]")
      assert has_element?(view, "#gateway-add[disabled]")

      render_submit(view, "gateway_save", %{
        "server" => %{"display_name_intent" => intent, "display_name" => "New label"}
      })

      render_click(view, "gateway_add", %{})
      refute_receive {:gateway_mutation_wire, "gateway.update", _}
      refute_receive {:gateway_mutation_wire, "gateway.add", _}
    end
  end

  defp open(conn) do
    signed = post(conn, "/sign-in", %{credential: "alice"})
    {:ok, view, _} = live(recycle(signed), "/gateway")
    render_async(view)
    {view, signed}
  end

  defp wait_idle(view, attempts \\ 80)
  defp wait_idle(_, 0), do: flunk("gateway operation did not finish")

  defp wait_idle(view, attempts) do
    if :sys.get_state(view.pid).socket.assigns.mutation_request do
      Process.sleep(20)
      wait_idle(view, attempts - 1)
    else
      render_async(view)
    end
  end
end
