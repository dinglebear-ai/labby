defmodule LabbyConsole.Observe do
  @moduledoc "Bounded native Observe projections over caller-authorized gateway reads."
  alias LabbyConsole.{UsageCalls, ServerLogs}

  @call_fields ~w(id ts_unix upstream tool capability operation subject_scoped outcome elapsed_ms attribution response_bytes)
  @log_fields ~w(timestamp level target message service)

  def preferences(value) do
    value = if is_map(value), do: value, else: %{}

    %{
      "tab" => allowed(value["tab"], ~w(activity logs traces), "activity"),
      "window" => allowed(value["window"], ~w(1h 24h 7d), "24h")
    }
  end

  def bounds(window, now), do: {max(0, now - seconds(window)), now}
  defp seconds("1h"), do: 3600
  defp seconds("7d"), do: 604_800
  defp seconds(_), do: 86_400

  def call_params(filters, window, now, cursor) do
    query = filters["query"] || ""
    upstream = filters["upstream"] || ""
    outcome = filters["outcome"] || ""

    if bounded?(query) && bounded?(upstream) && outcome in ["", "ok", "timeout", "upstream_error"] do
      {first, last} = bounds(window, now)

      %{since_unix: first, until_unix: last}
      |> optional(:search, query)
      |> optional(:upstream, upstream)
      |> optional(:outcome, outcome)
      |> optional(:cursor, cursor)
    else
      {:error, :invalid_filters}
    end
  end

  def fetch(id, "logs", filters, _window, _now, _cursor),
    do: ServerLogs.query(id, Map.take(filters, ~w(level query)))

  def fetch(id, _tab, filters, window, now, cursor) do
    case call_params(filters, window, now, cursor) do
      {:error, _} = error -> error
      params -> UsageCalls.list(id, params)
    end
  end

  def merge_calls(existing, rows),
    do: Enum.uniq_by(existing ++ rows, & &1["id"]) |> Enum.take(200)

  def selection(rows, selected),
    do: if(Enum.any?(rows, &(to_string(&1["id"]) == selected)), do: selected)

  def attribution(row) do
    metadata = row["attribution"] || %{}

    Enum.find_value(~w(client_name surface), "Unknown caller", fn key ->
      value = metadata[key]
      if is_binary(value) && String.trim(value) != "", do: value
    end)
  end

  def server_link(%{"upstream" => value}) when value in [".", ".."],
    do: "/gateway/upstream-runtime?" <> URI.encode_query(%{"name" => value})

  def server_link(%{"upstream" => value}) when is_binary(value) and value != "",
    do: "/gateway/upstreams/" <> URI.encode(value, &URI.char_unreserved?/1) <> "/runtime"

  def server_link(_), do: nil

  def export(tab, rows, metadata \\ %{}) do
    fields = if tab == "logs", do: @log_fields, else: @call_fields
    records = rows |> Enum.take(200) |> Enum.map(&Map.take(&1, fields))

    data =
      Jason.encode!(%{
        scope: "authorized_loaded_snapshot",
        view: tab,
        records: records,
        completeness: Map.take(metadata, [:truncated, :snapshot])
      })

    if byte_size(data) <= 65_536, do: {:ok, data}, else: {:error, :export_too_large}
  end

  defp optional(map, _key, value) when value in [nil, ""], do: map
  defp optional(map, key, value), do: Map.put(map, key, value)
  defp bounded?(value), do: is_binary(value) && String.valid?(value) && byte_size(value) <= 256
  defp allowed(value, values, default), do: if(value in values, do: value, else: default)
end
