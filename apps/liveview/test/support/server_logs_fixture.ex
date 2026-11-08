defmodule LabbyConsole.ServerLogsFixture do
  import Plug.Conn

  def respond(conn) do
    if owner = Application.get_env(:labby_console, :fixture_server_logs_owner),
      do: send(owner, {:logs_query, conn.body_params})

    if owner = Application.get_env(:labby_console, :fixture_server_logs_gate) do
      send(owner, {:logs_request, self()})

      receive do
        :release_logs -> :ok
      after
        1500 -> :ok
      end
    end

    entry = %{
      timestamp: "2026-10-27T14:00:00Z",
      level: "ERROR",
      target: "labby::gateway",
      message: "<script>boom</script>",
      service: "gateway",
      fields: %{token: "private-token"}
    }

    mode = Application.get_env(:labby_console, :fixture_server_logs, "normal")

    {status, body} =
      case mode do
        "forbidden" ->
          {403, "private failure"}

        "malformed" ->
          {200, "bad json"}

        "invalid" ->
          {200,
           Jason.encode!(%{kind: "server_logs", entries: [%{level: "invalid"}], truncated: false})}

        "oversized" ->
          {200, String.duplicate("x", 70_000)}

        "too_many" ->
          {200,
           Jason.encode!(%{
             kind: "server_logs",
             entries: List.duplicate(%{message: "x"}, 201),
             truncated: false
           })}

        "ansi" ->
          {200,
           Jason.encode!(%{
             kind: "server_logs",
             entries: [Map.put(entry, :message, "\e[31merror\e[0m")],
             truncated: false
           })}

        "long" ->
          {200,
           Jason.encode!(%{
             kind: "server_logs",
             entries:
               Enum.map(1..80, fn i ->
                 Map.put(entry, :message, "<script>boom</script> line #{i}")
               end),
             truncated: true
           })}

        "empty" ->
          {200, Jason.encode!(%{kind: "server_logs", entries: [], truncated: false})}

        _ ->
          {200,
           Jason.encode!(%{
             kind: "server_logs",
             entries: [entry],
             truncated: true,
             log_dir: "/private/path"
           })}
      end

    send_resp(conn, status, body)
  end
end
