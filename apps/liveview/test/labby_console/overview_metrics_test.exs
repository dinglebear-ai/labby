defmodule LabbyConsole.OverviewMetricsTest do
  use ExUnit.Case, async: true
  alias LabbyConsole.UsageMetrics

  defp sample do
    %{
      "window_total_calls" => 2,
      "total_calls" => 2,
      "error_calls" => 1,
      "avg_elapsed_ms" => 2.5,
      "p50_elapsed_ms" => 2,
      "p95_elapsed_ms" => 3,
      "p99_elapsed_ms" => 3,
      "distinct_tools" => 1,
      "distinct_actors" => 1,
      "peak_per_min" => 2,
      "timeseries" => [
        %{
          "ts_unix" => 100,
          "calls" => 2,
          "failed" => 1,
          "outcomes" => [%{"kind" => "timeout", "calls" => 1}]
        }
      ],
      "top_tools" => [
        %{
          "tool" => "read",
          "upstream" => "files",
          "capability" => "tool",
          "operation" => "call",
          "subject_scoped" => true,
          "calls" => 2,
          "failed" => 1,
          "secret" => "private"
        }
      ],
      "least_tools" => [],
      "slowest_tools" => [],
      "top_actors" => [%{"actor" => "subject", "calls" => 2}],
      "upstreams" => [%{"upstream" => "files", "calls" => 2, "failed" => 1}],
      "errors" => [%{"kind" => "timeout", "calls" => 1}],
      "private" => "not public"
    }
  end

  test "dashboard projection retains typed rankings and outcomes without private extras" do
    assert {:ok, data} = UsageMetrics.project(sample(), 0, 200)
    assert hd(data["top_tools"])["calls"] == 2
    assert hd(hd(data["timeseries"])["outcomes"])["kind"] == "timeout"
    refute inspect(data) =~ "private"
    refute Map.has_key?(data, "private")
  end

  test "invalid ranked telemetry is rejected and missing ranking is unknown" do
    row = sample()

    assert {:error, :unavailable} =
             UsageMetrics.project(Map.put(row, "top_tools", [%{"calls" => -1}]), 0, 200)

    assert {:error, :unavailable} =
             UsageMetrics.project(
               Map.put(row, "top_actors", List.duplicate(hd(row["top_actors"]), 101)),
               0,
               200
             )

    assert {:ok, absent} =
             UsageMetrics.project(
               Map.drop(row, ~w(top_tools least_tools slowest_tools top_actors errors upstreams)),
               0,
               200
             )

    refute Map.has_key?(absent, "top_tools")
    assert UsageMetrics.window("30d") == 2_592_000
  end

  test "per-server buckets cannot overstate the shared total series" do
    row = sample()

    invalid =
      Map.put(row, "upstream_timeseries", %{
        "files" => [%{"ts_unix" => 100, "calls" => 3, "failed" => 1, "outcomes" => []}]
      })

    assert {:error, :unavailable} = UsageMetrics.project(invalid, 0, 200)
    duplicate = Map.put(row, "timeseries", row["timeseries"] ++ row["timeseries"])
    assert {:error, :unavailable} = UsageMetrics.project(duplicate, 0, 200)
  end

  test "caller attribution is typed display metadata, not raw provider data" do
    row = sample()

    actor = %{
      "actor" => "subject",
      "calls" => 2,
      "attribution" => %{
        "client_name" => "Real client",
        "surface" => "mcp",
        "authorization" => "private-token"
      }
    }

    assert {:ok, projected} = UsageMetrics.project(Map.put(row, "top_actors", [actor]), 0, 200)
    assert hd(projected["top_actors"])["attribution"]["client_name"] == "Real client"
    refute inspect(projected) =~ "private-token"
  end
end
