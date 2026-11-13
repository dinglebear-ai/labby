defmodule LabbyConsole.Diagnostics do
  @moduledoc "Read-only doctor system findings for the current gateway caller."
  alias LabbyConsole.Auth

  def checks(session_id) do
    with {:ok, %{"findings" => findings}} when is_list(findings) <-
           Auth.read_action(session_id, "doctor", "system.checks", %{}),
         true <- length(findings) <= 500 && Enum.all?(findings, &valid?/1) do
      {:ok, Enum.map(findings, &Map.take(&1, ~w(service check severity message)))}
    else
      {:error, :unauthenticated} -> {:error, :unauthenticated}
      _ -> {:error, :unavailable}
    end
  end

  defp valid?(finding) when is_map(finding) do
    finding["severity"] in ~w(ok warn fail) &&
      Enum.all?(~w(service check message), fn key ->
        value = finding[key]
        is_binary(value) && String.valid?(value) && byte_size(value) in 1..4096
      end)
  end

  defp valid?(_), do: false
end
