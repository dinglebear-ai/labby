defmodule LabbyConsole.Runtime do
  @moduledoc "Bounded caller-authorized daemon runtime samples, with unsupported values preserved."
  @integers ~w(cpu_cores memory_used_bytes memory_total_bytes disk_used_bytes disk_total_bytes child_process_count child_rss_bytes)
  @numbers ~w(cpu_percent network_rx_bytes_per_second network_tx_bytes_per_second)
  @keys ~w(available scope hostname platform memory_limit_is_cgroup sample_ms) ++
          @integers ++ @numbers

  def metrics(id) do
    with {:ok, data} when is_map(data) <-
           LabbyConsole.Auth.read_action(id, "gateway", "gateway.host.metrics", %{}),
         true <- valid?(data) do
      {:ok, Map.take(data, @keys)}
    else
      {:error, :unauthenticated} -> {:error, :unauthenticated}
      _ -> {:error, :unavailable}
    end
  end

  defp valid?(data) do
    is_boolean(data["available"]) && is_boolean(data["memory_limit_is_cgroup"]) &&
      text?(data["scope"], 4096) && text?(data["platform"], 256) &&
      (is_nil(data["hostname"]) || text?(data["hostname"], 255)) &&
      is_integer(data["sample_ms"]) && data["sample_ms"] >= 0 &&
      Enum.all?(@integers, &(Map.has_key?(data, &1) && nullable_integer?(data[&1]))) &&
      Enum.all?(@numbers, &(Map.has_key?(data, &1) && nullable_number?(data[&1]))) &&
      (is_nil(data["cpu_percent"]) || data["cpu_percent"] <= 100)
  end

  defp text?(value, max), do: is_binary(value) && String.valid?(value) && byte_size(value) <= max
  defp nullable_integer?(nil), do: true
  defp nullable_integer?(value), do: is_integer(value) && value >= 0
  defp nullable_number?(nil), do: true
  defp nullable_number?(value), do: is_number(value) && value >= 0
end
