defmodule LabbyConsole.AuthDeadlineTest do
  use ExUnit.Case, async: false
  alias LabbyConsole.Auth
  import Phoenix.ConnTest
  @endpoint LabbyConsoleWeb.Endpoint

  setup do
    owner = self()
    state = start_supervised!({Agent, fn -> %{owner: owner, modes: %{}} end})

    server =
      start_supervised!(
        {Bandit,
         plug: {LabbyConsole.AuthDeadlineFixture, state: state}, port: 0, ip: {127, 0, 0, 1}}
      )

    {:ok, {_, port}} = ThousandIsland.listener_info(server)
    old = Application.get_env(:labby_console, :gateway_url)
    Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:#{port}")
    on_exit(fn -> Application.put_env(:labby_console, :gateway_url, old) end)
    %{state: state}
  end

  test "sign-in trickle has one absolute five-second bound", %{state: state} do
    modes(state, %{"/auth/bearer-session" => :trickle})
    started = now()
    assert {:error, :sign_in_failed} = Auth.sign_in("fixture-secret")
    assert (now() - started) in 4_700..5_700
    refute_receive {:gateway_action_ready, _}
  end

  test "current trickle times out retaining authority", %{state: state} do
    {:ok, id} = Auth.sign_in("fixture-secret")
    modes(state, %{"/auth/session" => :trickle})
    started = now()
    assert {:error, :unavailable} = Auth.current(id)
    assert (now() - started) in 4_700..5_700
    assert Auth.capture(id)
  end

  test "logout revokes locally before its bounded remote attempt", %{state: state} do
    {:ok, id} = Auth.sign_in("fixture-secret")
    Phoenix.PubSub.subscribe(LabbyConsole.PubSub, "console-session:" <> id)
    modes(state, %{"/auth/logout" => :trickle})
    started = now()
    task = Task.async(fn -> Auth.sign_out(id) end)
    assert_receive {:console_session_revoked, ^id}, 500
    assert is_nil(Auth.capture(id))
    assert {:error, :revocation_unconfirmed} = Task.await(task, 6_000)
    assert (now() - started) in 4_700..5_700
  end

  test "exchange and inspection share the same deadline", %{state: state} do
    modes(state, %{"/auth/bearer-session" => {:delay, 1_500}, "/auth/session" => :trickle})
    started = now()
    assert {:error, :sign_in_failed} = Auth.sign_in("fixture-secret")
    assert (now() - started) in 4_700..5_700
  end

  test "ordinary auth worker dies with its requesting process", %{state: state} do
    modes(state, %{"/auth/bearer-session" => :trickle})
    owner = spawn(fn -> Auth.sign_in("fixture-secret") end)
    assert_receive {:auth_request, "/auth/bearer-session", _}, 1_000
    [worker] = Task.Supervisor.children(LabbyConsole.GatewayAction.Tasks)
    monitor = Process.monitor(worker)
    Process.exit(owner, :kill)
    assert_receive {:DOWN, ^monitor, :process, ^worker, _}, 1_000
    eventually(fn -> Task.Supervisor.children(LabbyConsole.GatewayAction.Tasks) == [] end)
    refute_receive {:gateway_action_ready, _}
  end

  test "coordinator suspension cannot extend ordinary auth deadline", %{state: state} do
    modes(state, %{"/auth/bearer-session" => :trickle})
    task = Task.async(fn -> Auth.sign_in("fixture-secret") end)
    assert_receive {:auth_request, "/auth/bearer-session", _}, 1_000
    coordinator = Process.whereis(LabbyConsole.GatewayAction)
    :sys.suspend(coordinator)

    try do
      assert {:error, :sign_in_failed} = Task.await(task, 5_700)
      eventually(fn -> Task.Supervisor.children(LabbyConsole.GatewayAction.Tasks) == [] end)
    after
      if Process.alive?(coordinator), do: :sys.resume(coordinator)
    end
  end

  test "coordinator replacement stops auth work without replay", %{state: state} do
    modes(state, %{"/auth/bearer-session" => :trickle})
    task = Task.async(fn -> Auth.sign_in("fixture-secret") end)
    assert_receive {:auth_request, "/auth/bearer-session", _}, 1_000
    [worker] = Task.Supervisor.children(LabbyConsole.GatewayAction.Tasks)
    monitor = Process.monitor(worker)
    Process.exit(Process.whereis(LabbyConsole.GatewayAction), :kill)
    assert {:error, :sign_in_failed} = Task.await(task, 2_000)
    assert_receive {:DOWN, ^monitor, :process, ^worker, _}, 1_000
    refute_receive {:auth_request, "/auth/bearer-session", _}, 200

    assert {:ok, _} =
             (
               modes(state, %{})
               Auth.sign_in("fixture-secret")
             )
  end

  test "pre-session operations share the global 32-ticket budget" do
    coordinator = Process.whereis(LabbyConsole.GatewayAction)
    :sys.suspend(coordinator)
    tasks = for _ <- 1..32, do: Task.async(fn -> Auth.sign_in("fixture-secret") end)

    try do
      eventually(fn -> slot_count() == 32 end)
      started = now()
      assert {:error, :sign_in_failed} = Auth.sign_in("fixture-secret")
      assert now() - started < 500
      refute_receive {:auth_request, _, _}
    after
      Enum.each(tasks, &Task.shutdown(&1, :brutal_kill))
      if Process.alive?(coordinator), do: :sys.resume(coordinator)
    end

    eventually(fn -> slot_count() == 0 end)
  end

  test "four public actions retain their reservation through session inspection" do
    {:ok, id} = Auth.sign_in("fixture-secret")

    refs =
      for _ <- 1..4 do
        assert {:ok, ref} = Auth.start_action(id, "projects", "projects.list", %{})
        ref
      end

    for ref <- refs, do: assert_receive({:gateway_action, ^ref, {:ok, %{}}}, 2_000)
    refute_receive {:gateway_action, _, _}
  end

  test "unavailable and forbidden inspection retain snapshots but 401 revokes", %{state: state} do
    {:ok, id} = Auth.sign_in("fixture-secret")

    for {status, reason} <- [{503, :unavailable}, {403, :forbidden}] do
      modes(state, %{"/auth/session" => status})
      assert {:error, ^reason} = Auth.current(id)
      assert Auth.capture(id)
    end

    modes(state, %{"/auth/session" => 401})
    assert {:error, :unauthenticated} = Auth.current(id)
    assert is_nil(Auth.capture(id))
  end

  test "malformed oversized and missing-cookie login fail closed", %{state: state} do
    for modes <- [
          %{"/auth/session" => :malformed},
          %{"/auth/session" => :oversized},
          %{"/auth/bearer-session" => :no_cookie}
        ] do
      modes(state, modes)
      assert {:error, :sign_in_failed} = Auth.sign_in("fixture-secret")
    end
  end

  test "a suspended publication guardian cannot block another caller's local logout", %{
    state: state
  } do
    {:ok, first} = Auth.sign_in("fixture-secret")
    {:ok, second} = Auth.sign_in("fixture-secret")
    Phoenix.PubSub.subscribe(LabbyConsole.PubSub, "console-session:" <> second)
    modes(state, %{"/v1/projects" => :hold, "/auth/logout" => :trickle})
    {:ok, ref} = Auth.start_action(first, "projects", "projects.create", %{})
    assert_receive {:auth_request, "/v1/projects", peer}, 1_000
    guardian = Process.whereis(LabbyConsole.GatewayAction.Lifecycle)
    :sys.suspend(guardian)
    send(peer, {:respond, 200, "{}"})

    task =
      try do
        eventually(fn ->
          {:messages, messages} = Process.info(guardian, :messages)

          Enum.any?(messages, fn
            {:"$gen_call", _, {:publish, ^ref, _}} -> true
            {:"$gen_cast", {:notify, ^ref}} -> true
            _ -> false
          end)
        end)

        task = Task.async(fn -> Auth.sign_out(second) end)
        assert_receive {:console_session_revoked, ^second}, 500
        assert is_nil(Auth.capture(second))
        assert Task.yield(task, 0) == nil
        task
      after
        if Process.alive?(guardian), do: :sys.resume(guardian)
      end

    assert {:error, :revocation_unconfirmed} = Task.await(task, 5_700)
    assert_receive {:gateway_action, ^ref, {:ok, %{}}}, 1_000
  end

  test "auth rejects response headers larger than 64 KiB", %{state: state} do
    for path <- ["/auth/bearer-session", "/auth/session"] do
      modes(state, %{path => :oversized_headers})
      assert {:error, :sign_in_failed} = Auth.sign_in("fixture-secret")
    end

    modes(state, %{})
    {:ok, id} = Auth.sign_in("fixture-secret")
    modes(state, %{"/auth/logout" => :oversized_headers})
    assert {:error, :revocation_unconfirmed} = Auth.sign_out(id)
  end

  test "ordinary auth keeps credential cookie and gateway CSRF out of browser HTML" do
    signed_in = post(build_conn(), "/sign-in", %{credential: "fixture-secret"})
    assert redirected_to(signed_in) == "/gateway"
    page = signed_in |> recycle() |> get("/gateway") |> html_response(200)

    for secret <- ["fixture-secret", "caller=deadline", "private-csrf"],
        do: refute(page =~ secret)

    signed_out = signed_in |> recycle() |> post("/sign-out")
    assert redirected_to(signed_out) == "/sign-in"
  end

  defp slot_count do
    :ets.tab2list(LabbyConsole.GatewayAction.Lifecycle)
    |> Enum.count(&match?({{:slot, _}, _, _, _, _, _}, &1))
  end

  defp eventually(fun, attempts \\ 100)
  defp eventually(fun, 0), do: assert(fun.())

  defp eventually(fun, attempts) do
    if fun.(),
      do: :ok,
      else:
        (
          Process.sleep(10)
          eventually(fun, attempts - 1)
        )
  end

  defp modes(state, modes), do: Agent.update(state, &%{&1 | modes: modes})
  defp now, do: System.monotonic_time(:millisecond)
end
