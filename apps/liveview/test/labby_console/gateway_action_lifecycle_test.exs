defmodule LabbyConsole.GatewayActionLifecycleTest do
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
        {Bandit, plug: {LabbyConsole.ActionFixture, state: state}, port: 0, ip: {127, 0, 0, 1}}
      )

    {:ok, {_, port}} = ThousandIsland.listener_info(server)
    old = Application.get_env(:labby_console, :gateway_url)
    Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:#{port}")
    {:ok, id} = Auth.sign_in("alice")
    on_exit(fn -> Application.put_env(:labby_console, :gateway_url, old) end)
    %{id: id, state: state}
  end

  test "coordinator restart terminates every held write and retains one terminal receipt", %{
    id: id,
    state: state
  } do
    Agent.update(state, &%{&1 | response: :hold})

    refs =
      for _ <- 1..4 do
        {:ok, ref} = Auth.start_action(id, "projects", "projects.create", %{})
        assert_receive {:dispatch, _, _, _, _, _}
        ref
      end

    monitors = for pid <- Task.Supervisor.children(GatewayAction.Tasks), do: Process.monitor(pid)
    coordinator = Process.whereis(GatewayAction)
    monitor = Process.monitor(coordinator)
    Process.exit(coordinator, :kill)
    assert_receive {:DOWN, ^monitor, _, _, _}
    for child <- monitors, do: assert_receive({:DOWN, ^child, _, _, _}, 1000)

    for ref <- refs do
      assert_receive {:gateway_action, ^ref,
                      {:error, %{kind: :unavailable, side_effects: "unknown"}}},
                     1000

      refute_receive {:gateway_action, ^ref, _}
    end
  end

  @tag timeout: 10_000
  test "async absolute deadline and worker termination survive suspended admission", %{
    id: id,
    state: state
  } do
    Agent.update(state, &%{&1 | response: :trickle})
    started = System.monotonic_time(:millisecond)
    {:ok, ref} = Auth.start_action(id, "projects", "projects.create", %{})
    assert_receive {:dispatch, _, _, _, _, _}
    monitors = for pid <- Task.Supervisor.children(GatewayAction.Tasks), do: Process.monitor(pid)
    :ok = :sys.suspend(GatewayAction)

    try do
      assert_receive {:gateway_action, ^ref,
                      {:error, %{kind: :timeout, side_effects: "unknown"}}},
                     5500

      assert System.monotonic_time(:millisecond) - started <= 5500
      for child <- monitors, do: assert_receive({:DOWN, ^child, _, _, _}, 500)
    after
      :sys.resume(GatewayAction)
    end

    refute_receive {:gateway_action, ^ref, _}
  end

  test "owner death cancels HTTP work while admission remains suspended", %{id: id, state: state} do
    Agent.update(state, &%{&1 | response: :hold})
    parent = self()

    owner =
      spawn(fn ->
        send(parent, {:started, Auth.start_action(id, "projects", "projects.create", %{})})

        receive do
          :stop -> :ok
        end
      end)

    assert_receive {:started, {:ok, _}}
    assert_receive {:dispatch, _, _, _, _, _}
    monitors = for pid <- Task.Supervisor.children(GatewayAction.Tasks), do: Process.monitor(pid)
    :ok = :sys.suspend(GatewayAction)

    try do
      Process.exit(owner, :kill)
      for child <- monitors, do: assert_receive({:DOWN, ^child, _, _, _}, 1000)
    after
      :sys.resume(GatewayAction)
    end
  end

  test "burst reserves four per session and32 globally before the admission mailbox", %{
    state: state
  } do
    sessions =
      for caller <- 1..8 do
        {:ok, id} = Auth.sign_in("caller#{caller}")
        id
      end

    {:ok, overflow_id} = Auth.sign_in("overflow")
    :ok = :sys.suspend(GatewayAction)
    parent = self()

    owners =
      for id <- sessions, _ <- 1..4 do
        spawn(fn ->
          send(
            parent,
            {:reserved, self(), Auth.start_action(id, "projects", "projects.create", %{})}
          )

          receive do
            :stop -> :ok
          end
        end)
      end

    try do
      for _ <- owners, do: assert_receive({:reserved, _, {:ok, _}}, 1000)
      id = overflow_id
      started = System.monotonic_time(:millisecond)

      assert {:error, %{kind: :saturated, side_effects: "none_expected"}} =
               Auth.start_action(id, "projects", "projects.create", %{})

      assert System.monotonic_time(:millisecond) - started < 250

      {:message_queue_len, size} =
        Process.info(Process.whereis(GatewayAction), :message_queue_len)

      assert size <= 32
      {:messages, messages} = Process.info(Process.whereis(GatewayAction), :messages)
      assert Enum.all?(messages, &match?({:admit, ref} when is_reference(ref), &1))
    after
      for owner <- owners, do: Process.exit(owner, :kill)
      :sys.resume(GatewayAction)
    end

    Agent.update(state, &%{&1 | response: {200, "{}"}})
    refute_receive {:dispatch, _, _, _, _, _}
  end

  test "a session's fifth pending reservation is saturated before global capacity fills", %{
    id: id
  } do
    :ok = :sys.suspend(GatewayAction)

    refs =
      try do
        refs =
          for _ <- 1..4 do
            {:ok, ref} = Auth.start_action(id, "projects", "projects.create", %{})
            ref
          end

        started = System.monotonic_time(:millisecond)

        assert {:error, %{kind: :saturated, side_effects: "none_expected"}} =
                 Auth.start_action(id, "projects", "projects.create", %{})

        assert System.monotonic_time(:millisecond) - started < 250
        for ref <- refs, do: assert(:ok = Auth.cancel_action(id, ref))

        for ref <- refs,
            do:
              assert_receive(
                {:gateway_action, ^ref,
                 {:error, %{kind: :cancelled, side_effects: "none_expected"}}}
              )

        refs
      after
        :sys.resume(GatewayAction)
      end

    refute_receive {:dispatch, _, _, _, _, _}
    for ref <- refs, do: refute_receive({:gateway_action, ^ref, _})
    assert {:ok, %{}} = Auth.action(id, "projects", "projects.list", %{})
  end

  test "terminal arbitration stores the winner itself before any delivery", %{
    id: id,
    state: state
  } do
    Agent.update(state, &%{&1 | response: :hold})
    {:ok, ref} = Auth.start_action(id, "projects", "projects.create", %{})
    assert_receive {:dispatch, _, _, _, _, _}
    winner = {:ok, %{"created" => true}}
    assert ^winner = GatewayAction.Lifecycle.select_terminal(ref, winner)
    assert ^winner = GatewayAction.Lifecycle.select_terminal(ref, {:error, :timeout})
    assert ^winner = GatewayAction.Lifecycle.terminal_result(ref)
    assert :ok = Auth.cancel_action(id, ref)
    assert_receive {:gateway_action, ^ref, ^winner}
    refute_receive {:gateway_action, ^ref, _}
  end

  for transition <- [:deadline, :coordinator_down] do
    @tag timeout: 10_000
    test "synchronous return preserves a stored publication winner across #{transition}", %{
      id: id,
      state: state
    } do
      Agent.update(state, &%{&1 | response: :hold})
      parent = self()

      owner =
        spawn(fn ->
          result = Auth.action(id, "projects", "projects.create", %{})
          send(parent, {:returned, self(), result})

          receive do
            :probe -> send(parent, {:remaining, self(), Process.info(self(), :messages)})
          end
        end)

      assert_receive {:dispatch, _, _, _, _, _}

      [ref] =
        for {{:slot, _}, ref, _, ^owner, _, _} <- :ets.tab2list(GatewayAction.Lifecycle), do: ref

      winner = {:ok, %{"created" => true}}
      # The real arbitration primitive stores all winner data before any notification.
      # Pause its notifier so the caller reaches its deadline (or observes coordinator loss).
      assert ^winner = GatewayAction.Lifecycle.select_terminal(ref, winner)
      notifier = Process.whereis(GatewayAction.Lifecycle)
      :ok = :sys.suspend(notifier)

      try do
        if unquote(transition) == :coordinator_down,
          do: Process.exit(Process.whereis(GatewayAction), :kill)

        assert_receive {:returned, ^owner, ^winner}, 5500
      after
        resume_if_alive(notifier)
      end

      send(owner, :probe)
      assert_receive {:remaining, ^owner, {:messages, []}}
      refute_receive {:gateway_action, ^ref, _}
    end
  end

  @tag timeout: 10_000
  test "synchronous pre-dispatch deadline remains certain even if both notifiers pause", %{id: id} do
    :ok = :sys.suspend(GatewayAction)
    parent = self()

    owner =
      spawn(fn ->
        send(parent, {:returned_pending, Auth.action(id, "projects", "projects.create", %{})})
      end)

    try do
      await_registration(owner)
      :ok = :sys.suspend(GatewayAction.Lifecycle)

      try do
        assert_receive {:returned_pending,
                        {:error, %{kind: :timeout, side_effects: "none_expected"}}},
                       5500
      after
        :sys.resume(GatewayAction.Lifecycle)
      end
    after
      :sys.resume(GatewayAction)
    end

    refute_receive {:dispatch, _, _, _, _, _}
  end

  defp resume_if_alive(pid) do
    if Process.alive?(pid), do: :sys.resume(pid)
  catch
    :exit, _ -> :ok
  end

  defp await_registration(owner, attempts \\ 100)
  defp await_registration(_, 0), do: flunk("request was not registered")

  defp await_registration(owner, attempts) do
    state = :sys.get_state(GatewayAction.Lifecycle)

    unless Enum.any?(state, fn {_, op} -> op.owner == owner end) do
      receive do
      after
        10 -> await_registration(owner, attempts - 1)
      end
    end
  end

  test "pre-enqueue reservations expire and release session capacity while their owner lives", %{
    id: id
  } do
    deadline = System.monotonic_time(:millisecond) + 40

    for _ <- 1..4,
        do:
          assert(
            {:ok, _} =
              GatewayAction.Lifecycle.reserve(
                id,
                self(),
                deadline,
                Process.whereis(GatewayAction)
              )
          )

    receive do
    after
      100 -> :ok
    end

    # Drive the production reaper after the short synthetic reservation deadline;
    # synchronize its handler instead of assuming a25ms tick was scheduled by100ms.
    send(GatewayAction.Lifecycle, :reap)
    :sys.get_state(GatewayAction.Lifecycle)
    assert {:ok, %{}} = Auth.action(id, "projects", "projects.list", %{})
  end

  test "local session expiry broadcasts exactly once and drops the private snapshot", %{id: id} do
    :ok = Phoenix.PubSub.subscribe(LabbyConsole.PubSub, "console-session:" <> id)

    :sys.replace_state(Auth, fn state ->
      value = put_in(state[id], [:session, "expires_at"], System.system_time(:second) - 1)
      :ets.insert(Auth, {id, value})
      Map.put(state, id, value)
    end)

    assert {:ok, _} = Auth.sign_in("next")
    assert_receive {:console_session_revoked, ^id}
    assert is_nil(Auth.capture(id))
    assert {:error, :unauthenticated} = Auth.current(id)
    refute_receive {:console_session_revoked, ^id}
    assert {:error, :unauthenticated} = Auth.current(nil)
  end

  test "an unavailable coordinator never masquerades as invalid caller authority", %{id: id} do
    assert :ok = Supervisor.terminate_child(GatewayAction.Supervisor, GatewayAction)

    try do
      assert {:error, %{kind: :unavailable, side_effects: "none_expected"}} =
               Auth.action(id, "projects", "projects.create", %{})

      assert {:ok, _} = Auth.current(id)
    after
      assert {:ok, _} = Supervisor.restart_child(GatewayAction.Supervisor, GatewayAction)
    end
  end

  test "session deletion broadcasts once without exposing caller credentials", %{id: id} do
    :ok = Phoenix.PubSub.subscribe(LabbyConsole.PubSub, "console-session:" <> id)
    assert :ok = Auth.sign_out(id)
    assert_receive {:console_session_revoked, ^id}
    assert :ok = Auth.sign_out(id)
    refute_receive {:console_session_revoked, ^id}
  end
end
