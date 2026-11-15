defmodule LabbyConsoleWeb.AuthFlowTest do
  use LabbyConsoleWeb.ConnCase
  import Phoenix.LiveViewTest

  test "failed sign-in retains the themed retry form without reflecting the credential", %{
    conn: conn
  } do
    response = conn |> post("/sign-in", %{credential: ""}) |> html_response(401)
    assert response =~ "console-auth"
    assert response =~ "id=\"credential\""
    assert response =~ "Sign-in failed."
    assert response =~ "type=\"password\""
  end

  test "public sign-in has Labby identity, dedicated styles and a labelled credential form", %{
    conn: conn
  } do
    previous = Application.get_env(:labby_console, :gateway_url)
    Application.delete_env(:labby_console, :gateway_url)
    on_exit(fn -> Application.put_env(:labby_console, :gateway_url, previous) end)
    page = conn |> get("/sign-in") |> html_response(200)
    assert page =~ ~s(href="/assets/css/auth.css")
    assert page =~ ~s(data-labby-mark="fold")
    assert page =~ ~s(aria-labelledby="sign-in-title")
    assert page =~ ~s(aria-describedby="credential-help")
    assert page =~ ~s(id="credential-help")
    assert page =~ ~s(name="_csrf_token")
    refute page =~ "data-provider-login"
    refute page =~ ~s(name="credential" value=)
  end

  test "failed credential sign-in keeps the branded form and an associated safe error", %{
    conn: conn
  } do
    page =
      conn |> post("/sign-in", %{credential: "private-invalid-credential"}) |> html_response(401)

    assert page =~ ~s(data-labby-mark="fold")
    assert page =~ ~s(id="sign-in-error" role="alert")
    assert page =~ ~s(aria-describedby="credential-help sign-in-error")
    refute page =~ "private-invalid-credential"
  end

  test "protected gateway redirects an anonymous caller", %{conn: conn} do
    assert redirected_to(get(conn, "/gateway")) == "/sign-in"
  end

  test "sign-in is isolated, expiry blocks access, and sign-out revokes the gateway session", %{
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
    on_exit(fn -> Application.put_env(:labby_console, :gateway_url, old) end)
    signed_in = post(conn, "/sign-in", %{credential: "fixture-only"})
    assert redirected_to(signed_in) == "/gateway"
    on_exit(fn -> Application.delete_env(:labby_console, :fixture_inventory) end)
    id = get_session(signed_in, :console_session)
    assert {:ok, [%{"name" => "fixture-server"}]} = LabbyConsole.Auth.servers(id)
    assert {:error, :unauthenticated} = LabbyConsole.Auth.servers("unknown-caller")

    for mode <- [:malformed, :invalid_rows, :oversized] do
      Application.put_env(:labby_console, :fixture_inventory, mode)
      assert {:error, :unavailable} = LabbyConsole.Auth.servers(id)
    end

    Application.put_env(:labby_console, :fixture_inventory, :empty)
    assert {:ok, []} = LabbyConsole.Auth.servers(id)
    Application.delete_env(:labby_console, :fixture_inventory)
    authenticated = signed_in |> recycle() |> get("/gateway")
    assert html_response(authenticated, 200) =~ "fixture-user"
    {:ok, view, _} = live(recycle(signed_in), "/gateway")
    assert render_async(view, 5_000) =~ "fixture-server"
    view |> form("#servers-search", %{query: "missing-server"}) |> render_change()
    refute has_element?(view, "#server-list li")
    assert has_element?(view, "#servers-no-matches", "No servers match")
    view |> form("#servers-search", %{query: "fixture"}) |> render_change()
    assert has_element?(view, "#server-list li")
    Agent.update(state, fn _ -> false end)
    view |> element("#servers-refresh") |> render_click()
    assert_redirect(view, "/sign-in", 5_000)
    Agent.update(state, fn _ -> true end)
    assert redirected_to(get(build_conn(), "/gateway")) == "/sign-in"
    fresh = post(build_conn(), "/sign-in", %{credential: "fixture-only"})
    signed_out = fresh |> recycle() |> post("/sign-out")
    assert redirected_to(signed_out) == "/sign-in"
    assert Agent.get(state, & &1) == false
    assert redirected_to(signed_out |> recycle() |> get("/gateway")) == "/sign-in"
    Agent.update(state, fn _ -> true end)
    second = post(build_conn(), "/sign-in", %{credential: "fixture-only"})
    Agent.update(state, fn _ -> false end)
    assert redirected_to(second |> recycle() |> get("/gateway")) == "/sign-in"
  end

  test "transient session introspection failure denies reads without losing the session" do
    {state, _server} = auth_gateway()
    assert {:ok, id} = LabbyConsole.Auth.sign_in("fixture-only")
    Application.put_env(:labby_console, :fixture_auth_session, :unavailable)
    assert {:error, :unavailable} = LabbyConsole.Auth.current(id)
    assert {:error, :unavailable} = LabbyConsole.Auth.servers(id)

    assert {:error, :unavailable} =
             LabbyConsole.Auth.read_action(id, "gateway", "gateway.clients.list", %{})

    Application.delete_env(:labby_console, :fixture_auth_session)
    assert {:ok, %{"user" => %{"sub" => "fixture-user"}}} = LabbyConsole.Auth.current(id)
    assert {:ok, [_]} = LabbyConsole.Auth.servers(id)
    Agent.update(state, fn _ -> false end)
    assert {:error, :unauthenticated} = LabbyConsole.Auth.current(id)
    Agent.update(state, fn _ -> true end)
    assert {:error, :unauthenticated} = LabbyConsole.Auth.current(id)
  end

  test "transport failure does not discard the session" do
    {state, server} = auth_gateway()
    {:ok, {_, port}} = ThousandIsland.listener_info(server)
    assert {:ok, id} = LabbyConsole.Auth.sign_in("fixture-only")
    :ok = stop_supervised(:auth_gateway)
    assert {:error, :unavailable} = LabbyConsole.Auth.current(id)

    start_supervised!(
      {Bandit, plug: {LabbyConsole.AuthFixture, state: state}, port: port, ip: {127, 0, 0, 1}}
    )

    assert {:ok, _} = LabbyConsole.Auth.current(id)
  end

  test "expired sessions are discarded permanently" do
    auth_gateway()
    assert {:ok, id} = LabbyConsole.Auth.sign_in("fixture-only")
    Application.put_env(:labby_console, :fixture_auth_session, :expired)
    assert {:error, :unauthenticated} = LabbyConsole.Auth.current(id)
    Application.delete_env(:labby_console, :fixture_auth_session)
    assert {:error, :unauthenticated} = LabbyConsole.Auth.current(id)
  end

  test "failed remote logout leaves a usable CSRF-protected sign-in flow" do
    auth_gateway()
    page = csrf_conn() |> get("/sign-in")

    signed_in =
      page
      |> recycle()
      |> put_private(:plug_skip_csrf_protection, false)
      |> post("/sign-in", %{credential: "fixture-only", _csrf_token: csrf_token(page)})

    assert redirected_to(signed_in) == "/gateway"
    id = get_session(signed_in, :console_session)

    gateway =
      signed_in |> recycle() |> put_private(:plug_skip_csrf_protection, false) |> get("/sign-in")

    Application.put_env(:labby_console, :fixture_logout, :unavailable)

    signed_out =
      gateway
      |> recycle()
      |> put_private(:plug_skip_csrf_protection, false)
      |> post("/sign-out", %{_csrf_token: csrf_token(gateway)})

    assert {:error, :unauthenticated} = LabbyConsole.Auth.current(id)

    retry =
      if signed_out.status == 302 do
        assert redirected_to(signed_out) == "/sign-in"

        signed_out
        |> recycle()
        |> put_private(:plug_skip_csrf_protection, false)
        |> get("/sign-in")
      else
        signed_out
      end

    assert html_response(retry, 200) =~ "Gateway revocation was not confirmed"

    retried =
      retry
      |> recycle()
      |> put_private(:plug_skip_csrf_protection, false)
      |> post("/sign-in", %{credential: "fixture-only", _csrf_token: csrf_token(retry)})

    assert redirected_to(retried) == "/gateway"
  end

  defp csrf_conn, do: build_conn() |> put_private(:plug_skip_csrf_protection, false)

  defp csrf_token(conn) do
    [_, token] = Regex.run(~r/name="_csrf_token"[^>]*value="([^"]+)"/, conn.resp_body)
    token
  end

  defp auth_gateway do
    state = start_supervised!({Agent, fn -> true end})

    server =
      start_supervised!(
        Supervisor.child_spec(
          {Bandit, plug: {LabbyConsole.AuthFixture, state: state}, port: 0, ip: {127, 0, 0, 1}},
          id: :auth_gateway
        )
      )

    {:ok, {_, port}} = ThousandIsland.listener_info(server)
    old = Application.get_env(:labby_console, :gateway_url)
    Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:#{port}")

    on_exit(fn ->
      Application.put_env(:labby_console, :gateway_url, old)
      Application.delete_env(:labby_console, :fixture_auth_session)
      Application.delete_env(:labby_console, :fixture_logout)
    end)

    {state, server}
  end
end
