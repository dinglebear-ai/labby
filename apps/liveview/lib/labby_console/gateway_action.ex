defmodule LabbyConsole.GatewayAction do
  @moduledoc "Caller-owned, supervised gateway operations with immediate bounded admission."
  use GenServer

  @impl true
  def format_status(status),
    do: Map.merge(status, %{state: :redacted, message: :redacted, reason: :redacted, log: []})

  alias LabbyConsole.{Auth, GatewayAction.Error}

  @access_reads ~w(
    access.team.list
    access.project.effective.list
    access.gateway_credential.list
    access.team.member.list
    access.team_project.list
    access.team_invitation.list
    access.platform_admin.list
  )
  @access_mutations ~w(
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
  @actions %{
    "gateway" => ~w(gateway.get gateway.test gateway.add gateway.update gateway.remove),
    "projects" =>
      ~w(projects.list projects.get projects.create projects.update projects.archive projects.activate),
    "access" => @access_reads ++ @access_mutations ++ ["help"]
  }
  @reads %{
    "gateway" =>
      ~w(gateway.get gateway.list gateway.server.get gateway.discovered_tools gateway.discovered_resources gateway.discovered_prompts gateway.host.metrics gateway.clients.list gateway.mcp.list gateway.usage.metrics gateway.usage.calls),
    "doctor" => ~w(system.checks),
    "server_logs" => ~w(server_logs.query)
  }
  @gateway_mutations ~w(gateway.test gateway.add gateway.update gateway.remove)
  @mutations ~w(projects.create projects.update projects.archive projects.activate) ++
               @gateway_mutations ++ @access_mutations

  alias LabbyConsole.GatewayAction.Lifecycle
  def start_link(_), do: GenServer.start_link(__MODULE__, %{}, name: __MODULE__)
  @impl true
  def init(state), do: {:ok, state}

  def start(id, service, action, params, mode \\ :action) do
    budget = operation_budget(service, action, mode)
    destination = :erlang.alias([:reply])

    case do_start(id, service, action, params, mode, {:async, destination}, budget) do
      {:ok, _} = accepted ->
        accepted

      error ->
        :erlang.unalias(destination)
        error
    end
  end

  # Only Auth constructs this fixed family from configuration or captured vault state.
  # It shares the same reservation, watchdog, receipts and cancellation engine.
  @doc false
  def run_auth(kind, payload)
      when kind in [:exchange, :inspect, :logout, :handoff_start, :handoff_poll, :handoff_redeem] do
    run({:auth_owner, self()}, "auth", Atom.to_string(kind), %{}, {:auth, kind, payload})
  end

  defp do_start(id, "auth", action, %{}, {:auth, kind, payload}, delivery, budget)
       when kind in [:exchange, :inspect, :logout, :handoff_start, :handoff_poll, :handoff_redeem] do
    deadline = budget.deadline
    coordinator = Process.whereis(__MODULE__)

    with true <- is_pid(coordinator),
         {:ok, ref} <- Lifecycle.reserve(id, self(), deadline, coordinator) do
      op = %{
        id: id,
        ref: ref,
        owner: self(),
        coordinator: coordinator,
        generation: nil,
        auth_kind: kind,
        auth_payload: payload,
        service: "auth",
        action: action,
        mutation?: kind not in [:inspect, :handoff_poll],
        dispatch_gate: auth_gate(kind, payload),
        deadline: deadline,
        delivery: delivery
      }

      case Lifecycle.register(op) do
        :ok ->
          {:ok, ref}

        {:error, reason} ->
          Lifecycle.release(ref)
          {:error, Error.new(reason, "auth", action)}
      end
    else
      {:error, reason} -> {:error, Error.new(reason, "auth", action)}
      _ -> {:error, Error.new(:unavailable, "auth", action)}
    end
  end

  defp do_start(id, service, action, params, mode, delivery, budget) do
    deadline = budget.deadline
    policy = if mode == :read, do: @reads, else: @actions
    coordinator = Process.whereis(__MODULE__)

    cond do
      not is_binary(service) or not is_binary(action) or not is_map(params) or
        action not in Map.get(policy, service, []) or
          (service == "access" and action == "help" and map_size(params) != 0) ->
        {:error, Error.new(:not_allowed, nil, nil)}

      not is_pid(coordinator) ->
        {:error, Error.new(:unavailable, service, action)}

      true ->
        with %{generation: generation} = session <- Auth.capture(id),
             {:ok, ref} <- Lifecycle.reserve(id, self(), deadline, coordinator) do
          case Jason.encode(%{action: action, params: params}) do
            {:ok, json} when byte_size(json) <= 32_768 ->
              op = %{
                id: id,
                ref: ref,
                owner: self(),
                coordinator: coordinator,
                generation: generation,
                session: session,
                service: service,
                action: action,
                json: json,
                mutation?: action in @mutations,
                dispatch_gate: :atomics.new(1, signed: false),
                started_at: budget.started_at,
                deadline: deadline,
                receive_timeout: budget.receive_timeout,
                delivery: delivery
              }

              case Lifecycle.register(op) do
                :ok ->
                  {:ok, ref}

                {:error, reason} ->
                  Lifecycle.release(ref)
                  {:error, Error.new(reason, service, action)}
              end

            {:ok, _} ->
              Lifecycle.release(ref)
              {:error, Error.new(:request_too_large, service, action)}

            _ ->
              Lifecycle.release(ref)
              {:error, Error.new(:invalid_request, service, action)}
          end
        else
          nil -> {:error, Error.new(:unauthenticated, service, action)}
          {:error, reason} -> {:error, Error.new(reason, service, action)}
          _ -> {:error, Error.new(:unavailable, service, action)}
        end
    end
  end

  defp operation_budget(service, action, mode) do
    started = System.monotonic_time(:millisecond)

    {total, receive_timeout} =
      case {service, action} do
        {"gateway", action} when action in @gateway_mutations -> {30_000, 25_000}
        {"gateway", "gateway.host.metrics"} -> {12_000, 8_000}
        _ -> {5_000, 2_000}
      end

    deadline =
      case mode do
        {:auth, kind, payload} when kind in [:handoff_start, :handoff_poll, :handoff_redeem] ->
          min(started + total, payload.deadline)

        _ ->
          started + total
      end

    %{started_at: started, deadline: deadline, receive_timeout: receive_timeout}
  end

  @doc false
  def receive_timeout(op) do
    remaining = op.deadline - System.monotonic_time(:millisecond) - 250

    if remaining > 0,
      do: {:ok, min(op.receive_timeout, remaining)},
      else: {:error, :timeout}
  end

  defp auth_gate(kind, payload) when kind in [:handoff_start, :handoff_poll, :handoff_redeem],
    do: payload.network_gate

  defp auth_gate(_, _), do: :atomics.new(1, signed: false)

  def run(id, service, action, params, mode \\ :action) do
    budget = operation_budget(service, action, mode)

    alias_ref = :erlang.alias()

    try do
      with {:ok, ref} <- do_start(id, service, action, params, mode, {:sync, alias_ref}, budget) do
        result =
          receive do
            {:gateway_action_ready, ^ref, result} ->
              result

            {:gateway_action_ready, ^ref} ->
              Lifecycle.terminal_result(ref) ||
                {:error,
                 Error.new(:unavailable, service, action,
                   request_id: ref,
                   side_effects: if(action in @mutations, do: "unknown", else: "none_expected")
                 )}
          after
            max(0, budget.deadline - System.monotonic_time(:millisecond)) ->
              candidate =
                {:error,
                 Error.new(:timeout, service, action,
                   request_id: ref,
                   side_effects:
                     if(action in @mutations and Lifecycle.dispatched?(ref),
                       do: "unknown",
                       else: "none_expected"
                     )
                 )}

              winner = Lifecycle.select_terminal(ref, candidate)
              Lifecycle.notify(ref)
              winner
          end

        Lifecycle.ack(ref)
        complete_reply(alias_ref, ref, result)
      end
    after
      :erlang.unalias(alias_ref)
    end
  end

  @doc false
  def complete_reply(destination, ref, result) do
    :erlang.unalias(destination)
    drain_reply(ref)
    result
  end

  defp drain_reply(ref) do
    receive do
      {:gateway_action_ready, ^ref} -> drain_reply(ref)
      {:gateway_action_ready, ^ref, _result} -> drain_reply(ref)
    after
      0 -> :ok
    end
  end

  def cancel(id, ref), do: Lifecycle.cancel(id, ref)
  def revoke(id), do: Lifecycle.revoke(id)

  @impl true
  def handle_info({:admit, ref}, state) do
    Lifecycle.admit(ref, self())
    {:noreply, state}
  end
end
