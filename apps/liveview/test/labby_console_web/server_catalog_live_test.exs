defmodule LabbyConsoleWeb.ServerCatalogLiveTest do
  use LabbyConsoleWeb.ConnCase
  import Phoenix.LiveViewTest

  test "anonymous catalog access requires sign-in", %{conn: conn} do
    assert redirected_to(get(conn, "/gateway/servers/fixture-server/catalog")) == "/sign-in"
  end

  test "shows retained catalog, clears stale rows on failure, and recovers by explicit retry", %{
    conn: conn
  } do
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
      Application.delete_env(:labby_console, :fixture_server_catalog)
    end)

    signed_in = post(conn, "/sign-in", %{credential: "fixture-only"})
    {:ok, view, html} = live(recycle(signed_in), "/gateway/servers/fixture-server/catalog")
    assert html =~ "Loading catalog"
    assert render_async(view, 5_000) =~ "Fetch a document"
    assert has_element?(view, "#catalog-resources", "file:///README.md")
    refute render(view) =~ "do not render"
    Application.put_env(:labby_console, :fixture_server_catalog, "malformed")
    view |> element("#catalog-retry") |> render_click()
    render_async(view, 5_000)
    assert has_element?(view, "#catalog-error")
    refute has_element?(view, "#catalog-results")
    Application.put_env(:labby_console, :fixture_server_catalog, "empty")
    view |> element("#catalog-retry") |> render_click()
    render_async(view, 5_000)
    assert has_element?(view, "#catalog-tools-empty")
    Agent.update(state, fn _ -> false end)
    view |> element("#catalog-retry") |> render_click()
    assert_redirect(view, "/sign-in", 5_000)
  end
end
