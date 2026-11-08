defmodule LabbyConsoleWeb.ServerDetailsLiveTest do
  use LabbyConsoleWeb.ConnCase
  import Phoenix.LiveViewTest

  test "anonymous callers cannot view a server", %{conn: conn} do
    assert {:error, {:redirect, %{to: "/sign-in"}}} =
             live(conn, "/gateway/servers/fixture-server")
  end

  setup do
    state = start_supervised!({Agent, fn -> true end})

    server =
      start_supervised!(
        {Bandit, plug: {LabbyConsole.AuthFixture, state: state}, port: 0, ip: {127, 0, 0, 1}}
      )

    {:ok, {_, port}} = ThousandIsland.listener_info(server)
    old = Application.get_env(:labby_console, :gateway_url)
    Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:#{port}")

    on_exit(fn ->
      Application.put_env(:labby_console, :gateway_url, old)
      Application.delete_env(:labby_console, :fixture_server_details)
      Application.delete_env(:labby_console, :fixture_server_details_gate)
    end)

    %{state: state}
  end

  test "refresh removes stale details, recovers, and revoked access redirects", %{conn: conn} do
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    {:ok, view, _} = live(recycle(signed), "/gateway/servers/fixture-server")
    html = render_async(view, 5_000)
    assert html =~ "Fixture Fetch"
    assert has_element?(view, "[data-labby-mark='fold']")
    assert has_element?(view, "nav[aria-label='Server sections'] a", "Catalog")
    assert has_element?(view, "nav[aria-label='Primary navigation'] a[aria-current='page'] svg")
    assert has_element?(view, "#server-details-refresh[aria-label='Refresh details'] svg")
    assert has_element?(view, "#server-capabilities [data-family='tool']")
    assert has_element?(view, "[data-state='connected']", "Connected")
    refute html =~ "do-not-display"
    refute html =~ "private-warning"
    Application.put_env(:labby_console, :fixture_server_details, "malformed")
    view |> element("#server-details-refresh") |> render_click()
    render_async(view, 5_000)
    assert has_element?(view, "#server-details-error")
    refute has_element?(view, "#server-details-content")
    Application.put_env(:labby_console, :fixture_server_details, "normal")
    view |> element("#server-details-refresh") |> render_click()
    assert render_async(view, 5_000) =~ "Fixture Fetch"
    Application.put_env(:labby_console, :fixture_server_details, "forbidden")
    view |> element("#server-details-refresh") |> render_click()
    assert_redirect(view, "/sign-in", 5_000)
  end

  test "loading is visible and a late response cannot restore an expired page", %{conn: conn} do
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    Application.put_env(:labby_console, :fixture_server_details_gate, self())
    {:ok, view, _} = live(recycle(signed), "/gateway/servers/fixture-server")
    assert_receive {:details_request, request}, 2_000
    assert has_element?(view, "#server-details-loading")
    refute has_element?(view, "#server-details-content")
    send(view.pid, :session_expired)
    assert_redirect(view, "/sign-in", 2_000)
    send(request, :release_details)
  end
end
