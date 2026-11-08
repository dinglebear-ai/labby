defmodule LabbyConsole.GatewayActionTest do
  use ExUnit.Case, async: false
  alias LabbyConsole.Auth

  setup do
    owner = self()

    state =
      start_supervised!(
        {Agent, fn -> %{owner: owner, response: {200, "{}"}, session_status: 200} end}
      )

    server =
      start_supervised!(
        {Bandit, plug: {LabbyConsole.ActionFixture, state: state}, port: 0, ip: {127, 0, 0, 1}}
      )

    {:ok, {_, port}} = ThousandIsland.listener_info(server)
    old = Application.get_env(:labby_console, :gateway_url)
    Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:#{port}")
    {:ok, id} = Auth.sign_in("alice")
    on_exit(fn -> Application.put_env(:labby_console, :gateway_url, old) end)
    %{state: state, id: id}
  end

  test "explicit lifecycle actions forward caller cookie and authoritative CSRF", %{id: id} do
    for {service, action} <- [
          {"projects", "projects.list"},
          {"projects", "projects.get"},
          {"projects", "projects.create"},
          {"projects", "projects.update"},
          {"projects", "projects.archive"},
          {"projects", "projects.activate"},
          {"access", "access.team.list"}
        ] do
      assert {:ok, %{}} = Auth.action(id, service, action, %{"name" => "hello"})

      assert_receive {:dispatch, _, ^service,
                      %{"action" => ^action, "params" => %{"name" => "hello"}}, ["caller=alice"],
                      ["csrf-alice"]}
    end
  end

  test "unknown pair and non-object params never dispatch", %{id: id} do
    for {service, action, params} <- [
          {"projects", "projects.delete", %{}},
          {"access", "projects.list", %{}},
          {"gateway", "gateway.list", %{}},
          {"projects", "projects.create", []}
        ] do
      assert {:error, %{kind: :not_allowed, side_effects: "none_expected"}} =
               Auth.action(id, service, action, params)
    end

    refute_receive {:dispatch, _, _, _, _, _}
  end

  test "dispatch forbidden keeps login and subsequent existing read succeeds", %{
    id: id,
    state: state
  } do
    response(state, 403, ~s({"kind":"forbidden","message":"secret"}))

    assert {:error, %{kind: :forbidden, status: 403}} =
             Auth.action(id, "projects", "projects.create", %{})

    assert {:ok, _} = Auth.current(id)
    assert {:error, :forbidden} = Auth.read_action(id, "gateway", "gateway.clients.list", %{})
    assert {:error, :forbidden} = Auth.servers(id)
    response(state, 200, "[]")
    assert {:ok, []} = Auth.servers(id)
  end

  test "session introspection forbidden keeps authority", %{id: id, state: state} do
    Agent.update(state, &%{&1 | session_status: 403})
    assert {:error, :forbidden} = Auth.current(id)
    Agent.update(state, &%{&1 | session_status: 200})
    assert {:ok, _} = Auth.current(id)
  end

  test "session 401 removes authority permanently", %{id: id, state: state} do
    Agent.update(state, &%{&1 | session_status: 401})
    assert {:error, %{kind: :unauthenticated}} = Auth.action(id, "projects", "projects.list", %{})
    Agent.update(state, &%{&1 | session_status: 200})
    assert {:error, :unauthenticated} = Auth.current(id)
    refute_receive {:dispatch, _, _, _, _, _}
  end

  test "caller 401 revokes but structured upstream OAuth 401 does not", %{id: id, state: state} do
    response(
      state,
      401,
      Jason.encode!(%{
        contract_version: 1,
        kind: "oauth_needs_reauth",
        service: "projects",
        action: "projects.list",
        origin: "upstream_transport",
        side_effects: "none_expected",
        recovery: %{action: "reauthenticate", same_arguments: "conditional", guidance: "secret"}
      })
    )

    assert {:error, %{kind: :oauth_needs_reauth, recovery: %{"action" => "reauthenticate"}}} =
             Auth.action(id, "projects", "projects.list", %{})

    assert {:ok, _} = Auth.current(id)
    response(state, 401, ~s({"kind":"auth_failed"}))
    assert {:error, %{kind: :unauthenticated}} = Auth.action(id, "projects", "projects.list", %{})
    assert {:error, :unauthenticated} = Auth.current(id)
  end

  test "structured failures retain only bounded safe metadata", %{id: id, state: state} do
    response(
      state,
      422,
      Jason.encode!(%{
        contract_version: 1,
        kind: "invalid_param",
        message: "secret",
        origin: "validation",
        service: "projects",
        action: "projects.create",
        side_effects: "none_expected",
        param: "name",
        required_scopes: ["lab:admin"],
        recovery: %{
          action: "revise_and_retry",
          same_arguments: "safe",
          guidance: "secret",
          retry_after_ms: 200
        },
        hint: "secret"
      })
    )

    assert {:error, error} = Auth.action(id, "projects", "projects.create", %{})
    assert error.kind == :invalid_param
    assert error.service == "projects" and error.action == "projects.create"

    assert error.metadata == %{
             "contract_version" => 1,
             "origin" => "validation",
             "param" => "name",
             "required_scopes" => ["lab:admin"]
           }

    assert error.recovery == %{
             "action" => "revise_and_retry",
             "same_arguments" => "safe",
             "retry_after_ms" => 200
           }

    refute inspect(error) =~ "secret"
  end

  test "encoded request bytes enforce exact 32768 byte boundary", %{id: id} do
    n = 32_768 - byte_size(Jason.encode!(%{action: "projects.create", params: %{name: ""}}))

    assert {:ok, %{}} =
             Auth.action(id, "projects", "projects.create", %{name: String.duplicate("x", n)})

    assert_receive {:dispatch, _, _, _, _, _}

    assert {:error, %{kind: :request_too_large, side_effects: "none_expected"}} =
             Auth.action(id, "projects", "projects.create", %{name: String.duplicate("x", n + 1)})

    refute_receive {:dispatch, _, _, _, _, _}
  end

  test "response accepts 65536 bytes and excess makes writes uncertain", %{id: id, state: state} do
    response(state, 200, Jason.encode!(String.duplicate("x", 65_534)))
    assert {:ok, body} = Auth.action(id, "projects", "projects.create", %{})
    assert byte_size(body) == 65_534
    response(state, 200, Jason.encode!(String.duplicate("x", 65_535)))

    assert {:error,
            %{
              kind: :response_too_large,
              side_effects: "unknown",
              recovery: %{"same_arguments" => "discouraged"}
            }} = Auth.action(id, "projects", "projects.create", %{})
  end

  test "malformed write success is uncertain and never replayed", %{id: id, state: state} do
    response(state, 200, "not-json-secret")

    assert {:error, %{kind: :invalid_response, side_effects: "unknown"} = error} =
             Auth.action(id, "projects", "projects.update", %{})

    refute inspect(error) =~ "not-json-secret"
    assert_receive {:dispatch, _, _, _, _, _}
    refute_receive {:dispatch, _, _, _, _, _}
  end

  test "four held operations saturate one caller while isolating another", %{id: id, state: state} do
    hold(state)

    refs =
      for _ <- 1..4 do
        assert {:ok, ref} = Auth.start_action(id, "projects", "projects.list", %{})
        assert_receive {:dispatch, _, _, _, ["caller=alice"], ["csrf-alice"]}
        ref
      end

    assert {:error, %{kind: :saturated, side_effects: "none_expected"}} =
             Auth.start_action(id, "projects", "projects.list", %{})

    {:ok, other} = Auth.sign_in("bob")
    assert {:ok, other_ref} = Auth.start_action(other, "projects", "projects.list", %{})
    assert_receive {:dispatch, _, _, _, ["caller=bob"], ["csrf-bob"]}
    assert {:error, :not_found} = Auth.cancel_action(other, hd(refs))
    for ref <- refs, do: assert(:ok = Auth.cancel_action(id, ref))
    assert :ok = Auth.cancel_action(other, other_ref)
  end

  test "global admission rejects operation 33 without dispatch", %{state: state} do
    hold(state)

    operations =
      for caller <- 1..8, reduce: [] do
        operations ->
          {:ok, id} = Auth.sign_in("caller#{caller}")

          refs =
            for _ <- 1..4 do
              assert {:ok, ref} = Auth.start_action(id, "projects", "projects.list", %{})
              assert_receive {:dispatch, _, _, _, _, _}, 1000
              {id, ref}
            end

          refs ++ operations
      end

    {:ok, extra} = Auth.sign_in("extra")

    assert {:error, %{kind: :saturated}} =
             Auth.start_action(extra, "projects", "projects.list", %{})

    for {id, ref} <- operations, do: assert(:ok = Auth.cancel_action(id, ref))
  end

  test "cancellation after dispatch reports uncertainty and fences late success", %{
    id: id,
    state: state
  } do
    hold(state)
    {:ok, ref} = Auth.start_action(id, "projects", "projects.create", %{})
    assert_receive {:dispatch, server, _, _, _, _}
    assert :ok = Auth.cancel_action(id, ref)
    assert_receive {:gateway_action, ^ref, {:error, %{kind: :cancelled, side_effects: "unknown"}}}
    send(server, {:respond, 200, "{}"})
    refute_receive {:gateway_action, ^ref, {:ok, _}}
  end

  test "sign-out fences owned operations", %{id: id, state: state} do
    hold(state)
    {:ok, ref} = Auth.start_action(id, "projects", "projects.create", %{})
    assert_receive {:dispatch, server, _, _, _, _}
    assert :ok = Auth.sign_out(id)
    send(server, {:respond, 200, "{}"})

    assert_receive {:gateway_action, ^ref,
                    {:error, %{kind: :unauthenticated, side_effects: "unknown"}}}

    refute_receive {:gateway_action, ^ref, {:ok, _}}
  end

  @tag timeout: 10_000
  test "fixed operation deadline bounds an unresponsive mutation", %{id: id, state: state} do
    hold(state)
    start = System.monotonic_time(:millisecond)
    {:ok, ref} = Auth.start_action(id, "projects", "projects.create", %{})
    assert_receive {:dispatch, _, _, _, _, _}

    assert_receive {:gateway_action, ^ref, {:error, %{kind: kind, side_effects: "unknown"}}},
                   5_500

    assert kind in [:timeout, :transport_error]
    assert System.monotonic_time(:millisecond) - start <= 5_500
  end

  test "introspection failure before dispatch has no mutation side effects", %{
    id: id,
    state: state
  } do
    Agent.update(state, &%{&1 | session_status: 503})

    assert {:error, %{kind: :unavailable, side_effects: "none_expected"}} =
             Auth.action(id, "projects", "projects.create", %{})

    refute_receive {:dispatch, _, _, _, _, _}
  end

  test "403 remains forbidden even when a mismatched envelope claims authentication failure", %{
    id: id,
    state: state
  } do
    response(
      state,
      403,
      Jason.encode!(%{
        contract_version: 1,
        service: "projects",
        action: "projects.list",
        kind: "auth_failed"
      })
    )

    assert {:error, %{kind: :forbidden}} = Auth.action(id, "projects", "projects.list", %{})
    assert {:ok, _} = Auth.current(id)
  end

  test "uncertain structured errors cannot recommend safe unchanged mutation retry", %{
    id: id,
    state: state
  } do
    response(
      state,
      502,
      Jason.encode!(%{
        contract_version: 1,
        service: "projects",
        action: "projects.create",
        kind: "upstream_error",
        side_effects: "unknown",
        recovery: %{action: "retry_later", same_arguments: "safe"}
      })
    )

    assert {:error, %{side_effects: "unknown", recovery: %{"same_arguments" => "discouraged"}}} =
             Auth.action(id, "projects", "projects.create", %{})
  end

  test "caller death terminates its supervised work and releases its four slots", %{
    id: id,
    state: state
  } do
    hold(state)
    parent = self()

    owner =
      spawn(fn ->
        refs =
          for _ <- 1..4 do
            {:ok, ref} = Auth.start_action(id, "projects", "projects.create", %{})
            ref
          end

        send(parent, {:owned, refs})

        receive do
          :stop -> :ok
        end
      end)

    assert_receive {:owned, _}
    for _ <- 1..4, do: assert_receive({:dispatch, _, _, _, _, _})
    children = Task.Supervisor.children(LabbyConsole.GatewayAction.Tasks)
    monitors = Enum.map(children, &Process.monitor/1)
    Process.exit(owner, :kill)
    for monitor <- monitors, do: assert_receive({:DOWN, ^monitor, :process, _, _}, 1000)
    response(state, 200, "{}")
    assert {:ok, %{}} = Auth.action(id, "projects", "projects.create", %{})
  end

  @tag timeout: 10_000
  test "whole deadline interrupts a trickled body despite ongoing network data", %{
    id: id,
    state: state
  } do
    Agent.update(state, &%{&1 | response: :trickle})
    started = System.monotonic_time(:millisecond)

    assert {:error, %{kind: :timeout, side_effects: "unknown"}} =
             Auth.action(id, "projects", "projects.create", %{})

    elapsed = System.monotonic_time(:millisecond) - started
    assert elapsed >= 4500 and elapsed <= 5500
    assert_receive {:dispatch, _, _, _, _, _}
    refute_receive {:dispatch, _, _, _, _, _}
  end

  @tag timeout: 10_000
  test "existing host sampling retains its longer operation allowance", %{id: id, state: state} do
    hold(state)
    task = Task.async(fn -> Auth.read_action(id, "gateway", "gateway.host.metrics", %{}) end)
    assert_receive {:dispatch, server, "gateway", _, _, _}
    Process.send_after(server, {:respond, 200, "{}"}, 5500)
    assert {:ok, %{}} = Task.await(task, 7000)
  end

  test "disconnect after dispatch attempts a mutation exactly once", %{state: _} do
    owner = self()

    start_supervised!(%{
      id: :disconnect_fixture,
      start: {LabbyConsole.DisconnectFixture, :start_link, [owner]},
      restart: :temporary
    })

    assert_receive {:disconnect_fixture, port}
    Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:#{port}")
    {:ok, id} = Auth.sign_in("alice")

    assert {:error,
            %{
              kind: :transport_error,
              side_effects: "unknown",
              recovery: %{"same_arguments" => "discouraged"}
            }} = Auth.action(id, "projects", "projects.create", %{})

    assert_receive :write_attempt
    refute_receive :write_attempt
  end

  test "cancel before dispatch has no expected mutation effects", %{id: id, state: state} do
    Agent.update(state, &%{&1 | session_status: :hold})
    {:ok, ref} = Auth.start_action(id, "projects", "projects.create", %{})
    assert_receive {:session_request, server}
    assert :ok = Auth.cancel_action(id, ref)

    assert_receive {:gateway_action, ^ref,
                    {:error, %{kind: :cancelled, side_effects: "none_expected"}}}

    send(server, {:session_status, 200})
    refute_receive {:dispatch, _, _, _, _, _}
  end

  test "revoking one caller does not cancel another caller", %{id: id, state: state} do
    {:ok, bob} = Auth.sign_in("bob")
    hold(state)
    {:ok, alice_ref} = Auth.start_action(id, "projects", "projects.create", %{})
    assert_receive {:dispatch, alice_server, _, _, ["caller=alice"], _}
    {:ok, bob_ref} = Auth.start_action(bob, "projects", "projects.create", %{})
    assert_receive {:dispatch, bob_server, _, _, ["caller=bob"], _}
    assert :ok = Auth.sign_out(id)
    send(alice_server, {:respond, 200, "{}"})
    send(bob_server, {:respond, 200, "{}"})
    assert_receive {:gateway_action, ^alice_ref, {:error, %{kind: :unauthenticated}}}
    assert_receive {:gateway_action, ^bob_ref, {:ok, %{}}}
    assert {:ok, _} = Auth.current(bob)
  end

  test "mismatched OAuth envelope cannot preserve a caller rejected with 401", %{
    id: id,
    state: state
  } do
    response(
      state,
      401,
      Jason.encode!(%{
        contract_version: 1,
        service: "access",
        action: "access.team.list",
        kind: "oauth_needs_reauth"
      })
    )

    assert {:error, %{kind: :unauthenticated}} = Auth.action(id, "projects", "projects.list", %{})
    assert {:error, :unauthenticated} = Auth.current(id)
  end

  test "redirected write is never followed or replayed", %{id: id, state: state} do
    response(state, 307, "private redirect body")

    assert {:error, %{kind: :gateway_error, status: 307, side_effects: "unknown"}} =
             Auth.action(id, "projects", "projects.create", %{})

    assert_receive {:dispatch, _, _, _, _, _}
    refute_receive {:dispatch, _, _, _, _, _}
  end

  test "worker supervisor capacity rejects without crashing admission", %{id: id} do
    pids =
      for _ <- 1..32 do
        {:ok, pid} =
          Task.Supervisor.start_child(LabbyConsole.GatewayAction.Tasks, fn ->
            receive do
              :stop -> :ok
            end
          end)

        pid
      end

    try do
      assert {:ok, ref} = Auth.start_action(id, "projects", "projects.create", %{})

      assert_receive {:gateway_action, ^ref,
                      {:error, %{kind: :saturated, side_effects: "none_expected"}}}
    after
      for pid <- pids, do: Task.Supervisor.terminate_child(LabbyConsole.GatewayAction.Tasks, pid)
    end

    assert {:ok, %{}} = Auth.action(id, "projects", "projects.list", %{})
  end

  test "all current Rust origin and recovery enums survive the bounded adapter", %{
    id: id,
    state: state
  } do
    for {origin, recovery} <- [
          {"budget", "reduce_work"},
          {"bridge", "start_dependency"},
          {"code_mode", "inspect_and_escalate"},
          {"policy", "confirm"}
        ] do
      response(
        state,
        422,
        Jason.encode!(%{
          contract_version: 1,
          service: "projects",
          action: "projects.create",
          kind: "invalid_param",
          origin: origin,
          side_effects: "none_expected",
          recovery: %{action: recovery, same_arguments: "never"}
        })
      )

      assert {:error, error} = Auth.action(id, "projects", "projects.create", %{})
      assert error.metadata["origin"] == origin
      assert error.recovery["action"] == recovery
    end
  end

  test "confirmed caller authorization errors retain their HTTP status", %{id: id, state: state} do
    response(state, 401, ~s({"kind":"auth_failed"}))

    assert {:error, %{kind: :unauthenticated, status: 401}} =
             Auth.action(id, "projects", "projects.list", %{})
  end

  @tag timeout: 10_000
  test "a blocked vault cannot queue admission or extend the complete operation deadline", %{
    id: id
  } do
    :ok = :sys.suspend(Auth)

    ref =
      try do
        started = System.monotonic_time(:millisecond)
        assert {:ok, ref} = Auth.start_action(id, "projects", "projects.create", %{})
        assert System.monotonic_time(:millisecond) - started < 250
        assert_receive {:dispatch, _, _, _, _, _}, 1000

        assert_receive {:gateway_action, ^ref,
                        {:error, %{kind: :timeout, side_effects: "unknown"}}},
                       5500

        assert System.monotonic_time(:millisecond) - started <= 5500
        ref
      after
        :sys.resume(Auth)
      end

    refute_receive {:gateway_action, ^ref, {:ok, _}}
    assert {:ok, _} = Auth.current(id)
  end

  test "cancelled queued publication cannot deliver again when the vault resumes", %{id: id} do
    :ok = :sys.suspend(Auth)

    ref =
      try do
        {:ok, ref} = Auth.start_action(id, "projects", "projects.create", %{})
        assert_receive {:dispatch, _, _, _, _, _}
        await_queued_publication()
        assert :ok = Auth.cancel_action(id, ref)

        assert_receive {:gateway_action, ^ref,
                        {:error, %{kind: :cancelled, side_effects: "unknown"}}}

        ref
      after
        :sys.resume(Auth)
      end

    refute_receive {:gateway_action, ^ref, _}
    assert {:ok, _} = Auth.current(id)
  end

  @tag timeout: 10_000
  test "blocked admission reserves promptly and never dispatches stale queued work", %{id: id} do
    :ok = :sys.suspend(LabbyConsole.GatewayAction)

    ref =
      try do
        started = System.monotonic_time(:millisecond)
        assert {:ok, ref} = Auth.start_action(id, "projects", "projects.create", %{})
        assert System.monotonic_time(:millisecond) - started < 250

        assert_receive {:gateway_action, ^ref,
                        {:error, %{kind: :timeout, side_effects: "none_expected"}}},
                       5500

        ref
      after
        :sys.resume(LabbyConsole.GatewayAction)
      end

    refute_receive {:dispatch, _, _, _, _, _}
    refute_receive {:gateway_action, ^ref, _}
    assert {:ok, %{}} = Auth.action(id, "projects", "projects.list", %{})
  end

  test "the protected vault snapshot fails closed across its owner restart", %{
    id: id,
    state: state
  } do
    %{generation: generation} = Auth.capture(id)
    assert_raise ArgumentError, fn -> :ets.insert(Auth, {id, %{generation: generation}}) end
    hold(state)
    {:ok, ref} = Auth.start_action(id, "projects", "projects.create", %{})
    assert_receive {:dispatch, server, _, _, _, _}
    assert :ok = Supervisor.terminate_child(LabbyConsole.Supervisor, Auth)

    try do
      assert is_nil(Auth.capture(id))
      refute Auth.matches?(id, generation)

      assert {:error, %{kind: :unauthenticated}} =
               Auth.action(id, "projects", "projects.list", %{})
    after
      assert {:ok, _} = Supervisor.restart_child(LabbyConsole.Supervisor, Auth)
    end

    send(server, {:respond, 200, "{}"})

    assert_receive {:gateway_action, ^ref,
                    {:error, %{kind: :unauthenticated, side_effects: "unknown"}}}

    refute_receive {:gateway_action, ^ref, {:ok, _}}
    assert {:error, :unauthenticated} = Auth.current(id)
  end

  defp await_queued_publication(attempts \\ 50)
  defp await_queued_publication(0), do: flunk("worker did not queue its bounded publication")

  defp await_queued_publication(attempts) do
    {:messages, messages} = Process.info(Process.whereis(Auth), :messages)

    queued? =
      Enum.any?(messages, fn
        {:"$gen_call", _, message} when is_tuple(message) -> elem(message, 0) == :publish
        _ -> false
      end)

    unless queued? do
      receive do
      after
        10 -> await_queued_publication(attempts - 1)
      end
    end
  end

  defp response(state, status, body), do: Agent.update(state, &%{&1 | response: {status, body}})
  defp hold(state), do: Agent.update(state, &%{&1 | response: :hold})
end
