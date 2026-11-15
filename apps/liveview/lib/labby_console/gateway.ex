defmodule LabbyConsole.Gateway do
  @moduledoc "Bounded public readiness probe against one explicitly configured gateway."
  def readiness(url), do: request(url, "/ready")
  def liveness(url), do: request(url, "/health")

  defp request(url, path) do
    case target(url) do
      {:ok, base} -> probe(base, path)
      _ -> {:error, :invalid_target}
    end
  end

  defp target(url) when is_binary(url) do
    uri = URI.parse(url)

    if uri.scheme in ["http", "https"] and is_binary(uri.host) and uri.host != "" and
         is_nil(uri.userinfo) and is_nil(uri.query) and is_nil(uri.fragment) and
         uri.path in [nil, "", "/"], do: {:ok, String.trim_trailing(url, "/")}, else: :error
  end

  defp target(_), do: :error

  defp probe(url, path) do
    case Req.get(url <> path,
           retry: false,
           redirect: false,
           receive_timeout: 2_000,
           connect_options: [timeout: 2_000],
           decode_body: false,
           into: &collect/2
         ) do
      {:ok, %{status: status, body: body}} when status in [200, 503] -> decode(path, status, body)
      _ -> {:error, :unavailable}
    end
  end

  defp collect({:data, data}, {request, response}) do
    body = (response.body || "") <> data

    if byte_size(body) <= 65_536,
      do: {:cont, {request, %{response | body: body}}},
      else: {:halt, {request, %{response | status: 413, body: ""}}}
  end

  defp decode("/health", 200, body) do
    case Jason.decode(body) do
      {:ok, %{"status" => "ok"}} -> {:ok, %{status: "ok"}}
      _ -> {:error, :unavailable}
    end
  end

  defp decode("/health", _, _), do: {:error, :unavailable}

  defp decode("/ready", status, body) do
    case Jason.decode(body) do
      {:ok, %{"status" => "ready"}} when status == 200 ->
        {:ok, %{status: "ready"}}

      {:ok, %{"status" => value} = payload}
      when (value == "not_ready" and status == 503) or (value == "degraded" and status == 200) ->
        details = Map.get(payload, if(value == "not_ready", do: "pending", else: "degraded"), [])

        if is_list(details) and Enum.all?(details, &is_binary/1),
          do: {:ok, %{status: value, details: details}},
          else: {:error, :unavailable}

      _ ->
        {:error, :unavailable}
    end
  end
end
