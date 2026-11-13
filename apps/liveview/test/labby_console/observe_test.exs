defmodule LabbyConsole.ObserveTest do
  use ExUnit.Case, async: true
  alias LabbyConsole.Observe

  test "preferences allow bounded UI fields only" do
    assert Observe.preferences(%{
             "tab" => "logs",
             "window" => "7d",
             "cursor" => "secret",
             "query" => "secret"
           }) == %{"tab" => "logs", "window" => "7d"}

    assert Observe.preferences(nil) == %{"tab" => "activity", "window" => "24h"}
  end

  test "UTC windows include 23:59 and stay within seven days" do
    now = DateTime.to_unix(~U[2026-11-09 23:59:59Z])
    assert Observe.bounds("24h", now) == {now - 86_400, now}
    assert Observe.bounds("7d", now) == {now - 604_800, now}
    assert Observe.bounds("1h", 12) == {0, 12}
  end

  test "server predicates use supported exact upstream and bounded search" do
    assert Observe.call_params(
             %{"upstream" => "a/b", "query" => "fetch", "outcome" => "ok"},
             "1h",
             4000,
             nil
           ) == %{
             since_unix: 400,
             until_unix: 4000,
             upstream: "a/b",
             search: "fetch",
             outcome: "ok"
           }

    assert {:error, :invalid_filters} =
             Observe.call_params(%{"query" => String.duplicate("x", 257)}, "1h", 4000, nil)

    assert {:error, :invalid_filters} =
             Observe.call_params(%{"outcome" => "fake"}, "1h", 4000, nil)
  end

  test "pagination deduplicates IDs while retaining simultaneous records and bounds state" do
    assert Observe.merge_calls([%{"id" => 2, "ts_unix" => 10}], [
             %{"id" => 2, "ts_unix" => 10},
             %{"id" => 1, "ts_unix" => 10}
           ]) == [%{"id" => 2, "ts_unix" => 10}, %{"id" => 1, "ts_unix" => 10}]

    assert length(Observe.merge_calls([], for(n <- 1..300, do: %{"id" => n}))) == 200
  end

  test "selection stays only within authorized retained rows" do
    assert Observe.selection([%{"id" => 42}], "42") == "42"
    assert Observe.selection([%{"id" => 42}], "41") == nil
    assert Observe.selection([], "42") == nil
  end

  test "missing attribution stays unknown without exposing actor tags" do
    assert Observe.attribution(%{"actor" => "subject:secret"}) == "Unknown caller"

    assert Observe.attribution(%{"attribution" => %{"client_name" => "Real client"}}) ==
             "Real client"
  end

  test "exports project authorized loaded snapshot and enforce bytes" do
    assert {:ok, data} =
             Observe.export("activity", [%{"id" => 1, "tool" => "read", "payload" => "secret"}])

    assert Jason.decode!(data)["scope"] == "authorized_loaded_snapshot"
    refute data =~ "secret"

    assert {:error, :export_too_large} =
             Observe.export("logs", [%{"message" => String.duplicate("x", 70_000)}])
  end

  test "related runtime link encodes source and excludes empty source" do
    assert Observe.server_link(%{"upstream" => "a/b ?"}) ==
             "/gateway/upstreams/a%2Fb%20%3F/runtime"

    assert Observe.server_link(%{}) == nil
    assert Observe.server_link(%{"upstream" => ""}) == nil
  end
end
