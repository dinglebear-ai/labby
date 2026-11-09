defmodule LabbyConsoleWeb.GatewayFleetLiveTest do
  use LabbyConsoleWeb.ConnCase
  import Phoenix.LiveViewTest

  setup do
    state = start_supervised!({Agent, fn -> true end})

    server =
      start_supervised!(
        {Bandit,
         plug: {LabbyConsole.GatewayFleetFixture, state: state}, port: 0, ip: {127, 0, 0, 1}}
      )

    {:ok, {_, port}} = ThousandIsland.listener_info(server)
    old = Application.get_env(:labby_console, :gateway_url)
    Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:#{port}")

    on_exit(fn ->
      Application.put_env(:labby_console, :gateway_url, old)
      Application.delete_env(:labby_console, :fixture_inventory)
      Application.delete_env(:labby_console, :gateway_fleet_rows)
      Application.delete_env(:labby_console, :gateway_fleet_gate)
    end)

    :ok
  end

  test "fleet view controls, inspection and refresh reconciliation", %{conn: conn} do
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    {:ok, view, _} = live(recycle(signed), "/gateway")
    render_async(view)
    assert has_element?(view, ".gateway-fleet")
    view |> element("button[phx-value-layout=cards]") |> render_click()
    assert has_element?(view, "#gateway-inventory[data-layout=cards]")
    view |> element("button[aria-label^='Quick inspect']") |> render_click()
    assert has_element?(view, "#gateway-inspector", "fixture-server")
    assert has_element?(view, "#gateway-inspector", "Unknown")
    view |> form("#gateway-filters", %{lens: "disabled", source: "all"}) |> render_change()
    refute has_element?(view, "#server-list li")
    view |> form("#gateway-filters", %{lens: "all", source: "all"}) |> render_change()
    Application.put_env(:labby_console, :fixture_inventory, :empty)
    view |> element("#servers-refresh") |> render_click()
    render_async(view)
    refute has_element?(view, "#gateway-inspector")
    assert has_element?(view, "#servers-empty")
  end

  test "permission loss removes inventory and keeps login valid", %{conn: conn} do
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    {:ok, view, _} = live(recycle(signed), "/gateway")
    render_async(view)
    Application.put_env(:labby_console, :fixture_inventory, :forbidden)
    view |> element("#servers-refresh") |> render_click()
    render_async(view)
    assert has_element?(view, "#servers-forbidden")
    refute has_element?(view, "#server-list li")
    assert {:ok, _} = LabbyConsole.Auth.current(get_session(signed, :console_session))
  end

  test "old completion cannot replace the current snapshot", %{conn: conn} do
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    {:ok, view, _} = live(recycle(signed), "/gateway")
    render_async(view)
    send(view.pid, {:gateway_fleet_timeout, make_ref()})
    assert render(view) =~ "fixture-server"
    send(view.pid, :session_expired)
    assert_redirect(view, "/sign-in")
  end

  test "real wire observations and source families keep disabled and connected independent", %{
    conn: conn
  } do
    rows = [
      %{
        "id" => "custom:one",
        "name" => "shared",
        "display_name" => String.duplicate("Long name ", 40),
        "source" => "custom_gateway",
        "enabled" => false,
        "connected" => true,
        "capability_observation" => %{
          "tools" => %{"state" => "known", "discovered" => 0, "exposed" => 0},
          "resources" => %{"state" => "stale", "discovered" => 3, "exposed" => 2},
          "prompts" => %{"state" => "failed", "error" => "private-provider-error"}
        }
      },
      %{
        "id" => "virtual:two",
        "name" => "shared",
        "source" => "in_process",
        "enabled" => true,
        "connected" => false
      }
    ]

    Application.put_env(:labby_console, :gateway_fleet_rows, rows)
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    {:ok, view, _} = live(recycle(signed), "/gateway")
    html = render_async(view)
    assert has_element?(view, "#server-list li", "0 / 0")
    assert has_element?(view, "#server-list [data-observation=stale]", "2 / 3")
    assert has_element?(view, "#server-list [data-state=connected]", "Connected")
    assert has_element?(view, "#server-list [data-enabled=false]", "Disabled")
    refute html =~ "private-provider-error"
    view |> form("#gateway-filters", %{lens: "all", source: "in_process"}) |> render_change()
    assert length(view |> element("#server-list") |> render() |> String.split("<li")) == 2
    assert has_element?(view, "#server-list", "Lab")
  end

  test "revocation during an owned read redirects before late rows can publish", %{conn: conn} do
    Application.put_env(:labby_console, :gateway_fleet_rows, [
      %{"id" => "late", "name" => "late", "enabled" => true, "connected" => true}
    ])

    Application.put_env(:labby_console, :gateway_fleet_gate, self())
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    {:ok, view, _} = live(recycle(signed), "/gateway")
    assert_receive {:fleet_read, request}, 2_000
    assert has_element?(view, "#servers-loading")
    assert :ok = LabbyConsole.Auth.sign_out(get_session(signed, :console_session))
    assert_redirect(view, "/sign-in", 2_000)
    send(request, :release_fleet)
  end

  test "obsolete async completions are ignored" do
    socket = %Phoenix.LiveView.Socket{assigns: %{epoch: make_ref(), rows: [], __changed__: %{}}}

    assert {:noreply, ^socket} =
             LabbyConsoleWeb.GatewayLive.handle_async(
               {:servers, make_ref(), 1},
               {:ok, {:ok, [%{"id" => "stale"}]}},
               socket
             )
  end

  test "deadline cancels the owned read and late data stays absent", %{conn: conn} do
    Application.put_env(:labby_console, :gateway_fleet_rows, [
      %{"id" => "late", "name" => "late", "enabled" => true, "connected" => true}
    ])

    Application.put_env(:labby_console, :gateway_fleet_gate, self())
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    {:ok, view, _} = live(recycle(signed), "/gateway")
    assert_receive {:fleet_read, request}, 2_000
    epoch = :sys.get_state(view.pid).socket.assigns.epoch
    send(view.pid, {:gateway_fleet_timeout, epoch})
    assert has_element?(view, "#servers-error", "unavailable")
    refute has_element?(view, "#server-list li")
    send(request, :release_fleet)
    render_async(view)
    refute has_element?(view, "#server-list li")
  end

  test "inspector mount saves focus once while row switching only sends an event", %{conn: conn} do
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    {:ok, view, _} = live(recycle(signed), "/gateway")
    render_async(view)
    opener = "#server-list button[aria-label^='Quick inspect']"
    commands = inspect_commands(view, opener)
    assert Enum.map(commands, &hd/1) == ["push"]
    view |> element(opener) |> render_click()
    assert_inspector_mount_target(view)
    assert Enum.map(inspect_commands(view, opener), &hd/1) == ["push"]
    view |> element("button[aria-label='Close server inspector']") |> render_click()
    assert Enum.map(inspect_commands(view, opener), &hd/1) == ["push"]
  end

  test "cell labels survive hidden and reordered columns in every layout", %{conn: conn} do
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    {:ok, view, _} = live(recycle(signed), "/gateway?columns=skills,tools,source")
    render_async(view)

    for layout <- ~w(table cards list) do
      view |> element("button[phx-value-layout='#{layout}']") |> render_click()

      assert has_element?(
               view,
               "#server-list [data-column=skills] .gf-cell-label.sr-only",
               "Skills"
             )

      assert has_element?(
               view,
               "#server-list [data-column=tools] .gf-cell-label.sr-only",
               "Tools"
             )

      assert has_element?(
               view,
               "#server-list [data-column=source] .gf-cell-label.sr-only",
               "Source"
             )

      refute has_element?(view, "#server-list [data-column=resources]")
      assert has_element?(view, "#server-list .gf-mobile-label[aria-hidden=true]")
    end
  end

  test "retained refresh remount saves a currently rendered restoration target", %{conn: conn} do
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    {:ok, view, _} = live(recycle(signed), "/gateway")
    render_async(view)
    opener = "#server-list button[aria-label^='Quick inspect']"
    view |> element(opener) |> render_click()
    assert_inspector_mount_target(view)
    Application.put_env(:labby_console, :gateway_fleet_gate, self())

    Application.put_env(:labby_console, :gateway_fleet_rows, [
      %{
        "id" => "fixture-server",
        "name" => "fixture-server",
        "enabled" => true,
        "connected" => true
      }
    ])

    view |> element("#servers-refresh") |> render_click()
    assert_receive {:fleet_read, request}, 2_000
    refute has_element?(view, "#gateway-inspector")
    send(request, :release_fleet)
    render_async(view)
    assert_inspector_mount_target(view)
    view |> element("button[aria-label='Close server inspector']") |> render_click()
    refute has_element?(view, "#gateway-inspector")
  end

  defp assert_inspector_mount_target(view) do
    commands =
      view
      |> render()
      |> LazyHTML.from_fragment()
      |> LazyHTML.query("#gateway-inspector")
      |> LazyHTML.attribute("phx-mounted")
      |> hd()
      |> Jason.decode!()

    assert [["push_focus", %{"to" => target}], ["focus", %{"to" => "#gateway-inspector-title"}]] =
             commands

    assert has_element?(view, target)
  end

  defp inspect_commands(view, selector) do
    view
    |> render()
    |> LazyHTML.from_fragment()
    |> LazyHTML.query(selector)
    |> LazyHTML.attribute("phx-click")
    |> hd()
    |> Jason.decode!()
  end
end
