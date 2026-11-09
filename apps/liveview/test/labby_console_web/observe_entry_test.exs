defmodule LabbyConsoleWeb.ObserveEntryTest do
  use LabbyConsoleWeb.ConnCase
  import Phoenix.LiveViewTest

  setup do
    state = start_supervised!({Agent, fn -> true end})

    server =
      start_supervised!(
        {Bandit,
         plug: {LabbyConsole.ObserveFixture, state: state, owner: self()},
         port: 0,
         ip: {127, 0, 0, 1}}
      )

    {:ok, {_, port}} = ThousandIsland.listener_info(server)
    old = Application.get_env(:labby_console, :gateway_url)
    Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:#{port}")
    on_exit(fn -> Application.put_env(:labby_console, :gateway_url, old) end)
    %{state: state}
  end

  test "normal route renders real authorized projection and clears it on permission loss", %{
    conn: conn,
    state: state
  } do
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})

    row = %{
      id: 1,
      ts_unix: System.system_time(:second),
      elapsed_ms: 7,
      upstream: "owned",
      tool: "actual_read",
      capability: "tools",
      operation: "call",
      actor: "secret-actor",
      subject_scoped: false,
      outcome: "ok"
    }

    Agent.update(state, fn _ -> {:response, 200, %{calls: [row]}} end)
    {:ok, view, _} = live(recycle(signed), "/observe")
    render_async(view)
    assert has_element?(view, "#observe-row-1", "actual_read")
    assert has_element?(view, "#observe-row-1", "Unknown caller")
    refute render(view) =~ "secret-actor"
    view |> element("#observe-inspect-1") |> render_click()
    assert has_element?(view, "#observe-inspector", "actual_read")
    view |> element("#observe-close") |> render_click()
    refute has_element?(view, "#observe-inspector")
    Agent.update(state, fn _ -> {:response, 403, %{}} end)
    view |> element("button[phx-click=refresh]") |> render_click()
    render_async(view)
    assert has_element?(view, "#observe-error", "Permission denied")
    refute has_element?(view, "#observe-row-1")
    assert {:ok, _} = LabbyConsole.Auth.current(get_session(signed, :console_session))
  end

  test "normal route log filters and pause retain actual bounded snapshot", %{
    conn: conn,
    state: state
  } do
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    Agent.update(state, fn _ -> {:response, 200, %{calls: []}} end)
    {:ok, view, _} = live(recycle(signed), "/observe")
    render_async(view)

    Agent.update(state, fn _ ->
      {:response, 200,
       %{kind: "server_logs", entries: [%{message: "actual log", level: "INFO"}], truncated: true}}
    end)

    view |> element("#observe-tab-logs") |> render_click()
    render_async(view)
    assert has_element?(view, ".ob-logs", "actual log")
    view |> element("#observe-resume") |> render_click()
    render_async(view)
    timer = :sys.get_state(view.pid).socket.assigns.timer
    assert Process.read_timer(timer) in 4000..5000
    view |> element("#observe-pause") |> render_click()
    assert Process.read_timer(timer) == false
    assert render(view) =~ "Follow paused"
    assert has_element?(view, ".ob-logs li", "actual log")
    Agent.update(state, fn _ -> {:response, 503, %{}} end)
    view |> element("button[phx-click=refresh]") |> render_click()
    render_async(view)
    assert has_element?(view, ".ob-logs li", "actual log")
    assert has_element?(view, "#observe-error", "unknown freshness")
  end

  test "normal Observe outage retry preserves the destination and valid login", %{
    conn: conn,
    state: state
  } do
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    id = get_session(signed, :console_session)
    previous = Application.get_env(:labby_console, :fixture_auth_session)
    on_exit(fn -> Application.put_env(:labby_console, :fixture_auth_session, previous) end)
    Application.put_env(:labby_console, :fixture_auth_session, :unavailable)

    outage = get(recycle(signed), "/observe")
    assert redirected_to(outage) == "/connection?return_to=%2Fobserve"
    assert LabbyConsole.Auth.capture(id)
    {:ok, retry, _} = live(recycle(signed), redirected_to(outage))
    assert has_element?(retry, "#connection-retry[href='/observe']", "Retry connection")

    Application.put_env(:labby_console, :fixture_auth_session, previous)
    Agent.update(state, fn _ -> {:response, 200, %{calls: []}} end)
    {:ok, recovered, _} = live(recycle(signed), "/observe")
    render_async(recovered)
    assert has_element?(recovered, "#observe-main")
    assert {:ok, _} = LabbyConsole.Auth.current(id)
  end
end
