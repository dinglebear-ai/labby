defmodule LabbyConsole.Overview do
  @moduledoc "Overview presentation preferences and honest derived telemetry values."

  @cards [
    %{id: "traffic", label: "Gateway traffic", lane: "telemetry", span: 4},
    %{id: "attention", label: "Needs attention", lane: "insights", span: 2},
    %{id: "inventory", label: "Capability inventory", lane: "fixed", span: 6},
    %{id: "rankings", label: "Tool activity", lane: "telemetry", span: 2},
    %{id: "latency", label: "Latency & failures", lane: "telemetry", span: 2},
    %{id: "callers", label: "Who's calling", lane: "insights", span: 2},
    %{id: "runtime", label: "Gateway host", lane: "insights", span: 3},
    %{id: "clients", label: "Observed clients", lane: "insights", span: 3},
    %{id: "upstreams", label: "Most active servers", lane: "insights", span: 3},
    %{id: "fleet", label: "Recent servers", lane: "insights", span: 3},
    %{id: "calls", label: "Recent calls", lane: "telemetry", span: 6}
  ]

  def cards, do: @cards
  def default_layout, do: %{order: Enum.map(@cards, & &1.id), widths: %{}}

  def normalize(%{"order" => order} = params) when is_list(order) and length(order) <= 64 do
    ids = default_layout().order
    widths = params["widths"]
    widths = if is_map(widths) and map_size(widths) <= 32, do: widths, else: %{}

    %{
      order: Enum.uniq(Enum.filter(order, &(&1 in ids)) ++ ids),
      widths:
        Map.new(
          Enum.filter(widths, fn {id, wide} -> is_boolean(wide) and lane(id) == "telemetry" end)
        )
    }
  end

  def normalize(_), do: default_layout()
  def lane(id), do: Enum.find_value(@cards, fn card -> if card.id == id, do: card.lane end)

  def place(layout, source, target, after?) do
    if source != target and source in layout.order and target in layout.order and
         lane(source) == lane(target) and lane(source) != "fixed" do
      remaining = List.delete(layout.order, source)
      position = Enum.find_index(remaining, &(&1 == target)) + if(after?, do: 1, else: 0)
      %{layout | order: List.insert_at(remaining, position, source)}
    else
      layout
    end
  end

  def move(layout, source, direction) when direction in ~w(earlier later) do
    peers = Enum.filter(layout.order, &(lane(&1) == lane(source)))
    index = Enum.find_index(peers, &(&1 == source))
    next = if index, do: index + if(direction == "later", do: 1, else: -1), else: -1

    if lane(source) != "fixed" and next >= 0 and next < length(peers),
      do: place(layout, source, Enum.at(peers, next), direction == "later"),
      else: layout
  end

  def move(layout, _, _), do: layout

  def widen(layout, id) do
    if lane(id) == "telemetry",
      do: %{layout | widths: Map.put(layout.widths, id, !Map.get(layout.widths, id, false))},
      else: layout
  end

  def success_rate(%{"total_calls" => total, "error_calls" => failed}) when total > 0,
    do: (total - failed) / total * 100

  def success_rate(_), do: nil

  def rankings(nil, _, _), do: nil
  def rankings(metrics, "tools", "least"), do: metrics["least_tools"]
  def rankings(metrics, "tools", "slowest"), do: metrics["slowest_tools"]
  def rankings(metrics, "tools", _), do: metrics["top_tools"]
  def rankings(metrics, "servers", _), do: metrics["upstreams"]
  def rankings(metrics, "callers", _), do: metrics["top_actors"]

  @call_fields ~w(id ts_unix upstream tool capability operation subject_scoped actor outcome elapsed_ms response_bytes attribution)
  @call_strings ~w(upstream tool capability operation actor outcome)
  @filters ~w(upstream tool capability operation actor outcome subject_scoped)

  def details(session, filters, first, last) do
    if valid_query?(filters, first, last) do
      params = Map.merge(filters, %{"since_unix" => first, "until_unix" => last})

      with {:ok, raw} <-
             LabbyConsole.Auth.read_action(
               session,
               "gateway",
               "gateway.usage.metrics",
               Map.merge(params, %{
                 "bucket_count" => 24,
                 "timezone" => "UTC",
                 "include_facets" => false
               })
             ),
           {:ok, metrics} <- LabbyConsole.UsageMetrics.project(raw, first, last),
           {:ok, page} <- calls(session, filters, first, last) do
        {:ok, Map.put(page, :metrics, metrics)}
      end
    else
      {:error, :invalid_filters}
    end
  end

  def calls(session, filters, first, last) do
    if valid_query?(filters, first, last) do
      params = Map.merge(filters, %{"since_unix" => first, "until_unix" => last, "limit" => 25})

      with {:ok, %{"calls" => calls} = page} when is_list(calls) and length(calls) <= 25 <-
             LabbyConsole.Auth.read_action(session, "gateway", "gateway.usage.calls", params),
           true <- Enum.all?(calls, &valid_call?(&1, first, last)),
           true <- is_nil(page["next_cursor"]) || text?(page["next_cursor"], 1024) do
        {:ok, %{calls: Enum.map(calls, &project_call/1), next_cursor: page["next_cursor"]}}
      else
        {:error, reason} when reason in [:unauthenticated, :forbidden] -> {:error, reason}
        _ -> {:error, :unavailable}
      end
    else
      {:error, :invalid_filters}
    end
  end

  defp valid_query?(filters, first, last) do
    is_map(filters) && map_size(filters) <= 7 &&
      Enum.all?(filters, fn {key, value} ->
        key in @filters &&
          if(key == "subject_scoped", do: is_boolean(value), else: text?(value, 4096))
      end) && is_integer(first) && is_integer(last) && first >= 0 && last >= first &&
      last <= 253_402_300_799 && last - first <= 2_592_000
  end

  defp valid_call?(row, first, last) when is_map(row) do
    Enum.all?(@call_strings, &text?(row[&1], 4096)) &&
      Enum.all?(
        ~w(id ts_unix elapsed_ms),
        &(is_integer(row[&1]) && row[&1] >= 0 && row[&1] <= 9_223_372_036_854_775_807)
      ) &&
      row["ts_unix"] >= first && row["ts_unix"] <= last && is_boolean(row["subject_scoped"]) &&
      (is_nil(row["response_bytes"]) ||
         (is_integer(row["response_bytes"]) && row["response_bytes"] >= 0)) &&
      (is_nil(row["attribution"]) || is_map(row["attribution"]))
  end

  defp valid_call?(_, _, _), do: false

  defp project_call(row) do
    projected = Map.take(row, @call_fields)

    if is_map(row["attribution"]) do
      fields = ~w(client_name client_version surface agent_id task_id)

      attribution =
        Map.new(
          Enum.filter(Map.take(row["attribution"], fields), fn {_, value} ->
            text?(value, 4096)
          end)
        )

      Map.put(projected, "attribution", attribution)
    else
      projected
    end
  end

  defp text?(value, max), do: is_binary(value) && String.valid?(value) && byte_size(value) <= max

  def inventory(session) do
    with {:ok, rows} <- LabbyConsole.Auth.read_action(session, "gateway", "gateway.list", %{}),
         do: project_inventory(rows)
  end

  def project_inventory(rows) when is_list(rows) and length(rows) <= 1000 do
    if Enum.all?(rows, fn row ->
         is_map(row) && text?(row["id"], 4096) && text?(row["name"], 4096) &&
           is_boolean(row["enabled"]) && is_boolean(row["connected"]) &&
           (is_nil(row["display_name"]) || text?(row["display_name"], 4096)) &&
           valid_observations?(row["capability_observation"])
       end) do
      {:ok,
       Enum.map(rows, fn row ->
         value = Map.take(row, ~w(id name display_name enabled connected))

         if is_map(row["capability_observation"]),
           do:
             Map.put(
               value,
               "capability_observation",
               Map.new(
                 Enum.map(~w(tools resources prompts skills), fn family ->
                   {family,
                    Map.take(
                      row["capability_observation"][family] || %{},
                      ~w(state discovered exposed)
                    )}
                 end)
               )
             ),
           else: value
       end)}
    else
      {:error, :unavailable}
    end
  end

  def project_inventory(_), do: {:error, :unavailable}

  def capability_total(rows, family) do
    observations =
      Enum.map(rows, fn row -> get_in(row, ["capability_observation", family]) || %{} end)

    measured =
      Enum.filter(observations, fn row ->
        row["state"] in ~w(known stale failed) && is_integer(row["exposed"]) &&
          is_integer(row["discovered"])
      end)

    known = length(measured)

    %{
      exposed:
        if(known > 0 || rows == [], do: Enum.sum(Enum.map(measured, & &1["exposed"])), else: nil),
      discovered:
        if(known > 0 || rows == [],
          do: Enum.sum(Enum.map(measured, & &1["discovered"])),
          else: nil
        ),
      unknown: length(rows) - known,
      stale: Enum.count(observations, &(&1["state"] == "stale")),
      failed: Enum.count(observations, &(&1["state"] == "failed"))
    }
  end

  defp valid_observations?(nil), do: true

  defp valid_observations?(value) when is_map(value) do
    Enum.all?(~w(tools resources prompts skills), fn family ->
      case value[family] do
        nil ->
          true

        row when is_map(row) ->
          row["state"] in ~w(known unknown stale failed) &&
            Enum.all?(~w(exposed discovered), fn key ->
              is_nil(row[key]) ||
                (is_integer(row[key]) && row[key] >= 0 && row[key] <= 18_446_744_073_709_551_615)
            end) &&
            (row["state"] != "known" ||
               (is_integer(row["exposed"]) && is_integer(row["discovered"])))

        _ ->
          false
      end
    end)
  end

  defp valid_observations?(_), do: false
end
