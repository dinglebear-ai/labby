defmodule LabbyConsoleWeb.ServerLogsLiveTest do
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
      Application.delete_env(:labby_console, :fixture_server_logs)
      Application.delete_env(:labby_console, :fixture_server_logs_gate)
      Application.delete_env(:labby_console, :fixture_server_logs_owner)
    end)

    %{state: state}
  end

  test "logviewer follows, pauses, escapes text and recovers", %{conn: conn} do
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    {:ok, view, _} = live(recycle(signed), "/logs")
    html = render_async(view)
    assert html =~ "&lt;script&gt;boom&lt;/script&gt;"
    refute html =~ "private-token"
    assert has_element?(view, "#logs-follow", "Pause")
    view |> element("#logs-follow") |> render_click()
    assert has_element?(view, "#logs-follow", "Follow")
    Application.put_env(:labby_console, :fixture_server_logs, "malformed")
    send(view.pid, :poll)
    assert has_element?(view, ".console-log-line")
    view |> element("#logs-refresh") |> render_click()
    assert render_async(view) =~ "Logs unavailable"
    refute has_element?(view, ".console-log-line")
    Application.put_env(:labby_console, :fixture_server_logs, "empty")
    view |> element("#logs-refresh") |> render_click()
    assert render_async(view) =~ "No matching log entries"
    Application.put_env(:labby_console, :fixture_server_logs, "normal")
    view |> element("#logs-follow") |> render_click()
    assert render_async(view) =~ "boom"
    Application.put_env(:labby_console, :fixture_server_logs, "forbidden")
    view |> element("#logs-refresh") |> render_click()
    assert_redirect(view, "/sign-in", 5000)
  end

  test "pause cancels an in-flight load and ignores late results", %{conn: conn} do
    Application.put_env(:labby_console, :fixture_server_logs_gate, self())
    Application.put_env(:labby_console, :fixture_server_logs_owner, self())
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    {:ok, view, html} = live(recycle(signed), "/logs")
    assert html =~ "Loading logs"
    assert_receive {:logs_request, worker}, 2000
    view |> element("#logs-follow") |> render_click()
    send(worker, :release_logs)
    refute has_element?(view, ".console-log-line")
    send(view.pid, :poll)
    assert has_element?(view, "#logs-follow", "Follow")
    assert_receive {:logs_query, _}
    refute_receive {:logs_query, _}, 100
    send(view.pid, :session_expired)
    assert_redirect(view, "/sign-in")
  end
end
