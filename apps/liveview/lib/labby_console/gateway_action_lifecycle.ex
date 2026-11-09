defmodule LabbyConsole.GatewayAction.Lifecycle do
  @moduledoc "Bounded operation reservations, durable terminal outcomes and worker ownership."
  use GenServer

  @impl true
  def format_status(status),
    do: Map.merge(status, %{state: :redacted, message: :redacted, reason: :redacted, log: []})

  alias LabbyConsole.{Auth, GatewayAction, GatewayAction.Error}

  @dispatch_open 0
  @dispatch_granted 1
  @stopped_before_dispatch 2
  @stopped_after_dispatch 3

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

  # Local cancellation and timeout first close the same atomic gate used by
  # dispatch. Its monotonic phase determines certainty even if notify is delayed.
  # HTTP error metadata has a status and retains the gateway's own assessment.
  def select_terminal(ref, {:error, %Error{kind: kind, status: nil} = error})
      when kind in [:cancelled, :timeout] do
    candidate = local_failure(ref, error)
    select_result(ref, {:error, candidate})
  end

  def select_terminal(ref, candidate), do: select_result(ref, candidate)

  # The terminal payload remains independently durable. The dispatch gate is
  # permission arbitration, so a stopped selector cannot strand a claimed result.
  defp select_result(ref, candidate) do
    result =
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

    scrub_request(ref)
    result
  rescue
    ArgumentError -> candidate
  end

  # Receipts can outlive worker shutdown and synchronous acknowledgement. Keep
  # only ownership, dispatch certainty and handoff fencing in those reservations.
  defp control_metadata(op) do
    metadata = Map.drop(op, [:json, :session, :auth_payload])

    case op do
      %{auth_kind: kind, auth_payload: payload}
      when kind in [:handoff_start, :handoff_poll, :handoff_redeem] ->
        Map.put(metadata, :auth_payload, Map.take(payload, [:guardian, :auth_guardian, :gate]))

      _ ->
        metadata
    end
  end

  defp scrub_request(ref) do
    case :ets.lookup(__MODULE__, {:operation, ref}) do
      [{{:operation, ^ref}, op}] ->
        :ets.update_element(__MODULE__, {:operation, ref}, {2, control_metadata(op)})

      [] ->
        :ok
    end
  end

  defp local_failure(ref, error) do
    case :ets.lookup(__MODULE__, {:operation, ref}) do
      [{{:operation, ^ref}, %{dispatch_gate: gate, mutation?: mutation?}}] ->
        dispatched? = stop_dispatch(gate)
        risk = if mutation? and dispatched?, do: "unknown", else: "none_expected"
        recovery = Error.new(error.kind, error.service, error.action, side_effects: risk).recovery
        %{error | side_effects: risk, recovery: recovery}

      _ ->
        error
    end
  rescue
    ArgumentError -> error
  end

  defp stop_dispatch(gate) do
    case :atomics.compare_exchange(gate, 1, @dispatch_open, @stopped_before_dispatch) do
      :ok ->
        false

      @dispatch_granted ->
        # Another stopper can win this CAS, but both observe the prior grant.
        :atomics.compare_exchange(gate, 1, @dispatch_granted, @stopped_after_dispatch)
        true

      @stopped_before_dispatch ->
        false

      @stopped_after_dispatch ->
        true
    end
  end

  defp claim_dispatch(%{dispatch_gate: gate}) do
    :atomics.compare_exchange(gate, 1, @dispatch_open, @dispatch_granted) == :ok
  end

  defp claim_dispatch(_), do: false

  def terminal_result(ref) do
    case :ets.lookup(__MODULE__, {:result, ref}) do
      [{{:result, ^ref}, result}] -> result
      [] -> nil
    end
  rescue
    ArgumentError -> nil
  end

  def dispatched?(ref) do
    case :ets.lookup(__MODULE__, {:operation, ref}) do
      [{{:operation, ^ref}, %{dispatch_gate: gate}}] ->
        :atomics.get(gate, 1) in [@dispatch_granted, @stopped_after_dispatch]

      _ ->
        false
    end
  rescue
    ArgumentError -> true
  end

  def register(op) do
    if reserved?(op.ref) do
      :ets.insert_new(__MODULE__, {{:operation, op.ref}, op})
      if terminal_result(op.ref), do: scrub_request(op.ref)

      if reserved?(op.ref) do
        GenServer.cast(__MODULE__, {:register, control_metadata(op)})
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
        select_terminal(ref, failure(Map.put(op, :dispatched?, false), :cancelled))
        notify(ref)
        :ok

      _ ->
        {:error, :not_found}
    end
  rescue
    ArgumentError -> {:error, :not_found}
  end

  @doc false
  def cancel_handoff(gate) do
    stop_dispatch(gate)

    for {{:operation, ref}, %{dispatch_gate: ^gate, id: id}} <- :ets.tab2list(__MODULE__),
        do: cancel(id, ref)

    :ok
  rescue
    ArgumentError -> :ok
  end

  def revoke(id), do: GenServer.cast(__MODULE__, {:revoke, id})

  # Auth selects the immutable receipt while generation validation is serialized
  # by its vault. Guardian notification is nonblocking and recovery can deliver
  # the selected winner even if the guardian disappears before notification.
  def publish(ref, result) do
    case :ets.lookup(__MODULE__, {:operation, ref}) do
      [{{:operation, ^ref}, op}] ->
        op = Map.put(op, :dispatched?, dispatched?(ref))

        candidate =
          cond do
            now() >= op.deadline -> failure(op, :timeout)
            not Process.alive?(op.coordinator) -> failure(op, :unavailable)
            not Process.alive?(op.owner) -> failure(op, :cancelled)
            true -> result
          end

        select_terminal(ref, candidate)
        notify(ref)
        :ok

      _ ->
        :ok
    end
  rescue
    ArgumentError -> {:error, :unavailable}
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
            handoff_monitors: handoff_monitors(op),
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

          not authorized?(op) ->
            {:noreply, finish(state, ref, failure(op, :unauthenticated))}

          true ->
            lifecycle = self()
            request = :ets.lookup_element(__MODULE__, {:operation, ref}, 2)

            case Task.Supervisor.start_child(GatewayAction.Tasks, fn ->
                   result =
                     try do
                       terminal_result(ref) || execute(request, lifecycle, ref)
                     rescue
                       _ -> failure(%{op | dispatched?: true}, :transport_error)
                     end

                   case result do
                     {:ok, _} -> publish_success(op, result, lifecycle)
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
             authorized?(op) and claim_dispatch(op) do
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

  @impl true
  def handle_info({:result, ref, result}, state) do
    case state[ref] do
      %{complete?: false} = op ->
        result =
          cond do
            now() >= op.deadline -> failure(op, :timeout)
            not authorized?(op) -> failure(op, :unauthenticated)
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
           monitor in ([op.owner_monitor, op.coordinator_monitor, op.worker_monitor] ++
                         op.handoff_monitors)
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
        Enum.each(op.handoff_monitors, &Process.demonitor(&1, [:flush]))
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
              Map.merge(control_metadata(op), %{
                owner_monitor: Process.monitor(op.owner),
                coordinator_monitor: Process.monitor(op.coordinator),
                handoff_monitors: handoff_monitors(op),
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
          {{kind, ref}, _} when kind in [:operation, :result, :acked] -> ref
          _ -> nil
        end

      if is_reference(ref) and not reserved?(ref), do: :ets.delete_object(__MODULE__, row)
    end

    state
  end

  defp handoff_monitors(%{auth_kind: kind, auth_payload: payload})
       when kind in [:handoff_start, :handoff_poll, :handoff_redeem],
       do: Enum.map([payload.guardian, payload.auth_guardian], &Process.monitor/1)

  defp handoff_monitors(_), do: []

  defp authorized?(%{auth_kind: kind, auth_payload: payload})
       when kind in [:handoff_start, :handoff_poll, :handoff_redeem],
       do:
         Process.whereis(LabbyConsole.OAuthHandoff) == payload.guardian and
           Process.whereis(Auth) == payload.auth_guardian and :atomics.get(payload.gate, 1) == 0

  defp authorized?(%{auth_kind: kind}) when kind in [:exchange, :inspect, :logout], do: true
  defp authorized?(op), do: Auth.matches?(op.id, op.generation)

  defp execute(%{auth_kind: _} = op, lifecycle, ref),
    do: Auth.execute_auth(op, lifecycle, ref)

  defp execute(op, lifecycle, ref), do: Auth.execute_action(op, lifecycle, ref)

  defp publish_success(%{auth_kind: _} = op, result, lifecycle),
    do: send(lifecycle, {:result, op.ref, result})

  defp publish_success(op, result, _), do: Auth.publish_action(op, result)

  defp now, do: System.monotonic_time(:millisecond)

  defp failure(op, kind),
    do:
      {:error,
       Error.new(kind, op.service, op.action,
         request_id: op.ref,
         side_effects: if(op.mutation? and op.dispatched?, do: "unknown", else: "none_expected")
       )}
end
