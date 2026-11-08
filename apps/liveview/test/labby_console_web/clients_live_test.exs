defmodule LabbyConsoleWeb.ClientsLiveTest do
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
      Application.delete_env(:labby_console, :fixture_clients)
      Application.delete_env(:labby_console, :fixture_clients_gate)
    end)

    %{state: state}
  end

  test "observed clients load and failed refresh removes old history before recovery", %{
    conn: conn
  } do
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    {:ok, view, _} = live(recycle(signed), "/clients")
    assert render_async(view) =~ "42 observations"
    assert has_element?(view, "#clients-list", "Codex")
    assert render(view) =~ "Observation history"
    Application.put_env(:labby_console, :fixture_clients, "invalid")
    view |> element("#clients-refresh") |> render_click()
    assert render_async(view) =~ "Client observations are unavailable"
    refute has_element?(view, "#clients-list li")
    Application.put_env(:labby_console, :fixture_clients, "empty")
    view |> element("#clients-refresh") |> render_click()
    assert render_async(view) =~ "No client observations"
    Application.put_env(:labby_console, :fixture_clients, "normal")
    view |> element("#clients-refresh") |> render_click()
    assert render_async(view) =~ "42 observations"
    Application.put_env(:labby_console, :fixture_clients, "forbidden")
    view |> element("#clients-refresh") |> render_click()
    assert_redirect(view, "/sign-in", 5_000)
  end

  test "pending load is visible and session expiry redirects without exposing late results", %{
    conn: conn
  } do
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    Application.put_env(:labby_console, :fixture_clients_gate, self())
    {:ok, view, _} = live(recycle(signed), "/clients")
    assert_receive {:clients_request, request}, 1_000
    assert has_element?(view, "[role=status]", "Loading client observations")
    assert has_element?(view, "#clients-refresh[disabled]")
    refute has_element?(view, "#clients-list li")
    send(view.pid, :session_expired)
    assert_redirect(view, "/sign-in", 5_000)
    send(request, :release_clients)
  end

  test "anonymous browser cannot mount observed client history", %{conn: conn} do
    assert {:error, {:redirect, %{to: "/sign-in"}}} = live(conn, "/clients")
  end
end
