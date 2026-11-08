defmodule LabbyConsole.UsageCallsFixture do
  import Plug.Conn

  def respond(conn) do
    mode = Application.get_env(:labby_console, :fixture_usage_calls, "normal")
    p = conn.body_params["params"]

    cond do
      mode == "forbidden" ->
        send_resp(conn, 403, "private failure")

      mode == "malformed" ->
        send_resp(conn, 200, "invalid json")

      mode == "oversized" ->
        send_resp(conn, 200, String.duplicate("x", 70_000))

      mode == "invalid" ->
        send_resp(conn, 200, Jason.encode!(%{calls: [%{id: "bad"}]}))

      mode == "invalid_cursor" ->
        send_resp(conn, 200, Jason.encode!(%{calls: [], next_cursor: 123}))

      mode == "empty" ->
        send_resp(conn, 200, ~s({"calls":[]}))

      p["limit"] != 25 || !is_integer(p["since_unix"]) || !is_integer(p["until_unix"]) ->
        send_resp(conn, 400, "invalid query")

      true ->
        next? = p["cursor"] == "1730000000:41"

        row = %{
          id: if(next?, do: 40, else: 41),
          ts_unix: if(mode == "invalid_time", do: 9_223_372_036_854_775_807, else: 1_730_000_000),
          upstream: "fetch",
          tool:
            cond do
              p["until_unix"] >=
                  Application.get_env(
                    :labby_console,
                    :fixture_usage_calls_fresh_after,
                    9_223_372_036_854_775_807
                  ) ->
                "new-call-after-open"

              next? ->
                "fetch_html"

              true ->
                "fetch"
            end,
          capability: "tool",
          operation: "call",
          subject_scoped: true,
          actor: "fixture-user",
          outcome: p["outcome"] || "ok",
          elapsed_ms: 53,
          payload: "private-payload"
        }

        send_resp(
          conn,
          200,
          Jason.encode!(%{calls: [row], next_cursor: if(next?, do: nil, else: "1730000000:41")})
        )
    end
  end
end
