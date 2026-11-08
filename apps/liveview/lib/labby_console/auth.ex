defmodule LabbyConsole.Auth do
  @moduledoc "Per-caller gateway sessions. Credentials never enter browser storage."
  use GenServer
  alias LabbyConsole.{GatewayAction, GatewayAction.Error}
  def start_link(_), do: GenServer.start_link(__MODULE__, %{}, name: __MODULE__)

  def init(state) do
    :ets.new(__MODULE__, [:named_table, :set, :protected, read_concurrency: true])
    Process.send_after(self(), :expire, 60_000)
    {:ok, state}
  end

  def handle_info(:expire, state) do
    Process.send_after(self(), :expire, 60_000)
    {:noreply, prune(state)}
  end

  defp prune(state) do
    Map.reject(state, fn {id, value} ->
      expired = value.session["expires_at"] <= System.system_time(:second)

      if expired do
        :ets.delete(__MODULE__, id)
        GatewayAction.revoke(id)
      end

      expired
    end)
  end

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

  def handle_call(
        {:publish, id, generation, owner, ref, result, revoked, deadline, delivery},
        {worker, _},
        state
      ) do
    state = prune(state)

    if Process.alive?(worker) do
      result =
        cond do
          deadline != :infinity and System.monotonic_time(:millisecond) >= deadline ->
            {:error, error} = revoked
            {:error, %{error | kind: :timeout, status: nil}}

          match?(%{generation: ^generation}, state[id]) ->
            result

          true ->
            revoked
        end

      if GatewayAction.claim_delivery(delivery) == :ok,
        do: send(owner, {:gateway_action, ref, result})
    end

    {:reply, :ok, state}
  end

  def handle_call({:delete, id}, _, state) do
    :ets.delete(__MODULE__, id)
    GatewayAction.revoke(id)
    {:reply, :ok, Map.delete(state, id)}
  end

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
  def publish_action(
        id,
        generation,
        owner,
        ref,
        result,
        revoked,
        deadline \\ :infinity,
        delivery \\ nil
      ) do
    delivery = delivery || :atomics.new(1, signed: false)

    GenServer.call(
      __MODULE__,
      {:publish, id, generation, owner, ref, result, revoked, deadline, delivery}
    )
  end

  def sign_in(token) when is_binary(token) and byte_size(token) in 1..4096 do
    target = Application.get_env(:labby_console, :gateway_url)

    with true <- secure_target?(target),
         {:ok, response} <-
           request(target, :post, "/auth/bearer-session", [{"authorization", "Bearer " <> token}]),
         200 <- response.status,
         [cookie] <- Req.Response.get_header(response, "set-cookie"),
         cookie <- hd(String.split(cookie, ";")),
         {:ok, session} <- inspect_session(target, cookie) do
      GenServer.call(__MODULE__, {:put, %{target: target, cookie: cookie, session: session}})
    else
      _ -> {:error, :sign_in_failed}
    end
  end

  def sign_in(_), do: {:error, :sign_in_failed}

  def current(id) do
    with %{target: target, cookie: cookie, generation: generation} <- capture(id),
         {:ok, session} <- inspect_session(target, cookie),
         true <- matches?(id, generation) do
      {:ok, session}
    else
      {:error, reason} when reason in [:unavailable, :forbidden] ->
        {:error, reason}

      _ ->
        GenServer.call(__MODULE__, {:delete, id})
        {:error, :unauthenticated}
    end
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
    with {:ok, session} <- current(op.id),
         true <- matches?(op.id, op.generation),
         :ok <- GenServer.call(coordinator, {:dispatch, ref}) do
      case request(
             op.session.target,
             :post,
             "/v1/" <> op.service,
             [
               {"cookie", op.session.cookie},
               {"x-csrf-token", session["csrf_token"]},
               {"content-type", "application/json"}
             ],
             body: op.json,
             receive_timeout: if(op.action == "gateway.host.metrics", do: 8_000, else: 2_000)
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
      {:error, reason} when reason in [:unauthenticated, :forbidden, :unavailable] ->
        {:error, Error.new(reason, op.service, op.action)}

      _ ->
        {:error, Error.new(:cancelled, op.service, op.action)}
    end
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
      %{target: target, cookie: cookie, session: session} ->
        case request(target, :post, "/auth/logout", [
               {"cookie", cookie},
               {"x-csrf-token", session["csrf_token"]}
             ]) do
          {:ok, %{status: status}} when status in 200..299 -> :ok
          _ -> {:error, :revocation_unconfirmed}
        end

      _ ->
        :ok
    end
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
          connect_options: [timeout: 2_000],
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
