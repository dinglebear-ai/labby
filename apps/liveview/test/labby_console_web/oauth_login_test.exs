defmodule LabbyConsoleWeb.OAuthLoginTest do
  use LabbyConsoleWeb.ConnCase, async: false
  alias LabbyConsoleWeb.SessionController
  alias LabbyConsole.OAuthHandoff

  setup do
    owner = self()

    state =
      start_supervised!({Agent, fn -> %{owner: owner, modes: %{}, origin: "", ready: false} end})

    server =
      start_supervised!(
        {Bandit,
         plug: {LabbyConsole.OAuthHandoffFixture, state: state}, port: 0, ip: {127, 0, 0, 1}}
      )

    {:ok, {_, port}} = ThousandIsland.listener_info(server)
    origin = "http://127.0.0.1:#{port}"
    Agent.update(state, &%{&1 | origin: origin})
    old = Application.get_env(:labby_console, :gateway_url)
    Application.put_env(:labby_console, :gateway_url, origin)

    on_exit(fn ->
      for {id, flow} <- :sys.get_state(OAuthHandoff),
          do: LabbyConsole.Auth.oauth_cancel(flow.owner, id)

      for {id, value} <- :ets.tab2list(LabbyConsole.Auth),
          value.session["user"]["sub"] == "provider-user",
          do: LabbyConsole.Auth.sign_out(id)

      Application.put_env(:labby_console, :gateway_url, old)
    end)

    %{fixture: state}
  end

  test "provider routes enforce opaque input and normal browser routing", %{conn: conn} do
    for path <- ["/sign-in/provider/start", "/sign-in/provider/poll", "/sign-in/provider/cancel"] do
      response = conn |> post(path, %{target: "forged"}) |> json_response(400)
      assert response["error"] == "provider_sign_in_failed"
    end
  end

  test "all provider POST routes enforce CSRF", %{conn: conn} do
    for path <- ["/sign-in/provider/start", "/sign-in/provider/poll", "/sign-in/provider/cancel"] do
      assert_raise Plug.CSRFProtection.InvalidCSRFTokenError, fn ->
        conn |> Plug.Conn.put_private(:plug_skip_csrf_protection, false) |> post(path, %{})
      end
    end
  end

  test "sign-in page creates a stable signed browser owner", %{conn: conn} do
    first = get(conn, "/sign-in")
    owner = get_session(first, :oauth_owner)
    assert is_binary(owner) and byte_size(owner) >= 32
    second = first |> recycle() |> get("/sign-in")
    assert get_session(second, :oauth_owner) == owner
  end

  test "provider choice has accessible link and bearer remains explicit", %{conn: conn} do
    page = conn |> get("/sign-in") |> html_response(200)
    assert page =~ "data-provider-login"
    assert page =~ "Sign in with provider"
    assert page =~ "Gateway credential"
    assert page =~ ~s(rel="noopener noreferrer")
  end

  test "controller stores signed owner and returns only public flow fields", %{conn: conn} do
    conn = conn |> init_test_session(%{}) |> SessionController.provider_start(%{})
    response = json_response(conn, 200)
    assert Enum.sort(Map.keys(response)) == ~w(authorization_url expires_at flow_id)
    assert is_binary(get_session(conn, :oauth_owner))
    refute conn.resp_body =~ "private-poll"
    refute conn.resp_body =~ "private-redeem"
    owner = get_session(conn, :oauth_owner)

    forged =
      build_conn()
      |> init_test_session(%{oauth_owner: "forged-owner-aaaaaaaa"})
      |> SessionController.provider_poll(%{"flow_id" => response["flow_id"]})

    assert json_response(forged, 400)["error"] == "provider_sign_in_failed"

    valid =
      build_conn()
      |> init_test_session(%{oauth_owner: owner, oauth_flow_id: response["flow_id"]})
      |> SessionController.provider_cancel(%{"flow_id" => response["flow_id"]})

    assert json_response(valid, 200)["status"] == "cancelled"
  end

  test "normal route completes using renewed vault session and rejects forged fields", %{
    conn: conn,
    fixture: fixture
  } do
    started = conn |> post("/sign-in/provider/start", %{})
    flow = json_response(started, 200)["flow_id"]
    owner = get_session(started, :oauth_owner)
    Agent.update(fixture, &%{&1 | ready: true})

    :sys.replace_state(OAuthHandoff, fn state ->
      put_in(state, [flow, :next_poll], System.monotonic_time(:millisecond) - 1)
    end)

    finished = recycle(started) |> post("/sign-in/provider/poll", %{flow_id: flow})
    assert json_response(finished, 200) == %{"status" => "complete"}
    sid = get_session(finished, :console_session)
    assert is_binary(sid)
    assert is_nil(get_session(finished, :oauth_owner))
    assert get_session(finished, :console_session) != owner
    assert LabbyConsole.Auth.capture(sid).session["user"]["sub"] == "provider-user"
    assert finished.resp_body =~ "complete"
    refute finished.resp_body =~ "private-cookie"
    body_lost = recycle(finished) |> post("/sign-in/provider/poll", %{flow_id: flow})
    assert json_response(body_lost, 200) == %{"status" => "complete"}
    assert get_session(body_lost, :console_session) == sid
    whole_response_lost = recycle(started) |> post("/sign-in/provider/poll", %{flow_id: flow})
    assert json_response(whole_response_lost, 200) == %{"status" => "complete"}
    assert get_session(whole_response_lost, :console_session) == sid

    :sys.replace_state(OAuthHandoff, fn state ->
      put_in(state, [flow, :deadline], System.monotonic_time(:millisecond) - 1)
    end)

    send(OAuthHandoff, :expire)
    reset = recycle(started) |> post("/sign-in/provider/cancel", %{flow_id: flow})
    assert json_response(reset, 200)["status"] == "cancelled"
    assert is_nil(get_session(reset, :oauth_owner))
    assert is_nil(LabbyConsole.Auth.capture(sid))
    restarted = recycle(reset) |> post("/sign-in/provider/start", %{})
    assert json_response(restarted, 200)["flow_id"] != flow
  end

  test "reset reports failed remote revocation and can recover its own lost response", %{
    conn: conn,
    fixture: fixture
  } do
    started = post(conn, "/sign-in/provider/start", %{})
    flow = json_response(started, 200)["flow_id"]
    owner = get_session(started, :oauth_owner)

    Agent.update(
      fixture,
      &%{&1 | ready: true, modes: %{"/auth/logout" => {:response, 503, "{}"}}}
    )

    :sys.replace_state(OAuthHandoff, fn state ->
      put_in(state, [flow, :next_poll], System.monotonic_time(:millisecond) - 1)
    end)

    {:ok, sid} = LabbyConsole.Auth.oauth_poll(owner, flow)
    reset = recycle(started) |> post("/sign-in/provider/cancel", %{flow_id: flow})

    assert json_response(reset, 200) == %{
             "status" => "cancelled",
             "revocation_unconfirmed" => true
           }

    assert is_nil(LabbyConsole.Auth.capture(sid))
    recovered = recycle(reset) |> post("/sign-in/provider/cancel", %{flow_id: flow})
    assert json_response(recovered, 200)["status"] == "cancelled"
  end

  test "existing authenticated browser cannot start provider replacement", %{conn: conn} do
    conn =
      conn
      |> init_test_session(%{console_session: "existing"})
      |> SessionController.provider_start(%{})

    assert json_response(conn, 409)["error"] == "sign_out_required"
  end
end
