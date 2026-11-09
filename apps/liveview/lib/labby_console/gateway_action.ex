defmodule LabbyConsole.GatewayAction do
  @moduledoc "Caller-owned, supervised gateway operations with immediate bounded admission."
  use GenServer

  @impl true
  def format_status(status),
    do: Map.merge(status, %{state: :redacted, message: :redacted, reason: :redacted, log: []})

  alias LabbyConsole.{Auth, GatewayAction.Error}

  @actions %{
    "projects" =>
      ~w(projects.list projects.get projects.create projects.update projects.archive projects.activate),
    "access" => ~w(access.team.list)
  }
  @reads %{
    "gateway" =>
      ~w(gateway.list gateway.server.get gateway.discovered_tools gateway.discovered_resources gateway.discovered_prompts gateway.host.metrics gateway.clients.list gateway.mcp.list gateway.usage.metrics gateway.usage.calls),
    "doctor" => ~w(system.checks),
    "server_logs" => ~w(server_logs.query)
  }
  @mutations ~w(projects.create projects.update projects.archive projects.activate)

  alias LabbyConsole.GatewayAction.Lifecycle
  def start_link(_), do: GenServer.start_link(__MODULE__, %{}, name: __MODULE__)
  @impl true
  def init(state), do: {:ok, state}

  def start(id, service, action, params, mode \\ :action) do
    destination = :erlang.alias([:reply])

    case do_start(id, service, action, params, mode, {:async, destination}) do
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

  defp do_start(id, "auth", action, %{}, {:auth, kind, payload}, delivery)
       when kind in [:exchange, :inspect, :logout, :handoff_start, :handoff_poll, :handoff_redeem] do
    deadline = auth_deadline(kind, payload)
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

  defp do_start(id, service, action, params, mode, delivery) do
    started = System.monotonic_time(:millisecond)
    deadline = started + if(action == "gateway.host.metrics", do: 12_000, else: 5_000)
    policy = if mode == :read, do: @reads, else: @actions
    coordinator = Process.whereis(__MODULE__)

    cond do
      not is_binary(service) or not is_binary(action) or not is_map(params) or
          action not in Map.get(policy, service, []) ->
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
                deadline: deadline,
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

  defp auth_deadline(kind, payload) when kind in [:handoff_start, :handoff_poll, :handoff_redeem],
    do: min(System.monotonic_time(:millisecond) + 5_000, payload.deadline)

  defp auth_deadline(_, _), do: System.monotonic_time(:millisecond) + 5_000

  defp auth_gate(kind, payload) when kind in [:handoff_start, :handoff_poll, :handoff_redeem],
    do: payload.network_gate

  defp auth_gate(_, _), do: :atomics.new(1, signed: false)

  def run(id, service, action, params, mode \\ :action) do
    deadline =
      System.monotonic_time(:millisecond) +
        if(action == "gateway.host.metrics", do: 12_000, else: 5_000)

    alias_ref = :erlang.alias()

    try do
      with {:ok, ref} <- do_start(id, service, action, params, mode, {:sync, alias_ref}) do
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
            max(0, deadline - System.monotonic_time(:millisecond)) ->
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
