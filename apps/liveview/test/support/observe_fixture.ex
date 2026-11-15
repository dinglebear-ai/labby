defmodule LabbyConsole.ObserveFixture do
  @moduledoc false
  import Plug.Conn
  def init(opts), do: opts

  def call(%{request_path: path} = conn, opts) when path in ["/v1/gateway", "/v1/server_logs"] do
    conn = Plug.Parsers.call(conn, Plug.Parsers.init(parsers: [:json], json_decoder: Jason))
    send(opts[:owner], {:observe_wire, conn.body_params, get_req_header(conn, "cookie")})

    case Agent.get(opts[:state], & &1) do
      {:response, status, body} -> send_resp(conn, status, Jason.encode!(body))
      _ -> send_resp(conn, 503, "{}")
    end
  end

  def call(conn, opts),
    do: LabbyConsole.AuthFixture.call(conn, LabbyConsole.AuthFixture.init(state: opts[:state]))
end
