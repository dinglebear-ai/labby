defmodule LabbyConsole.RuntimeFixture do
  import Plug.Conn

  def respond(conn) do
    if conn.body_params["params"] != %{}, do: raise("runtime params must be empty")

    case Application.get_env(:labby_console, :fixture_runtime, "normal") do
      "malformed" ->
        send_resp(conn, 200, "not json")

      "unauthorized" ->
        send_resp(conn, 401, "revoked caller")

      "forbidden" ->
        send_resp(conn, 403, "private failure")

      "unavailable" ->
        send_resp(conn, 503, "private failure")

      "invalid" ->
        send_resp(conn, 200, Jason.encode!(Map.put(sample(), :cpu_percent, -1)))

      "oversized" ->
        send_resp(conn, 200, String.duplicate("x", 70_000))

      "unsupported" ->
        send_resp(
          conn,
          200,
          Jason.encode!(
            Map.new(sample(), fn {key, value} ->
              {key,
               cond do
                 key == :available ->
                   false

                 key in [
                   :cpu_cores,
                   :cpu_percent,
                   :memory_used_bytes,
                   :memory_total_bytes,
                   :disk_used_bytes,
                   :disk_total_bytes,
                   :child_process_count,
                   :child_rss_bytes,
                   :network_rx_bytes_per_second,
                   :network_tx_bytes_per_second
                 ] ->
                   nil

                 true ->
                   value
               end}
            end)
          )
        )

      "slow" ->
        Process.sleep(2_200)
        send_resp(conn, 200, Jason.encode!(sample()))

      _ ->
        send_resp(conn, 200, Jason.encode!(sample()))
    end
  end

  defp sample do
    %{
      available: true,
      scope: "Daemon and descendants; data filesystem; shared namespace network",
      hostname: "labby-dev",
      platform: "linux/x86_64",
      cpu_cores: 8,
      cpu_percent: 6.4,
      memory_used_bytes: 356_515_840,
      memory_total_bytes: 8_589_934_592,
      memory_limit_is_cgroup: true,
      disk_used_bytes: 52_613_349_376,
      disk_total_bytes: 257_698_037_760,
      child_process_count: 4,
      child_rss_bytes: 198_180_864,
      network_rx_bytes_per_second: 18_432.0,
      network_tx_bytes_per_second: 7168.0,
      sample_ms: 5000,
      private: "not projected"
    }
  end
end
