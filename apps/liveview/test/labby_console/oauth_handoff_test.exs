defmodule LabbyConsole.OAuthHandoffTest do
  use ExUnit.Case, async: false
  alias LabbyConsole.{Auth, OAuthHandoff}

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

    %{fixture: state, origin: origin}
  end

  test "exact protocol, secret boundary, owner binding and retained receipt", %{
    fixture: fixture,
    origin: origin
  } do
    owner = "signed-owner-aaaaaaaa"
    assert {:ok, flow} = Auth.oauth_start(owner)
    assert Map.keys(flow) |> Enum.sort() == [:authorization_url, :expires_at, :flow_id]
    assert_receive {:handoff_request, "/auth/desktop/start", headers, raw}
    assert {"origin", origin} in headers
    assert {"host", URI.parse(origin).authority} in headers
    assert byte_size(Jason.decode!(raw)["code_challenge"]) == 43
    assert {:error, :invalid_flow} = Auth.oauth_poll("signed-owner-bbbbbbbb", flow.flow_id)
    assert {:error, :invalid_flow} = Auth.oauth_cancel("signed-owner-bbbbbbbb", flow.flow_id)
    assert {:pending, _} = Auth.oauth_poll(owner, flow.flow_id)
    refute_receive {:handoff_request, "/auth/desktop/poll", _, _}, 20
    due(flow.flow_id)
    assert {:pending, 5_000} = Auth.oauth_poll(owner, flow.flow_id)
    assert_receive {:handoff_request, "/auth/desktop/poll", _, _}
    Agent.update(fixture, &%{&1 | ready: true})
    due(flow.flow_id)
    assert {:ok, sid} = Auth.oauth_poll(owner, flow.flow_id)
    assert Auth.capture(sid).session["user"]["sub"] == "provider-user"
    assert {:error, :invalid_flow} = Auth.oauth_cancel("signed-owner-bbbbbbbb", flow.flow_id)
    assert Auth.capture(sid)
    assert {:ok, ^sid} = Auth.oauth_poll(owner, flow.flow_id)
    assert_receive {:handoff_request, "/auth/desktop/redeem", _, _}
    refute_receive {:handoff_request, "/auth/desktop/redeem", _, _}, 20
    receipt = :sys.get_state(OAuthHandoff)[flow.flow_id]
    assert :ets.tab2list(OAuthHandoff) == []
    assert {:error, :invalid_flow} = Auth.adopt_handoff(owner, flow.flow_id, make_ref())
    refute Map.has_key?(receipt, :verifier)
    assert (receipt.deadline - System.monotonic_time(:millisecond)) in 29_000..30_000
    assert {:error, :sign_out_required} = Auth.oauth_start(owner)
    :ok = Auth.sign_out(sid)
  end

  test "delivery receipt expires and preserves the authenticated owner until sign-out", %{
    fixture: fixture
  } do
    Agent.update(fixture, &%{&1 | ready: true})
    owner = "signed-owner-aaaaaaaa"
    {:ok, flow} = Auth.oauth_start(owner)
    due(flow.flow_id)
    {:ok, sid} = Auth.oauth_poll(owner, flow.flow_id)

    :sys.replace_state(OAuthHandoff, fn state ->
      put_in(state, [flow.flow_id, :deadline], System.monotonic_time(:millisecond) - 1)
    end)

    assert {:error, :invalid_flow} = Auth.oauth_poll(owner, flow.flow_id)
    assert {:error, :sign_out_required} = Auth.oauth_start(owner)
    assert Auth.capture(sid)
    :ok = Auth.oauth_cancel(owner, flow.flow_id)
    assert is_nil(Auth.capture(sid))
    assert {:ok, _} = Auth.oauth_start(owner)
  end

  test "supersession, cancel and four-flow admission" do
    assert {:ok, first} = Auth.oauth_start("signed-owner-aaaaaaaa")
    assert {:ok, second} = Auth.oauth_start("signed-owner-aaaaaaaa")
    assert {:error, :invalid_flow} = Auth.oauth_poll("signed-owner-aaaaaaaa", first.flow_id)
    assert :ok = Auth.oauth_cancel("signed-owner-aaaaaaaa", second.flow_id)
    for n <- 1..4, do: assert({:ok, _} = Auth.oauth_start("signed-owner-number-#{n}"))
    assert {:error, :capacity} = Auth.oauth_start("signed-owner-number-5")
  end

  test "malformed and oversized protocol replies fail closed", %{fixture: fixture} do
    for mode <- [
          {:response, 200, "{}"},
          {:response, 201, "invalid"},
          {:response, 302, "{}"},
          {:response, 201, String.duplicate("x", 70_000)},
          :oversized_headers
        ] do
      Agent.update(fixture, &%{&1 | modes: %{"/auth/desktop/start" => mode}})
      assert {:error, _} = Auth.oauth_start("signed-owner-aaaaaaaa")
    end

    assert map_size(:sys.get_state(OAuthHandoff)) == 0
  end

  test "uncertain redemption is terminal and never replayed", %{fixture: fixture} do
    Agent.update(
      fixture,
      &%{&1 | ready: true, modes: %{"/auth/desktop/redeem" => {:response, 200, "{}"}}}
    )

    {:ok, flow} = Auth.oauth_start("signed-owner-aaaaaaaa")
    due(flow.flow_id)

    assert {:error, :fresh_sign_in_required} =
             Auth.oauth_poll("signed-owner-aaaaaaaa", flow.flow_id)

    assert {:error, :invalid_flow} = Auth.oauth_poll("signed-owner-aaaaaaaa", flow.flow_id)
    assert_receive {:handoff_request, "/auth/desktop/redeem", _, _}
    refute_receive {:handoff_request, "/auth/desktop/redeem", _, _}, 30
  end

  test "cancel and caller death fence in-flight start", %{fixture: fixture} do
    Agent.update(fixture, &%{&1 | modes: %{"/auth/desktop/start" => :hold}})
    task = Task.async(fn -> Auth.oauth_start("signed-owner-aaaaaaaa") end)
    assert_receive {:handoff_request, "/auth/desktop/start", _, _}, 1_000
    [{id, _}] = Map.to_list(:sys.get_state(OAuthHandoff))
    assert :ok = Auth.oauth_cancel("signed-owner-aaaaaaaa", id)
    assert {:error, _} = Task.await(task, 1_000)
    task = spawn(fn -> Auth.oauth_start("signed-owner-bbbbbbbb") end)
    assert_receive {:handoff_request, "/auth/desktop/start", _, _}, 1_000
    Process.exit(task, :kill)
    Process.sleep(30)
    assert map_size(:sys.get_state(OAuthHandoff)) == 0
  end

  test "cancel before lifecycle dispatch prevents the mutating HTTP request" do
    lifecycle = Process.whereis(LabbyConsole.GatewayAction.Lifecycle)
    :sys.suspend(lifecycle)
    task = Task.async(fn -> Auth.oauth_start("signed-owner-aaaaaaaa") end)

    try do
      wait_for(fn -> map_size(:sys.get_state(OAuthHandoff)) == 1 end)
      [{id, _}] = Map.to_list(:sys.get_state(OAuthHandoff))
      assert :ok = Auth.oauth_cancel("signed-owner-aaaaaaaa", id)
      refute_receive {:handoff_request, "/auth/desktop/start", _, _}, 50
    after
      :sys.resume(lifecycle)
    end

    assert {:error, _} = Task.await(task, 1_000)
    refute_receive {:handoff_request, "/auth/desktop/start", _, _}, 50
  end

  test "expiry and sixty browser attempts release secrets without another upstream request" do
    {:ok, flow} = Auth.oauth_start("signed-owner-aaaaaaaa")
    :sys.replace_state(OAuthHandoff, fn state -> put_in(state, [flow.flow_id, :attempts], 60) end)
    due(flow.flow_id)
    assert {:error, :expired} = Auth.oauth_poll("signed-owner-aaaaaaaa", flow.flow_id)
    refute_receive {:handoff_request, "/auth/desktop/poll", _, _}, 20
    {:ok, flow} = Auth.oauth_start("signed-owner-aaaaaaaa")

    :sys.replace_state(OAuthHandoff, fn state ->
      put_in(state, [flow.flow_id, :deadline], System.monotonic_time(:millisecond) - 1)
    end)

    send(OAuthHandoff, :expire)
    wait_for(fn -> map_size(:sys.get_state(OAuthHandoff)) == 0 end)
  end

  test "guardian restart fences late start and drops every usable flow", %{fixture: fixture} do
    Agent.update(fixture, &%{&1 | modes: %{"/auth/desktop/start" => :hold}})
    task = Task.async(fn -> Auth.oauth_start("signed-owner-aaaaaaaa") end)
    assert_receive {:handoff_request, "/auth/desktop/start", _, _}, 1_000
    old = Process.whereis(OAuthHandoff)
    Process.exit(old, :kill)
    wait_for(fn -> Process.whereis(OAuthHandoff) not in [nil, old] end)
    assert {:error, _} = Task.await(task, 1_000)
    assert map_size(:sys.get_state(OAuthHandoff)) == 0
  end

  defp wait_for(check, attempts \\ 80)
  defp wait_for(_, 0), do: flunk("bounded condition did not become true")

  defp wait_for(check, attempts) do
    if check.(),
      do: :ok,
      else:
        (
          Process.sleep(10)
          wait_for(check, attempts - 1)
        )
  end

  test "poll rate-limit remains bounded pending without spinning", %{fixture: fixture} do
    Agent.update(fixture, &%{&1 | modes: %{"/auth/desktop/poll" => {:response, 429, "{}"}}})
    {:ok, flow} = Auth.oauth_start("signed-owner-aaaaaaaa")
    due(flow.flow_id)
    assert {:pending, 5_000} = Auth.oauth_poll("signed-owner-aaaaaaaa", flow.flow_id)
    assert {:pending, _} = Auth.oauth_poll("signed-owner-aaaaaaaa", flow.flow_id)
    assert_receive {:handoff_request, "/auth/desktop/poll", _, _}
    refute_receive {:handoff_request, "/auth/desktop/poll", _, _}, 30
  end

  test "early browser polls also consume the sixty-attempt budget" do
    {:ok, flow} = Auth.oauth_start("signed-owner-aaaaaaaa")

    for _ <- 1..60,
        do: assert({:pending, _} = Auth.oauth_poll("signed-owner-aaaaaaaa", flow.flow_id))

    assert {:error, :expired} = Auth.oauth_poll("signed-owner-aaaaaaaa", flow.flow_id)
    refute_receive {:handoff_request, "/auth/desktop/poll", _, _}, 20
  end

  test "redeem and introspection share five seconds and cannot replay", %{fixture: fixture} do
    Agent.update(fixture, &%{&1 | ready: true, modes: %{"/auth/session" => :trickle}})
    {:ok, flow} = Auth.oauth_start("signed-owner-aaaaaaaa")
    due(flow.flow_id)
    started = System.monotonic_time(:millisecond)

    assert {:error, :fresh_sign_in_required} =
             Auth.oauth_poll("signed-owner-aaaaaaaa", flow.flow_id)

    assert (System.monotonic_time(:millisecond) - started) in 4_700..5_900
    assert {:error, :invalid_flow} = Auth.oauth_poll("signed-owner-aaaaaaaa", flow.flow_id)
    assert_receive {:handoff_request, "/auth/desktop/redeem", _, _}
    refute_receive {:handoff_request, "/auth/desktop/redeem", _, _}, 20
  end

  test "cancellation closes publication while the vault is suspended", %{fixture: fixture} do
    Agent.update(fixture, &%{&1 | ready: true})
    owner = "signed-owner-aaaaaaaa"
    {:ok, flow} = Auth.oauth_start(owner)
    due(flow.flow_id)
    vault = Process.whereis(Auth)
    :sys.suspend(vault)
    poll = Task.async(fn -> Auth.oauth_poll(owner, flow.flow_id) end)

    try do
      wait_for(fn ->
        Enum.any?(:ets.tab2list(OAuthHandoff), fn {key, _} -> match?({:receipt, _}, key) end)
      end)

      [{{:receipt, _}, receipt}] =
        Enum.filter(:ets.tab2list(OAuthHandoff), fn {key, _} -> match?({:receipt, _}, key) end)

      cancel = Task.async(fn -> Auth.oauth_cancel(owner, flow.flow_id) end)
      Process.sleep(30)
      assert :atomics.get(receipt.gate, 1) == 2
      :sys.resume(vault)
      assert {:error, :fresh_sign_in_required} = Task.await(poll, 2_000)
      assert :ok = Task.await(cancel, 2_000)
    after
      if Process.alive?(vault), do: :sys.resume(vault)
      if Process.alive?(poll.pid), do: Task.shutdown(poll, :brutal_kill)
    end
  end

  test "absolute start deadline", %{fixture: fixture} do
    Agent.update(fixture, &%{&1 | modes: %{"/auth/desktop/start" => :trickle}})
    started = System.monotonic_time(:millisecond)
    assert {:error, _} = Auth.oauth_start("signed-owner-aaaaaaaa")
    assert (System.monotonic_time(:millisecond) - started) in 4_700..5_900
  end

  defp due(id),
    do:
      :sys.replace_state(OAuthHandoff, fn state ->
        put_in(state, [id, :next_poll], System.monotonic_time(:millisecond) - 1)
      end)

  test "explicit internal/public mapping sends canonical default-port Host", %{fixture: fixture} do
    Application.put_env(:labby_console, :gateway_auth_origin, "https://PUBLIC.example:443/")
    on_exit(fn -> Application.delete_env(:labby_console, :gateway_auth_origin) end)
    Agent.update(fixture, &%{&1 | origin: "https://public.example"})
    assert {:ok, _} = Auth.oauth_start("signed-owner-aaaaaaaa")
    assert_receive {:handoff_request, "/auth/desktop/start", headers, _}
    assert {"origin", "https://public.example"} in headers
    assert {"host", "public.example"} in headers
  end

  test "server diagnostic formatting removes flow and request secrets" do
    status = %{
      state: %{verifier: "private-verifier"},
      message: {:request, "private-cookie"},
      reason: "private-redeem",
      log: ["private-poll"]
    }

    for module <- [
          OAuthHandoff,
          Auth,
          LabbyConsole.GatewayAction,
          LabbyConsole.GatewayAction.Lifecycle
        ] do
      formatted = module.format_status(status) |> inspect()
      refute formatted =~ "private-"
    end
  end

  test "strict origin and authorization URL validation" do
    assert {:ok, "http://[::1]:8000"} = OAuthHandoff.origin("http://[::1]:8000/")
    assert {:ok, "https://example.test"} = OAuthHandoff.origin("https://EXAMPLE.test:443")

    for bad <- [
          "http://example.test",
          "https://example.test/prefix",
          "https://u@example.test",
          "https://example.test?q=x"
        ] do
      assert {:error, :configuration} = OAuthHandoff.origin(bad)
    end

    assert :ok =
             OAuthHandoff.authorization_url(
               "https://example.test/auth/desktop/authorize?state=abc",
               "https://example.test"
             )

    for suffix <- ["?state=a&state=b", "?state=a&extra=b", "?state=", "?state=a#x"] do
      assert {:error, :invalid_response} =
               OAuthHandoff.authorization_url(
                 "https://example.test/auth/desktop/authorize" <> suffix,
                 "https://example.test"
               )
    end

    assert {:error, :origin_mismatch} =
             OAuthHandoff.authorization_url(
               "https://wrong.test/auth/desktop/authorize?state=a",
               "https://example.test"
             )
  end

  test "unknown owner or flow cannot poll or cancel" do
    assert {:error, :invalid_flow} = Auth.oauth_poll("owner-a", "unknown")
    assert {:error, :invalid_flow} = Auth.oauth_cancel("owner-b", "unknown")
  end
end
