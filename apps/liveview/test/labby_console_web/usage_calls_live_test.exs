defmodule LabbyConsoleWeb.UsageCallsLiveTest do
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
      Application.delete_env(:labby_console, :fixture_usage_calls)
      Application.delete_env(:labby_console, :fixture_usage_calls_fresh_after)
    end)

    %{state: state}
  end

  test "call pages filter, reset their cursor and recover without stale rows", %{conn: conn} do
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    {:ok, view, _} = live(recycle(signed), "/activity")
    assert render_async(view) =~ "fetch"
    view |> element("#usage-calls-next") |> render_click()
    assert render_async(view) =~ "fetch_html"
    refute has_element?(view, "#usage-calls-next:not([disabled])")

    view
    |> element("#usage-calls-filters")
    |> render_change(%{"window" => "1h", "outcome" => "timeout"})

    assert render_async(view) =~ "timeout"
    refute render(view) =~ "fetch_html"
    assert has_element?(view, "#usage-calls-next:not([disabled])")
    Application.put_env(:labby_console, :fixture_usage_calls, "malformed")
    view |> element("#usage-calls-refresh") |> render_click()
    assert render_async(view) =~ "Retained calls are unavailable"
    refute has_element?(view, "#usage-calls-table tbody tr")
    Application.put_env(:labby_console, :fixture_usage_calls, "empty")
    view |> element("#usage-calls-refresh") |> render_click()
    assert render_async(view) =~ "No retained calls match"
    Application.put_env(:labby_console, :fixture_usage_calls, "normal")
    view |> element("#usage-calls-refresh") |> render_click()
    assert render_async(view) =~ "fetch"
    send(view.pid, :session_expired)
    assert_redirect(view, "/sign-in", 5000)
  end

  test "refresh includes calls occurring after the page was opened", %{conn: conn} do
    Application.put_env(
      :labby_console,
      :fixture_usage_calls_fresh_after,
      System.system_time(:second) + 1
    )

    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    {:ok, view, _} = live(recycle(signed), "/activity")
    refute render_async(view) =~ "new-call-after-open"
    Process.sleep(1100)
    view |> element("#usage-calls-refresh") |> render_click()
    assert render_async(view) =~ "new-call-after-open"
  end

  test "anonymous activity never exposes rows", %{conn: conn} do
    assert {:error, {:redirect, %{to: "/sign-in"}}} = live(conn, "/activity")
  end
end
