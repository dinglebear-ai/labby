defmodule LabbyConsole.AuthDeadlineFixture do
  use Plug.Router
  plug :match
  plug :dispatch

  match _ do
    %{owner: owner, modes: modes} = Agent.get(conn.private.fixture, & &1)
    send(owner, {:auth_request, conn.request_path, self()})
    mode = Map.get(modes, conn.request_path, :normal)

    conn =
      if conn.request_path == "/auth/bearer-session",
        do: put_resp_header(conn, "set-cookie", "caller=deadline; HttpOnly"),
        else: conn

    body =
      if conn.request_path == "/auth/session",
        do:
          Jason.encode!(%{
            authenticated: true,
            expires_at: System.system_time(:second) + 120,
            user: %{sub: "deadline"},
            csrf_token: "private-csrf"
          }),
        else: "{}"

    case mode do
      :hold ->
        receive do
          {:respond, status, response} -> send_resp(conn, status, response)
        after
          2_000 -> send_resp(conn, 503, "{}")
        end

      :trickle ->
        trickle(send_chunked(conn, 200), body, 18)

      {:delay, delay} ->
        Process.sleep(delay)
        send_resp(conn, 200, body)

      :oversized_headers ->
        conn
        |> put_resp_header("x-padding", String.duplicate("x", 70_000))
        |> send_resp(200, body)

      :malformed ->
        send_resp(conn, 200, "invalid")

      :oversized ->
        send_resp(conn, 200, String.duplicate("x", 70_000))

      :no_cookie ->
        conn |> delete_resp_header("set-cookie") |> send_resp(200, body)

      status when is_integer(status) ->
        send_resp(conn, status, body)

      _ ->
        send_resp(conn, 200, body)
    end
  end

  defp trickle(conn, body, 0), do: elem(chunk(conn, body), 1)

  defp trickle(conn, body, count) do
    Process.sleep(400)

    case chunk(conn, " ") do
      {:ok, conn} -> trickle(conn, body, count - 1)
      {:error, _} -> conn
    end
  end

  def init(opts), do: opts
  def call(conn, opts), do: super(put_private(conn, :fixture, opts[:state]), opts)
end
