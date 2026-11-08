defmodule LabbyConsoleWeb.SessionContextFixture do
  use Plug.Router
  plug :match
  plug :dispatch

  post "/auth/bearer-session" do
    ["Bearer " <> user] = get_req_header(conn, "authorization")

    conn
    |> put_resp_header("set-cookie", "context_session=#{user}; HttpOnly")
    |> send_resp(200, "{}")
  end

  get "/auth/session" do
    ["context_session=" <> user] = get_req_header(conn, "cookie")

    send_resp(
      conn,
      200,
      Jason.encode!(%{
        authenticated: true,
        expires_at: System.system_time(:second) + 60,
        csrf_token: "context-fixture-csrf",
        user: %{sub: "same-external-subject"},
        authority_state: "ready",
        authority: %{principal_id: user, organization_id: "fixture-org", authority_generation: 1},
        capabilities: ["platform.manage"],
        is_admin: true
      })
    )
  end
end

defmodule LabbyConsoleWeb.SessionHarnessLive do
  use LabbyConsoleWeb, :live_view
  on_mount {LabbyConsoleWeb.LiveSession, :authenticated}

  def mount(_, _, socket), do: {:ok, assign(socket, private_rows: ["protected-row"])}

  def render(assigns) do
    ~H"""
    <main>
      <span id="session-subject">{@subject}</span>
      <span id="session-namespace">{@preference_key}</span>
      <p :for={row <- @private_rows}>{row}</p>
    </main>
    """
  end
end

defmodule LabbyConsoleWeb.LiveSessionTest do
  use LabbyConsoleWeb.ConnCase
  import Phoenix.LiveViewTest
  alias LabbyConsole.Auth
  alias LabbyConsoleWeb.LiveSession

  setup do
    state = start_supervised!({Agent, fn -> true end})

    server =
      start_supervised!(
        {Bandit, plug: {LabbyConsole.AuthFixture, state: state}, port: 0, ip: {127, 0, 0, 1}}
      )

    {:ok, {_, port}} = ThousandIsland.listener_info(server)
    previous = Application.get_env(:labby_console, :gateway_url)
    Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:#{port}")
    on_exit(fn -> Application.put_env(:labby_console, :gateway_url, previous) end)
    assert {:ok, id} = Auth.sign_in("fixture-only")
    %{session_id: id}
  end

  test "shared mount rejects an unknown caller", %{conn: conn} do
    assert {:error, {:redirect, %{to: "/sign-in"}}} =
             live_isolated(conn, LabbyConsoleWeb.SessionHarnessLive,
               session: %{"console_session" => "unknown"}
             )
  end

  test "mount exposes verified identity and a stable namespace without gateway secrets", %{
    conn: conn,
    session_id: id
  } do
    {:ok, view, html} =
      live_isolated(conn, LabbyConsoleWeb.SessionHarnessLive, session: %{"console_session" => id})

    assert has_element?(view, "#session-subject", "fixture-user")
    [_, namespace] = Regex.run(~r/id="session-namespace">([a-f0-9]{64})</, html)
    assert {:ok, context} = LiveSession.context(id)
    assert context.preference_key == namespace
    refute Map.has_key?(context.current_scope, "csrf_token")
    refute inspect(context) =~ "fixture-csrf"
    refute inspect(context) =~ "fixture_session=one"
    refute html =~ "fixture-csrf"

    assert {:ok, another_id} = Auth.sign_in("fixture-only")
    assert {:ok, another} = LiveSession.context(another_id)
    refute another.preference_key == namespace
  end

  test "preference namespace uses durable principal and organization, not ambiguous external subjects" do
    server =
      start_supervised!(
        Supervisor.child_spec(
          {Bandit, plug: LabbyConsoleWeb.SessionContextFixture, port: 0, ip: {127, 0, 0, 1}},
          id: :context_gateway
        )
      )

    {:ok, {_, port}} = ThousandIsland.listener_info(server)
    Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:#{port}")
    assert {:ok, alpha} = Auth.sign_in("alpha")
    assert {:ok, again} = Auth.sign_in("alpha")
    assert {:ok, beta} = Auth.sign_in("beta")
    assert {:ok, a} = LiveSession.context(alpha)
    assert {:ok, repeated} = LiveSession.context(again)
    assert {:ok, b} = LiveSession.context(beta)
    assert a.preference_key == repeated.preference_key
    refute a.preference_key == b.preference_key
    assert a.current_scope["authority"]["principal_id"] == "alpha"
    refute Map.has_key?(a.current_scope, "csrf_token")
  end

  test "sign-out revokes an already connected view", %{conn: conn, session_id: id} do
    {:ok, view, _} =
      live_isolated(conn, LabbyConsoleWeb.SessionHarnessLive, session: %{"console_session" => id})

    assert render(view) =~ "protected-row"
    assert :ok = Auth.sign_out(id)
    assert_redirect(view, "/sign-in", 1_000)
  end

  test "another session's revocation does not redirect this caller", %{
    conn: conn,
    session_id: id
  } do
    {:ok, view, _} =
      live_isolated(conn, LabbyConsoleWeb.SessionHarnessLive, session: %{"console_session" => id})

    Phoenix.PubSub.broadcast(
      LabbyConsole.PubSub,
      "console-session:another-session",
      {:console_session_revoked, "another-session"}
    )

    assert render(view) =~ "protected-row"
    assert has_element?(view, "#session-subject", "fixture-user")
  end

  test "every protected read page uses the shared revocation and caller namespace" do
    for path <- [
          "/overview",
          "/gateway",
          "/runtime",
          "/clients",
          "/diagnostics",
          "/usage",
          "/activity",
          "/logs",
          "/gateway/servers/demo",
          "/gateway/servers/demo/catalog",
          "/gateway/upstreams/demo/runtime"
        ] do
      assert {:ok, id} = Auth.sign_in("fixture-only")
      conn = build_conn() |> Plug.Test.init_test_session(%{console_session: id})
      {:ok, view, html} = live(conn, path)
      assert html =~ ~r/data-preference-key="[a-f0-9]{64}"/, path
      assert html =~ "fixture-user", path
      assert :ok = Auth.sign_out(id)
      assert_redirect(view, "/sign-in", 1_000)
    end
  end

  test "scope replacement fences an older completion", %{session_id: id} do
    assert {:ok, context} = LiveSession.context(id)

    socket =
      %Phoenix.LiveView.Socket{}
      |> Phoenix.Component.assign(context)
      |> Phoenix.Component.assign(workspace_scope: %{}, scope_generation: 0, pending_actions: %{})

    request = make_ref()
    tracked = LiveSession.track(socket, :projects, request)
    assert LiveSession.current?(tracked, :projects, request)
    replaced = LiveSession.set_scope(tracked, %{team_id: "another-team"})
    refute LiveSession.current?(replaced, :projects, request)
    assert replaced.assigns.workspace_scope == %{team_id: "another-team"}
    refute LiveSession.current?(tracked, :projects, make_ref())
  end

  test "temporary introspection outage routes to retry without discarding the session", %{
    conn: conn,
    session_id: id
  } do
    previous = Application.get_env(:labby_console, :fixture_auth_session)
    Application.put_env(:labby_console, :fixture_auth_session, :unavailable)
    on_exit(fn -> Application.put_env(:labby_console, :fixture_auth_session, previous) end)

    assert {:error, {:redirect, %{to: "/connection?return_to=%2Foverview"}}} =
             live_isolated(conn, LabbyConsoleWeb.SessionHarnessLive,
               session: %{"console_session" => id}
             )

    assert Auth.capture(id)
  end

  test "connection retry uses only an implemented local destination", %{conn: conn} do
    {:ok, view, html} = live(conn, "/connection?return_to=https%3A%2F%2Fexample.test")
    assert has_element?(view, "#connection-retry[href='/overview']", "Retry connection")
    refute html =~ "https://example.test"

    {:ok, local, _} = live(build_conn(), "/connection?return_to=%2Fprojects")
    assert has_element?(local, "#connection-retry[href='/projects']")
  end
end
