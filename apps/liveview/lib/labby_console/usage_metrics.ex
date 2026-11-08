defmodule LabbyConsole.UsageMetrics do
  @moduledoc "Bounded projection of exact, caller-scoped gateway usage aggregates."
  alias LabbyConsole.Auth
  @windows %{"1h" => 3600, "24h" => 86400, "7d" => 604_800}
  @counts ~w(window_total_calls total_calls error_calls p50_elapsed_ms p95_elapsed_ms p99_elapsed_ms distinct_tools distinct_actors peak_per_min)
  def fetch(session, window, now \\ System.system_time(:second)) do
    case Map.fetch(@windows, window) do
      {:ok, duration} ->
        with {:ok, row} <-
               Auth.read_action(session, "gateway", "gateway.usage.metrics", %{
                 "since_unix" => now - duration,
                 "until_unix" => now,
                 "bucket_count" => 24,
                 "timezone" => "UTC",
                 "include_facets" => false,
                 "include_upstream_timeseries" => false
               }) do
          project(row, now - duration, now)
        end

      :error ->
        {:error, :unavailable}
    end
  end

  defp project(row, first, last) when is_map(row) do
    series = row["timeseries"]

    valid =
      Enum.all?(@counts, &(is_integer(row[&1]) && row[&1] >= 0)) &&
        is_number(row["avg_elapsed_ms"]) && row["avg_elapsed_ms"] >= 0 &&
        row["error_calls"] <= row["total_calls"] &&
        row["total_calls"] <= row["window_total_calls"] &&
        is_list(series) && length(series) <= 168 &&
        Enum.all?(series, &valid_bucket?(&1, first, last))

    if valid do
      {:ok,
       Map.take(row, @counts ++ ["avg_elapsed_ms"])
       |> Map.put("timeseries", Enum.map(series, &Map.take(&1, ~w(ts_unix calls failed))))}
    else
      {:error, :unavailable}
    end
  end

  defp project(_, _, _), do: {:error, :unavailable}

  defp valid_bucket?(bucket, first, last) when is_map(bucket) do
    Enum.all?(~w(ts_unix calls failed), &is_integer(bucket[&1])) && bucket["ts_unix"] >= first &&
      bucket["ts_unix"] <= last && bucket["calls"] >= 0 && bucket["failed"] >= 0 &&
      bucket["failed"] <= bucket["calls"]
  end

  defp valid_bucket?(_, _, _), do: false
end
