defmodule LabbyConsole.GatewayFleet do
  @moduledoc "Bounded caller-visible fleet projection and presentation preferences."
  @families ~w(tools resources prompts skills)
  @columns ~w(source status tools resources prompts skills)
  @defaults %{
    "query" => "",
    "lens" => "all",
    "source" => "all",
    "layout" => "table",
    "sort" => "name",
    "direction" => "ascending",
    "columns" => @columns
  }

  def columns, do: @columns

  def fetch(session) do
    with {:ok, rows} <- LabbyConsole.Auth.read_action(session, "gateway", "gateway.list", %{}),
         do: project(rows)
  end

  def project(rows) when is_list(rows) and length(rows) <= 1000 do
    if Enum.all?(rows, &valid_row?/1) && Enum.uniq_by(rows, & &1["id"]) == rows do
      {:ok, Enum.map(rows, &project_row/1)}
    else
      {:error, :unavailable}
    end
  end

  def project(_), do: {:error, :unavailable}

  defp valid_row?(row) when is_map(row) do
    Enum.all?(~w(id name), &text?(row[&1], 1024)) && row["id"] != "" && row["name"] != "" &&
      Enum.all?(~w(enabled connected), &is_boolean(row[&1])) &&
      (is_nil(row["display_name"]) || text?(row["display_name"], 1024)) &&
      (is_nil(row["source"]) || text?(row["source"], 128))
  end

  defp valid_row?(_), do: false
  defp text?(value, max), do: is_binary(value) && byte_size(value) <= max && String.valid?(value)

  defp project_row(row) do
    observation = row["capability_observation"]
    observation = if is_map(observation), do: observation, else: %{}

    row
    |> Map.take(~w(id name display_name source enabled connected))
    |> Map.put_new("source", nil)
    |> Map.put("observations", Map.new(@families, &{&1, family(observation[&1])}))
  end

  defp family(%{"state" => state} = raw) when state in ~w(known stale failed) do
    measured = Enum.all?(~w(discovered exposed), &(is_integer(raw[&1]) && raw[&1] >= 0))

    %{
      "state" => state,
      "discovered" => if(measured, do: raw["discovered"]),
      "exposed" => if(measured, do: raw["exposed"])
    }
  end

  defp family(_), do: %{"state" => "unknown", "discovered" => nil, "exposed" => nil}

  def measurement(row, family) do
    case row["observations"][family] do
      %{"exposed" => exposed, "discovered" => discovered}
      when is_integer(exposed) and is_integer(discovered) ->
        "#{exposed} / #{discovered}"

      _ ->
        "Unknown"
    end
  end

  def attention?(row),
    do:
      (row["enabled"] && !row["connected"]) ||
        Enum.any?(row["observations"], fn {_, value} -> value["state"] in ~w(stale failed) end)

  def label(row), do: row["display_name"] || row["name"]
  def source_label("in_process"), do: "Lab"
  def source_label("custom_gateway"), do: "Custom"
  def source_label(_), do: "Unknown"

  def preferences(params) when is_map(params) do
    columns = params["columns"]

    columns =
      if is_list(columns) && length(columns) <= 12,
        do: Enum.uniq(Enum.filter(columns, &(&1 in @columns))),
        else: @columns

    query = if text?(params["query"], 4096), do: String.slice(params["query"], 0, 256), else: ""

    @defaults
    |> Map.put("query", query)
    |> Map.put("columns", if(columns == [], do: @columns, else: columns))
    |> choice(params, "lens", ~w(all enabled disabled connected disconnected attention))
    |> choice(params, "source", ~w(all in_process custom_gateway unknown))
    |> choice(params, "layout", ~w(table cards list))
    |> choice(params, "sort", ~w(name source enabled connected tools resources prompts skills))
    |> choice(params, "direction", ~w(ascending descending))
  end

  def preferences(_), do: @defaults

  defp choice(prefs, params, key, allowed),
    do: if(params[key] in allowed, do: Map.put(prefs, key, params[key]), else: prefs)

  def move_column(prefs, column, direction) when direction in ~w(earlier later) do
    columns = prefs["columns"]
    index = Enum.find_index(columns, &(&1 == column))
    target = if index, do: index + if(direction == "earlier", do: -1, else: 1), else: -1

    if target >= 0 && target < length(columns),
      do:
        Map.put(
          prefs,
          "columns",
          columns |> List.delete(column) |> List.insert_at(target, column)
        ),
      else: prefs
  end

  def move_column(prefs, _, _), do: prefs

  def visible(rows, prefs) do
    query = prefs["query"] |> String.trim() |> String.downcase()

    filtered =
      Enum.filter(rows, fn row ->
        String.contains?(String.downcase(label(row) <> " " <> row["name"]), query) &&
          lens?(row, prefs["lens"]) && source?(row, prefs["source"])
      end)

    Enum.sort_by(
      filtered,
      &sort_value(&1, prefs["sort"]),
      if(prefs["direction"] == "descending", do: :desc, else: :asc)
    )
  end

  defp lens?(_, "all"), do: true
  defp lens?(row, "enabled"), do: row["enabled"]
  defp lens?(row, "disabled"), do: !row["enabled"]
  defp lens?(row, "connected"), do: row["connected"]
  defp lens?(row, "disconnected"), do: !row["connected"]
  defp lens?(row, "attention"), do: attention?(row)
  defp source?(_, "all"), do: true
  defp source?(row, "unknown"), do: row["source"] not in ~w(in_process custom_gateway)
  defp source?(row, source), do: row["source"] == source
  defp sort_value(row, "name"), do: {String.downcase(label(row)), row["id"]}

  defp sort_value(row, family) when family in @families,
    do: {row["observations"][family]["exposed"], row["id"]}

  defp sort_value(row, field), do: {row[field], row["id"]}
end
