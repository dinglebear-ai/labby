defmodule LabbyConsole.Auth do
  @moduledoc "Per-caller gateway sessions. Credentials never enter browser storage."
  use GenServer
  def start_link(_), do: GenServer.start_link(__MODULE__, %{}, name: __MODULE__)

  def init(state) do
    Process.send_after(self(), :expire, 60_000)
    {:ok, state}
  end

  def handle_info(:expire, state) do
    Process.send_after(self(), :expire, 60_000)
    {:noreply, prune(state)}
  end

  defp prune(state),
    do:
      Map.reject(state, fn {_, value} ->
        value.session["expires_at"] <= System.system_time(:second)
      end)

  def handle_call({:put, value}, _, state) do
    id = Base.url_encode64(:crypto.strong_rand_bytes(32), padding: false)
    state = prune(state)

    if map_size(state) < 10_000,
      do: {:reply, {:ok, id}, Map.put(state, id, value)},
      else: {:reply, {:error, :capacity}, state}
  end

  def handle_call({:get, id}, _, state), do: {:reply, Map.get(state, id), state}
  def handle_call({:delete, id}, _, state), do: {:reply, :ok, Map.delete(state, id)}

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
    with %{target: target, cookie: cookie} <- GenServer.call(__MODULE__, {:get, id}),
         {:ok, session} <- inspect_session(target, cookie) do
      {:ok, session}
    else
      {:error, :unavailable} ->
        {:error, :unavailable}

      _ ->
        GenServer.call(__MODULE__, {:delete, id})
        {:error, :unauthenticated}
    end
  end

  @reads %{
    "gateway" =>
      ~w(gateway.server.get gateway.discovered_tools gateway.discovered_resources gateway.discovered_prompts gateway.host.metrics gateway.clients.list gateway.mcp.list gateway.usage.metrics gateway.usage.calls),
    "doctor" => ~w(system.checks),
    "server_logs" => ~w(server_logs.query)
  }
  def read_action(id, service, action, params) when is_map(params) do
    if action in Map.get(@reads, service, []) do
      with {:ok, session} <- current(id),
           %{target: target, cookie: cookie} <- GenServer.call(__MODULE__, {:get, id}),
           {:ok, response} <-
             request(
               target,
               :post,
               "/v1/" <> service,
               [{"cookie", cookie}, {"x-csrf-token", session["csrf_token"]}],
               json: %{action: action, params: params},
               receive_timeout: if(action == "gateway.host.metrics", do: 8_000, else: 2_000)
             ) do
        case response do
          %{status: status} when status in [401, 403] ->
            GenServer.call(__MODULE__, {:delete, id})
            {:error, :unauthenticated}

          %{status: 200, body: body} ->
            with true <- not is_nil(GenServer.call(__MODULE__, {:get, id})),
                 {:ok, data} <- Jason.decode(body),
                 do: {:ok, data},
                 else: (_ -> {:error, :unavailable})

          _ ->
            {:error, :unavailable}
        end
      else
        {:error, :unauthenticated} -> {:error, :unauthenticated}
        _ -> {:error, :unavailable}
      end
    else
      {:error, :unavailable}
    end
  end

  def read_action(_, _, _, _), do: {:error, :unavailable}

  def servers(id) do
    with {:ok, session} <- current(id),
         %{target: target, cookie: cookie} <- GenServer.call(__MODULE__, {:get, id}),
         {:ok, response} <-
           request(
             target,
             :post,
             "/v1/gateway",
             [{"cookie", cookie}, {"x-csrf-token", session["csrf_token"]}],
             json: %{action: "gateway.list", params: %{}}
           ) do
      case response do
        %{status: status} when status in [401, 403] ->
          GenServer.call(__MODULE__, {:delete, id})
          {:error, :unauthenticated}

        %{status: 200, body: body} ->
          if GenServer.call(__MODULE__, {:get, id}),
            do: decode_servers(body),
            else: {:error, :unauthenticated}

        _ ->
          {:error, :unavailable}
      end
    else
      {:error, :unauthenticated} -> {:error, :unauthenticated}
      _ -> {:error, :unavailable}
    end
  end

  defp decode_servers(body) do
    with {:ok, rows} when is_list(rows) <- Jason.decode(body),
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
      {:ok, %{status: status}} when status in [401, 403] -> {:error, :unauthenticated}
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
              else: {:halt, {req, %{resp | status: 413, body: ""}}}
          end
        ],
        extra
      )
    )
  end
end
