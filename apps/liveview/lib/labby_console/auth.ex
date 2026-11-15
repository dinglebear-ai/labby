defmodule LabbyConsole.Auth do
  @moduledoc "Per-caller gateway sessions. Credentials never enter browser storage."
  use GenServer

  @impl true
  def format_status(status),
    do: Map.merge(status, %{state: :redacted, message: :redacted, reason: :redacted, log: []})

  alias LabbyConsole.{GatewayAction, GatewayAction.Error}

  @access_inventory_reads ~w(access.team.member.list access.team_project.list access.team_invitation.list access.platform_admin.list)
  def start_link(_), do: GenServer.start_link(__MODULE__, %{}, name: __MODULE__)

  @impl true
  def init(state) do
    :ets.new(__MODULE__, [:named_table, :set, :protected, read_concurrency: true])
    Process.send_after(self(), :expire, 60_000)
    {:ok, state}
  end

  @impl true
  def handle_info(:expire, state) do
    Process.send_after(self(), :expire, 60_000)
    {:noreply, prune(state)}
  end

  defp prune(state) do
    Map.reject(state, fn {id, value} ->
      expired = value.session["expires_at"] <= System.system_time(:second)

      if expired do
        :ets.delete(__MODULE__, id)
        broadcast_revocation(id)
        GatewayAction.revoke(id)
      end

      expired
    end)
  end

  @impl true
  def handle_call({:put, value}, _, state) do
    id = Base.url_encode64(:crypto.strong_rand_bytes(32), padding: false)
    state = prune(state)

    if map_size(state) < 10_000 do
      value = Map.put(value, :generation, make_ref())
      :ets.insert(__MODULE__, {id, value})
      {:reply, {:ok, id}, Map.put(state, id, value)}
    else
      {:reply, {:error, :capacity}, state}
    end
  end

  def handle_call({:get, id}, _, state) do
    state = prune(state)
    {:reply, Map.get(state, id), state}
  end

  def handle_call({:matches, id, generation}, _, state) do
    state = prune(state)
    {:reply, match?(%{generation: ^generation}, state[id]), state}
  end

  def handle_call({:publish, op, result}, {worker, _}, state) do
    state = prune(state)

    if Process.alive?(worker) do
      result =
        if match?(%{generation: generation} when generation == op.generation, state[op.id]),
          do: result,
          else:
            {:error,
             Error.new(:unauthenticated, op.service, op.action,
               request_id: op.ref,
               side_effects: if(op.mutation?, do: "unknown", else: "none_expected")
             )}

      LabbyConsole.GatewayAction.Lifecycle.publish(op.ref, result)
    end

    {:reply, :ok, state}
  end

  def handle_call({:delete, id}, _, state) do
    :ets.delete(__MODULE__, id)
    if Map.has_key?(state, id), do: broadcast_revocation(id)
    GatewayAction.revoke(id)
    {:reply, :ok, Map.delete(state, id)}
  end

  def handle_call({:handoff_owner_authenticated, owner}, _, state) do
    state = prune(state)

    authenticated =
      is_binary(owner) and
        Enum.any?(state, fn {_, value} -> Map.get(value, :oauth_owner) == owner end)

    {:reply, authenticated, state}
  end

  def handle_call({:take_handoff_session, owner, flow_id}, _, state) do
    state = prune(state)

    found =
      if is_binary(owner) and is_binary(flow_id),
        do:
          Enum.find(state, fn {_, value} ->
            Map.get(value, :oauth_owner) == owner and Map.get(value, :oauth_flow_id) == flow_id
          end)

    case found do
      {id, value} ->
        :ets.delete(__MODULE__, id)
        broadcast_revocation(id)
        GatewayAction.revoke(id)
        {:reply, value, Map.delete(state, id)}

      _ ->
        {:reply, nil, state}
    end
  end

  def handle_call({:adopt_handoff, owner, flow_id, completed_ref}, {source, _}, state) do
    state = prune(state)

    receipt =
      if source == Process.whereis(LabbyConsole.OAuthHandoff),
        do: handoff_receipt(owner, flow_id, completed_ref),
        else: nil

    if source == Process.whereis(LabbyConsole.OAuthHandoff) and is_map(receipt) and
         receipt.guardian == self() and Process.alive?(receipt.caller) and
         System.monotonic_time(:millisecond) < receipt.deadline and map_size(state) < 10_000 and
         :atomics.compare_exchange(receipt.gate, 1, 0, 1) == :ok do
      id = Base.url_encode64(:crypto.strong_rand_bytes(32), padding: false)

      value =
        receipt.value
        |> Map.put(:generation, make_ref())
        |> Map.put(:oauth_owner, owner)
        |> Map.put(:oauth_flow_id, flow_id)

      :ets.insert(__MODULE__, {id, value})
      {:reply, {:ok, id}, Map.put(state, id, value)}
    else
      {:reply, {:error, :invalid_flow}, state}
    end
  end

  defp handoff_receipt(owner, flow_id, completed_ref) do
    case :ets.lookup(LabbyConsole.OAuthHandoff, {:receipt, completed_ref}) do
      [{{:receipt, ^completed_ref}, %{owner: ^owner, flow_id: ^flow_id} = value}] -> value
      _ -> nil
    end
  rescue
    ArgumentError -> nil
  end

  defp broadcast_revocation(id) when is_binary(id) do
    if Process.whereis(LabbyConsole.PubSub),
      do:
        Phoenix.PubSub.broadcast(
          LabbyConsole.PubSub,
          "console-session:" <> id,
          {:console_session_revoked, id}
        )

    :ok
  end

  defp broadcast_revocation(_), do: :ok

  @doc false
  def capture(id) do
    case :ets.lookup(__MODULE__, id) do
      [{^id, value}] ->
        if value.session["expires_at"] > System.system_time(:second), do: value, else: nil

      [] ->
        nil
    end
  rescue
    ArgumentError -> nil
  end

  @doc false
  def matches?(id, generation), do: match?(%{generation: ^generation}, capture(id))

  @doc false
  def publish_action(op, result), do: GenServer.call(__MODULE__, {:publish, op, result})

  def oauth_start(owner) do
    if GenServer.call(__MODULE__, {:handoff_owner_authenticated, owner}),
      do: {:error, :sign_out_required},
      else: LabbyConsole.OAuthHandoff.start(owner)
  end

  def oauth_poll(owner, flow), do: LabbyConsole.OAuthHandoff.poll(owner, flow)

  def oauth_cancel(owner, flow) do
    result = LabbyConsole.OAuthHandoff.cancel(owner, flow)

    case GenServer.call(__MODULE__, {:take_handoff_session, owner, flow}) do
      nil ->
        result

      snapshot ->
        case GatewayAction.run_auth(:logout, snapshot) do
          {:ok, _} -> :ok
          _ -> {:error, :revocation_unconfirmed}
        end
    end
  end

  @doc false
  def adopt_handoff(owner, flow_id, completed_ref) do
    GenServer.call(__MODULE__, {:adopt_handoff, owner, flow_id, completed_ref})
  catch
    :exit, _ -> {:error, :unavailable}
  end

  def sign_in(token) when is_binary(token) and byte_size(token) in 1..4096 do
    case GatewayAction.run_auth(:exchange, token) do
      {:ok, value} -> GenServer.call(__MODULE__, {:put, value})
      _ -> {:error, :sign_in_failed}
    end
  end

  def sign_in(_), do: {:error, :sign_in_failed}

  def current(id) do
    case capture(id) do
      nil ->
        {:error, :unauthenticated}

      snapshot ->
        case GatewayAction.run_auth(:inspect, snapshot) do
          {:ok, session} ->
            current_result(id, snapshot.generation, {:ok, session})

          {:error, reason} when reason in [:unauthenticated, :forbidden] ->
            current_result(id, snapshot.generation, {:error, reason})

          _ ->
            {:error, :unavailable}
        end
    end
  end

  defp current_result(id, generation, {:ok, session}) do
    if matches?(id, generation), do: {:ok, session}, else: {:error, :unauthenticated}
  end

  defp current_result(_id, _generation, {:error, reason})
       when reason in [:forbidden, :unavailable],
       do: {:error, reason}

  defp current_result(id, _generation, _) do
    GenServer.call(__MODULE__, {:delete, id})
    {:error, :unauthenticated}
  end

  # Action preflight is already inside an admitted worker and its absolute deadline.
  defp current_in_action(op) do
    current_result(op.id, op.generation, inspect_session(op.session.target, op.session.cookie))
  end

  @doc "Run a reviewed caller-bound action. Mutations are never automatically retried."
  def action(id, service, action, params), do: GatewayAction.run(id, service, action, params)

  @doc "Start an owned action; receive {:gateway_action, request_id, result}. Owner death cancels it."
  def start_action(id, service, action, params),
    do: GatewayAction.start(id, service, action, params)

  @doc "Cancel a request belonging to this session. Dispatched writes may have taken effect."
  def cancel_action(id, request_id), do: GatewayAction.cancel(id, request_id)

  def read_action(id, service, action, params),
    do: GatewayAction.run(id, service, action, params, :read) |> read_result()

  def servers(id) do
    with {:ok, data} <-
           GatewayAction.run(id, "gateway", "gateway.list", %{}, :read) |> read_result(),
         do: decode_servers(data)
  end

  defp read_result({:error, %Error{kind: kind}}) when kind in [:unauthenticated, :forbidden],
    do: {:error, kind}

  defp read_result({:error, %Error{}}), do: {:error, :unavailable}
  defp read_result(result), do: result

  @doc false
  def execute_action(op, coordinator, ref) do
    with {:ok, session} <- current_in_action(op),
         true <- matches?(op.id, op.generation),
         {:ok, receive_timeout} <- GatewayAction.receive_timeout(op),
         {:ok, access_headers} <- access_read_headers(op, receive_timeout),
         :ok <- GenServer.call(coordinator, {:dispatch, ref}) do
      case request(
             op.session.target,
             :post,
             action_path(op.service),
             [
               {"cookie", op.session.cookie},
               {"x-csrf-token", session["csrf_token"]},
               {"content-type", "application/json"}
             ] ++ access_headers,
             body: op.json,
             receive_timeout: receive_timeout
           ) do
        {:ok, response} ->
          action_response(op, response)

        _ ->
          {:error,
           Error.new(:transport_error, op.service, op.action,
             side_effects: if(op.mutation?, do: "unknown", else: "none_expected")
           )}
      end
    else
      {:error, reason} when reason in [:unauthenticated, :forbidden, :unavailable, :timeout] ->
        {:error, Error.new(reason, op.service, op.action)}

      _ ->
        {:error, Error.new(:cancelled, op.service, op.action)}
    end
  end

  defp access_read_headers(%{service: "access", action: action} = op, receive_timeout)
       when action in @access_inventory_reads do
    remaining = op.deadline - System.monotonic_time(:millisecond)
    budget = min(1_500, min(remaining - 250, receive_timeout - 250))

    if budget > 0,
      do: {:ok, [{"x-labby-access-read-budget-ms", Integer.to_string(budget)}]},
      else: {:error, :timeout}
  end

  defp access_read_headers(_, _), do: {:ok, []}

  defp action_path("access"), do: "/v1/access/admin"
  defp action_path(service), do: "/v1/" <> service

  defp action_response(
         %{service: "access", action: action} = op,
         %{private: %{labby_response_too_large: true}}
       )
       when action in @access_inventory_reads do
    {:error,
     Error.new(:response_too_large, op.service, op.action,
       metadata: %{"origin" => "budget"},
       recovery: %{"action" => "reduce_work", "same_arguments" => "never"}
     )}
  end

  defp action_response(op, %{private: %{labby_response_too_large: true}}),
    do:
      {:error,
       Error.new(:response_too_large, op.service, op.action,
         side_effects: if(op.mutation?, do: "unknown", else: "none_expected")
       )}

  defp action_response(op, %{status: status, body: body}) when status in 200..299 do
    case Jason.decode(body) do
      {:ok, data} ->
        {:ok, data}

      _ ->
        {:error,
         Error.new(:invalid_response, op.service, op.action,
           side_effects: if(op.mutation?, do: "unknown", else: "none_expected")
         )}
    end
  end

  defp action_response(op, %{status: status, body: body}) do
    data =
      case Jason.decode(body) do
        {:ok, data} -> data
        _ -> nil
      end

    upstream_oauth? =
      Error.envelope?(data, op.service, op.action) and data["kind"] == "oauth_needs_reauth"

    if status == 401 and not upstream_oauth? do
      GenServer.call(__MODULE__, {:delete, op.id})
      {:error, Error.new(:unauthenticated, op.service, op.action, status: 401)}
    else
      {:error, Error.from_response(status, data, op.service, op.action, op.mutation?)}
    end
  end

  defp decode_servers(rows) do
    with true <- is_list(rows),
         true <-
           length(rows) <= 1000 &&
             Enum.all?(rows, fn row ->
               is_map(row) && is_binary(row["id"]) && is_binary(row["name"]) &&
                 is_boolean(row["enabled"]) && is_boolean(row["connected"]) &&
                 (is_nil(row["display_name"]) || is_binary(row["display_name"]))
             end) do
      {:ok, Enum.map(rows, &Map.take(&1, ~w(id name display_name enabled connected)))}
    else
      _ -> {:error, :unavailable}
    end
  end

  def sign_out(id) do
    value = GenServer.call(__MODULE__, {:get, id})
    GenServer.call(__MODULE__, {:delete, id})

    case value do
      %{target: _, cookie: _, session: _} ->
        case GatewayAction.run_auth(:logout, value) do
          {:ok, _} -> :ok
          _ -> {:error, :revocation_unconfirmed}
        end

      _ ->
        :ok
    end
  end

  @doc false
  def execute_auth(op, lifecycle, ref) do
    with :ok <- GenServer.call(lifecycle, {:dispatch, ref}) do
      auth_request(op.auth_kind, op.auth_payload)
    else
      _ -> {:error, :unavailable}
    end
  end

  defp auth_request(:handoff_start, %{target: target, origin: origin, verifier: verifier}) do
    challenge = :crypto.hash(:sha256, verifier) |> Base.url_encode64(padding: false)

    with {:ok, %{status: 201, body: body}} <-
           handoff_request(target, origin, "/auth/desktop/start", %{code_challenge: challenge}),
         {:ok, data} <- Jason.decode(body),
         true <-
           is_map(data) and
             Enum.sort(Map.keys(data)) == ~w(authorization_url expires_at poll_token redeem_code),
         %{
           "authorization_url" => url,
           "expires_at" => expiry,
           "poll_token" => poll,
           "redeem_code" => redeem
         } <- data,
         true <-
           is_integer(expiry) and expiry > System.system_time(:second) and
             expiry <= System.system_time(:second) + 305,
         true <-
           is_binary(poll) and byte_size(poll) in 1..256 and is_binary(redeem) and
             byte_size(redeem) in 1..256,
         :ok <- LabbyConsole.OAuthHandoff.authorization_url(url, origin) do
      {:ok, %{authorization_url: url, expires_at: expiry, poll_token: poll, redeem_code: redeem}}
    else
      {:error, :origin_mismatch} -> {:error, :origin_mismatch}
      _ -> {:error, :invalid_response}
    end
  end

  defp auth_request(:handoff_poll, flow) do
    with {:ok, %{status: status, body: body}} when status in [200, 202] <-
           handoff_request(flow.target, flow.origin, "/auth/desktop/poll", %{
             poll_token: flow.poll_token
           }),
         {:ok, %{"ready" => ready, "expires_at" => expiry} = data} <- Jason.decode(body),
         true <-
           map_size(data) == 2 and is_integer(expiry) and expiry >= 0 and
             expiry <= flow.expires_at + 5,
         true <-
           (status == 202 and ready == false) or
             (status == 200 and ready == true and expiry > System.system_time(:second) and
                expiry <= flow.expires_at + 5) do
      {:ok, if(ready, do: :ready, else: :pending)}
    else
      {:ok, %{status: status}} when status in [429, 503] -> {:ok, :pending}
      _ -> {:error, :invalid_response}
    end
  end

  defp auth_request(:handoff_redeem, flow) do
    with true <- System.monotonic_time(:millisecond) < flow.deadline,
         {:ok, %{status: 204} = response} <-
           handoff_request(flow.target, flow.origin, "/auth/desktop/redeem", %{
             redeem_code: flow.redeem_code,
             code_verifier: flow.verifier
           }),
         [cookie] <- Req.Response.get_header(response, "set-cookie"),
         cookie <- hd(String.split(cookie, ";")),
         {:ok, session} <- inspect_session(flow.target, cookie) do
      {:ok, %{target: flow.target, cookie: cookie, session: session}}
    else
      _ -> {:error, :fresh_sign_in_required}
    end
  end

  defp auth_request(:exchange, token) do
    target = Application.get_env(:labby_console, :gateway_url)

    with true <- secure_target?(target),
         {:ok, response} <-
           request(target, :post, "/auth/bearer-session", [{"authorization", "Bearer " <> token}]),
         200 <- response.status,
         [cookie] <- Req.Response.get_header(response, "set-cookie"),
         cookie <- hd(String.split(cookie, ";")),
         {:ok, session} <- inspect_session(target, cookie) do
      {:ok, %{target: target, cookie: cookie, session: session}}
    else
      _ -> {:error, :sign_in_failed}
    end
  end

  defp auth_request(:inspect, %{target: target, cookie: cookie}),
    do: inspect_session(target, cookie)

  defp auth_request(:logout, %{target: target, cookie: cookie, session: session}) do
    case request(target, :post, "/auth/logout", [
           {"cookie", cookie},
           {"x-csrf-token", session["csrf_token"]}
         ]) do
      {:ok, %{status: status}} when status in 200..299 -> {:ok, :revoked}
      _ -> {:error, :revocation_unconfirmed}
    end
  end

  defp handoff_request(target, origin, path, body) do
    authority = origin |> String.split("://", parts: 2) |> List.last()

    request(
      target,
      :post,
      path,
      [{"origin", origin}, {"host", authority}, {"content-type", "application/json"}],
      body: Jason.encode!(body)
    )
  end

  defp inspect_session(target, cookie) do
    case request(target, :get, "/auth/session", [{"cookie", cookie}]) do
      {:ok, %{status: 200, body: body}} -> validate_session(body)
      {:ok, %{status: 401}} -> {:error, :unauthenticated}
      {:ok, %{status: 403}} -> {:error, :forbidden}
      _ -> {:error, :unavailable}
    end
  end

  defp validate_session(body) do
    case Jason.decode(body) do
      {:ok, %{"authenticated" => false}} ->
        {:error, :unauthenticated}

      {:ok,
       %{
         "authenticated" => true,
         "expires_at" => expiry,
         "csrf_token" => csrf,
         "user" => %{"sub" => sub}
       } = session}
      when is_integer(expiry) and is_binary(csrf) and csrf != "" and is_binary(sub) ->
        if expiry > System.system_time(:second),
          do: {:ok, session},
          else: {:error, :unauthenticated}

      _ ->
        {:error, :unavailable}
    end
  end

  defp secure_target?(target) when is_binary(target) do
    uri = URI.parse(target)

    uri.scheme in ["http", "https"] && uri.path in [nil, "", "/"] && is_nil(uri.userinfo) &&
      is_nil(uri.query) && is_nil(uri.fragment) &&
      (uri.scheme == "https" || uri.host in ["127.0.0.1", "::1", "localhost"])
  end

  defp secure_target?(_), do: false

  defp request(target, method, path, headers, extra \\ []) do
    Req.request(
      Keyword.merge(
        [
          method: method,
          url: String.trim_trailing(target, "/") <> path,
          headers: headers,
          retry: false,
          redirect: false,
          decode_body: false,
          receive_timeout: 2_000,
          finch: [
            conn_opts: [
              transport_opts: [timeout: 2_000],
              max_header_list_size: 65_536,
              client_settings: [max_header_list_size: 65_536]
            ]
          ],
          into: fn {:data, data}, {req, resp} ->
            body = (resp.body || "") <> data

            if byte_size(body) <= 65_536,
              do: {:cont, {req, %{resp | body: body}}},
              else:
                {:halt,
                 {req,
                  %{
                    resp
                    | status: 413,
                      body: "",
                      private: Map.put(resp.private, :labby_response_too_large, true)
                  }}}
          end
        ],
        extra
      )
    )
  end
end
