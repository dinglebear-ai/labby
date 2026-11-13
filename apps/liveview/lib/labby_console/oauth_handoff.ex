defmodule LabbyConsole.OAuthHandoff do
  @moduledoc "Owner-bound desktop handoffs. Network work remains with the requesting process."
  use GenServer

  @impl true
  def format_status(status),
    do: Map.merge(status, %{state: :redacted, message: :redacted, reason: :redacted, log: []})

  alias LabbyConsole.{Auth, GatewayAction}
  @receipt_ms 30_000
  def start_link(_), do: GenServer.start_link(__MODULE__, %{}, name: __MODULE__)

  @impl true
  def init(state) do
    :ets.new(__MODULE__, [:named_table, :set, :protected])
    Process.send_after(self(), :expire, 1_000)
    {:ok, state}
  end

  def configured?(), do: match?({:ok, _}, configuration())

  def configuration do
    target = Application.get_env(:labby_console, :gateway_url)
    configured = Application.get_env(:labby_console, :gateway_auth_origin, target)

    with {:ok, _} <- origin(target),
         {:ok, value} <- origin(configured),
         do: {:ok, %{target: target, origin: value}}
  end

  def origin(value) when is_binary(value) and byte_size(value) <= 2048 do
    uri = URI.parse(value)

    if uri.scheme in ["https", "http"] and is_binary(uri.host) and uri.host != "" and
         uri.path in [nil, "", "/"] and is_nil(uri.userinfo) and is_nil(uri.query) and
         is_nil(uri.fragment) and is_integer(uri.port) and uri.port in 1..65535 and
         (uri.scheme == "https" or uri.host in ["localhost", "127.0.0.1", "::1"]) do
      {:ok, URI.to_string(%{uri | path: nil, host: String.downcase(uri.host)})}
    else
      {:error, :configuration}
    end
  rescue
    _ -> {:error, :configuration}
  end

  def origin(_), do: {:error, :configuration}

  def authorization_url(url, expected) when is_binary(url) and byte_size(url) <= 4096 do
    uri = URI.parse(url)

    with {:ok, actual} <-
           origin(%{uri | path: nil, query: nil, fragment: nil} |> URI.to_string()),
         :ok <- match_origin(actual, expected),
         true <-
           is_nil(uri.fragment) and is_nil(uri.userinfo) and uri.path == "/auth/desktop/authorize",
         [{"state", state}] <- URI.query_decoder(uri.query || "") |> Enum.to_list(),
         true <- byte_size(state) in 1..256 do
      :ok
    else
      {:error, :origin_mismatch} -> {:error, :origin_mismatch}
      _ -> {:error, :invalid_response}
    end
  rescue
    _ -> {:error, :invalid_response}
  end

  def authorization_url(_, _), do: {:error, :invalid_response}

  defp match_origin(origin, origin), do: :ok
  defp match_origin(_, _), do: {:error, :origin_mismatch}

  def start(owner) when is_binary(owner) and byte_size(owner) in 16..128 do
    with {:ok, config} <- configuration(),
         {:ok, id, claim, flow} <- call({:start, owner, config}),
         result <- GatewayAction.run_auth(:handoff_start, flow),
         response <- call({:started, owner, id, claim, result}) do
      response
    end
  end

  def start(_), do: {:error, :invalid_owner}

  def poll(owner, id) do
    case call({:poll, owner, id}) do
      {:network, claim, flow} ->
        result = GatewayAction.run_auth(:handoff_poll, flow)

        case call({:polled, owner, id, claim, result}) do
          {:redeem, claim, flow} ->
            result = GatewayAction.run_auth(:handoff_redeem, flow)
            call({:redeemed, owner, id, claim, result}, 6_000)

          other ->
            other
        end

      other ->
        other
    end
  end

  def cancel(owner, id) do
    owned = close_owned_gate(owner, id)

    case call({:cancel, owner, id}) do
      {:error, :invalid_flow} when owned -> :ok
      result -> result
    end
  end

  defp close_owned_gate(owner, id) do
    case :ets.lookup(__MODULE__, {:flow, id}) do
      [{{:flow, ^id}, %{owner: ^owner} = flow}] ->
        :atomics.compare_exchange(flow.gate, 1, 0, 2)
        LabbyConsole.GatewayAction.Lifecycle.cancel_handoff(flow.network_gate)
        true

      _ ->
        false
    end
  rescue
    ArgumentError -> false
  end

  @impl true
  def handle_call({:start, owner, config}, {pid, _}, state) do
    state = prune(state)

    state =
      Map.reject(state, fn {id, flow} ->
        if flow.owner == owner, do: destroy(id, flow)
        flow.owner == owner
      end)

    if Enum.count(state, fn {_, flow} -> flow.phase != :done end) >= 4 or map_size(state) >= 128 do
      {:reply, {:error, :capacity}, state}
    else
      id = random()
      verifier = random()
      claim = make_ref()

      flow =
        Map.merge(config, %{
          owner: owner,
          verifier: verifier,
          claim: claim,
          monitor: Process.monitor(pid),
          caller: pid,
          phase: :starting,
          deadline: now() + 300_000,
          gate: :atomics.new(1, signed: false),
          network_gate: :atomics.new(1, signed: false),
          guardian: self(),
          auth_guardian: Process.whereis(Auth),
          attempts: 0
        })

      {:reply, {:ok, id, claim, flow}, put_flow(state, id, flow)}
    end
  end

  def handle_call({:cancel, owner, id}, _, state) do
    case state[id] do
      %{owner: ^owner} = flow ->
        destroy(id, flow)
        {:reply, :ok, Map.delete(state, id)}

      _ ->
        {:reply, {:error, :invalid_flow}, state}
    end
  end

  def handle_call({:poll, owner, id}, {pid, _}, state) do
    state = prune(state)

    state =
      case state[id] do
        %{owner: ^owner, attempts: attempts} = flow ->
          Map.put(state, id, %{flow | attempts: attempts + 1})

        _ ->
          state
      end

    case state[id] do
      %{owner: ^owner, attempts: attempts} = flow when attempts > 60 ->
        terminal(state, id, flow, {:error, :expired})

      %{owner: ^owner, phase: :done, session_id: sid} = receipt ->
        if Auth.capture(sid),
          do: {:reply, {:ok, sid}, state},
          else: terminal(state, id, receipt, {:error, :invalid_flow})

      %{owner: ^owner, phase: :pending} = flow ->
        cond do
          now() < flow.next_poll ->
            {:reply, {:pending, flow.next_poll - now()}, state}

          true ->
            claim = make_ref()

            flow = %{
              flow
              | phase: :polling,
                network_gate: :atomics.new(1, signed: false),
                claim: claim,
                monitor: Process.monitor(pid),
                caller: pid
            }

            {:reply, {:network, claim, flow}, put_flow(state, id, flow)}
        end

      %{owner: ^owner} ->
        {:reply, {:pending, 5_000}, state}

      _ ->
        {:reply, {:error, :invalid_flow}, state}
    end
  end

  def handle_call({event, owner, id, claim, result}, {pid, _}, state)
      when event in [:started, :polled, :redeemed] do
    state = prune(state)

    case state[id] do
      %{owner: ^owner, claim: ^claim, caller: ^pid} = flow ->
        finish(event, id, flow, result, state)

      _ ->
        {:reply, {:error, :invalid_flow}, state}
    end
  end

  defp finish(:started, id, flow, {:ok, data}, state) do
    Process.demonitor(flow.monitor, [:flush])

    flow =
      Map.merge(flow, %{
        phase: :pending,
        monitor: nil,
        next_poll: now() + 5_000,
        poll_token: data.poll_token,
        redeem_code: data.redeem_code,
        expires_at: data.expires_at,
        deadline:
          min(flow.deadline, now() + (data.expires_at - System.system_time(:second)) * 1000)
      })

    {:reply,
     {:ok,
      %{flow_id: id, authorization_url: data.authorization_url, expires_at: data.expires_at}},
     put_flow(state, id, flow)}
  end

  defp finish(:polled, id, flow, {:ok, :pending}, state) do
    Process.demonitor(flow.monitor, [:flush])
    flow = %{flow | phase: :pending, monitor: nil, next_poll: now() + 5_000}
    {:reply, {:pending, 5_000}, put_flow(state, id, flow)}
  end

  defp finish(:polled, id, flow, {:ok, :ready}, state) do
    flow = %{flow | phase: :redeeming, network_gate: :atomics.new(1, signed: false)}
    {:reply, {:redeem, flow.claim, flow}, put_flow(state, id, flow)}
  end

  defp finish(:redeemed, id, flow, {:ok, value}, state) do
    :ets.insert(
      __MODULE__,
      {{:receipt, flow.claim},
       %{
         owner: flow.owner,
         flow_id: id,
         gate: flow.gate,
         deadline: flow.deadline,
         caller: flow.caller,
         guardian: flow.auth_guardian,
         value: value
       }}
    )

    result =
      try do
        Auth.adopt_handoff(flow.owner, id, flow.claim)
      after
        :ets.delete(__MODULE__, {:receipt, flow.claim})
      end

    case result do
      {:ok, sid} ->
        Process.demonitor(flow.monitor, [:flush])

        receipt = %{
          owner: flow.owner,
          phase: :done,
          session_id: sid,
          deadline: now() + @receipt_ms,
          monitor: nil,
          gate: flow.gate
        }

        {:reply, {:ok, sid}, put_flow(state, id, receipt)}

      _ ->
        terminal(state, id, flow, {:error, :fresh_sign_in_required})
    end
  end

  defp finish(:started, id, flow, {:error, :origin_mismatch}, state),
    do: terminal(state, id, flow, {:error, :origin_mismatch})

  defp finish(:redeemed, id, flow, _, state),
    do: terminal(state, id, flow, {:error, :fresh_sign_in_required})

  defp finish(_, id, flow, _, state), do: terminal(state, id, flow, {:error, :unavailable})

  @impl true
  def handle_info(:expire, state) do
    Process.send_after(self(), :expire, 1_000)
    {:noreply, prune(state)}
  end

  def handle_info({:DOWN, monitor, :process, _, _}, state) do
    {:noreply,
     Map.reject(state, fn {id, flow} ->
       if flow.monitor == monitor, do: destroy(id, flow)
       flow.monitor == monitor
     end)}
  end

  defp terminal(state, id, flow, reply) do
    destroy(id, flow)
    {:reply, reply, Map.delete(state, id)}
  end

  defp prune(state),
    do:
      Map.reject(state, fn {id, flow} ->
        if flow.deadline <= now(), do: destroy(id, flow)
        flow.deadline <= now()
      end)

  defp put_flow(state, id, %{phase: :done} = flow) do
    :ets.delete(__MODULE__, {:flow, id})
    Map.put(state, id, flow)
  end

  defp put_flow(state, id, flow) do
    :ets.insert(__MODULE__, {{:flow, id}, Map.take(flow, [:owner, :gate, :network_gate])})
    Map.put(state, id, flow)
  end

  defp destroy(id, flow) do
    :ets.delete(__MODULE__, {:flow, id})
    :atomics.compare_exchange(flow.gate, 1, 0, 2)

    if Map.has_key?(flow, :network_gate),
      do: LabbyConsole.GatewayAction.Lifecycle.cancel_handoff(flow.network_gate)

    if flow.monitor, do: Process.demonitor(flow.monitor, [:flush])
    :ok
  end

  defp call(message, timeout \\ 5_000) do
    GenServer.call(__MODULE__, message, timeout)
  catch
    :exit, _ -> {:error, :unavailable}
  end

  defp random, do: Base.url_encode64(:crypto.strong_rand_bytes(32), padding: false)
  defp now, do: System.monotonic_time(:millisecond)
end
