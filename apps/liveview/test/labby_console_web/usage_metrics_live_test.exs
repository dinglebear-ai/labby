defmodule LabbyConsoleWeb.UsageMetricsLiveTest do
  use LabbyConsoleWeb.ConnCase
  import Phoenix.LiveViewTest

  test "anonymous callers cannot view usage", %{conn: conn} do
    assert {:error, {:redirect, %{to: "/sign-in"}}} =
             live(conn, "/usage")
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
      Application.delete_env(:labby_console, :fixture_usage_metrics)
      Application.delete_env(:labby_console, :fixture_usage_metrics_gate)
      Application.delete_env(:labby_console, :fixture_usage_metrics_observer)
    end)

    %{state: state}
  end

  test "refresh removes stale analytics, recovers, and revoked access redirects", %{conn: conn} do
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    {:ok, view, _} = live(recycle(signed), "/usage")
    html = render_async(view, 5_000)
    assert html =~ "Usage analytics"
    assert has_element?(view, "[data-labby-mark='fold']")
    assert has_element?(view, "#usage-refresh[aria-label='Refresh usage'] svg")
    assert has_element?(view, "#usage-summary")
    assert has_element?(view, "#usage-summary", "Calls")
    refute html =~ "do-not-display"
    refute html =~ "private-owner"
    Application.put_env(:labby_console, :fixture_usage_metrics, "malformed")
    view |> element("#usage-refresh") |> render_click()
    render_async(view, 5_000)
    assert has_element?(view, "#usage-error")
    refute has_element?(view, "#usage-content")
    Application.put_env(:labby_console, :fixture_usage_metrics, "normal")
    view |> element("#usage-refresh") |> render_click()
    assert render_async(view, 5_000) =~ "Usage analytics"
    Application.put_env(:labby_console, :fixture_usage_metrics, "forbidden")
    view |> element("#usage-refresh") |> render_click()
    assert_redirect(view, "/sign-in", 5_000)
  end

  test "loading is visible and a late response cannot restore an expired page", %{conn: conn} do
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    Application.put_env(:labby_console, :fixture_usage_metrics_gate, self())
    {:ok, view, _} = live(recycle(signed), "/usage")
    assert_receive {:usage_request, request}, 2_000
    assert has_element?(view, "#usage-loading")
    refute has_element?(view, "#usage-content")
    send(view.pid, :session_expired)
    assert_redirect(view, "/sign-in", 2_000)
    send(request, :release_usage)
  end

  test "window selection reloads exact aggregates and empty results stay explicit", %{conn: conn} do
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    {:ok, view, _} = live(recycle(signed), "/usage")
    render_async(view, 5_000)
    Application.put_env(:labby_console, :fixture_usage_metrics_observer, self())
    view |> element("form[phx-change='window']") |> render_change(%{"window" => "7d"})
    render_async(view, 5_000)
    assert_receive {:usage_params, params}
    assert params["until_unix"] - params["since_unix"] == 604_800
    Application.delete_env(:labby_console, :fixture_usage_metrics_observer)
    Application.put_env(:labby_console, :fixture_usage_metrics, "empty")
    view |> element("#usage-refresh") |> render_click()
    render_async(view, 5_000)
    assert has_element?(view, "#usage-empty", "No calls")
  end
end
