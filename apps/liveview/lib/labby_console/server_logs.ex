defmodule LabbyConsole.ServerLogs do
  @moduledoc "Bounded, caller-authorized server process log snapshots."
  alias LabbyConsole.Auth
  @levels ~w(TRACE DEBUG INFO WARN ERROR)
  def query(id, filters \\ %{}) do
    level = filters["level"] || ""
    query = filters["query"] || ""

    if level in ["" | @levels] && is_binary(query) && byte_size(query) <= 256 do
      params = %{limit: 200, max_scan_bytes: 32_768, stop_after_limit: true}
      params = if level == "", do: params, else: Map.put(params, :level, level)
      params = if query == "", do: params, else: Map.put(params, :query, query)

      with {:ok, %{"kind" => "server_logs", "entries" => rows, "truncated" => truncated}} <-
             Auth.read_action(id, "server_logs", "server_logs.query", params),
           true <-
             is_boolean(truncated) && is_list(rows) && length(rows) <= 200 &&
               Enum.all?(rows, &valid?/1) do
        {:ok,
         %{
           entries:
             Enum.map(
               Enum.reverse(rows),
               &project/1
             ),
           truncated: truncated
         }}
      else
        {:error, :unauthenticated} -> {:error, :unauthenticated}
        _ -> {:error, :unavailable}
      end
    else
      {:error, :unavailable}
    end
  end

  defp project(row) do
    row
    |> Map.take(~w(timestamp level target message service))
    |> Map.new(fn {key, value} ->
      {key,
       if(is_binary(value),
         do: Regex.replace(~r/\x1B\[[0-?]*[ -\/]*[@-~]/, value, ""),
         else: value
       )}
    end)
  end

  defp valid?(row) when is_map(row) do
    (is_nil(row["level"]) || row["level"] in @levels) &&
      Enum.all?(~w(timestamp target message service), fn key ->
        value = row[key]
        is_nil(value) || (is_binary(value) && String.valid?(value) && byte_size(value) <= 8192)
      end)
  end

  defp valid?(_), do: false
end
