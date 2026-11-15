defmodule LabbyConsole.ClientsFixture do
  import Plug.Conn

  def respond(conn) do
    if owner = Application.get_env(:labby_console, :fixture_clients_gate) do
      send(owner, {:clients_request, self()})

      receive do
        :release_clients -> :ok
      after
        5_000 -> :ok
      end
    end

    if conn.body_params != %{"action" => "gateway.clients.list", "params" => %{}} do
      send_resp(conn, 400, "invalid contract")
    else
      case Application.get_env(:labby_console, :fixture_clients, "normal") do
        "malformed" ->
          send_resp(conn, 200, "invalid json")

        "invalid" ->
          send_resp(
            conn,
            200,
            Jason.encode!([
              %{transport: "http", connected_at: "2026-10-27T10:00:00Z", observation_count: -1}
            ])
          )

        "oversized" ->
          send_resp(conn, 200, String.duplicate("x", 70_000))

        "empty" ->
          send_resp(conn, 200, "[]")

        "unauthorized" ->
          send_resp(conn, 401, "revoked caller")

        "forbidden" ->
          send_resp(conn, 403, "private failure")

        "unavailable" ->
          send_resp(conn, 503, "private failure")

        "minimal" ->
          send_resp(
            conn,
            200,
            Jason.encode!([%{transport: "stdio", connected_at: "2026-10-27T10:00:00Z"}])
          )

        _ ->
          send_resp(
            conn,
            200,
            Jason.encode!([
              %{
                client_name: "Codex",
                client_version: "0.46.0",
                subject: "sub:ab12cd34ef56",
                authorized_client_id: "fixture-client",
                transport: "streamable-http",
                connected_at: "2026-10-27T10:00:00Z",
                last_seen_at: "2026-10-27T10:05:00Z",
                observation_count: 42,
                credential: "not projected"
              }
            ])
          )
      end
    end
  end
end
