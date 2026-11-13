defmodule LabbyConsoleWeb.DiagnosticsLiveTest do
  use LabbyConsoleWeb.ConnCase
  import Phoenix.LiveViewTest

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
      Application.delete_env(:labby_console, :fixture_diagnostics)
    end)

    %{state: state}
  end

  test "system findings load and refresh without retaining failed snapshots", %{conn: conn} do
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    {:ok, view, _} = live(recycle(signed), "/diagnostics")
    assert render_async(view) =~ "Disk space is low"
    Application.put_env(:labby_console, :fixture_diagnostics, "invalid")
    view |> element("#diagnostics-refresh") |> render_click()
    assert render_async(view) =~ "System checks are unavailable"
    refute has_element?(view, "#diagnostics-findings li")
    Application.put_env(:labby_console, :fixture_diagnostics, "empty")
    view |> element("#diagnostics-refresh") |> render_click()
    assert render_async(view) =~ "No system findings"
    Application.put_env(:labby_console, :fixture_diagnostics, "normal")
    view |> element("#diagnostics-refresh") |> render_click()
    assert render_async(view) =~ "Disk space is low"
    Application.put_env(:labby_console, :fixture_diagnostics, "forbidden")
    view |> element("#diagnostics-refresh") |> render_click()
    assert render_async(view) =~ "unavailable"
    assert {:ok, _} = LabbyConsole.Auth.current(get_session(signed, :console_session))
    Application.put_env(:labby_console, :fixture_diagnostics, "unauthorized")
    view |> element("#diagnostics-refresh") |> render_click()
    assert_redirect(view, "/sign-in", 5_000)
  end
end
