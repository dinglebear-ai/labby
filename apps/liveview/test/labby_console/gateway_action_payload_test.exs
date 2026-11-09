defmodule LabbyConsole.GatewayActionPayloadTest do
  use ExUnit.Case, async: false
  alias LabbyConsole.{Auth, GatewayAction}
  alias LabbyConsole.GatewayAction.{Error, Lifecycle}

  setup do
    owner = self()

    fixture =
      start_supervised!({Agent, fn -> %{owner: owner, response: :hold, session_status: 200} end})

    server =
      start_supervised!(
        {Bandit, plug: {LabbyConsole.ActionFixture, state: fixture}, port: 0, ip: {127, 0, 0, 1}}
      )

    {:ok, {_, port}} = ThousandIsland.listener_info(server)
    previous = Application.get_env(:labby_console, :gateway_url)
    Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:#{port}")
    {:ok, id} = Auth.sign_in("payload-owner")
    on_exit(fn -> Application.put_env(:labby_console, :gateway_url, previous) end)

    %{
      id: id,
      fixture: fixture,
      token: "synthetic-request-" <> Base.encode16(:crypto.strong_rand_bytes(16))
    }
  end

  test "a synchronous response drops request data before its owner acknowledges", %{
    id: id,
    token: token
  } do
    {owner, ref, peer} = start_sync(id, token)

    try do
      send(peer, {:respond, 200, "{\"accepted\":true}"})
      await(fn -> Lifecycle.terminal_result(ref) != nil end)
      assert {:ok, %{"accepted" => true}} = Lifecycle.terminal_result(ref)
      assert_compact(ref, token)
      assert Lifecycle.reserved?(ref)
      await(fn -> :sys.get_state(Lifecycle)[ref].complete? end)
    after
      :erlang.resume_process(owner)
      send(peer, {:respond, 200, "{}"})
    end

    assert_receive {:sync_result, ^owner, {:ok, %{"accepted" => true}}}, 1000
    send(owner, :stop)
    await(fn -> not Lifecycle.reserved?(ref) end)
  end

  test "terminal selection scrubs ETS while its guardian is paused and late registration cannot restore it",
       %{id: id, token: token} do
    {:ok, ref} = Auth.start_action(id, "projects", "projects.create", %{"token" => token})
    assert_receive {:dispatch, peer, _, _, _, _}
    [{{:operation, ^ref}, original}] = :ets.lookup(Lifecycle, {:operation, ref})
    guardian = Process.whereis(Lifecycle)
    :ok = :sys.suspend(guardian)
    winner = {:ok, %{"accepted" => true}}

    try do
      assert ^winner = Lifecycle.select_terminal(ref, winner)
      assert_compact_ets(ref, token)
      assert :ok = Lifecycle.register(original)
      assert_compact_ets(ref, token)
      assert ^winner = Lifecycle.select_terminal(ref, {:error, :late_failure})
      assert Lifecycle.reserved?(ref)
    after
      :sys.resume(guardian)
    end

    assert_compact(ref, token)
    Lifecycle.notify(ref)
    assert_receive {:gateway_action, ^ref, ^winner}, 1000
    refute_receive {:gateway_action, ^ref, _}
    send(peer, {:respond, 200, "{}"})
    await(fn -> not Lifecycle.reserved?(ref) end)
  end

  test "normal async publication scrubs the request before guardian notification", %{
    id: id,
    token: token
  } do
    {:ok, ref} = Auth.start_action(id, "projects", "projects.create", %{"token" => token})
    assert_receive {:dispatch, peer, _, _, _, _}
    metadata = :sys.get_state(Lifecycle)[ref]
    retained? = retains_payload?(metadata, token)
    refute retained?
    guardian = Process.whereis(Lifecycle)
    :ok = :sys.suspend(guardian)

    try do
      send(peer, {:respond, 200, "{\"accepted\":true}"})
      await(fn -> Lifecycle.terminal_result(ref) != nil end)
      assert {:ok, %{"accepted" => true}} = Lifecycle.terminal_result(ref)
      assert_compact_ets(ref, token)
      assert Lifecycle.reserved?(ref)
    after
      :sys.resume(guardian)
    end

    assert_receive {:gateway_action, ^ref, {:ok, %{"accepted" => true}}}, 1000
    refute_receive {:gateway_action, ^ref, _}
    send(peer, {:respond, 200, "{}"})
    await(fn -> not Lifecycle.reserved?(ref) end)
  end

  for kind <- [:cancelled, :timeout] do
    test "#{kind} removes the request before delayed terminal notification", %{
      id: id,
      token: token
    } do
      {:ok, ref} = Auth.start_action(id, "projects", "projects.create", %{"token" => token})
      assert_receive {:dispatch, peer, _, _, _, _}
      guardian = Process.whereis(Lifecycle)
      :ok = :sys.suspend(guardian)
      kind = unquote(kind)
      candidate = {:error, Error.new(kind, "projects", "projects.create")}

      try do
        assert {:error, %{kind: ^kind, side_effects: "unknown"}} =
                 Lifecycle.select_terminal(ref, candidate)

        assert_compact_ets(ref, token)
        assert Lifecycle.dispatched?(ref)
        assert Lifecycle.reserved?(ref)
      after
        :sys.resume(guardian)
      end

      assert_compact(ref, token)
      Lifecycle.notify(ref)

      assert_receive {:gateway_action, ^ref, {:error, %{kind: ^kind, side_effects: "unknown"}}},
                     1000

      refute_receive {:gateway_action, ^ref, _}
      send(peer, {:respond, 200, "{}"})
      await(fn -> not Lifecycle.reserved?(ref) end)
    end
  end

  test "cancellation scrubs a preflight request without losing the dispatch fence", %{
    id: id,
    token: token,
    fixture: fixture
  } do
    Agent.update(fixture, &%{&1 | session_status: :hold})
    {:ok, ref} = Auth.start_action(id, "projects", "projects.create", %{"token" => token})
    assert_receive {:session_request, preflight}
    guardian = Process.whereis(Lifecycle)
    :ok = :sys.suspend(guardian)

    try do
      assert :ok = Auth.cancel_action(id, ref)
      assert_compact_ets(ref, token)
      refute Lifecycle.dispatched?(ref)
      send(preflight, {:session_status, 200})
    after
      :sys.resume(guardian)
    end

    assert_receive {:gateway_action, ^ref,
                    {:error, %{kind: :cancelled, side_effects: "none_expected"}}},
                   1000

    refute_receive {:dispatch, _, _, _, _, _}
  end

  test "recovery retains compact synchronous receipts and rejects stale payload restoration", %{
    id: id,
    token: token
  } do
    {owner, ref, peer} = start_sync(id, token)
    [{{:operation, ^ref}, original}] = :ets.lookup(Lifecycle, {:operation, ref})
    winner = {:ok, %{"accepted" => true}}

    try do
      assert ^winner = Lifecycle.select_terminal(ref, winner)
      previous = Process.whereis(Lifecycle)
      Process.exit(Process.whereis(GatewayAction), :kill)

      await(fn ->
        is_pid(Process.whereis(Lifecycle)) and Process.whereis(Lifecycle) != previous
      end)

      assert_compact(ref, token)
      assert :ok = Lifecycle.register(original)
      assert_compact(ref, token)
      assert :sys.get_state(Lifecycle)[ref].complete?
      assert ^winner = Lifecycle.terminal_result(ref)
      assert Lifecycle.reserved?(ref)
    after
      :erlang.resume_process(owner)
      send(peer, {:respond, 200, "{}"})
    end

    assert_receive {:sync_result, ^owner, ^winner}, 1000
    send(owner, :stop)
    refute_receive {:gateway_action, ^ref, _}
    await(fn -> not Lifecycle.reserved?(ref) end)
  end

  test "recovery scrubs an active request when no publisher selected a winner", %{
    id: id,
    token: token
  } do
    {owner, ref, peer} = start_sync(id, token)

    try do
      previous = Process.whereis(Lifecycle)
      Process.exit(Process.whereis(GatewayAction), :kill)

      await(fn ->
        is_pid(Process.whereis(Lifecycle)) and Process.whereis(Lifecycle) != previous
      end)

      assert_compact(ref, token)

      assert {:error, %{kind: :unavailable, side_effects: "unknown"}} =
               Lifecycle.terminal_result(ref)

      assert :sys.get_state(Lifecycle)[ref].complete?
      assert Lifecycle.reserved?(ref)
    after
      :erlang.resume_process(owner)
      send(peer, {:respond, 200, "{}"})
    end

    assert_receive {:sync_result, ^owner,
                    {:error, %{kind: :unavailable, side_effects: "unknown"}}},
                   1000

    send(owner, :stop)
    await(fn -> not Lifecycle.reserved?(ref) end)
  end

  test "auth payloads are absent from guardian metadata and terminal operation rows", %{
    token: token
  } do
    parent = self()
    :ok = :sys.suspend(GatewayAction)

    owner =
      spawn(fn ->
        result = GatewayAction.run_auth(:exchange, token)
        send(parent, {:auth_result, self(), match?({:ok, _}, result)})
        receive do: (:stop -> :ok)
      end)

    on_exit(fn -> Process.exit(owner, :kill) end)
    ref = await_ref(owner)
    true = :erlang.suspend_process(owner)

    try do
      :sys.resume(GatewayAction)
      await(fn -> Lifecycle.terminal_result(ref) != nil end)
      assert_compact(ref, token)
      await(fn -> :sys.get_state(Lifecycle)[ref].complete? end)
    after
      :erlang.resume_process(owner)
    end

    assert_receive {:auth_result, ^owner, true}, 1000
    send(owner, :stop)
    await(fn -> not Lifecycle.reserved?(ref) end)
  end

  test "recovery repairs a persisted winner whose publisher died before request scrubbing", %{
    id: id,
    token: token
  } do
    {owner, ref, peer} = start_sync(id, token)
    winner = {:ok, %{"accepted" => true}}
    parent = self()

    # Reproduce the durable boundary between the two ETS steps without a product hook.
    # The requesting owner stays alive while coordinator restart repairs the retained input.
    {publisher, monitor} =
      spawn_monitor(fn ->
        true = :ets.insert_new(Lifecycle, {{:result, ref}, winner})
        send(parent, {:winner_persisted, self()})
        receive do: (:continue -> Lifecycle.select_terminal(ref, winner))
      end)

    on_exit(fn -> Process.exit(publisher, :kill) end)

    try do
      assert_receive {:winner_persisted, ^publisher}
      Process.exit(publisher, :kill)
      assert_receive {:DOWN, ^monitor, :process, ^publisher, :killed}
      [{{:operation, ^ref}, interrupted}] = :ets.lookup(Lifecycle, {:operation, ref})
      retained? = retains_payload?(interrupted, token)
      assert retained?
      assert ^winner = Lifecycle.terminal_result(ref)

      previous = Process.whereis(Lifecycle)
      Process.exit(Process.whereis(GatewayAction), :kill)

      await(fn ->
        is_pid(Process.whereis(Lifecycle)) and Process.whereis(Lifecycle) != previous
      end)

      assert_compact(ref, token)
      assert ^winner = Lifecycle.terminal_result(ref)
      assert :sys.get_state(Lifecycle)[ref].complete?
      assert Lifecycle.reserved?(ref)
    after
      :erlang.resume_process(owner)
      send(peer, {:respond, 200, "{}"})
    end

    assert_receive {:sync_result, ^owner, ^winner}, 1000
    send(owner, :stop)
    await(fn -> not Lifecycle.reserved?(ref) end)
  end

  defp start_sync(id, token) do
    parent = self()

    owner =
      spawn(fn ->
        result = Auth.action(id, "projects", "projects.create", %{"token" => token})
        send(parent, {:sync_result, self(), result})
        receive do: (:stop -> :ok)
      end)

    assert_receive {:dispatch, peer, _, _, _, _}, 1000
    on_exit(fn -> Process.exit(owner, :kill) end)
    ref = await_ref(owner)
    true = :erlang.suspend_process(owner)
    {owner, ref, peer}
  end

  defp await_ref(owner) do
    await(fn ->
      Enum.any?(:ets.tab2list(Lifecycle), &match?({{:operation, _}, %{owner: ^owner}}, &1))
    end)

    [ref] = for {{:operation, ref}, %{owner: ^owner}} <- :ets.tab2list(Lifecycle), do: ref
    ref
  end

  defp assert_compact(ref, token) do
    op = :sys.get_state(Lifecycle)[ref]
    assert_compact_ets(ref, token)
    assert is_map(op)
    retained? = retains_payload?(op, token)
    refute retained?
  end

  defp assert_compact_ets(ref, token) do
    [{{:operation, ^ref}, op}] = :ets.lookup(Lifecycle, {:operation, ref})
    retained? = retains_payload?(op, token)
    refute retained?
    assert is_reference(op.dispatch_gate)
    assert is_pid(op.owner)
    assert is_pid(op.coordinator)
    assert is_reference(op.ref)
  end

  defp retains_payload?(op, token) do
    Map.has_key?(op, :json) or Map.has_key?(op, :session) or
      :binary.match(:erlang.term_to_binary(op), token) != :nomatch
  end

  defp await(predicate, attempts \\ 200)
  defp await(_, 0), do: flunk("operation did not reach the expected state")

  defp await(predicate, attempts) do
    unless predicate.() do
      Process.sleep(5)
      await(predicate, attempts - 1)
    end
  end
end
