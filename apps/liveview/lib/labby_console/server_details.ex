defmodule LabbyConsole.ServerDetails do
  @moduledoc "Read-only, credential-free projection of a caller-visible Gateway ServerView."
  alias LabbyConsole.Auth

  def fetch(session_id, id) do
    with {:ok, _} <- Auth.current(session_id) do
      fetch_visible(session_id, id)
    end
  end

  defp fetch_visible(session_id, id) do
    with {:ok, row} <-
           Auth.read_action(session_id, "gateway", "gateway.server.get", %{"id" => id}) do
      project(row, id)
    end
  end

  @counts for family <- ~w(tool resource prompt skill),
              kind <- ~w(exposed discovered),
              do: "#{kind}_#{family}_count"
  @identity ~w(id name display_name source configured enabled connected)

  defp project(row, id) when is_map(row) do
    valid =
      row["id"] == id && is_binary(row["name"]) && is_binary(row["source"]) &&
        (is_nil(row["display_name"]) || is_binary(row["display_name"])) &&
        Enum.all?(~w(configured enabled connected), &is_boolean(Map.get(row, &1, false))) &&
        Enum.all?(@counts, fn key ->
          is_integer(Map.get(row, key, 0)) && Map.get(row, key, 0) >= 0
        end)

    if valid do
      defaults =
        Map.new(@counts, &{&1, 0})
        |> Map.merge(%{"configured" => false, "enabled" => false, "connected" => false})

      {:ok, Map.merge(defaults, Map.take(row, @identity ++ @counts))}
    else
      {:error, :unavailable}
    end
  end

  defp project(_, _), do: {:error, :unavailable}
end
