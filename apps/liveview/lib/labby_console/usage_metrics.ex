defmodule LabbyConsole.UsageMetrics do
  @moduledoc "Bounded projection of exact, caller-scoped gateway usage aggregates."
  alias LabbyConsole.Auth
  @windows %{"1h" => 3600, "24h" => 86400, "7d" => 604_800, "30d" => 2_592_000}
  @counts ~w(window_total_calls total_calls error_calls p50_elapsed_ms p95_elapsed_ms p99_elapsed_ms distinct_tools distinct_actors peak_per_min)
  @tool_text ~w(upstream tool capability operation)
  @lists ~w(top_tools least_tools slowest_tools top_actors errors upstreams)
  @max_integer 9_223_372_036_854_775_807

  def window(value), do: @windows[value]

  def fetch(session, window, now \\ System.system_time(:second)) do
    case @windows[window] do
      duration
      when is_integer(duration) and is_integer(now) and now >= 0 and now <= 253_402_300_799 ->
        first = max(0, now - duration)

        with {:ok, row} <-
               Auth.read_action(session, "gateway", "gateway.usage.metrics", %{
                 "since_unix" => first,
                 "until_unix" => now,
                 "bucket_count" => 24,
                 "timezone" => "UTC",
                 "include_facets" => false,
                 "include_upstream_timeseries" => true
               }) do
          project(row, first, now)
        end

      _ ->
        {:error, :unavailable}
    end
  end

  @doc "Validate gateway output without converting unknown optional telemetry to zeros."
  def project(row, first, last) when is_map(row) do
    series = row["timeseries"]

    valid =
      Enum.all?(@counts, &count?(row[&1])) && number?(row["avg_elapsed_ms"]) &&
        row["error_calls"] <= row["total_calls"] &&
        row["total_calls"] <= row["window_total_calls"] &&
        is_list(series) && length(series) <= 168 &&
        Enum.all?(series, &valid_bucket?(&1, first, last)) &&
        Enum.map(series, & &1["ts_unix"]) ==
          Enum.sort(Enum.uniq(Enum.map(series, & &1["ts_unix"]))) &&
        Enum.all?(@lists, &valid_list?(row, &1)) &&
        valid_upstream_series?(row["upstream_timeseries"], series, first, last)

    if valid do
      projected = Map.take(row, @counts ++ ["avg_elapsed_ms"])
      projected = Map.put(projected, "timeseries", Enum.map(series, &bucket/1))

      projected =
        Enum.reduce(@lists, projected, fn key, acc ->
          if Map.has_key?(row, key),
            do: Map.put(acc, key, Enum.map(row[key], &rank(&1, key))),
            else: acc
        end)

      projected =
        if is_map(row["upstream_timeseries"]),
          do:
            Map.put(
              projected,
              "upstream_timeseries",
              Map.new(row["upstream_timeseries"], fn {key, values} ->
                {key, Enum.map(values, &bucket/1)}
              end)
            ),
          else: projected

      {:ok, projected}
    else
      {:error, :unavailable}
    end
  end

  def project(_, _, _), do: {:error, :unavailable}

  defp valid_list?(row, key) do
    !Map.has_key?(row, key) ||
      (is_list(row[key]) && length(row[key]) <= 100 && Enum.all?(row[key], &valid_rank?(&1, key)))
  end

  defp valid_rank?(row, key)
       when is_map(row) and key in ~w(top_tools least_tools slowest_tools) do
    Enum.all?(@tool_text, &text?(row[&1])) && is_boolean(row["subject_scoped"]) &&
      if(key == "slowest_tools",
        do: number?(row["avg_elapsed_ms"]),
        else: count?(row["calls"]) && count?(row["failed"]) && row["failed"] <= row["calls"]
      )
  end

  defp valid_rank?(row, "top_actors") when is_map(row),
    do: text?(row["actor"]) && count?(row["calls"])

  defp valid_rank?(row, "errors") when is_map(row), do: text?(row["kind"]) && count?(row["calls"])

  defp valid_rank?(row, "upstreams") when is_map(row),
    do:
      text?(row["upstream"]) && count?(row["calls"]) && count?(row["failed"]) &&
        row["failed"] <= row["calls"]

  defp valid_rank?(_, _), do: false

  defp rank(row, "slowest_tools"),
    do: Map.take(row, @tool_text ++ ~w(subject_scoped avg_elapsed_ms))

  defp rank(row, key) when key in ~w(top_tools least_tools),
    do: Map.take(row, @tool_text ++ ~w(subject_scoped calls failed))

  defp rank(row, "top_actors") do
    projected = Map.take(row, ~w(actor calls))

    if is_map(row["attribution"]) do
      attribution =
        row["attribution"]
        |> Map.take(~w(client_name client_version surface agent_id task_id))
        |> Enum.filter(fn {_, value} -> text?(value) end)
        |> Map.new()

      Map.put(projected, "attribution", attribution)
    else
      projected
    end
  end

  defp rank(row, "errors"), do: Map.take(row, ~w(kind calls))
  defp rank(row, "upstreams"), do: Map.take(row, ~w(upstream calls failed))

  defp valid_bucket?(bucket, first, last) when is_map(bucket) do
    Enum.all?(~w(ts_unix calls failed), &count?(bucket[&1])) && bucket["ts_unix"] >= first &&
      bucket["ts_unix"] <= last && bucket["failed"] <= bucket["calls"] &&
      (!Map.has_key?(bucket, "outcomes") ||
         (is_list(bucket["outcomes"]) && length(bucket["outcomes"]) <= 100 &&
            Enum.all?(bucket["outcomes"], &valid_rank?(&1, "errors")) &&
            Enum.sum(Enum.map(bucket["outcomes"], & &1["calls"])) <= bucket["calls"]))
  end

  defp valid_bucket?(_, _, _), do: false

  defp valid_upstream_series?(nil, _, _, _), do: true

  defp valid_upstream_series?(value, totals, first, last)
       when is_map(value) and map_size(value) <= 4 do
    Enum.all?(value, fn {name, rows} ->
      text?(name) && is_list(rows) && length(rows) <= 168 &&
        Enum.all?(rows, &valid_bucket?(&1, first, last)) &&
        Enum.map(rows, & &1["ts_unix"]) == Enum.map(totals, & &1["ts_unix"])
    end) &&
      Enum.all?(Enum.with_index(totals), fn {total, index} ->
        rows = Enum.map(value, fn {_, series} -> Enum.at(series, index) end)

        Enum.sum(Enum.map(rows, & &1["calls"])) <= total["calls"] &&
          Enum.sum(Enum.map(rows, & &1["failed"])) <= total["failed"]
      end)
  end

  defp valid_upstream_series?(_, _, _, _), do: false

  defp bucket(row) do
    value = Map.take(row, ~w(ts_unix calls failed))

    if Map.has_key?(row, "outcomes"),
      do: Map.put(value, "outcomes", Enum.map(row["outcomes"], &Map.take(&1, ~w(kind calls)))),
      else: value
  end

  defp text?(value), do: is_binary(value) && String.valid?(value) && byte_size(value) <= 4096
  defp count?(value), do: is_integer(value) && value >= 0 && value <= @max_integer
  defp number?(value), do: is_number(value) && value >= 0 && value <= @max_integer
end
