defmodule LabbyConsole.OverviewTest do
  use ExUnit.Case, async: true
  alias LabbyConsole.Overview

  test "layout restoration bounds untrusted preferences and preserves all native cards" do
    defaults = Overview.default_layout()

    restored =
      Overview.normalize(%{
        "order" => ["calls", "calls", "private"],
        "widths" => %{"traffic" => true, "inventory" => true, "calls" => "yes"},
        "credential" => "secret"
      })

    assert hd(restored.order) == "calls"
    assert Enum.sort(restored.order) == Enum.sort(defaults.order)
    assert restored.widths == %{"traffic" => true}
    refute Map.has_key?(restored, :credential)
    assert Overview.normalize(String.duplicate("x", 10000)) == defaults
  end

  test "reordering remains within lanes and fixed inventory cannot move or widen" do
    layout = Overview.default_layout()
    assert Overview.place(layout, "traffic", "runtime", false) == layout
    assert Overview.move(layout, "inventory", "later") == layout
    assert Overview.widen(layout, "inventory") == layout
    moved = Overview.place(layout, "calls", "traffic", false)
    assert hd(moved.order) == "calls"
    assert Overview.move(moved, "calls", "later").order != moved.order
    assert Overview.widen(layout, "traffic").widths["traffic"]
  end

  test "unknown telemetry differs from known empty telemetry and zero-call latency" do
    assert Overview.success_rate(nil) == nil
    assert Overview.success_rate(%{"total_calls" => 0, "error_calls" => 0}) == nil
    assert Overview.success_rate(%{"total_calls" => 10, "error_calls" => 1}) == 90.0
    assert Overview.rankings(%{}, "tools", "top") == nil
    assert Overview.rankings(%{"top_tools" => []}, "tools", "top") == []
  end

  test "capability observations keep unknown counts separate from measured zero and stale snapshots" do
    row = %{
      "id" => "x",
      "name" => "x",
      "enabled" => true,
      "connected" => false,
      "capability_observation" => %{
        "tools" => %{"state" => "stale", "discovered" => 4, "exposed" => 2, "error" => "secret"}
      }
    }

    assert {:ok, [projected]} = Overview.project_inventory([row])
    refute inspect(projected) =~ "secret"
    totals = Overview.capability_total([projected], "tools")
    assert totals.exposed == 2
    assert totals.stale == 1
    assert Overview.capability_total([%{}], "tools").exposed == nil
    assert Overview.capability_total([], "tools").exposed == 0
  end
end
