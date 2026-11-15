defmodule LabbyConsole.GatewayCrudAdmissionTest do
  use ExUnit.Case, async: false
  alias LabbyConsole.{Auth, GatewayAction}

  setup do
    owner = self()

    state =
      start_supervised!(
        {Agent, fn -> %{owner: owner, response: {200, "{}"}, session_status: 200} end}
      )

    server =
      start_supervised!(
        {Bandit,
         plug: {LabbyConsole.GatewayCrudFixture, state: state}, port: 0, ip: {127, 0, 0, 1}}
      )

    {:ok, {_, port}} = ThousandIsland.listener_info(server)
    old = Application.get_env(:labby_console, :gateway_url)
    Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:#{port}")
    {:ok, id} = Auth.sign_in("alice")
    on_exit(fn -> Application.put_env(:labby_console, :gateway_url, old) end)
    %{state: state, id: id}
  end

  for action <- ~w(gateway.get gateway.test gateway.add gateway.update gateway.remove) do
    test "#{action} uses fixed Gateway endpoint and captured auth with exact JSON", %{id: id} do
      action = unquote(action)

      params = %{
        "name" => "example",
        "headers" => %{"x-csrf-token" => "upstream-only", "x-upstream-secret" => "private"},
        "env" => %{"COOKIE" => "upstream-env"}
      }

      assert {:ok, %{}} = Auth.action(id, "gateway", action, params)
      assert_receive {:gateway_wire, _, "/v1/gateway", headers, body}
      assert body == Jason.encode!(%{action: action, params: params})
      assert {"cookie", "caller=alice"} in headers
      assert {"x-csrf-token", "csrf-alice"} in headers
      assert {"content-type", "application/json"} in headers

      refute Enum.any?(headers, fn {key, value} ->
               key in ["x-upstream-secret", "env", "authorization"] or
                 value in ["private", "upstream-only", "upstream-env"]
             end)

      refute Map.has_key?(Jason.decode!(body)["params"], "confirm")
    end
  end

  test "only get is admitted as a read and unrelated actions remain rejected", %{id: id} do
    assert {:ok, %{}} = Auth.read_action(id, "gateway", "gateway.get", %{"name" => "example"})
    assert_receive {:gateway_wire, _, _, _, _}

    for action <-
          ~w(gateway.test gateway.add gateway.update gateway.remove gateway.reload gateway.import gateway.discover arbitrary) do
      assert {:error, %{kind: :not_allowed}} =
               GatewayAction.run(id, "gateway", action, %{}, :read)
    end

    for action <- ~w(gateway.list gateway.reload gateway.import gateway.discover arbitrary) do
      assert {:error, %{kind: :not_allowed}} = Auth.action(id, "gateway", action, %{})
    end

    refute_receive {:gateway_wire, _, _, _, _}
  end

  test "every effect-capable action retains uncertainty on invalid response", %{
    id: id,
    state: state
  } do
    Agent.update(state, &%{&1 | response: {200, "invalid"}})

    for action <- ~w(gateway.test gateway.add gateway.update gateway.remove) do
      assert {:error, %{kind: :invalid_response, side_effects: "unknown"}} =
               Auth.action(id, "gateway", action, %{})
    end

    assert {:error, %{kind: :invalid_response, side_effects: "none_expected"}} =
             Auth.action(id, "gateway", "gateway.get", %{})
  end

  test "operation admission preserves exact deadlines and receive caps", %{id: id} do
    :ok = :sys.suspend(GatewayAction)

    try do
      for {service, action, mode, total, cap} <- [
            {"gateway", "gateway.test", :action, 30_000, 25_000},
            {"gateway", "gateway.add", :action, 30_000, 25_000},
            {"gateway", "gateway.update", :action, 30_000, 25_000},
            {"gateway", "gateway.remove", :action, 30_000, 25_000},
            {"gateway", "gateway.get", :action, 5_000, 2_000},
            {"gateway", "gateway.get", :read, 5_000, 2_000},
            {"gateway", "gateway.host.metrics", :read, 12_000, 8_000},
            {"projects", "projects.create", :action, 5_000, 2_000}
          ] do
        started = System.monotonic_time(:millisecond)
        assert {:ok, ref} = GatewayAction.start(id, service, action, %{}, mode)
        [{{:operation, ^ref}, op}] = :ets.lookup(GatewayAction.Lifecycle, {:operation, ref})
        assert op.deadline - op.started_at == total
        assert op.started_at >= started
        assert {:ok, ^cap} = GatewayAction.receive_timeout(op)
        assert :ok = Auth.cancel_action(id, ref)
        assert_receive {:gateway_action, ^ref, {:error, %{side_effects: "none_expected"}}}
      end
    after
      :sys.resume(GatewayAction)
    end

    refute_receive {:gateway_wire, _, _, _, _}
  end

  test "receive allowance uses original remaining time and rejects insufficient headroom" do
    now = System.monotonic_time(:millisecond)
    op = %{deadline: now + 1_000, receive_timeout: 25_000}
    assert {:ok, allowance} = GatewayAction.receive_timeout(op)
    assert allowance > 0 and allowance <= 750
    assert {:error, :timeout} = GatewayAction.receive_timeout(%{op | deadline: now + 250})
    assert {:error, :timeout} = GatewayAction.receive_timeout(%{op | deadline: now - 1})
  end

  test "slow preflight consumes original deadline and rejects before dispatch gate", %{
    id: id,
    state: state
  } do
    Agent.update(state, &%{&1 | session_status: :hold})
    parent = self()
    snapshot = Auth.capture(id)

    op = %{
      id: id,
      generation: snapshot.generation,
      session: snapshot,
      service: "gateway",
      action: "gateway.test",
      json: "{}",
      mutation?: true,
      deadline: System.monotonic_time(:millisecond) + 350,
      receive_timeout: 25_000
    }

    task = Task.async(fn -> Auth.execute_action(op, parent, make_ref()) end)
    assert_receive {:session_request, server}
    Process.send_after(server, {:session_status, 200}, 150)
    assert {:error, %{kind: :timeout, side_effects: "none_expected"}} = Task.await(task, 1_000)
    refute_receive {:"$gen_call", _, {:dispatch, _}}
    refute_receive {:gateway_wire, _, _, _, _}
  end

  @tag timeout: 36_000
  test "sync and async Gateway deadlines both interrupt trickled responses after thirty seconds",
       %{id: id, state: state} do
    Agent.update(state, &%{&1 | response: :trickle})
    started = System.monotonic_time(:millisecond)
    sync = Task.async(fn -> Auth.action(id, "gateway", "gateway.test", %{}) end)
    assert {:ok, ref} = Auth.start_action(id, "gateway", "gateway.update", %{})
    for _ <- 1..2, do: assert_receive({:gateway_wire, _, _, _, _}, 1000)
    assert {:error, %{kind: :timeout, side_effects: "unknown"}} = Task.await(sync, 31_000)

    assert_receive {:gateway_action, ^ref, {:error, %{kind: :timeout, side_effects: "unknown"}}},
                   1000

    elapsed = System.monotonic_time(:millisecond) - started
    assert elapsed >= 29_500 and elapsed <= 31_500
    refute_receive {:gateway_action, ^ref, _}
  end
end
