defmodule LabbyConsole.GatewayActionRecoveryTest do
  use ExUnit.Case, async: false
  alias LabbyConsole.{Auth, GatewayAction}
  alias LabbyConsole.GatewayAction.Lifecycle

  setup do
    owner = self()

    state =
      start_supervised!(
        {Agent, fn -> %{owner: owner, response: :trickle, session_status: 200} end}
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
    %{id: id, state: state}
  end

  test "guardian restart stops all accepted writes before replacement admission and delivers once",
       %{id: id, state: state} do
    refs =
      for _ <- 1..4 do
        {:ok, ref} = Auth.start_action(id, "projects", "projects.create", %{})
        assert_receive {:dispatch, _, _, _, _, _}
        ref
      end

    workers = Task.Supervisor.children(GatewayAction.Tasks)
    monitors = Enum.map(workers, &Process.monitor/1)
    guardian = Process.whereis(Lifecycle)
    Process.exit(guardian, :kill)
    for monitor <- monitors, do: assert_receive({:DOWN, ^monitor, _, _, _}, 1000)

    for ref <- refs do
      assert_receive {:gateway_action, ^ref,
                      {:error, %{kind: :unavailable, side_effects: "unknown"}}},
                     1000

      refute_receive {:gateway_action, ^ref, _}
    end

    await_replacement(guardian)
    refute Enum.any?(workers, &Process.alive?/1)
    assert :ets.info(Lifecycle, :owner) != Process.whereis(Lifecycle)
    Agent.update(state, &%{&1 | response: {200, "{}"}})
    assert {:ok, %{}} = Auth.action(id, "projects", "projects.create", %{})
    assert {:ok, _} = Auth.current(id)
  end

  test "cancel and owner death during guardian replacement cannot release untracked work", %{
    id: id,
    state: state
  } do
    Agent.update(state, &%{&1 | response: :hold})
    {:ok, cancelled} = Auth.start_action(id, "projects", "projects.create", %{})
    assert_receive {:dispatch, _, _, _, _, _}
    {:ok, retained} = Auth.start_action(id, "projects", "projects.create", %{})
    assert_receive {:dispatch, _, _, _, _, _}
    parent = self()

    owner =
      spawn(fn ->
        send(parent, {:owned, Auth.start_action(id, "projects", "projects.create", %{})})

        receive do
          :stop -> :ok
        end
      end)

    assert_receive {:owned, {:ok, _}}
    assert_receive {:dispatch, _, _, _, _, _}
    workers = Task.Supervisor.children(GatewayAction.Tasks)
    monitors = Enum.map(workers, &Process.monitor/1)
    guardian = Process.whereis(Lifecycle)
    supervisor = guardian_parent(guardian)
    :ok = :sys.suspend(supervisor)

    try do
      death = Process.monitor(guardian)
      Process.exit(guardian, :kill)
      assert_receive {:DOWN, ^death, _, _, _}
      assert :ok = Auth.cancel_action(id, cancelled)
      Process.exit(owner, :kill)
      {:ok, pending} = Auth.start_action(id, "projects", "projects.create", %{})

      assert {:error, %{kind: :saturated}} =
               Auth.start_action(id, "projects", "projects.create", %{})

      refute_receive {:dispatch, _, _, _, _, _}
      send(self(), {:pending, pending})
    after
      :sys.resume(supervisor)
    end

    for monitor <- monitors, do: assert_receive({:DOWN, ^monitor, _, _, _}, 1000)
    assert_receive {:pending, pending}

    assert_receive {:gateway_action, ^cancelled,
                    {:error, %{kind: :cancelled, side_effects: "unknown"}}},
                   1000

    assert_receive {:gateway_action, ^retained,
                    {:error, %{kind: :unavailable, side_effects: "unknown"}}},
                   1000

    assert_receive {:gateway_action, ^pending, {:error, %{side_effects: "none_expected"}}}, 1000
    for ref <- [cancelled, retained, pending], do: refute_receive({:gateway_action, ^ref, _})
    await_replacement(guardian)
  end

  test "recovery preserves a selected winner when its notifier dies after storage", %{id: id} do
    {:ok, ref} = Auth.start_action(id, "projects", "projects.create", %{})
    assert_receive {:dispatch, _, _, _, _, _}
    winner = {:ok, %{"winner" => "stored-before-restart"}}
    assert ^winner = Lifecycle.select_terminal(ref, winner)
    guardian = Process.whereis(Lifecycle)
    :ok = :sys.suspend(guardian)
    Process.exit(guardian, :kill)
    assert_receive {:gateway_action, ^ref, ^winner}, 1000
    refute_receive {:gateway_action, ^ref, _}
    await_replacement(guardian)
  end

  test "recovery cannot duplicate an async reply already queued before guardian death", %{id: id} do
    {:ok, ref} = Auth.start_action(id, "projects", "projects.create", %{})
    assert_receive {:dispatch, _, _, _, _, _}

    [{{:operation, ^ref}, %{delivery: {:async, destination}}}] =
      :ets.lookup(Lifecycle, {:operation, ref})

    winner = {:ok, %{"winner" => "already-queued"}}
    assert ^winner = Lifecycle.select_terminal(ref, winner)
    guardian = Process.whereis(Lifecycle)
    :ok = :sys.suspend(guardian)
    send(destination, {:gateway_action, ref, winner})
    {:messages, queued} = Process.info(self(), :messages)
    assert Enum.count(queued, &(&1 == {:gateway_action, ref, winner})) == 1
    Process.exit(guardian, :kill)
    await_replacement(guardian)
    {:messages, recovered} = Process.info(self(), :messages)
    assert Enum.count(recovered, &match?({:gateway_action, ^ref, _}, &1)) == 1
    assert_receive {:gateway_action, ^ref, ^winner}
    refute_receive {:gateway_action, ^ref, _}
  end

  test "guardian loss during an authorized publication preserves unrelated login authority", %{
    id: id,
    state: state
  } do
    Agent.update(state, &%{&1 | response: :hold})
    {:ok, ref} = Auth.start_action(id, "projects", "projects.create", %{})
    assert_receive {:dispatch, peer, _, _, _, _}
    guardian = Process.whereis(Lifecycle)
    auth = Process.whereis(Auth)
    :ok = :sys.suspend(guardian)
    send(peer, {:respond, 200, "{}"})
    await_publish_call(guardian, ref)
    Process.exit(guardian, :kill)

    assert_receive {:gateway_action, ^ref,
                    {:error, %{kind: :unavailable, side_effects: "unknown"}}},
                   1000

    await_replacement(guardian)
    assert Process.whereis(Auth) == auth
    assert {:ok, _} = Auth.current(id)
    refute_receive {:gateway_action, ^ref, _}
  end

  test "a queued registration cannot reinstall an already recovered synchronous receipt", %{
    id: id
  } do
    parent = self()

    owner =
      spawn(fn ->
        result = Auth.action(id, "projects", "projects.create", %{})
        send(parent, {:owner_result, result})

        receive do
          :stop -> :ok
        end
      end)

    assert_receive {:dispatch, _, _, _, _, _}
    [ref] = for {{:slot, _}, ref, _, ^owner, _, _} <- :ets.tab2list(Lifecycle), do: ref
    [{{:operation, ^ref}, op}] = :ets.lookup(Lifecycle, {:operation, ref})
    winner = {:ok, %{"winner" => "retained-sync-receipt"}}
    assert ^winner = Lifecycle.select_terminal(ref, winner)
    true = :erlang.suspend_process(owner)

    try do
      guardian = Process.whereis(Lifecycle)
      Process.exit(guardian, :kill)
      await_replacement(guardian)
      before = :sys.get_state(Lifecycle)[ref]
      assert before.complete?
      assert :ok = Lifecycle.register(op)
      after_registration = :sys.get_state(Lifecycle)[ref]
      assert after_registration.complete?
      assert after_registration.owner_monitor == before.owner_monitor
      assert after_registration.coordinator_monitor == before.coordinator_monitor
      assert ^winner = Lifecycle.terminal_result(ref)
    after
      if Process.alive?(owner), do: :erlang.resume_process(owner)
    end

    assert_receive {:owner_result, ^winner}, 1000
    send(owner, :stop)
    refute_receive {:owner_result, _}
  end

  test "an accepted registration still reports failure if expiry reaped its ticket before delivery",
       %{id: id} do
    guardian = Process.whereis(Lifecycle)
    :ok = :sys.suspend(guardian)

    ref =
      try do
        {:ok, ref} = Auth.start_action(id, "projects", "projects.create", %{})
        # Inject the real reaper's cleanup at the queued-registration boundary.
        # The fixed production5s watchdog is separately covered without overrides.
        Lifecycle.release(ref)
        ref
      after
        :sys.resume(guardian)
      end

    assert_receive {:gateway_action, ^ref,
                    {:error, %{kind: :timeout, side_effects: "none_expected"}}},
                   1000

    refute_receive {:dispatch, _, _, _, _, _}
    refute_receive {:gateway_action, ^ref, _}
  end

  test "a reply queued before synchronous cleanup is drained without consuming unrelated messages",
       %{id: id} do
    deadline = System.monotonic_time(:millisecond) + 5000
    {:ok, ref} = Lifecycle.reserve(id, self(), deadline, Process.whereis(GatewayAction))
    on_exit(fn -> Lifecycle.release(ref) end)
    winner = {:ok, %{"winner" => "selected-before-cleanup"}}
    assert ^winner = Lifecycle.select_terminal(ref, winner)
    destination = :erlang.alias()
    other = make_ref()
    send(destination, {:gateway_action_ready, ref})
    send(destination, {:gateway_action_ready, ref})
    send(self(), {:gateway_action_ready, other})
    send(self(), :unrelated)
    {:messages, queued} = Process.info(self(), :messages)
    assert Enum.count(queued, &(&1 == {:gateway_action_ready, ref})) == 2

    assert ^winner =
             GatewayAction.complete_reply(destination, ref, Lifecycle.terminal_result(ref))

    refute_receive {:gateway_action_ready, ^ref}
    assert_receive {:gateway_action_ready, ^other}
    assert_receive :unrelated
    assert ^winner = Lifecycle.terminal_result(ref)
    send(destination, {:gateway_action_ready, ref})
    refute_receive {:gateway_action_ready, ^ref}
  end

  defp await_publish_call(pid, ref, attempts \\ 100)

  defp await_publish_call(_, _, 0),
    do: flunk("authorization did not reach the guardian publication boundary")

  defp await_publish_call(pid, ref, attempts) do
    {:messages, messages} = Process.info(pid, :messages)

    unless Enum.any?(messages, &match?({:"$gen_call", _, {:publish, ^ref, _}}, &1)) do
      receive do
      after
        10 -> await_publish_call(pid, ref, attempts - 1)
      end
    end
  end

  defp guardian_parent(pid) do
    {:dictionary, dictionary} = Process.info(pid, :dictionary)
    [parent | _] = Keyword.fetch!(dictionary, :"$ancestors")
    if is_pid(parent), do: parent, else: Process.whereis(parent)
  end

  defp await_replacement(old, remaining \\ 100)
  defp await_replacement(_, 0), do: flunk("transport did not restart")

  defp await_replacement(old, remaining) do
    guardian = Process.whereis(Lifecycle)
    coordinator = Process.whereis(GatewayAction)

    unless is_pid(guardian) and guardian != old and is_pid(coordinator) do
      receive do
      after
        10 -> await_replacement(old, remaining - 1)
      end
    end
  end
end
