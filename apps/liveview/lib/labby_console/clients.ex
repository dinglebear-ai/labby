defmodule LabbyConsole.Clients do
  @moduledoc "Bounded observed MCP client history, scoped to the gateway caller."
  alias LabbyConsole.Auth
  @optional ~w(subject authorized_client_id client_name client_version last_seen_at)
  @fields ~w(subject authorized_client_id client_name client_version transport connected_at last_seen_at observation_count)

  def list(session_id) do
    with {:ok, rows} when is_list(rows) and length(rows) <= 1000 <-
           Auth.read_action(session_id, "gateway", "gateway.clients.list", %{}),
         true <- Enum.all?(rows, &valid?/1) do
      {:ok, Enum.map(rows, &(Map.take(&1, @fields) |> Map.put_new("observation_count", 0)))}
    else
      {:error, :unauthenticated} -> {:error, :unauthenticated}
      _ -> {:error, :unavailable}
    end
  end

  defp valid?(row) when is_map(row) do
    Enum.all?(~w(transport connected_at), &text?(row[&1])) &&
      Enum.all?(@optional, &(is_nil(row[&1]) || text?(row[&1]))) &&
      is_integer(Map.get(row, "observation_count", 0)) &&
      Map.get(row, "observation_count", 0) in 0..18_446_744_073_709_551_615
  end

  defp valid?(_), do: false
  defp text?(text), do: is_binary(text) && String.valid?(text) && byte_size(text) in 1..4096
end
