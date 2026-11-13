defmodule LabbyConsole.AuthFixture do
  use Plug.Router
  plug Plug.Parsers, parsers: [:json], json_decoder: Jason
  plug :match
  plug :dispatch

  post "/auth/bearer-session" do
    case Plug.Conn.get_req_header(conn, "authorization") do
      ["Bearer fixture-only"] ->
        Agent.update(conn.private.state, fn _ -> true end)

        conn
        |> Plug.Conn.put_resp_header("set-cookie", "fixture_session=one; HttpOnly; Path=/")
        |> send_resp(200, "{}")

      _ ->
        send_resp(conn, 401, "{}")
    end
  end

  get "/auth/session" do
    case Application.get_env(:labby_console, :fixture_auth_session) do
      :unavailable ->
        send_resp(conn, 503, "{}")

      :expired ->
        send_resp(
          conn,
          200,
          Jason.encode!(%{
            authenticated: true,
            user: %{sub: "fixture-user"},
            expires_at: 1,
            csrf_token: "fixture-csrf"
          })
        )

      _ ->
        inspect_session(conn)
    end
  end

  defp inspect_session(conn) do
    if Agent.get(conn.private.state, & &1) &&
         Plug.Conn.get_req_header(conn, "cookie") == ["fixture_session=one"] do
      send_resp(
        conn,
        200,
        Jason.encode!(%{
          authenticated: true,
          user: %{sub: "fixture-user"},
          expires_at: System.system_time(:second) + 60,
          csrf_token: "fixture-csrf"
        })
      )
    else
      send_resp(conn, 401, "{}")
    end
  end

  post "/auth/logout" do
    if Application.get_env(:labby_console, :fixture_logout) == :unavailable do
      send_resp(conn, 503, "{}")
    else
      logout(conn)
    end
  end

  defp logout(conn) do
    if Plug.Conn.get_req_header(conn, "x-csrf-token") == ["fixture-csrf"] do
      Agent.update(conn.private.state, fn _ -> false end)
      send_resp(conn, 204, "")
    else
      send_resp(conn, 403, "")
    end
  end

  post "/v1/gateway" do
    action = conn.body_params["action"]

    if action != "gateway.list" do
      if authorized?(conn) do
        case action do
          "gateway.usage.metrics" ->
            LabbyConsole.UsageMetricsFixture.respond(conn)

          "gateway.usage.calls" ->
            LabbyConsole.UsageCallsFixture.respond(conn)

          "gateway.host.metrics" ->
            LabbyConsole.RuntimeFixture.respond(conn)

          "gateway.clients.list" ->
            LabbyConsole.ClientsFixture.respond(conn)

          "gateway.mcp.list" ->
            LabbyConsole.ServerRuntimeFixture.respond(conn)

          "gateway.server.get" ->
            LabbyConsole.ServerDetailsFixture.respond(conn)

          a
          when a in [
                 "gateway.discovered_tools",
                 "gateway.discovered_resources",
                 "gateway.discovered_prompts"
               ] ->
            LabbyConsole.ServerCatalogFixture.respond(conn)

          _ ->
            send_resp(conn, 404, "{}")
        end
      else
        send_resp(conn, 401, "{}")
      end
    else
      mode = Application.get_env(:labby_console, :fixture_inventory, :normal)

      if Agent.get(conn.private.state, & &1) &&
           Plug.Conn.get_req_header(conn, "cookie") == ["fixture_session=one"] &&
           Plug.Conn.get_req_header(conn, "x-csrf-token") == ["fixture-csrf"] &&
           conn.body_params == %{"action" => "gateway.list", "params" => %{}} do
        case mode do
          :forbidden ->
            send_resp(conn, 403, "private failure")

          :unauthorized ->
            send_resp(conn, 401, "revoked caller")

          :invalid_rows ->
            send_resp(
              conn,
              200,
              Jason.encode!([
                %{
                  id: "bad",
                  name: "bad",
                  enabled: true,
                  connected: true,
                  display_name: %{unsafe: "data"}
                }
              ])
            )

          :oversized ->
            send_resp(conn, 200, String.duplicate("x", 70_000))

          :malformed ->
            send_resp(conn, 200, "not json")

          :empty ->
            send_resp(conn, 200, "[]")

          _ ->
            conn
            |> put_resp_content_type("application/json")
            |> send_resp(
              200,
              Jason.encode!([
                %{id: "fixture-server", name: "fixture-server", enabled: true, connected: true}
              ])
            )
        end
      else
        send_resp(conn, 401, "{}")
      end
    end
  end

  post "/v1/server_logs" do
    if authorized?(conn),
      do: LabbyConsole.ServerLogsFixture.respond(conn),
      else: send_resp(conn, 401, "{}")
  end

  post "/v1/doctor" do
    if authorized?(conn),
      do: LabbyConsole.DiagnosticsFixture.respond(conn),
      else: send_resp(conn, 401, "{}")
  end

  post "/fixture/:slice/:mode"
       when slice in [
              "server_details",
              "server_catalog",
              "diagnostics",
              "runtime",
              "clients",
              "server_runtime",
              "usage_metrics",
              "usage_calls",
              "server_logs"
            ] do
    key =
      case slice do
        "usage_metrics" -> :fixture_usage_metrics
        "usage_calls" -> :fixture_usage_calls
        "server_logs" -> :fixture_server_logs
        "runtime" -> :fixture_runtime
        "clients" -> :fixture_clients
        "server_runtime" -> :fixture_server_runtime
        "server_details" -> :fixture_server_details
        "server_catalog" -> :fixture_server_catalog
        "diagnostics" -> :fixture_diagnostics
      end

    Application.put_env(:labby_console, key, mode)
    send_resp(conn, 204, "")
  end

  post "/fixture/inventory/:mode" do
    mode =
      case mode do
        "empty" -> :empty
        "malformed" -> :malformed
        "forbidden" -> :forbidden
        "unauthorized" -> :unauthorized
        _ -> :normal
      end

    Application.put_env(:labby_console, :fixture_inventory, mode)
    send_resp(conn, 204, "")
  end

  post "/fixture/expire" do
    Agent.update(conn.private.state, fn _ -> false end)
    send_resp(conn, 204, "")
  end

  match _ do
    send_resp(conn, 404, "")
  end

  defp authorized?(conn),
    do:
      Agent.get(conn.private.state, & &1) &&
        Plug.Conn.get_req_header(conn, "cookie") == ["fixture_session=one"] &&
        Plug.Conn.get_req_header(conn, "x-csrf-token") == ["fixture-csrf"]

  def init(opts), do: opts
  def call(conn, opts), do: super(Plug.Conn.put_private(conn, :state, opts[:state]), opts)
end
