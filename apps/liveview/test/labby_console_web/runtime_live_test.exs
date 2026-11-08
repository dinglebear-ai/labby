defmodule LabbyConsoleWeb.RuntimeLiveTest do
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
      Application.delete_env(:labby_console, :fixture_runtime)
    end)

    %{}
  end

  test "runtime refresh removes failed samples and recovers without inventing unsupported values",
       %{conn: conn} do
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    {:ok, view, _} = live(recycle(signed), "/runtime")
    assert render_async(view) =~ "6.4%"
    Application.put_env(:labby_console, :fixture_runtime, "malformed")
    view |> element("#runtime-refresh") |> render_click()
    assert render_async(view) =~ "Runtime sample is unavailable"
    refute has_element?(view, "#runtime-metrics")
    Application.put_env(:labby_console, :fixture_runtime, "unsupported")
    view |> element("#runtime-refresh") |> render_click()
    assert render_async(view) =~ "Sampling is not available"
    assert has_element?(view, "#runtime-cpu", "Unavailable")
    refute render(view) =~ "Unavailable /s"
    Application.put_env(:labby_console, :fixture_runtime, "normal")
    view |> element("#runtime-refresh") |> render_click()
    assert render_async(view) =~ "6.4%"
    Application.put_env(:labby_console, :fixture_runtime, "forbidden")
    view |> element("#runtime-refresh") |> render_click()
    assert_redirect(view, "/sign-in", 5000)
  end

  test "loading clears the snapshot and expiry redirects while a sample is pending", %{conn: conn} do
    Application.put_env(:labby_console, :fixture_runtime, "slow")
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    {:ok, view, html} = live(recycle(signed), "/runtime")
    assert html =~ "Sampling runtime resources"
    assert has_element?(view, "#runtime-refresh[disabled]")
    refute has_element?(view, "#runtime-metrics")
    send(view.pid, :session_expired)
    assert_redirect(view, "/sign-in")
  end
end
