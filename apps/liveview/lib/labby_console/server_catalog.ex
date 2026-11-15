defmodule LabbyConsole.ServerCatalog do
  @moduledoc "Bounded, read-only retained capability snapshots for a single upstream."
  alias LabbyConsole.Auth

  def load(session_id, name) when is_binary(name) and byte_size(name) in 1..256 do
    with {:ok, tools} <- read(session_id, "tools", name),
         {:ok, resources} <- read(session_id, "resources", name),
         {:ok, prompts} <- read(session_id, "prompts", name),
         true <- valid_tools?(tools) && valid_names?(resources) && valid_names?(prompts) do
      {:ok,
       %{
         tools: Enum.map(tools, &Map.take(&1, ~w(name description exposed))),
         resources: resources,
         prompts: prompts
       }}
    else
      {:error, :unauthenticated} -> {:error, :unauthenticated}
      _ -> {:error, :unavailable}
    end
  end

  def load(_, _), do: {:error, :unavailable}

  defp read(id, family, name),
    do: Auth.read_action(id, "gateway", "gateway.discovered_" <> family, %{name: name})

  defp valid_tools?(rows) when is_list(rows) and length(rows) <= 1000 do
    Enum.all?(rows, fn row ->
      is_map(row) && valid_text?(row["name"]) && is_boolean(row["exposed"]) &&
        (is_nil(row["description"]) ||
           (is_binary(row["description"]) && byte_size(row["description"]) <= 4096))
    end)
  end

  defp valid_tools?(_), do: false

  defp valid_names?(rows) when is_list(rows) and length(rows) <= 1000,
    do: Enum.all?(rows, &valid_text?/1)

  defp valid_names?(_), do: false
  defp valid_text?(value), do: is_binary(value) && byte_size(value) in 1..4096
end
