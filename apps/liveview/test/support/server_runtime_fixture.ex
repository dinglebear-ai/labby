defmodule LabbyConsole.ServerRuntimeFixture do
  @moduledoc false
  import Plug.Conn

  def respond(conn) do
    if owner = Application.get_env(:labby_console, :fixture_server_runtime_gate) do
      send(owner, {:runtime_request, self()})

      receive do
        :release_runtime -> :ok
      after
        5_000 -> :ok
      end
    end

    mode = Application.get_env(:labby_console, :fixture_server_runtime, "normal")
    name = conn.body_params["params"]["name"]

    case mode do
      "forbidden" ->
        send_resp(conn, 403, "private upstream failure")

      "unavailable" ->
        send_resp(conn, 503, "private upstream failure")

      "malformed" ->
        send_resp(conn, 200, Jason.encode!([%{name: name, pid: "invalid"}]))

      "wrong_id" ->
        send_resp(conn, 200, Jason.encode!([row("another-server")]))

      "empty" ->
        send_resp(conn, 200, "[]")

      "disconnected" ->
        send_resp(
          conn,
          200,
          Jason.encode!([
            Map.merge(row(name), %{connected: false, pid: nil, pgid: nil, age_seconds: nil})
          ])
        )

      _ ->
        send_resp(conn, 200, Jason.encode!([row(name)]))
    end
  end

  defp row(name) do
    %{
      name: name,
      enabled: true,
      connected: true,
      pid: 4821,
      pgid: 4821,
      age_seconds: 3600,
      likely_stale_count: 2,
      transport: "stdio",
      server_version: "1.4.0",
      protocol_version: "2025-06-18",
      exposed_tool_count: 3,
      discovered_tool_count: 4,
      target: "private-command --secret=do-not-display",
      owner: %{subject: "private-owner"},
      runtime_state_path: "/private/state",
      notification_incidents: %{secret: "private-error"}
    }
  end
end
