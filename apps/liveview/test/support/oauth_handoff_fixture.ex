defmodule LabbyConsole.OAuthHandoffFixture do
  use Plug.Router
  plug :match
  plug :dispatch

  match _ do
    {:ok, raw, conn} = read_body(conn)
    state = Agent.get(conn.private.fixture, & &1)
    send(state.owner, {:handoff_request, conn.request_path, conn.req_headers, raw})
    mode = Map.get(state.modes, conn.request_path)

    {status, body, cookie} =
      case conn.request_path do
        "/auth/desktop/start" ->
          data = Jason.decode!(raw)
          Agent.update(conn.private.fixture, &Map.put(&1, :challenge, data["code_challenge"]))

          {201,
           Jason.encode!(%{
             authorization_url: state.origin <> "/auth/desktop/authorize?state=fixture",
             poll_token: "private-poll",
             redeem_code: "private-redeem",
             expires_at: System.system_time(:second) + 300
           }), nil}

        "/auth/desktop/poll" ->
          {if(state.ready, do: 200, else: 202),
           Jason.encode!(%{ready: state.ready, expires_at: System.system_time(:second) + 290}),
           nil}

        "/auth/desktop/redeem" ->
          data = Jason.decode!(raw)

          actual =
            :crypto.hash(:sha256, data["code_verifier"]) |> Base.url_encode64(padding: false)

          if data["redeem_code"] == "private-redeem" and actual == state.challenge,
            do: {204, "", "provider=private-cookie; HttpOnly"},
            else: {400, "{}", nil}

        "/auth/session" ->
          {200,
           Jason.encode!(%{
             authenticated: true,
             expires_at: System.system_time(:second) + 120,
             user: %{sub: "provider-user"},
             csrf_token: "private-csrf"
           }), nil}

        "/auth/logout" ->
          {204, "", nil}

        _ ->
          {404, "", nil}
      end

    conn = if cookie, do: put_resp_header(conn, "set-cookie", cookie), else: conn

    case mode do
      {:response, status, body} ->
        send_resp(conn, status, body)

      :oversized_headers ->
        conn
        |> put_resp_header("x-padding", String.duplicate("x", 70_000))
        |> send_resp(status, body)

      :trickle ->
        trickle(send_chunked(conn, status), 18)

      :hold ->
        receive do
          :release -> send_resp(conn, status, body)
        after
          6_000 -> send_resp(conn, status, body)
        end

      _ ->
        send_resp(conn, status, body)
    end
  end

  defp trickle(conn, 0), do: conn

  defp trickle(conn, count) do
    Process.sleep(400)

    case chunk(conn, " ") do
      {:ok, conn} -> trickle(conn, count - 1)
      {:error, _} -> conn
    end
  end

  def init(opts), do: opts
  def call(conn, opts), do: super(put_private(conn, :fixture, opts[:state]), opts)
end
