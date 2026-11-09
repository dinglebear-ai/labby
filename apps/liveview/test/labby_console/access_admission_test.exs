defmodule LabbyConsole.AccessAdmissionTest do
  use ExUnit.Case, async: false
  alias LabbyConsole.{Auth, GatewayAction.Error}
  alias LabbyConsole.GatewayAction.Lifecycle

  @inventories ~w(access.team.member.list access.team_project.list access.team_invitation.list access.platform_admin.list)
  @reads ~w(access.team.list access.project.effective.list access.gateway_credential.list) ++
           @inventories
  @writes ~w(
    access.team.create
    access.team.member.add
    access.team.member.role.set
    access.team.member.suspend
    access.team.member.remove
    access.team.member.activate
    access.team.suspend
    access.team.activate
    access.team_invitation.create
    access.team_invitation.accept
    access.team_project.assign
    access.team_project.unassign
    access.platform_admin.grant
    access.platform_admin.revoke
    access.gateway_credential.bind
    access.gateway_credential.revoke
  )

  setup do
    owner = self()

    state =
      start_supervised!(
        {Agent, fn -> %{owner: owner, response: {200, "{}"}, preflight: :normal} end}
      )

    server =
      start_supervised!(
        {Bandit,
         plug: {LabbyConsole.AccessActionFixture, state: state}, port: 0, ip: {127, 0, 0, 1}}
      )

    {:ok, {_, port}} = ThousandIsland.listener_info(server)
    previous = Application.get_env(:labby_console, :gateway_url)
    Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:#{port}")
    {:ok, id} = Auth.sign_in("fixture-access")
    on_exit(fn -> Application.put_env(:labby_console, :gateway_url, previous) end)
    %{id: id, state: state}
  end

  for action <- @reads ++ @writes do
    @action action
    @risk if(action in @writes, do: "unknown", else: "none_expected")
    @budget if(action in @inventories, do: ["1500"], else: [])
    test "#{action} uses fixed authenticated route and preserves its malformed-success risk", %{
      id: id,
      state: state
    } do
      action = @action
      Agent.update(state, &%{&1 | response: {200, "private-malformed-body"}})

      assert {:error, %Error{kind: :invalid_response, side_effects: @risk} = error} =
               Auth.action(id, "access", action, %{
                 "team_id" => "team",
                 "x-labby-access-read-budget-ms" => "999999"
               })

      assert_receive {:access_dispatch, _, "/v1/access/admin", %{"action" => ^action},
                      ["caller=access"], ["csrf-access"], @budget, _}

      refute inspect(error) =~ "private-malformed-body"
      refute_receive {:access_dispatch, _, _, _, _, _, _, _}
    end
  end

  test "empty help shares authenticated route without a read-budget header", %{id: id} do
    assert {:ok, %{}} = Auth.action(id, "access", "help", %{})

    assert_receive {:access_dispatch, _, "/v1/access/admin",
                    %{"action" => "help", "params" => %{}}, ["caller=access"], ["csrf-access"],
                    [], _}
  end

  test "nonempty help and unrelated action names cannot dispatch", %{id: id} do
    for {service, action, params} <- [
          {"access", "help", %{"action" => "access.team.create"}},
          {"access", "help", %{ignored: nil}},
          {"projects", "help", %{}},
          {"access", "access.platform_admin.delete", %{}},
          {"access", "access.team.member.list.extra", %{}},
          {"access", "gateway.get", %{}},
          {"gateway", "access.team.list", %{}},
          {"access", "access.team.list", []}
        ] do
      assert {:error, %Error{kind: :not_allowed, side_effects: "none_expected"}} =
               Auth.action(id, service, action, params)
    end

    assert {:error, :unavailable} = Auth.read_action(id, "access", "access.team.member.list", %{})
    refute_receive {:access_dispatch, _, _, _, _, _, _, _}
  end

  test "existing Projects and Gateway requests omit Access read hints", %{id: id} do
    for {service, action} <- [
          {"projects", "projects.list"},
          {"projects", "projects.create"},
          {"gateway", "gateway.test"},
          {"gateway", "gateway.add"}
        ] do
      assert {:ok, %{}} = Auth.action(id, service, action, %{})
      assert_receive {:access_dispatch, _, _, %{"action" => ^action}, _, _, [], _}
    end
  end

  @tag timeout: 10_000
  test "preflight consumes original deadline and the hint uses clamped receive headroom", %{
    id: id,
    state: state
  } do
    Agent.update(state, &%{&1 | preflight: :hold})
    {:ok, ref} = Auth.start_action(id, "access", "access.team.member.list", %{})
    assert_receive {:access_preflight, peer}, 1_000
    [{{:operation, ^ref}, op}] = :ets.lookup(Lifecycle, {:operation, ref})
    assert op.deadline - op.started_at == 5_000
    Process.sleep(max(0, op.deadline - now() - 1_000))
    released = now()
    send(peer, :release)
    assert_receive {:access_dispatch, _, _, _, _, _, [hint], received}, 1_000
    budget = String.to_integer(hint)
    # At this point receive_timeout is remaining - 250, so its extra
    # 250 ms reserve is the tighter bound than remaining - 250.
    assert budget > 0 and budget <= op.deadline - released - 500
    assert budget >= op.deadline - received - 500
    assert budget < 600
    assert_receive {:gateway_action, ^ref, {:ok, %{}}}, 1_000
  end

  @tag timeout: 10_000
  test "exhausted read hint rejects before the dispatch gate", %{id: id, state: state} do
    Agent.update(state, &%{&1 | preflight: :hold})
    {:ok, ref} = Auth.start_action(id, "access", "access.team_project.list", %{})
    assert_receive {:access_preflight, peer}, 1_000
    [{{:operation, ^ref}, op}] = :ets.lookup(Lifecycle, {:operation, ref})
    Process.sleep(max(0, op.deadline - now() - 400))
    send(peer, :release)

    assert_receive {:gateway_action, ^ref,
                    {:error, %Error{kind: :timeout, side_effects: "none_expected"}}},
                   1_000

    assert :atomics.get(op.dispatch_gate, 1) == 2
    refute_receive {:access_dispatch, _, _, _, _, _, _, _}
  end

  test "read and write cancellation distinguish dispatched effects", %{id: id, state: state} do
    Agent.update(state, &%{&1 | response: :hold})

    for {action, risk} <- [
          {"access.team.member.list", "none_expected"},
          {"help", "none_expected"},
          {"access.team_invitation.accept", "unknown"}
        ] do
      {:ok, ref} = Auth.start_action(id, "access", action, %{})
      assert_receive {:access_dispatch, peer, _, _, _, _, _, _}, 1_000
      assert :ok = Auth.cancel_action(id, ref)

      assert_receive {:gateway_action, ^ref,
                      {:error, %Error{kind: :cancelled, side_effects: ^risk}}},
                     1_000

      send(peer, {:respond, 200, "{}"})
      refute_receive {:gateway_action, ^ref, _}
    end
  end

  test "write cancellation before preflight finishes has no effects", %{id: id, state: state} do
    Agent.update(state, &%{&1 | preflight: :hold})
    {:ok, ref} = Auth.start_action(id, "access", "access.gateway_credential.bind", %{})
    assert_receive {:access_preflight, peer}, 1_000
    assert :ok = Auth.cancel_action(id, ref)
    assert_receive {:gateway_action, ^ref, {:error, %Error{side_effects: "none_expected"}}}, 1_000
    send(peer, :release)
    refute_receive {:access_dispatch, _, _, _, _, _, _, _}
  end

  test "local body cap keeps inventory read errors typed and writes uncertain", %{
    id: id,
    state: state
  } do
    Agent.update(state, &%{&1 | response: {200, Jason.encode!(String.duplicate("x", 65_535))}})

    for {action, risk} <- [
          {"access.team.member.list", "none_expected"},
          {"access.team_invitation.accept", "unknown"}
        ] do
      assert {:error, %Error{kind: :response_too_large, side_effects: ^risk}} =
               Auth.action(id, "access", action, %{})

      assert_receive {:access_dispatch, _, _, _, _, _, _, _}
      refute_receive {:access_dispatch, _, _, _, _, _, _, _}
    end
  end

  test "local inventory overflow offers bounded reduce-work recovery", %{id: id, state: state} do
    Agent.update(state, &%{&1 | response: {200, Jason.encode!(String.duplicate("x", 65_535))}})

    assert {:error,
            %Error{
              kind: :response_too_large,
              side_effects: "none_expected",
              recovery: %{"action" => "reduce_work", "same_arguments" => "never"},
              metadata: %{"origin" => "budget"}
            }} = Auth.action(id, "access", "access.team.member.list", %{})
  end

  @tag timeout: 10_000
  test "unresponsive Access writes terminate once within the original operation budget", %{
    id: id,
    state: state
  } do
    Agent.update(state, &%{&1 | response: :hold})
    started = now()
    {:ok, ref} = Auth.start_action(id, "access", "access.team_invitation.accept", %{})
    assert_receive {:access_dispatch, _, _, _, _, _, [], _}, 1_000

    assert_receive {:gateway_action, ^ref,
                    {:error,
                     %Error{
                       kind: kind,
                       side_effects: "unknown",
                       recovery: %{"action" => "do_not_retry"}
                     }}},
                   5_500

    assert kind in [:timeout, :transport_error]
    assert now() - started <= 5_500
    refute_receive {:access_dispatch, _, _, _, _, _, _, _}
  end

  test "owner death selects unknown effects for a dispatched Access write", %{
    id: id,
    state: state
  } do
    Agent.update(state, &%{&1 | response: :hold})
    parent = self()

    owner =
      spawn(fn ->
        {:ok, ref} = Auth.start_action(id, "access", "access.team_project.unassign", %{})
        send(parent, {:access_owned, ref})

        receive do
          :stop -> :ok
        end
      end)

    assert_receive {:access_owned, ref}, 1_000
    assert_receive {:access_dispatch, peer, _, _, _, _, _, _}, 1_000
    guardian = Process.whereis(Lifecycle)
    worker = :sys.get_state(guardian)[ref].worker
    worker_monitor = Process.monitor(worker)

    :erlang.trace_pattern(
      {Lifecycle, :select_terminal, 2},
      [{[ref, :_], [], [{:return_trace}]}],
      [:local]
    )

    :erlang.trace(guardian, true, [:call])

    try do
      Process.exit(owner, :kill)

      assert_receive {:trace, ^guardian, :return_from, {Lifecycle, :select_terminal, 2},
                      {:error,
                       %Error{
                         kind: :cancelled,
                         side_effects: "unknown",
                         recovery: %{"action" => "do_not_retry"}
                       }}},
                     1_000

      assert_receive {:DOWN, ^worker_monitor, :process, ^worker, _}, 1_000
      send(peer, {:respond, 200, "{}"})
      refute_receive {:access_dispatch, _, _, _, _, _, _, _}
    after
      :erlang.trace(guardian, false, [:call])
      :erlang.trace_pattern({Lifecycle, :select_terminal, 2}, false, [:local])
      if Process.alive?(owner), do: Process.exit(owner, :kill)
    end
  end

  test "selected terminal receipt excludes invitation input and records the operation-retention gap",
       %{id: id, state: state} do
    Agent.update(state, &%{&1 | response: :hold})
    token = String.duplicate("a5", 32)

    {:ok, ref} =
      Auth.start_action(id, "access", "access.team_invitation.accept", %{"token" => token})

    assert_receive {:access_dispatch, peer, _, _, _, _, _, _}, 1_000
    guardian = Process.whereis(Lifecycle)
    :sys.suspend(guardian)

    try do
      Lifecycle.select_terminal(
        ref,
        {:error,
         Error.new(:cancelled, "access", "access.team_invitation.accept", request_id: ref)}
      )

      result = Lifecycle.terminal_result(ref)
      refute inspect(result) =~ token
      [{{:operation, ^ref}, op}] = :ets.lookup(Lifecycle, {:operation, ref})

      IO.puts(
        "Access terminal retention observation. result_input=false operation_input=#{String.contains?(Map.get(op, :json) || "", token)}"
      )

      Lifecycle.notify(ref)
    after
      :sys.resume(guardian)
    end

    assert_receive {:gateway_action, ^ref, {:error, %Error{side_effects: "unknown"}}}, 1_000
    send(peer, {:respond, 200, "{}"})
  end

  test "forbidden committed-expiry contract preserves possible effects and login", %{
    id: id,
    state: state
  } do
    response =
      envelope("access.team_invitation.accept", "forbidden", %{
        "side_effects" => "possible",
        "recovery" => %{"action" => "do_not_retry", "same_arguments" => "never"}
      })

    Agent.update(state, &%{&1 | response: {403, Jason.encode!(response)}})

    assert {:error,
            %Error{
              kind: :forbidden,
              status: 403,
              side_effects: "possible",
              recovery: %{"action" => "do_not_retry", "same_arguments" => "never"}
            }} =
             Auth.action(id, "access", "access.team_invitation.accept", %{})

    assert {:ok, _} = Auth.current(id)
  end

  test "finite credential admission preserves backend unknown-action availability", %{
    id: id,
    state: state
  } do
    action = "access.gateway_credential.list"

    response =
      envelope(action, "unknown_action", %{
        "recovery" => %{"action" => "rediscover", "same_arguments" => "never"}
      })

    Agent.update(state, &%{&1 | response: {400, Jason.encode!(response)}})

    assert {:error, %Error{kind: :unknown_action, side_effects: "none_expected"}} =
             Auth.action(id, "access", action, %{})

    assert {:ok, _} = Auth.current(id)
  end

  test "four inventory conflicts retain only the fixed inventory discriminator", %{
    id: id,
    state: state
  } do
    for action <- @inventories do
      response =
        envelope(action, "conflict", %{
          "existing_id" => "access_inventory",
          "message" => "private-token",
          "diagnostic" => "private-token",
          "recovery" => %{"action" => "rediscover", "same_arguments" => "never"}
        })

      Agent.update(state, &%{&1 | response: {409, Jason.encode!(response)}})

      assert {:error,
              %Error{
                kind: :conflict,
                metadata: metadata,
                recovery: %{"action" => "rediscover", "same_arguments" => "never"}
              } = error} = Auth.action(id, "access", action, %{})

      assert metadata == %{"contract_version" => 1, "existing_id" => "access_inventory"}
      refute inspect(error) =~ "private-token"
    end
  end

  test "owner discriminator is restricted to actual last-owner mutation paths" do
    for action <-
          ~w(access.team.member.role.set access.team.member.suspend access.team.member.remove) do
      error =
        Error.from_response(
          409,
          envelope(action, "conflict", %{"existing_id" => "team_owner"}),
          "access",
          action,
          true
        )

      assert error.metadata["existing_id"] == "team_owner"
    end

    for {action, id, kind} <- [
          {"access.team.member.add", "team_owner", "conflict"},
          {"access.team.member.activate", "team_owner", "conflict"},
          {"access.team.create", "team_owner", "conflict"},
          {"access.team.member.list", "team_owner", "conflict"},
          {"access.team.member.remove", "access_inventory", "conflict"},
          {"access.team.list", "access_inventory", "conflict"},
          {"access.team.member.list", "arbitrary-private-id", "conflict"},
          {"access.team.member.list", "access_inventory", "invalid_param"}
        ] do
      error =
        Error.from_response(
          409,
          envelope(action, kind, %{"existing_id" => id}),
          "access",
          action,
          false
        )

      refute Map.has_key?(error.metadata, "existing_id")
    end
  end

  test "remote inventory overflow preserves only boolean terminal budget metadata", %{
    id: id,
    state: state
  } do
    for action <- @inventories, terminal <- [true, false] do
      recovery = if terminal, do: "inspect_and_escalate", else: "reduce_work"

      response =
        envelope(action, "response_too_large", %{
          "origin" => "budget",
          "budget_terminal" => terminal,
          "recovery" => %{"action" => recovery, "same_arguments" => "never"}
        })

      Agent.update(state, &%{&1 | response: {413, Jason.encode!(response)}})

      assert {:error,
              %Error{
                kind: :response_too_large,
                side_effects: "none_expected",
                metadata: metadata,
                recovery: decoded
              }} = Auth.action(id, "access", action, %{})

      assert metadata == %{
               "contract_version" => 1,
               "origin" => "budget",
               "budget_terminal" => terminal
             }

      assert decoded == %{"action" => recovery, "same_arguments" => "never"}
    end
  end

  test "invalid contracts and unsupported metadata cannot import private fields" do
    action = "access.team.member.list"

    data =
      envelope(action, "response_too_large", %{
        "budget_terminal" => true,
        "existing_id" => "access_inventory",
        "origin" => "sqlite-private",
        "param" => "private/token",
        "required_scopes" => ["private-token"],
        "message" => "private-token",
        "token" => "private-token",
        "recovery" => %{"guidance" => "private-token"}
      })

    for changed <- [
          Map.put(data, "contract_version", 2),
          Map.put(data, "contract_version", 1.0),
          Map.put(data, "contract_version", "1"),
          Map.put(data, "service", "projects"),
          Map.put(data, "action", "access.team.list")
        ] do
      error = Error.from_response(413, changed, "access", action, false)
      assert error.kind == :gateway_error and error.metadata == %{}
      refute inspect(error) =~ "private-token"
    end

    error = Error.from_response(413, data, "access", action, false)
    assert error.metadata == %{"contract_version" => 1, "budget_terminal" => true}
    refute inspect(error) =~ "private-token"

    for {service, action, kind, terminal} <- [
          {"access", action, "response_too_large", "true"},
          {"access", action, "response_too_large", 1},
          {"access", action, "conflict", true},
          {"access", "access.team.list", "response_too_large", true},
          {"access", "access.team.create", "response_too_large", true},
          {"projects", "access.team.member.list", "response_too_large", true}
        ] do
      data =
        envelope(action, kind, %{
          "service" => service,
          "budget_terminal" => terminal,
          "existing_id" => "access_inventory"
        })

      error = Error.from_response(413, data, service, action, false)
      refute Map.has_key?(error.metadata, "budget_terminal")
      if service != "access", do: refute(Map.has_key?(error.metadata, "existing_id"))
    end
  end

  defp envelope(action, kind, extra),
    do:
      Map.merge(
        %{
          "contract_version" => 1,
          "service" => "access",
          "action" => action,
          "kind" => kind,
          "side_effects" => "none_expected"
        },
        extra
      )

  defp now, do: System.monotonic_time(:millisecond)
end
