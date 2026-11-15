defmodule LabbyConsole.GatewayActionCancellationTest do
  use ExUnit.Case, async: false
  alias LabbyConsole.{Auth, GatewayAction.Error}
  alias LabbyConsole.GatewayAction.Lifecycle

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
    previous = Application.get_env(:labby_console, :gateway_url)
    Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:#{port}")
    {:ok, id} = Auth.sign_in("alice")
    Agent.update(state, &%{&1 | session_status: :hold})
    on_exit(fn -> Application.put_env(:labby_console, :gateway_url, previous) end)
    %{id: id, state: state}
  end

  test "cancellation selected during real preflight forbids HTTP dispatch before notify", %{
    id: id
  } do
    {:ok, ref} = Auth.start_action(id, "projects", "projects.create", %{"name" => "cancel-first"})
    assert_receive {:session_request, preflight}, 1000
    candidate = local_cancellation(ref)

    assert {:error, %{kind: :cancelled, side_effects: "none_expected"}} =
             Lifecycle.select_terminal(ref, candidate)

    # Force the legal preemption between cancellation selection and its notify.
    # The real HTTP worker resumes session inspection and attempts dispatch.
    send(preflight, {:session_status, 200})

    assert_receive {:gateway_action, ^ref,
                    {:error, %{kind: :cancelled, side_effects: "none_expected"}}},
                   1000

    refute_receive {:dispatch, _, _, _, _, _}
    Lifecycle.notify(ref)
    refute_receive {:gateway_action, ^ref, _}
  end

  test "dispatch granted before cancellation upgrades a stale preflight candidate to unknown", %{
    id: id,
    state: state
  } do
    Agent.update(state, &%{&1 | response: :hold})

    {:ok, ref} =
      Auth.start_action(id, "projects", "projects.create", %{"name" => "dispatch-first"})

    assert_receive {:session_request, preflight}, 1000
    candidate = local_cancellation(ref)
    refute Lifecycle.dispatched?(ref)
    send(preflight, {:session_status, 200})

    assert_receive {:dispatch, peer, "projects",
                    %{"action" => "projects.create", "params" => %{"name" => "dispatch-first"}},
                    _, _},
                   1000

    assert Lifecycle.dispatched?(ref)

    # The candidate was constructed before the grant. Selection must arbitrate
    # against that grant rather than preserve the candidate's stale certainty.
    assert {:error,
            %{
              kind: :cancelled,
              side_effects: "unknown",
              recovery: %{"same_arguments" => "discouraged"}
            }} = Lifecycle.select_terminal(ref, candidate)

    assert :ok = Auth.cancel_action(id, ref)

    assert_receive {:gateway_action, ^ref,
                    {:error, %{kind: :cancelled, side_effects: "unknown"}}},
                   1000

    send(peer, {:respond, 200, "{}"})
    refute_receive {:dispatch, _, _, _, _, _}
    refute_receive {:gateway_action, ^ref, _}
  end

  defp local_cancellation(ref) do
    {:error,
     Error.new(:cancelled, "projects", "projects.create",
       request_id: ref,
       side_effects: "none_expected"
     )}
  end
end
