defmodule LabbyConsole.GatewayLifecycleAdmissionTest do
  use ExUnit.Case, async: false
  alias LabbyConsole.{Auth, GatewayAction}

  @lifecycle ~w(gateway.mcp.enable gateway.mcp.disable gateway.mcp.restart)

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

  for action <- @lifecycle do
    test "#{action} dispatches exactly its own parameters with captured caller auth", %{id: id} do
      action = unquote(action)
      params = %{"name" => "example", "cleanup" => false, "aggressive" => false}

      assert {:ok, %{}} = Auth.action(id, "gateway", action, params)
      assert_receive {:gateway_wire, _, "/v1/gateway", headers, body}
      assert body == Jason.encode!(%{action: action, params: params})
      assert {"cookie", "caller=alice"} in headers
      assert {"x-csrf-token", "csrf-alice"} in headers
      refute Map.has_key?(Jason.decode!(body)["params"], "confirm")
      refute Map.has_key?(Jason.decode!(body)["params"], "owner")
    end
  end

  test "lifecycle actions are mutations, never reads", %{id: id} do
    for action <- @lifecycle do
      assert {:error, %{kind: :not_allowed}} =
               GatewayAction.run(id, "gateway", action, %{"name" => "example"}, :read)
    end

    refute_receive {:gateway_wire, _, _, _, _}
  end

  test "neighbouring lifecycle actions stay unwired and are never dispatched", %{id: id} do
    for action <- ~w(gateway.reload gateway.mcp.cleanup gateway.discover gateway.import) do
      assert {:error, %{kind: :not_allowed}} = Auth.action(id, "gateway", action, %{})

      assert {:error, %{kind: :not_allowed}} =
               GatewayAction.run(id, "gateway", action, %{}, :read)
    end

    refute_receive {:gateway_wire, _, _, _, _}
  end

  test "an invalid lifecycle response retains unknown side effects", %{id: id, state: state} do
    Agent.update(state, &%{&1 | response: {200, "invalid"}})

    for action <- @lifecycle do
      assert {:error, %{kind: :invalid_response, side_effects: "unknown"}} =
               Auth.action(id, "gateway", action, %{"name" => "example"})
    end
  end

  test "lifecycle operations use the Gateway write budget", %{id: id} do
    :ok = :sys.suspend(GatewayAction)

    try do
      for action <- @lifecycle do
        assert {:ok, ref} = GatewayAction.start(id, "gateway", action, %{"name" => "example"})
        [{{:operation, ^ref}, op}] = :ets.lookup(GatewayAction.Lifecycle, {:operation, ref})
        assert op.deadline - op.started_at == 30_000
        assert {:ok, 25_000} = GatewayAction.receive_timeout(op)
        assert op.mutation? == true
        assert :ok = Auth.cancel_action(id, ref)
        assert_receive {:gateway_action, ^ref, {:error, %{side_effects: "none_expected"}}}
      end
    after
      :sys.resume(GatewayAction)
    end

    refute_receive {:gateway_wire, _, _, _, _}
  end
end
