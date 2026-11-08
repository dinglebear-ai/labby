defmodule LabbyConsole.ServerCatalogFixture do
  import Plug.Conn

  def respond(conn) do
    mode = Application.get_env(:labby_console, :fixture_server_catalog, "normal")
    action = conn.body_params["action"]

    cond do
      conn.body_params["params"] != %{"name" => "fixture-server"} ->
        send_resp(conn, 404, "{}")

      mode == "empty_description" ->
        send_resp(
          conn,
          200,
          Jason.encode!(
            if action == "gateway.discovered_tools",
              do: [%{name: "fetch", description: "", exposed: false}],
              else: []
          )
        )

      mode == "forbidden" ->
        send_resp(conn, 403, "{}")

      mode == "malformed" ->
        send_resp(conn, 200, "{broken")

      mode == "invalid" ->
        send_resp(conn, 200, Jason.encode!([%{name: "fetch", exposed: "yes"}]))

      mode == "oversized" ->
        send_resp(conn, 200, String.duplicate("x", 70_000))

      mode == "empty" ->
        send_resp(conn, 200, "[]")

      action == "gateway.discovered_tools" ->
        send_resp(
          conn,
          200,
          Jason.encode!([
            %{
              name: "fetch",
              description: "Fetch a document",
              exposed: true,
              private: "do not render"
            }
          ])
        )

      action == "gateway.discovered_resources" ->
        send_resp(conn, 200, Jason.encode!(["file:///README.md"]))

      action == "gateway.discovered_prompts" ->
        send_resp(conn, 200, Jason.encode!(["review"]))

      true ->
        send_resp(conn, 404, "{}")
    end
  end
end
