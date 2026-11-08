defmodule LabbyConsole.GatewayAction do
  @moduledoc "Caller-owned, supervised gateway operations with immediate bounded admission."
  use GenServer
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

  def start_link(_), do: GenServer.start_link(__MODULE__, %{}, name: __MODULE__)
  @impl true
  def init(state), do: {:ok, state}

  def start(id, service, action, params, mode \\ :action),
    do: do_start(id, service, action, params, mode, :atomics.new(1, signed: false))

  defp do_start(id, service, action, params, mode, delivery) do
    started = System.monotonic_time(:millisecond)
    policy = if mode == :read, do: @reads, else: @actions

    cond do
      not is_binary(service) or not is_binary(action) or not is_map(params) or
          action not in Map.get(policy, service, []) ->
        {:error, Error.new(:not_allowed, nil, nil)}

      true ->
        with {:ok, json} <- Jason.encode(%{action: action, params: params}),
             true <- byte_size(json) <= 32_768,
             %{generation: generation} = session <- Auth.capture(id) do
          operation = %{
            id: id,
            generation: generation,
            session: session,
            service: service,
            action: action,
            json: json,
            mutation?: action in @mutations,
            started: started,
            delivery: delivery,
            deadline: started + if(action == "gateway.host.metrics", do: 12_000, else: 5_000)
          }

          try do
            GenServer.call(
              __MODULE__,
              {:start, operation},
              max(1, operation.deadline - System.monotonic_time(:millisecond))
            )
          catch
            :exit, {:timeout, _} -> admission_failure(operation, :timeout)
            :exit, _ -> admission_failure(operation, :unavailable)
          end
        else
          false -> {:error, Error.new(:request_too_large, service, action)}
          nil -> {:error, Error.new(:unauthenticated, service, action)}
          _ -> {:error, Error.new(:invalid_request, service, action)}
        end
    end
  end

  def run(id, service, action, params, mode \\ :action) do
    deadline =
      System.monotonic_time(:millisecond) +
        if(action == "gateway.host.metrics", do: 12_000, else: 5_000)

    delivery = :atomics.new(1, signed: false)

    with {:ok, ref} <- do_start(id, service, action, params, mode, delivery) do
      pid = Process.whereis(__MODULE__)

      if is_pid(pid) do
        monitor = Process.monitor(pid)

        result =
          receive do
            {:gateway_action, ^ref, result} -> result
            {:DOWN, ^monitor, _, _, _} -> operation_failure(service, action, :unavailable)
          after
            max(0, deadline - System.monotonic_time(:millisecond)) ->
              claim_delivery(delivery)
              GenServer.cast(__MODULE__, {:cancel, id, ref})
              operation_failure(service, action, :timeout)
          end

        Process.demonitor(monitor, [:flush])
        result
      else
        operation_failure(service, action, :unavailable)
      end
    end
  end

  defp admission_failure(op, kind) do
    claim_delivery(op.delivery)
    operation_failure(op.service, op.action, kind)
  end

  defp operation_failure(service, action, kind),
    do:
      {:error,
       Error.new(kind, service, action,
         side_effects: if(action in @mutations, do: "unknown", else: "none_expected")
       )}

  def cancel(id, ref), do: GenServer.call(__MODULE__, {:cancel, id, ref})
  def revoke(id), do: GenServer.cast(__MODULE__, {:revoke, id})

  @doc false
  def claim_delivery(delivery), do: :atomics.compare_exchange(delivery, 1, 0, 1)

  @impl true
  def handle_call({:start, op}, {owner, _}, state) do
    cond do
      System.monotonic_time(:millisecond) >= op.deadline ->
        {:reply, {:error, Error.new(:timeout, op.service, op.action)}, state}

      map_size(state) >= 32 or Enum.count(state, fn {_, value} -> value.id == op.id end) >= 4 ->
        {:reply, {:error, Error.new(:saturated, op.service, op.action)}, state}

      not Auth.matches?(op.id, op.generation) ->
        {:reply, {:error, Error.new(:unauthenticated, op.service, op.action)}, state}

      true ->
        ref = make_ref()
        coordinator = self()

        case Task.Supervisor.start_child(LabbyConsole.GatewayAction.Tasks, fn ->
               result =
                 try do
                   Auth.execute_action(op, coordinator, ref)
                 rescue
                   _ -> failure(Map.put(op, :dispatched?, true), :transport_error)
                 end

               result =
                 case result do
                   {:ok, _} ->
                     {:error, error} = failure(Map.put(op, :dispatched?, true), :unauthenticated)
                     revoked = {:error, %{error | request_id: ref}}

                     Auth.publish_action(
                       op.id,
                       op.generation,
                       owner,
                       ref,
                       result,
                       revoked,
                       op.deadline,
                       op.delivery
                     )

                     :published

                   other ->
                     other
                 end

               send(coordinator, {ref, result})
             end) do
          {:ok, pid} ->
            timer =
              Process.send_after(
                self(),
                {:deadline, ref},
                max(0, op.deadline - System.monotonic_time(:millisecond))
              )

            value =
              Map.merge(op, %{
                owner: owner,
                owner_monitor: Process.monitor(owner),
                task: %{pid: pid, ref: Process.monitor(pid)},
                timer: timer,
                dispatched?: false
              })

            {:reply, {:ok, ref}, Map.put(state, ref, value)}

          {:error, :max_children} ->
            {:reply, {:error, Error.new(:saturated, op.service, op.action)}, state}

          {:error, _} ->
            {:reply, {:error, Error.new(:unavailable, op.service, op.action)}, state}
        end
    end
  end

  def handle_call({:dispatch, ref}, _, state) do
    case state[ref] do
      nil ->
        {:reply, :cancelled, state}

      op ->
        if Auth.matches?(op.id, op.generation) and
             System.monotonic_time(:millisecond) < op.deadline,
           do: {:reply, :ok, Map.put(state, ref, %{op | dispatched?: true})},
           else: {:reply, :cancelled, state}
    end
  end

  def handle_call({:cancel, id, ref}, _, state) do
    case state[ref] do
      %{id: ^id} -> {:reply, :ok, finish(state, ref, :cancelled)}
      _ -> {:reply, {:error, :not_found}, state}
    end
  end

  @impl true
  def handle_cast({:cancel, id, ref}, state) do
    case state[ref] do
      %{id: ^id} -> {:noreply, finish(state, ref, :cancelled)}
      _ -> {:noreply, state}
    end
  end

  def handle_cast({:revoke, id}, state) do
    refs = for {ref, %{id: ^id}} <- state, do: ref
    {:noreply, Enum.reduce(refs, state, &finish(&2, &1, :unauthenticated))}
  end

  @impl true
  def handle_info({task_ref, result}, state) when is_reference(task_ref) do
    case Map.fetch(state, task_ref) do
      :error ->
        {:noreply, state}

      {:ok, op} ->
        ref = task_ref

        result =
          cond do
            not Auth.matches?(op.id, op.generation) -> failure(op, :unauthenticated)
            System.monotonic_time(:millisecond) >= op.deadline -> failure(op, :timeout)
            true -> result
          end

        {:noreply, finish(state, ref, result)}
    end
  end

  def handle_info({:deadline, ref}, state), do: {:noreply, finish(state, ref, :timeout)}

  def handle_info({:DOWN, monitor, :process, _, _}, state) do
    case Enum.find(state, fn {_, op} -> op.owner_monitor == monitor or op.task.ref == monitor end) do
      nil ->
        {:noreply, state}

      {ref, op} ->
        {:noreply,
         finish(
           state,
           ref,
           if(op.owner_monitor == monitor, do: :cancelled, else: :transport_error)
         )}
    end
  end

  defp finish(state, ref, result) do
    case Map.pop(state, ref) do
      {nil, state} ->
        state

      {op, state} ->
        Process.cancel_timer(op.timer)
        Process.demonitor(op.owner_monitor, [:flush])
        # Never block the admission server waiting for an upstream worker to terminate.
        Process.demonitor(op.task.ref, [:flush])
        Process.exit(op.task.pid, :kill)

        result =
          if is_atom(result) and result != :published, do: failure(op, result), else: result

        result =
          case result do
            {:error, %Error{} = error} -> {:error, %{error | request_id: ref}}
            value -> value
          end

        if result != :published and claim_delivery(op.delivery) == :ok,
          do: send(op.owner, {:gateway_action, ref, result})

        state
    end
  end

  defp failure(op, kind),
    do:
      {:error,
       Error.new(kind, op.service, op.action,
         side_effects: if(op.mutation? and op.dispatched?, do: "unknown", else: "none_expected")
       )}
end
