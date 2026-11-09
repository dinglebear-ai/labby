defmodule LabbyConsoleWeb.RuntimeNavigationTest do
  use LabbyConsoleWeb.ConnCase
  import Phoenix.LiveViewTest
  alias LabbyConsole.Observe
  alias LabbyConsoleWeb.ServerRuntimeLive

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
    previous = Application.get_env(:labby_console, :gateway_url)
    Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:#{port}")
    on_exit(fn -> Application.put_env(:labby_console, :gateway_url, previous) end)
    %{state: state}
  end

  test "dot-only related links preserve identifiers through actual query route", %{
    conn: conn,
    state: state
  } do
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    Agent.update(state, fn _ -> {:response, 200, []} end)

    for name <- [".", ".."] do
      path = Observe.server_link(%{"upstream" => name})
      assert path == "/gateway/upstream-runtime?name=" <> name
      {:ok, view, _} = live(recycle(signed), path)
      render_async(view)
      assert has_element?(view, "#server-runtime-empty")

      assert_receive {:observe_wire,
                      %{"action" => "gateway.mcp.list", "params" => %{"name" => ^name}},
                      ["fixture_session=one"]}
    end
  end

  test "query alias has the same authentication boundary", %{conn: conn} do
    assert redirected_to(get(conn, "/gateway/upstream-runtime?name=..")) =~ "/sign-in"
  end

  test "ordinary existing runtime path remains compatible including maximum supported name", %{
    conn: conn,
    state: state
  } do
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})
    Agent.update(state, fn _ -> {:response, 200, []} end)

    for name <- ["existing-Name_1.2", String.duplicate("a", 128)] do
      path = Observe.server_link(%{"upstream" => name})
      assert path == "/gateway/upstreams/" <> name <> "/runtime"
      {:ok, view, _} = live(recycle(signed), path)
      render_async(view)
      assert has_element?(view, "#server-runtime-empty")
      assert_receive {:observe_wire, %{"params" => %{"name" => ^name}}, _}
    end
  end

  test "missing and malformed routed names redirect before any runtime request", %{conn: conn} do
    signed = post(conn, "/sign-in", %{credential: "fixture-only"})

    for path <- [
          "/gateway/upstream-runtime",
          "/gateway/upstream-runtime?name=",
          "/gateway/upstream-runtime?name[]=x",
          "/gateway/upstream-runtime?name=unsafe%2Fname",
          "/gateway/upstream-runtime?name=" <> String.duplicate("a", 129)
        ] do
      assert {:error, {:redirect, %{to: "/gateway"}}} = live(recycle(signed), path)
      refute_receive {:observe_wire, _, _}, 10
    end
  end

  test "direct mount rejects nonbinary and invalid UTF-8 values without retaining them" do
    for params <- [%{}, %{"name" => nil}, %{"name" => 1}, %{"name" => %{}}, %{"name" => <<255>>}] do
      assert {:ok, socket} = ServerRuntimeLive.mount(params, %{}, %Phoenix.LiveView.Socket{})
      assert socket.redirected == {:redirect, %{status: 302, to: "/gateway"}}
      refute Map.has_key?(socket.assigns, :server_id)
      refute_receive {:observe_wire, _, _}, 10
    end
  end
end
