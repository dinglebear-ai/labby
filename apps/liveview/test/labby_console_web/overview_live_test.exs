defmodule LabbyConsoleWeb.OverviewLiveTest do
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
      Application.delete_env(:labby_console, :fixture_usage_metrics)
      Application.delete_env(:labby_console, :fixture_usage_metrics_gate)
      Application.delete_env(:labby_console, :fixture_usage_metrics_observer)
    end)

    %{state: state}
  end

  test "overview requires a session", %{conn: conn} do
    assert {:error, {:redirect, %{to: "/sign-in"}}} = live(conn, "/overview")
  end

  test "overview combines scoped inventory and usage and clears failed summaries", %{conn: conn} do
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    {:ok, view, _} = live(recycle(signed), "/overview")
    settle(view)
    assert has_element?(view, "#overview-inventory")
    assert has_element?(view, "#overview-usage")
    assert has_element?(view, "a[aria-current=page]", "Overview")
    Application.put_env(:labby_console, :fixture_usage_metrics, "malformed")
    view |> element("#overview-refresh") |> render_click()
    settle(view)
    assert has_element?(view, "#overview-usage-error")
    refute has_element?(view, "#overview-usage")
    assert has_element?(view, "#overview-inventory")
    send(view.pid, :session_expired)
    assert_redirect(view, "/sign-in", 2_000)
  end

  defp settle(view) do
    render_async(view, 5_000)
    render_async(view, 5_000)
    refute has_element?(view, "#overview-refresh[disabled]")
  end
end
