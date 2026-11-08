defmodule LabbyConsole.DiagnosticsFixture do
  import Plug.Conn

  def respond(conn) do
    case Application.get_env(:labby_console, :fixture_diagnostics, "normal") do
      "malformed" ->
        send_resp(conn, 200, "invalid json")

      "invalid" ->
        send_resp(
          conn,
          200,
          Jason.encode!(%{
            findings: [
              %{service: "system", check: "disk", severity: "unexpected", message: "Bad"}
            ]
          })
        )

      "empty" ->
        send_resp(conn, 200, ~s({"findings":[]}))

      "forbidden" ->
        send_resp(conn, 403, "private diagnostics failure")

      "oversized" ->
        send_resp(conn, 200, String.duplicate("x", 70_000))

      _ ->
        send_resp(
          conn,
          200,
          Jason.encode!(%{
            findings: [
              %{
                service: "system",
                check: "disk:space",
                severity: "warn",
                message: "Disk space is low",
                ignored: "not projected"
              }
            ]
          })
        )
    end
  end
end
