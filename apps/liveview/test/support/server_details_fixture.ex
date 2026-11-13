defmodule LabbyConsole.ServerDetailsFixture do
  @moduledoc false
  import Plug.Conn

  def respond(conn) do
    if owner = Application.get_env(:labby_console, :fixture_server_details_gate) do
      send(owner, {:details_request, self()})

      receive do
        :release_details -> :ok
      after
        5_000 -> :ok
      end
    end

    case Application.get_env(:labby_console, :fixture_server_details, "normal") do
      "unauthorized" ->
        send_resp(conn, 401, "revoked caller")

      "forbidden" ->
        send_resp(conn, 403, "private upstream failure")

      "unavailable" ->
        send_resp(conn, 503, "private upstream failure")

      "malformed" ->
        send_resp(conn, 200, Jason.encode!(%{id: "fixture-server", name: %{invalid: true}}))

      "wrong_id" ->
        send_resp(conn, 200, Jason.encode!(row("another-server")))

      _ ->
        send_resp(conn, 200, Jason.encode!(row(conn.body_params["params"]["id"])))
    end
  end

  defp row(id) do
    %{
      id: id,
      name: id,
      display_name: "Fixture Fetch",
      source: "custom_gateway",
      configured: true,
      enabled: true,
      connected: true,
      discovered_tool_count: 4,
      exposed_tool_count: 3,
      discovered_resource_count: 2,
      exposed_resource_count: 1,
      discovered_prompt_count: 0,
      exposed_prompt_count: 0,
      discovered_skill_count: 1,
      exposed_skill_count: 1,
      config_summary: %{url: "https://secret.example/?credential=do-not-display"},
      warnings: [%{message: "private-warning"}]
    }
  end
end
