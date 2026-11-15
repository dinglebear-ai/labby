defmodule LabbyConsole.GatewayFleetFixture do
  @moduledoc false
  import Plug.Conn
  def init(opts), do: opts

  def call(conn, opts) do
    conn = Plug.Parsers.call(conn, Plug.Parsers.init(parsers: [:json], json_decoder: Jason))
    rows = Application.get_env(:labby_console, :gateway_fleet_rows)

    authorized =
      Agent.get(opts[:state], & &1) && get_req_header(conn, "cookie") == ["fixture_session=one"] &&
        get_req_header(conn, "x-csrf-token") == ["fixture-csrf"]

    if conn.request_path == "/v1/gateway" &&
         conn.body_params == %{"action" => "gateway.list", "params" => %{}} && rows && authorized do
      if owner = Application.get_env(:labby_console, :gateway_fleet_gate) do
        send(owner, {:fleet_read, self()})

        receive do
          :release_fleet -> :ok
        after
          2_000 -> :ok
        end
      end

      send_resp(conn, 200, Jason.encode!(rows))
    else
      LabbyConsole.AuthFixture.call(conn, opts)
    end
  end
end
