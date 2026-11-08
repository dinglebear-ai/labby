defmodule LabbyConsole.UsageMetricsFixture do
  import Plug.Conn

  def respond(conn) do
    if owner = Application.get_env(:labby_console, :fixture_usage_metrics_gate) do
      send(owner, {:usage_request, self()})

      receive do
        :release_usage -> :ok
      after
        5_000 -> :ok
      end
    end

    params = conn.body_params["params"]

    if owner = Application.get_env(:labby_console, :fixture_usage_metrics_observer),
      do: send(owner, {:usage_params, params})

    row = %{
      window_total_calls: 8,
      total_calls: 8,
      error_calls: 1,
      avg_elapsed_ms: 125.5,
      p50_elapsed_ms: 50,
      p95_elapsed_ms: 300,
      p99_elapsed_ms: 500,
      distinct_tools: 3,
      distinct_actors: 2,
      peak_per_min: 4,
      timeseries: [%{ts_unix: params["since_unix"], calls: 8, failed: 1}],
      private_token: "do-not-display"
    }

    case Application.get_env(:labby_console, :fixture_usage_metrics, "normal") do
      "forbidden" ->
        send_resp(conn, 403, "private error")

      "unavailable" ->
        send_resp(conn, 503, "private error")

      "malformed" ->
        send_resp(conn, 200, Jason.encode!(%{row | error_calls: 9}))

      "oversized" ->
        send_resp(
          conn,
          200,
          Jason.encode!(%{row | timeseries: List.duplicate(hd(row.timeseries), 169)})
        )

      "empty" ->
        send_resp(
          conn,
          200,
          Jason.encode!(
            Map.new(row, fn {k, v} ->
              {k,
               cond do
                 k == :timeseries -> []
                 is_number(v) -> 0
                 true -> v
               end}
            end)
          )
        )

      _ ->
        send_resp(conn, 200, Jason.encode!(row))
    end
  end
end
