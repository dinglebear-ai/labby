defmodule LabbyConsole.GatewayFleetTest do
  use ExUnit.Case, async: true
  alias LabbyConsole.GatewayFleet, as: Fleet

  def row(overrides \\ %{}) do
    Map.merge(
      %{
        "id" => "custom:a",
        "name" => "same",
        "source" => "custom_gateway",
        "enabled" => false,
        "connected" => true
      },
      overrides
    )
  end

  test "wire projection separates source, enabled, connected and observation facts" do
    observation = %{
      "scope" => "credential",
      "tools" => %{"state" => "known", "discovered" => 0, "exposed" => 0, "error" => "secret"},
      "resources" => %{"state" => "stale", "discovered" => 2, "exposed" => 1},
      "prompts" => %{"state" => "failed", "error" => "secret"}
    }

    assert {:ok, [a, b]} =
             Fleet.project([
               row(%{
                 "capability_observation" => observation,
                 "config" => %{"token" => "secret"}
               }),
               row(%{"id" => "virtual:b", "source" => "in_process"})
             ])

    assert a["enabled"] == false
    assert a["connected"] == true
    assert a["source"] == "custom_gateway"
    assert b["source"] == "in_process"
    assert Fleet.measurement(a, "tools") == "0 / 0"
    assert Fleet.measurement(a, "resources") == "1 / 2"
    assert Fleet.measurement(b, "tools") == "Unknown"
    assert a["observations"]["prompts"]["state"] == "failed"
    refute inspect(a) =~ "secret"
  end

  test "legacy zeros and missing source never fabricate observations or custom identity" do
    assert {:ok, [a]} = Fleet.project([row(%{"source" => nil, "discovered_tool_count" => 0})])
    assert a["source"] == nil
    assert Fleet.measurement(a, "tools") == "Unknown"
  end

  test "malformed rows and duplicate identities fail closed" do
    for rows <- [
          [row(%{"id" => 3})],
          [row(), row()],
          [row(%{"connected" => "true"})],
          [%{"config" => "secret"}],
          %{}
        ] do
      assert {:error, :unavailable} = Fleet.project(rows)
    end
  end

  test "stable sorting and independent lenses operate only on loaded facts" do
    {:ok, rows} =
      Fleet.project([
        row(%{"id" => "b"}),
        row(%{"id" => "a", "source" => "in_process", "enabled" => true, "connected" => false})
      ])

    prefs = Fleet.preferences(%{})
    assert Enum.map(Fleet.visible(rows, prefs), & &1["id"]) == ["a", "b"]
    assert Enum.map(Fleet.visible(rows, %{prefs | "lens" => "connected"}), & &1["id"]) == ["b"]
    assert Enum.map(Fleet.visible(rows, %{prefs | "lens" => "enabled"}), & &1["id"]) == ["a"]

    assert Enum.map(Fleet.visible(rows, %{prefs | "source" => "custom_gateway"}), & &1["id"]) == [
             "b"
           ]

    assert Fleet.visible(rows, %{prefs | "query" => "missing"}) == []
  end

  test "preferences admit bounded presentation fields and safe column moves" do
    prefs =
      Fleet.preferences(%{
        "layout" => "cards",
        "query" => String.duplicate("x", 300),
        "columns" => ["tools", "tools", "secret"],
        "sort" => "config",
        "direction" => "descending",
        "records" => [row()]
      })

    assert prefs["layout"] == "cards"
    assert String.length(prefs["query"]) <= 256
    assert prefs["columns"] == ["tools"]
    assert prefs["sort"] == "name"
    refute Map.has_key?(prefs, "records")
    assert Fleet.move_column(prefs, "tools", "earlier") == prefs
    assert Fleet.preferences(%{"columns" => []})["columns"] == Fleet.columns()
  end
end
