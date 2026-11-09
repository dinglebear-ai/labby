defmodule LabbyConsole.UsageCalls do
  @moduledoc "Bounded retained call pages; cursors remain opaque and totals are not inferred."
  alias LabbyConsole.Auth

  @fields ~w(id ts_unix upstream tool capability operation subject_scoped actor outcome elapsed_ms response_bytes attribution)
  @strings ~w(upstream tool capability operation actor outcome)

  def list(session_id, params) when is_map(params) do
    if valid_params?(params) do
      with {:ok, %{"calls" => calls} = page} when is_list(calls) and length(calls) <= 25 <-
             Auth.read_action(
               session_id,
               "gateway",
               "gateway.usage.calls",
               Map.put(params, :limit, 25)
             ),
           true <- Enum.all?(calls, &valid_row?/1),
           true <- cursor?(page["next_cursor"]) do
        {:ok, %{calls: Enum.map(calls, &project/1), next_cursor: page["next_cursor"]}}
      else
        {:error, reason} when reason in [:unauthenticated, :forbidden] -> {:error, reason}
        _ -> {:error, :unavailable}
      end
    else
      {:error, :invalid_filters}
    end
  end

  def list(_, _), do: {:error, :invalid_filters}

  defp valid_params?(p) do
    Enum.all?(
      Map.keys(p),
      &(&1 in [:since_unix, :until_unix, :outcome, :cursor, :upstream, :search])
    ) &&
      is_integer(p[:since_unix]) && is_integer(p[:until_unix]) &&
      p[:since_unix] >= 0 && p[:until_unix] >= p[:since_unix] &&
      p[:until_unix] - p[:since_unix] <= 604_800 &&
      p[:outcome] in [nil, "ok", "timeout", "upstream_error", "tool_error"] && cursor?(p[:cursor]) &&
      Enum.all?(
        [:upstream, :search],
        &(is_nil(p[&1]) || (is_binary(p[&1]) && String.valid?(p[&1]) && byte_size(p[&1]) <= 256))
      )
  end

  defp cursor?(nil), do: true
  defp cursor?(v), do: is_binary(v) && String.valid?(v) && byte_size(v) in 1..1024

  defp project(row) do
    result = Map.take(row, @fields)

    case result["attribution"] do
      metadata when is_map(metadata) ->
        Map.put(
          result,
          "attribution",
          Map.take(metadata, ~w(actor_kind surface client_name client_version))
        )

      _ ->
        Map.delete(result, "attribution")
    end
  end

  defp valid_metadata?(row) do
    bytes = row["response_bytes"]
    metadata = row["attribution"]

    (is_nil(bytes) || (is_integer(bytes) && bytes >= 0 && bytes <= 9_223_372_036_854_775_807)) &&
      (is_nil(metadata) ||
         (is_map(metadata) &&
            Enum.all?(~w(actor_kind surface client_name client_version), fn key ->
              value = metadata[key]

              is_nil(value) ||
                (is_binary(value) && String.valid?(value) && byte_size(value) <= 512)
            end)))
  end

  defp valid_row?(r) when is_map(r) do
    valid_metadata?(r) &&
      Enum.all?(@strings, &(is_binary(r[&1]) && String.valid?(r[&1]) && byte_size(r[&1]) <= 4096)) &&
      Enum.all?(
        ~w(id ts_unix elapsed_ms),
        &(is_integer(r[&1]) && r[&1] >= 0 && r[&1] <= 9_223_372_036_854_775_807)
      ) &&
      r["ts_unix"] <= 253_402_300_799 && is_boolean(r["subject_scoped"])
  end

  defp valid_row?(_), do: false
end
