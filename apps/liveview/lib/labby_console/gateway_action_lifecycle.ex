defmodule LabbyConsole.GatewayAction.Lifecycle do
  @moduledoc "Bounded operation reservations, durable terminal outcomes and worker ownership."
  use GenServer
  alias LabbyConsole.{Auth, GatewayAction, GatewayAction.Error}

  def start_link(_), do: GenServer.start_link(__MODULE__, %{}, name: __MODULE__)
  @impl true
  def init(state) do
    true = :ets.info(__MODULE__, :owner) == Process.whereis(LabbyConsole.Supervisor)
    state = recover_receipts(state)
    Process.send_after(self(), :reap, 25)
    {:ok, state}
  end

  # Fixed global slots are reserved before any body reaches a process mailbox.
  # Each global slot can own only one of the four atomic per-session slots.
  def reserve(id, owner, deadline, coordinator) do
    ref = make_ref()

    slot =
      Enum.find_value(1..32, fn index ->
        row = {{:slot, index}, ref, id, owner, deadline, coordinator}
        if :ets.insert_new(__MODULE__, row), do: row
      end)

    if slot do
      session_slot =
        Enum.find_value(1..4, fn index ->
          row = {{:session, id, index}, ref}
          if :ets.insert_new(__MODULE__, row), do: row
        end)

      if not is_nil(session_slot) and reserved?(ref) and
           System.monotonic_time(:millisecond) < deadline do
        {:ok, ref}
      else
        release(ref)
        {:error, :saturated}
      end
    else
      {:error, :saturated}
    end
  rescue
    ArgumentError -> {:error, :unavailable}
  end

  def release(ref) do
    :ets.delete(__MODULE__, {:operation, ref})
    # Delete the global ticket before session rows. A reserver racing expiry
    # fails its final ticket check and cleans its own session row.
    for index <- 1..32 do
      case :ets.lookup(__MODULE__, {:slot, index}) do
        [{_, ^ref, _, _, _, _} = row] -> :ets.delete_object(__MODULE__, row)
        _ -> :ok
      end
    end

    for row <- :ets.tab2list(__MODULE__),
        reserved_row?(row, ref),
        do: :ets.delete_object(__MODULE__, row)

    :ets.delete(__MODULE__, {:result, ref})
    :ets.delete(__MODULE__, {:dispatched, ref})
    :ets.delete(__MODULE__, {:acked, ref})
    :ok
  rescue
    ArgumentError -> :ok
  end

  defp reserved_row?({{:slot, _}, ref, _, _, _, _}, ref), do: true
  defp reserved_row?({{:session, _, _}, ref}, ref), do: true
  defp reserved_row?(_, _), do: false

  def reserved?(ref) do
    Enum.any?(1..32, fn index ->
      case :ets.lookup(__MODULE__, {:slot, index}) do
        [{_, ^ref, _, _, _, _}] -> true
        _ -> false
      end
    end)
  rescue
    ArgumentError -> false
  end

  # Insert the complete immutable result atomically, never a claim without data.
  def select_terminal(ref, candidate) do
    case terminal_result(ref) do
      nil ->
        if reserved?(ref) do
          :ets.insert_new(__MODULE__, {{:result, ref}, candidate})
          terminal_result(ref)
        else
          candidate
        end

      winner ->
        winner
    end
  rescue
    ArgumentError -> candidate
  end

  def terminal_result(ref) do
    case :ets.lookup(__MODULE__, {:result, ref}) do
      [{{:result, ^ref}, result}] -> result
      [] -> nil
    end
  rescue
    ArgumentError -> nil
  end

  def dispatched?(ref) do
    :ets.member(__MODULE__, {:dispatched, ref})
  rescue
    ArgumentError -> true
  end

  def register(op) do
    if reserved?(op.ref) do
      :ets.insert(__MODULE__, {{:operation, op.ref}, op})

      if reserved?(op.ref) do
        GenServer.cast(__MODULE__, {:register, op})
        :ok
      else
        :ets.delete(__MODULE__, {:operation, op.ref})
        {:error, :unavailable}
      end
    else
      {:error, :unavailable}
    end
  rescue
    ArgumentError -> {:error, :unavailable}
  end

  def admit(ref, coordinator), do: GenServer.cast(__MODULE__, {:admit, ref, coordinator})
  def notify(ref), do: GenServer.cast(__MODULE__, {:notify, ref})

  def ack(ref) do
    if reserved?(ref), do: :ets.insert(__MODULE__, {{:acked, ref}, true})
    GenServer.cast(__MODULE__, {:ack, ref})
  rescue
    ArgumentError -> :ok
  end

  def cancel(id, ref) do
    case :ets.lookup(__MODULE__, {:operation, ref}) do
      [{{:operation, ^ref}, %{id: ^id} = op}] ->
        select_terminal(ref, failure(Map.put(op, :dispatched?, dispatched?(ref)), :cancelled))
        notify(ref)
        :ok

      _ ->
        {:error, :not_found}
    end
  rescue
    ArgumentError -> {:error, :not_found}
  end

  def revoke(id), do: GenServer.cast(__MODULE__, {:revoke, id})

  def publish(ref, result) do
    GenServer.call(__MODULE__, {:publish, ref, result})
  catch
    :exit, _ -> {:error, :unavailable}
  end

  @impl true
  def handle_cast({:register, op}, state) do
    cond do
      Map.has_key?(state, op.ref) ->
        {:noreply, state}

      reserved?(op.ref) ->
        value =
          Map.merge(op, %{
            owner_monitor: Process.monitor(op.owner),
            coordinator_monitor: Process.monitor(op.coordinator),
            timer: Process.send_after(self(), {:deadline, op.ref}, max(0, op.deadline - now())),
            worker: nil,
            worker_monitor: nil,
            dispatched?: false,
            complete?: false,
            acked?: false
          })

        send(op.coordinator, {:admit, op.ref})
        {:noreply, Map.put(state, op.ref, value)}

      true ->
        # Expiry can retire a pre-registration ticket while its bounded cast
        # is queued. An accepted caller must still receive a terminal receipt.
        result = failure(Map.put(op, :dispatched?, false), :timeout)

        case op.delivery do
          {:async, destination} -> send(destination, {:gateway_action, op.ref, result})
          {:sync, destination} -> send(destination, {:gateway_action_ready, op.ref, result})
        end

        {:noreply, state}
    end
  end

  def handle_cast({:admit, ref, coordinator}, state) do
    case state[ref] do
      %{coordinator: ^coordinator, complete?: false, worker: nil} = op ->
        cond do
          now() >= op.deadline ->
            {:noreply, finish(state, ref, failure(op, :timeout))}

          not Process.alive?(op.owner) ->
            {:noreply, finish(state, ref, failure(op, :cancelled))}

          not Process.alive?(coordinator) ->
            {:noreply, finish(state, ref, failure(op, :unavailable))}

          not Auth.matches?(op.id, op.generation) ->
            {:noreply, finish(state, ref, failure(op, :unauthenticated))}

          true ->
            lifecycle = self()

            case Task.Supervisor.start_child(GatewayAction.Tasks, fn ->
                   result =
                     try do
                       Auth.execute_action(op, lifecycle, ref)
                     rescue
                       _ -> failure(%{op | dispatched?: true}, :transport_error)
                     end

                   case result do
                     {:ok, _} -> Auth.publish_action(op, result)
                     other -> send(lifecycle, {:result, ref, other})
                   end
                 end) do
              {:ok, worker} ->
                {:noreply,
                 Map.put(state, ref, %{
                   op
                   | worker: worker,
                     worker_monitor: Process.monitor(worker)
                 })}

              {:error, :max_children} ->
                {:noreply, finish(state, ref, failure(op, :saturated))}

              {:error, _} ->
                {:noreply, finish(state, ref, failure(op, :unavailable))}
            end
        end

      _ ->
        {:noreply, state}
    end
  end

  def handle_cast({:notify, ref}, state) do
    case {state[ref], terminal_result(ref)} do
      {nil, _} -> {:noreply, state}
      {_, nil} -> {:noreply, state}
      {_, result} -> {:noreply, finish(state, ref, result)}
    end
  end

  def handle_cast({:ack, ref}, state), do: {:noreply, acknowledge(state, ref)}

  def handle_cast({:revoke, id}, state) do
    refs = for {ref, %{id: ^id, complete?: false}} <- state, do: ref

    {:noreply,
     Enum.reduce(refs, state, fn ref, acc ->
       finish(acc, ref, failure(acc[ref], :unauthenticated))
     end)}
  end

  @impl true
  def handle_call({:dispatch, ref}, _, state) do
    case state[ref] do
      %{complete?: false} = op ->
        if now() < op.deadline and Process.alive?(op.coordinator) and Process.alive?(op.owner) and
             Auth.matches?(op.id, op.generation) do
          :ets.insert(__MODULE__, {{:dispatched, ref}, true})
          {:reply, :ok, Map.put(state, ref, %{op | dispatched?: true})}
        else
          {:reply, :cancelled, state}
        end

      _ ->
        {:reply, :cancelled, state}
    end
  end

  def handle_call({:cancel, id, ref}, _, state) do
    case state[ref] do
      %{id: ^id} = op -> {:reply, :ok, finish(state, ref, failure(op, :cancelled))}
      _ -> {:reply, {:error, :not_found}, state}
    end
  end

  def handle_call({:publish, ref, result}, _, state) do
    case state[ref] do
      %{complete?: false} = op ->
        candidate =
          cond do
            now() >= op.deadline -> failure(op, :timeout)
            not Process.alive?(op.coordinator) -> failure(op, :unavailable)
            not Process.alive?(op.owner) -> failure(op, :cancelled)
            true -> result
          end

        {:reply, :ok, finish(state, ref, candidate)}

      _ ->
        {:reply, :ok, state}
    end
  end

  @impl true
  def handle_info({:result, ref, result}, state) do
    case state[ref] do
      %{complete?: false} = op ->
        result =
          cond do
            now() >= op.deadline -> failure(op, :timeout)
            not Auth.matches?(op.id, op.generation) -> failure(op, :unauthenticated)
            true -> result
          end

        {:noreply, finish(state, ref, result)}

      _ ->
        {:noreply, state}
    end
  end

  def handle_info({:deadline, ref}, state) do
    case state[ref] do
      %{complete?: false} = op -> {:noreply, finish(state, ref, failure(op, :timeout))}
      _ -> {:noreply, state}
    end
  end

  def handle_info({:DOWN, monitor, :process, _, _}, state) do
    case Enum.find(state, fn {_, op} ->
           monitor in [op.owner_monitor, op.coordinator_monitor, op.worker_monitor]
         end) do
      nil ->
        {:noreply, state}

      {ref, op} ->
        cond do
          monitor == op.worker_monitor ->
            state = Map.put(state, ref, %{op | worker: nil, worker_monitor: nil})

            state =
              if op.complete?, do: state, else: finish(state, ref, failure(op, :transport_error))

            {:noreply, retire(state, ref)}

          monitor == op.owner_monitor ->
            state = if op.complete?, do: state, else: finish(state, ref, failure(op, :cancelled))
            {:noreply, acknowledge(state, ref)}

          true ->
            state =
              if op.complete?, do: state, else: finish(state, ref, failure(op, :unavailable))

            {:noreply, state}
        end
    end
  end

  def handle_info(:reap, state) do
    for index <- 1..32 do
      case :ets.lookup(__MODULE__, {:slot, index}) do
        [{_, ref, _, owner, deadline, _}] ->
          if not Map.has_key?(state, ref) and (not Process.alive?(owner) or now() >= deadline),
            do: release(ref)

        _ ->
          :ok
      end
    end

    Process.send_after(self(), :reap, 25)
    {:noreply, state}
  end

  defp finish(state, ref, candidate) do
    op = state[ref]

    if op.complete? do
      state
    else
      candidate =
        case candidate do
          {:error, %Error{} = error} -> {:error, %{error | request_id: ref}}
          other -> other
        end

      result = select_terminal(ref, candidate)
      if is_reference(op.timer), do: Process.cancel_timer(op.timer)
      if op.worker, do: Process.exit(op.worker, :kill)

      case op.delivery do
        {:async, destination} -> send(destination, {:gateway_action, ref, result})
        {:sync, alias_ref} -> send(alias_ref, {:gateway_action_ready, ref})
      end

      op = %{op | complete?: true, acked?: match?({:async, _}, op.delivery)}
      retire(Map.put(state, ref, op), ref)
    end
  end

  defp acknowledge(state, ref) do
    case state[ref] do
      nil -> state
      op -> retire(Map.put(state, ref, %{op | acked?: true}), ref)
    end
  end

  defp retire(state, ref) do
    case state[ref] do
      %{complete?: true, acked?: true, worker: nil} = op ->
        Process.demonitor(op.owner_monitor, [:flush])
        Process.demonitor(op.coordinator_monitor, [:flush])
        release(ref)
        Map.delete(state, ref)

      _ ->
        state
    end
  end

  # one_for_all restarts the HTTP supervisor before this child. Every original
  # worker is therefore down before recovery can release capacity or notify.
  defp recover_receipts(state) do
    entries = for {{:operation, ref}, op} <- :ets.tab2list(__MODULE__), do: {ref, op}

    state =
      Enum.reduce(entries, state, fn {ref, op}, acc ->
        cond do
          not Process.alive?(op.owner) or :ets.member(__MODULE__, {:acked, ref}) ->
            release(ref)
            acc

          true ->
            recovered =
              Map.merge(op, %{
                owner_monitor: Process.monitor(op.owner),
                coordinator_monitor: Process.monitor(op.coordinator),
                timer: nil,
                worker: nil,
                worker_monitor: nil,
                dispatched?: dispatched?(ref),
                complete?: false,
                acked?: false
              })

            finish(Map.put(acc, ref, recovered), ref, failure(recovered, :unavailable))
        end
      end)

    # Callers interrupted before storing an operation cannot dispatch against
    # the old coordinator; their global tickets still bound the transition.
    for index <- 1..32 do
      case :ets.lookup(__MODULE__, {:slot, index}) do
        [{_, ref, _, owner, deadline, _coordinator}] ->
          if not Map.has_key?(state, ref) and
               (not Process.alive?(owner) or now() >= deadline),
             do: release(ref)

        _ ->
          :ok
      end
    end

    for row <- :ets.tab2list(__MODULE__) do
      ref =
        case row do
          {{:session, _, _}, ref} -> ref
          {{kind, ref}, _} when kind in [:operation, :result, :dispatched, :acked] -> ref
          _ -> nil
        end

      if is_reference(ref) and not reserved?(ref), do: :ets.delete_object(__MODULE__, row)
    end

    state
  end

  defp now, do: System.monotonic_time(:millisecond)

  defp failure(op, kind),
    do:
      {:error,
       Error.new(kind, op.service, op.action,
         request_id: op.ref,
         side_effects: if(op.mutation? and op.dispatched?, do: "unknown", else: "none_expected")
       )}
end
