defmodule LabbyConsole.GatewayLifecycleFixture do
  @moduledoc false
  use Plug.Router
  plug Plug.Parsers, parsers: [:json], json_decoder: Jason
  plug :match
  plug :dispatch

  post "/v1/gateway" do
    action = conn.body_params["action"]
    params = conn.body_params["params"]
    state = Agent.get(conn.private.fixture, & &1)
    send(state.owner, {:gateway_lifecycle_wire, action, params})

    {status, body} =
      cond do
        state[:fail] == action ->
          {503,
           %{
             contract_version: 1,
             service: "gateway",
             action: action,
             kind: "upstream_unavailable",
             side_effects: "unknown"
           }}

        state[:malformed] == action ->
          {200, %{}}

        true ->
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

    case action do
      "gateway.list" ->
        {200,
         Enum.map(rows, fn {id, row} ->
           %{
             id: id,
             name: id,
             display_name: nil,
             source: row["source"],
             enabled: row["enabled"],
             connected: row["connected"]
           }
         end)}

      "gateway.get" ->
        row = rows[name]

        {200,
         %{
           config: %{"name" => name, "enabled" => row["enabled"]},
           runtime: %{"name" => name, "connected" => row["connected"]}
         }}

      "gateway.mcp.enable" ->
        {200, set(agent, name, %{"enabled" => true, "connected" => true})}

      "gateway.mcp.disable" ->
        gateway = set(agent, name, %{"enabled" => false, "connected" => false})

        cleanup =
          if params["cleanup"],
            do: %{
              upstream: name,
              aggressive: params["aggressive"],
              dry_run: false,
              gateway_matched: 2,
              local_matched: 0,
              aggressive_matched: 0,
              gateway_killed: 2,
              local_killed: 0,
              aggressive_killed: 0
            }

        {200, %{gateway: gateway, cleanup: cleanup}}

      "gateway.mcp.restart" ->
        if Agent.get(agent, & &1[:restart_incomplete]) do
          {200, %{completed: false}}
        else
          {200,
           %{
             completed: true,
             gateway: set(agent, name, %{"connected" => true}),
             cleanup: nil
           }}
        end
    end
  end

  defp set(agent, name, changes) do
    Agent.update(agent, &put_in(&1, [:rows, name], Map.merge(&1.rows[name], changes)))
    row = Agent.get(agent, & &1.rows[name])

    %{
      config: %{"name" => name, "enabled" => row["enabled"]},
      runtime: %{"name" => name, "connected" => row["connected"]}
    }
  end

  def init(opts), do: opts
  def call(conn, opts), do: super(put_private(conn, :fixture, opts[:state]), opts)
end

defmodule LabbyConsoleWeb.GatewayLifecycleLiveTest do
  use LabbyConsoleWeb.ConnCase
  import Phoenix.LiveViewTest
  alias LabbyConsoleWeb.GatewayMutationComponents, as: Mutations

  setup do
    owner = self()

    rows = %{
      "running" => %{"source" => "custom_gateway", "enabled" => true, "connected" => true},
      "stopped" => %{"source" => "custom_gateway", "enabled" => false, "connected" => false},
      "builtin" => %{"source" => "in_process", "enabled" => true, "connected" => true}
    }

    state =
      start_supervised!(
        {Agent, fn -> %{owner: owner, session_status: 200, response: {200, "{}"}, rows: rows} end}
      )

    server =
      start_supervised!(
        {Bandit,
         plug: {LabbyConsole.GatewayLifecycleFixture, state: state}, port: 0, ip: {127, 0, 0, 1}}
      )

    {:ok, {_, port}} = ThousandIsland.listener_info(server)
    old = Application.get_env(:labby_console, :gateway_url)
    Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:#{port}")
    on_exit(fn -> Application.put_env(:labby_console, :gateway_url, old) end)
    %{state: state}
  end

  test "lifecycle controls appear only for configured upstreams and follow saved enablement", %{
    conn: conn
  } do
    {view, _} = open(conn)

    assert has_element?(view, "#" <> Mutations.action_id("stop", "running"), "Stop")
    refute has_element?(view, "#" <> Mutations.action_id("start", "running"))
    assert has_element?(view, "#" <> Mutations.action_id("reload", "running"), "Reload")

    assert has_element?(view, "#" <> Mutations.action_id("start", "stopped"), "Start")
    refute has_element?(view, "#" <> Mutations.action_id("stop", "stopped"))
    refute has_element?(view, "#" <> Mutations.action_id("reload", "stopped"))

    for action <- ~w(start stop reload) do
      refute has_element?(view, "#" <> Mutations.action_id(action, "builtin"))
    end
  end

  test "start confirms, enables the named upstream and reconciles the real fleet", %{
    conn: conn,
    state: state
  } do
    {view, _} = open(conn)
    view |> element("#" <> Mutations.action_id("start", "stopped")) |> render_click()

    assert has_element?(view, "#gl-dialog[role=dialog][aria-modal=true]", "Start server")
    refute_receive {:gateway_lifecycle_wire, "gateway.mcp.enable", _}

    view |> element("#gl-confirm") |> render_click()

    assert_receive {:gateway_lifecycle_wire, "gateway.mcp.enable", %{"name" => "stopped"}}, 2_000
    wait_idle(view)

    assert has_element?(view, "#gl-notice", "enabled")
    refute has_element?(view, "#gl-dialog")
    assert Agent.get(state, & &1.rows["stopped"]["enabled"]) == true
    assert has_element?(view, "#" <> Mutations.action_id("stop", "stopped"))
  end

  test "cancelling a lifecycle confirmation dispatches nothing", %{conn: conn} do
    {view, _} = open(conn)
    view |> element("#" <> Mutations.action_id("stop", "running")) |> render_click()
    view |> element("#gl-cancel") |> render_click()

    refute has_element?(view, "#gl-dialog")
    refute_receive {:gateway_lifecycle_wire, "gateway.mcp.disable", _}
  end

  test "stop requests runtime cleanup only when asked and reports its counts", %{conn: conn} do
    {view, _} = open(conn)
    view |> element("#" <> Mutations.action_id("stop", "running")) |> render_click()

    view
    |> form("#gl-form", lifecycle: %{cleanup: "true"})
    |> render_submit()

    assert_receive {:gateway_lifecycle_wire, "gateway.mcp.disable", params}, 2_000
    assert params == %{"name" => "running", "cleanup" => true, "aggressive" => false}
    wait_idle(view)

    assert has_element?(view, "#gl-notice", "disabled")
    assert has_element?(view, "#gl-notice", "2 matched")
  end

  test "stop without cleanup never asks the backend to kill processes", %{conn: conn} do
    {view, _} = open(conn)
    view |> element("#" <> Mutations.action_id("stop", "running")) |> render_click()
    view |> element("#gl-confirm") |> render_click()

    assert_receive {:gateway_lifecycle_wire, "gateway.mcp.disable", params}, 2_000
    assert params["cleanup"] == false
    wait_idle(view)
    refute render(view) =~ "matched"
  end

  test "reload replaces the connection and bounds its backend wait", %{conn: conn} do
    {view, _} = open(conn)
    view |> element("#" <> Mutations.action_id("reload", "running")) |> render_click()
    view |> element("#gl-confirm") |> render_click()

    assert_receive {:gateway_lifecycle_wire, "gateway.mcp.restart", params}, 2_000
    assert params["name"] == "running"
    assert params["wait_ms"] <= 20_000
    wait_idle(view)
    assert has_element?(view, "#gl-notice", "reconnected")
  end

  test "an unfinished reload is never reported as finished and retains its receipt", %{
    conn: conn,
    state: state
  } do
    Agent.update(state, &Map.put(&1, :restart_incomplete, true))
    {view, _} = open(conn)
    view |> element("#" <> Mutations.action_id("reload", "running")) |> render_click()
    view |> element("#gl-confirm") |> render_click()

    assert_receive {:gateway_lifecycle_wire, "gateway.mcp.restart", _}, 2_000
    wait_idle(view)

    assert has_element?(view, "#gl-receipt")
    assert has_element?(view, "#gl-notice", "may continue")
    refute render(view) =~ "reconnected"
  end

  test "a failed lifecycle operation with unknown effects retains a receipt and blocks writes", %{
    conn: conn,
    state: state
  } do
    Agent.update(state, &Map.put(&1, :fail, "gateway.mcp.disable"))
    {view, _} = open(conn)
    view |> element("#" <> Mutations.action_id("stop", "running")) |> render_click()
    view |> element("#gl-confirm") |> render_click()

    assert_receive {:gateway_lifecycle_wire, "gateway.mcp.disable", _}, 2_000
    wait_idle(view)

    assert has_element?(view, "#gl-receipt")
    assert has_element?(view, "#gl-notice", "uncertain")
    assert has_element?(view, "#" <> Mutations.action_id("reload", "running") <> "[disabled]")
    assert has_element?(view, "#gateway-add[disabled]")
  end

  test "a lifecycle response that cannot be verified for the named upstream is not settled", %{
    conn: conn,
    state: state
  } do
    Agent.update(state, &Map.put(&1, :malformed, "gateway.mcp.enable"))
    {view, _} = open(conn)
    view |> element("#" <> Mutations.action_id("start", "stopped")) |> render_click()
    view |> element("#gl-confirm") |> render_click()

    assert_receive {:gateway_lifecycle_wire, "gateway.mcp.enable", _}, 2_000
    wait_idle(view)

    assert has_element?(view, "#gl-receipt")
    refute render(view) =~ "now enabled"
  end

  test "an uncertain stop is cleared only by an enablement read that matches", %{
    conn: conn,
    state: state
  } do
    Agent.update(state, &Map.put(&1, :fail, "gateway.mcp.disable"))
    {view, _} = open(conn)
    view |> element("#" <> Mutations.action_id("stop", "running")) |> render_click()
    view |> element("#gl-confirm") |> render_click()
    assert_receive {:gateway_lifecycle_wire, "gateway.mcp.disable", _}, 2_000
    wait_idle(view)

    refute has_element?(view, "#gl-acknowledge")
    view |> element("#gl-reconcile") |> render_click()
    assert_receive {:gateway_lifecycle_wire, "gateway.get", %{"name" => "running"}}, 2_000
    wait_idle(view)

    assert has_element?(view, "#gl-receipt")
    assert has_element?(view, "#gl-notice", "does not match")

    Agent.update(state, &put_in(&1, [:rows, "running", "enabled"], false))
    view |> element("#gl-reconcile") |> render_click()
    assert_receive {:gateway_lifecycle_wire, "gateway.get", _}, 2_000
    wait_idle(view)

    refute has_element?(view, "#gl-receipt")
    assert has_element?(view, "#gl-notice", "now matches")
    refute has_element?(view, "#gateway-add[disabled]")
  end

  test "an unverifiable reload receipt is cleared only by an explicit acknowledgement", %{
    conn: conn,
    state: state
  } do
    Agent.update(state, &Map.put(&1, :restart_incomplete, true))
    {view, _} = open(conn)
    view |> element("#" <> Mutations.action_id("reload", "running")) |> render_click()
    view |> element("#gl-confirm") |> render_click()
    assert_receive {:gateway_lifecycle_wire, "gateway.mcp.restart", _}, 2_000
    wait_idle(view)

    refute has_element?(view, "#gl-reconcile")
    view |> element("#gl-acknowledge") |> render_click()
    render_async(view)

    refute has_element?(view, "#gl-receipt")
    assert has_element?(view, "#gl-notice", "without verification")
    assert has_element?(view, "#gl-notice", "cannot confirm")
    refute has_element?(view, "#gateway-add[disabled]")
    refute_receive {:gateway_lifecycle_wire, "gateway.mcp.restart", _}
  end

  defp open(conn) do
    signed = post(conn, "/sign-in", %{credential: "alice"})
    {:ok, view, _} = live(recycle(signed), "/gateway")
    render_async(view)
    {view, signed}
  end

  defp wait_idle(view, attempts \\ 120)
  defp wait_idle(_, 0), do: flunk("gateway lifecycle operation did not finish")

  defp wait_idle(view, attempts) do
    if :sys.get_state(view.pid).socket.assigns.mutation_request do
      Process.sleep(20)
      wait_idle(view, attempts - 1)
    else
      render_async(view)
    end
  end
end
