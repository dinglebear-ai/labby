defmodule LabbyConsole.OverviewFixture do
  import Plug.Conn
  def init(opts), do: opts

  def call(conn, opts) do
    conn = Plug.Parsers.call(conn, Plug.Parsers.init(parsers: [:json], json_decoder: Jason))

    if conn.request_path == "/v1/gateway" and
         conn.body_params["action"] == "gateway.usage.metrics" do
      %{owner: owner, mode: mode} = Agent.get(opts[:data_state], & &1)
      params = conn.body_params["params"]
      send(owner, {:overview_metrics_params, params})

      if mode == :hold do
        send(owner, {:overview_hold, self()})

        receive do
          :release_overview -> :ok
        after
          4000 -> :ok
        end
      end

      cond do
        get_req_header(conn, "cookie") != ["fixture_session=one"] -> send_resp(conn, 401, "{}")
        mode == :forbidden -> send_resp(conn, 403, "{}")
        mode == :malformed -> send_resp(conn, 200, ~s({"calls":"bad"}))
        true -> send_resp(conn, 200, Jason.encode!(sample(params, mode)))
      end
    else
      if conn.request_path == "/v1/gateway" and
           conn.body_params["action"] == "gateway.usage.calls" do
        %{owner: owner} = Agent.get(opts[:data_state], & &1)
        p = conn.body_params["params"]
        send(owner, {:overview_calls_params, p})

        row = %{
          id: 41,
          ts_unix: p["since_unix"],
          upstream: p["upstream"] || "files",
          tool: p["tool"] || "read_file",
          capability: "tool",
          operation: "call",
          subject_scoped: true,
          actor: p["actor"] || "verified-actor",
          outcome: p["outcome"] || "ok",
          elapsed_ms: 53,
          payload: "private-payload"
        }

        send_resp(conn, 200, Jason.encode!(%{calls: [row], next_cursor: "opaque-next"}))
      else
        LabbyConsole.AuthFixture.call(conn, opts)
      end
    end
  end

  defp sample(p, mode) do
    empty = mode == :empty

    count =
      cond do
        empty -> 0
        mode == :hold -> 99
        true -> 8
      end

    tool = %{
      upstream: "files",
      tool: "read_file",
      capability: "tool",
      operation: "call",
      subject_scoped: true,
      calls: count,
      failed: 0
    }

    %{
      window_total_calls: count,
      total_calls: count,
      error_calls: 0,
      avg_elapsed_ms: 125.5,
      p50_elapsed_ms: 50,
      p95_elapsed_ms: 300,
      p99_elapsed_ms: 500,
      distinct_tools: if(empty, do: 0, else: 1),
      distinct_actors: if(empty, do: 0, else: 1),
      peak_per_min: count,
      timeseries: [%{ts_unix: p["since_unix"], calls: count, failed: 0, outcomes: []}],
      top_tools: if(empty, do: [], else: [tool]),
      least_tools: if(empty, do: [], else: [tool]),
      slowest_tools: if(empty, do: [], else: [Map.put(tool, :avg_elapsed_ms, 125.5)]),
      top_actors: if(empty, do: [], else: [%{actor: "verified-actor", calls: count}]),
      upstreams: if(empty, do: [], else: [%{upstream: "files", calls: count, failed: 0}]),
      errors: [],
      private: "never-render-this"
    }
  end
end
