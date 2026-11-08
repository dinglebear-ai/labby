defmodule LabbyConsoleWeb.ServerRuntimeLiveTest do
  use LabbyConsoleWeb.ConnCase
  import Phoenix.LiveViewTest

  test "anonymous callers cannot view a server", %{conn: conn} do
    assert {:error, {:redirect, %{to: "/sign-in"}}} =
             live(conn, "/gateway/upstreams/fixture-server/runtime")
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
      Application.delete_env(:labby_console, :fixture_server_runtime)
      Application.delete_env(:labby_console, :fixture_server_runtime_gate)
    end)

    %{state: state}
  end

  test "refresh removes stale runtime, recovers, and revoked access redirects", %{conn: conn} do
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    {:ok, view, _} = live(recycle(signed), "/gateway/upstreams/fixture-server/runtime")
    html = render_async(view, 5_000)
    assert html =~ "Runtime snapshot"
    assert has_element?(view, "[data-labby-mark='fold']")
    assert has_element?(view, "#server-runtime-refresh[aria-label='Refresh runtime'] svg")
    assert has_element?(view, "#server-runtime-process")
    assert has_element?(view, "[data-state='connected']", "Connected")
    refute html =~ "do-not-display"
    refute html =~ "private-owner"
    Application.put_env(:labby_console, :fixture_server_runtime, "malformed")
    view |> element("#server-runtime-refresh") |> render_click()
    render_async(view, 5_000)
    assert has_element?(view, "#server-runtime-error")
    refute has_element?(view, "#server-runtime-content")
    Application.put_env(:labby_console, :fixture_server_runtime, "normal")
    view |> element("#server-runtime-refresh") |> render_click()
    assert render_async(view, 5_000) =~ "Runtime snapshot"
    Application.put_env(:labby_console, :fixture_server_runtime, "forbidden")
    view |> element("#server-runtime-refresh") |> render_click()
    assert_redirect(view, "/sign-in", 5_000)
  end

  test "loading is visible and a late response cannot restore an expired page", %{conn: conn} do
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    Application.put_env(:labby_console, :fixture_server_runtime_gate, self())
    {:ok, view, _} = live(recycle(signed), "/gateway/upstreams/fixture-server/runtime")
    assert_receive {:runtime_request, request}, 2_000
    assert has_element?(view, "#server-runtime-loading")
    refute has_element?(view, "#server-runtime-content")
    send(view.pid, :session_expired)
    assert_redirect(view, "/sign-in", 2_000)
    send(request, :release_runtime)
  end

  test "empty and disconnected snapshots have explicit states", %{conn: conn} do
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    Application.put_env(:labby_console, :fixture_server_runtime, "empty")
    {:ok, view, _} = live(recycle(signed), "/gateway/upstreams/fixture-server/runtime")
    render_async(view, 5_000)
    assert has_element?(view, "#server-runtime-empty", "No visible runtime snapshot")
    refute has_element?(view, "#server-runtime-content")
    Application.put_env(:labby_console, :fixture_server_runtime, "disconnected")
    view |> element("#server-runtime-refresh") |> render_click()
    render_async(view, 5_000)
    assert has_element?(view, "[data-state='disconnected']", "Disconnected")
    assert has_element?(view, "#server-runtime-process", "Not reported")
  end
end
