defmodule LabbyConsoleWeb.OverviewNativeLiveTest do
  use LabbyConsoleWeb.ConnCase
  import Phoenix.LiveViewTest

  setup do
    state = start_supervised!({Agent, fn -> true end})
    owner = self()

    data =
      start_supervised!(
        Supervisor.child_spec({Agent, fn -> %{owner: owner, mode: :normal} end},
          id: :overview_data
        )
      )

    server =
      start_supervised!(
        {Bandit,
         plug: {LabbyConsole.OverviewFixture, state: state, data_state: data},
         port: 0,
         ip: {127, 0, 0, 1}}
      )

    {:ok, {_, port}} = ThousandIsland.listener_info(server)
    old = Application.get_env(:labby_console, :gateway_url)
    Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:#{port}")
    on_exit(fn -> Application.put_env(:labby_console, :gateway_url, old) end)
    %{data: data}
  end

  defp open(conn) do
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    {:ok, view, _} = live(recycle(signed), "/overview")
    render_async(view, 5000)
    render_async(view, 5000)
    {signed, view}
  end

  test "native dashboard uses real rankings, 30-day bounds and accessible bucket details", %{
    conn: conn
  } do
    {_, view} = open(conn)
    assert has_element?(view, "#ov-rankings", "read_file")
    assert has_element?(view, "#overview-inventory")
    assert has_element?(view, "#ov-runtime")
    assert has_element?(view, "#ov-call-rows tr")
    refute render(view) =~ "never-render-this"
    view |> element("#ov-window-30d") |> render_click()
    render_async(view, 5000)
    render_async(view, 5000)
    assert has_element?(view, "#ov-window-30d[aria-pressed=true]")
    assert_receive {:overview_metrics_params, %{"since_unix" => first, "until_unix" => last}}
    # The first request is the initial 24h load; the switched request must use30d.
    assert last - first == 86400
    assert_receive {:overview_metrics_params, %{"since_unix" => first30, "until_unix" => last30}}
    assert last30 - first30 == 2_592_000
    view |> element("#ov-ranking-slowest") |> render_click()
    assert has_element?(view, "#ov-rankings", "Mean elapsed")
    view |> element("#ov-activity-callers") |> render_click()
    assert has_element?(view, "#ov-rankings", "verified-actor")
    view |> element("#ov-bucket-0") |> render_click()
    render_async(view, 5000)
    assert has_element?(view, "#ov-inspector[role=dialog]", "Time bucket")
    assert has_element?(view, "#ov-inspector", "Retained calls")
    render_click(view, "close_inspector", %{})
    refute has_element?(view, "#ov-inspector")
  end

  test "card editing persists only bounded layout and refuses cross-lane forged moves", %{
    conn: conn
  } do
    {_, view} = open(conn)
    view |> element("#ov-customize") |> render_click()
    assert has_element?(view, "#ov-card-grid[data-editing=true]")

    render_click(view, "overview_place", %{
      "source" => "calls",
      "target" => "traffic",
      "after" => false
    })

    assert_push_event(view, "overview-layout-state", %{layout: %{order: ["calls" | _]}})

    render_click(view, "overview_place", %{
      "source" => "calls",
      "target" => "runtime",
      "after" => false
    })

    assert has_element?(view, "#ov-layout-status", "Cards stay within their lane.")
    render_click(view, "overview_width", %{"id" => "traffic"})
    assert has_element?(view, "#ov-card-traffic[data-wide=true]")

    render_click(view, "overview_preferences", %{
      "order" => ["private", "calls"],
      "widths" => %{"traffic" => true},
      "credential" => "never-save"
    })

    refute render(view) =~ "never-save"
    assert has_element?(view, "#ov-card-traffic[data-wide=true]")
    render_click(view, "overview_reset", %{})
    assert has_element?(view, "#ov-card-traffic[data-wide=false]")
  end

  test "forbidden telemetry preserves valid login and clears previous protected summaries", %{
    conn: conn,
    data: data
  } do
    {signed, view} = open(conn)
    Agent.update(data, &%{&1 | mode: :forbidden})
    view |> element("#overview-refresh") |> render_click()
    render_async(view, 5000)
    assert has_element?(view, "#overview-usage-error")
    refute has_element?(view, "#overview-usage")
    assert {:ok, _} = LabbyConsole.Auth.current(get_session(signed, :console_session))
  end

  test "zero calls are not a perfect success rate or zero observed latency", %{
    conn: conn,
    data: data
  } do
    Agent.update(data, &%{&1 | mode: :empty})
    {_, view} = open(conn)
    assert has_element?(view, "#ov-success-value", "—")
    assert has_element?(view, "#ov-p95-value", "—")
    assert has_element?(view, "#ov-rankings", "No tool activity")
  end

  test "a superseded window cannot publish its delayed usage into the new selection", %{
    conn: conn,
    data: data
  } do
    Agent.update(data, &%{&1 | mode: :hold})
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    {:ok, view, _} = live(recycle(signed), "/overview")
    assert_receive {:overview_hold, request}, 2000
    Agent.update(data, &%{&1 | mode: :normal})
    view |> element("#ov-window-30d") |> render_click()
    render_async(view, 5000)
    render_async(view, 5000)
    assert has_element?(view, "#ov-window-30d[aria-pressed=true]")
    assert has_element?(view, "#ov-calls-value", "8")
    send(request, :release_overview)
    render_async(view, 5000)
    refute has_element?(view, "#ov-calls-value", "99")
  end

  test "forged leader indices do not query another scope and denied drawers retain caller authority",
       %{conn: conn, data: data} do
    {signed, view} = open(conn)
    render_click(view, "inspect_leader", %{"kind" => "tools", "index" => "999"})
    refute has_element?(view, "#ov-inspector")
    Agent.update(data, &%{&1 | mode: :forbidden})
    view |> element("#ov-rankings-open-0") |> render_click()
    render_async(view, 5000)
    assert has_element?(view, "#ov-inspector [role=alert]", "Scoped usage is unavailable")
    assert {:ok, _} = LabbyConsole.Auth.current(get_session(signed, :console_session))
  end
end
