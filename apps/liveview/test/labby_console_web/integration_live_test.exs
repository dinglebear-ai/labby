defmodule LabbyConsoleWeb.IntegrationLiveTest do
  use LabbyConsoleWeb.ConnCase
  import Phoenix.LiveViewTest

  test "shows the console shell with only implemented navigation", %{conn: conn} do
    {:ok, view, _} = live(conn, "/")

    assert has_element?(
             view,
             "nav[aria-label='Primary navigation'] a[aria-current='page']",
             "Gateway"
           )

    assert has_element?(view, "a[href='#console-integration']", "Skip to content")
    refute has_element?(view, "a[href='/depot']")
    assert has_element?(view, "#console-scope", "Public probes")
  end

  test "shows real readiness and retries after the gateway becomes unavailable", %{conn: conn} do
    state = start_supervised!({Agent, fn -> {200, ~s({"status":"ready"})} end})

    server =
      start_supervised!(
        {Bandit, plug: {LabbyConsole.GatewayFixture, state: state}, port: 0, ip: {127, 0, 0, 1}}
      )

    {:ok, {_, port}} = ThousandIsland.listener_info(server)
    previous = Application.get_env(:labby_console, :gateway_url)
    Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:#{port}")
    on_exit(fn -> Application.put_env(:labby_console, :gateway_url, previous) end)
    {:ok, view, _} = live(conn, "/")
    assert render_async(view, 5_000) =~ "Gateway ready"
    Agent.update(state, fn _ -> {500, "internal details must not leak"} end)
    view |> element("#gateway-retry") |> render_click()
    assert render_async(view, 5_000) =~ "Gateway unavailable"
    refute render(view) =~ "internal details must not leak"
  end
end
