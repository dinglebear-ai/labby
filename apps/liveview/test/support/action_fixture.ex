defmodule LabbyConsole.ActionFixture do
  @moduledoc false
  use Plug.Router
  plug Plug.Parsers, parsers: [:json], json_decoder: Jason, length: 100_000
  plug :match
  plug :dispatch

  post "/auth/bearer-session" do
    ["Bearer " <> caller] = get_req_header(conn, "authorization")
    conn |> put_resp_header("set-cookie", "caller=#{caller}; HttpOnly") |> send_resp(200, "{}")
  end

  get "/auth/session" do
    mode = Agent.get(conn.private.fixture, & &1.session_status)

    mode =
      if mode == :hold do
        owner = Agent.get(conn.private.fixture, & &1.owner)
        send(owner, {:session_request, self()})

        receive do
          {:session_status, status} -> status
        after
          2000 -> 503
        end
      else
        mode
      end

    ["caller=" <> caller] = get_req_header(conn, "cookie")

    send_resp(
      conn,
      mode,
      Jason.encode!(%{
        authenticated: true,
        expires_at: System.system_time(:second) + 60,
        user: %{sub: caller},
        csrf_token: "csrf-" <> caller
      })
    )
  end

  post "/auth/logout" do
    send_resp(conn, 204, "")
  end

  post "/v1/:service" do
    {owner, response} = Agent.get(conn.private.fixture, &{&1.owner, &1.response})

    send(
      owner,
      {:dispatch, self(), service, conn.body_params, get_req_header(conn, "cookie"),
       get_req_header(conn, "x-csrf-token")}
    )

    case response do
      :trickle ->
        conn = send_chunked(conn, 200)
        trickle(conn, 16)

      other ->
        {status, body} =
          case other do
            :hold ->
              receive do
                {:respond, status, body} -> {status, body}
              after
                8_000 -> {503, "{}"}
              end

            value ->
              value
          end

        send_resp(conn, status, body)
    end
  end

  defp trickle(conn, 0), do: conn

  defp trickle(conn, n) do
    receive do
      :stop -> conn
    after
      400 ->
        case chunk(conn, " ") do
          {:ok, conn} -> trickle(conn, n - 1)
          {:error, _} -> conn
        end
    end
  end

  def init(opts), do: opts
  def call(conn, opts), do: super(put_private(conn, :fixture, opts[:state]), opts)
end

defmodule LabbyConsole.DisconnectFixture do
  @moduledoc false
  # A real peer that closes the accepted write socket before sending any response.
  def start_link(owner) do
    Task.start_link(fn ->
      {:ok, listener} =
        :gen_tcp.listen(0, [:binary, active: false, reuseaddr: true, ip: {127, 0, 0, 1}])

      {:ok, {_, port}} = :inet.sockname(listener)
      send(owner, {:disconnect_fixture, port})

      try do
        for _ <- 1..4 do
          {:ok, socket} = :gen_tcp.accept(listener, 4000)
          {:ok, request} = :gen_tcp.recv(socket, 0, 2000)

          cond do
            String.starts_with?(request, "POST /auth/bearer-session ") ->
              reply(socket, "{}", "set-cookie: caller=alice; HttpOnly\r\n")

            String.starts_with?(request, "GET /auth/session ") ->
              reply(
                socket,
                Jason.encode!(%{
                  authenticated: true,
                  expires_at: System.system_time(:second) + 60,
                  user: %{sub: "alice"},
                  csrf_token: "csrf-alice"
                }),
                ""
              )

            String.starts_with?(request, "POST /v1/projects ") ->
              send(owner, :write_attempt)
          end

          :gen_tcp.close(socket)
        end
      after
        :gen_tcp.close(listener)
      end
    end)
  end

  defp reply(socket, body, extra) do
    :gen_tcp.send(
      socket,
      "HTTP/1.1 200 OK\r\nconnection: close\r\ncontent-length: #{byte_size(body)}\r\n#{extra}\r\n#{body}"
    )
  end
end
