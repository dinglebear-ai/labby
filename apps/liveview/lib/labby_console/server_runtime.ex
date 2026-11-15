defmodule LabbyConsole.ServerRuntime do
  @moduledoc "A passive, credential-free projection of the caller-visible MCP runtime snapshot."
  alias LabbyConsole.Auth

  @counts for family <- ~w(tool resource prompt skill),
              kind <- ~w(exposed discovered),
              do: "#{kind}_#{family}_count"
  @optional_numbers ~w(pid pgid age_seconds)
  @optional_strings ~w(transport server_version protocol_version)

  def fetch(session_id, name) when is_binary(name) and byte_size(name) in 1..256 do
    with {:ok, rows} <-
           Auth.read_action(session_id, "gateway", "gateway.mcp.list", %{"name" => name}) do
      project(rows, name)
    end
  end

  def fetch(_, _), do: {:error, :unavailable}

  defp project([], _), do: {:ok, nil}

  defp project([row], name) when is_map(row) do
    valid =
      row["name"] == name &&
        Enum.all?(~w(enabled connected), &is_boolean(Map.get(row, &1, false))) &&
        Enum.all?(
          @counts ++ ["likely_stale_count"],
          &(is_integer(Map.get(row, &1, 0)) && Map.get(row, &1, 0) >= 0)
        ) &&
        Enum.all?(@optional_numbers, &(is_nil(row[&1]) || (is_integer(row[&1]) && row[&1] >= 0))) &&
        Enum.all?(
          @optional_strings,
          &(is_nil(row[&1]) || (is_binary(row[&1]) && byte_size(row[&1]) <= 256))
        )

    if valid do
      defaults =
        Map.new(@counts ++ ["likely_stale_count"], &{&1, 0})
        |> Map.merge(%{"enabled" => false, "connected" => false})

      {:ok,
       Map.merge(
         defaults,
         Map.take(
           row,
           ~w(name enabled connected likely_stale_count) ++
             @counts ++ @optional_numbers ++ @optional_strings
         )
       )}
    else
      {:error, :unavailable}
    end
  end

  defp project(_, _), do: {:error, :unavailable}
end
