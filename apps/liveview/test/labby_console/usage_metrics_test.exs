defmodule LabbyConsole.UsageMetricsTest do
  use ExUnit.Case, async: false

  setup do
    state = start_supervised!({Agent, fn -> true end})

    server =
      start_supervised!(
        {Bandit, plug: {LabbyConsole.AuthFixture, state: state}, port: 0, ip: {127, 0, 0, 1}}
      )

    {:ok, {_, port}} = ThousandIsland.listener_info(server)
    old = Application.get_env(:labby_console, :gateway_url)
    Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:#{port}")

    on_exit(fn ->
      Application.put_env(:labby_console, :gateway_url, old)

      for key <- [:fixture_usage_metrics, :fixture_usage_metrics_observer],
          do: Application.delete_env(:labby_console, key)
    end)

    {:ok, session} = LabbyConsole.Auth.sign_in("fixture-only")
    %{session: session}
  end

  test "exact scoped aggregates send bounded UTC windows and omit private fields", %{
    session: session
  } do
    Application.put_env(:labby_console, :fixture_usage_metrics_observer, self())
    assert {:ok, row} = LabbyConsole.UsageMetrics.fetch(session, "1h", 10_000)
    assert row["total_calls"] == 8
    refute Map.has_key?(row, "private_token")

    assert_receive {:usage_params,
                    %{
                      "since_unix" => 6400,
                      "until_unix" => 10000,
                      "bucket_count" => 24,
                      "timezone" => "UTC"
                    }}

    assert {:error, :unavailable} = LabbyConsole.UsageMetrics.fetch(session, "invalid", 10000)
  end

  test "malformed oversized and failed responses are unavailable; zero is valid", %{
    session: session
  } do
    for mode <- ["malformed", "oversized", "unavailable"] do
      Application.put_env(:labby_console, :fixture_usage_metrics, mode)
      assert {:error, :unavailable} = LabbyConsole.UsageMetrics.fetch(session, "24h")
    end

    Application.put_env(:labby_console, :fixture_usage_metrics, "empty")

    assert {:ok, %{"total_calls" => 0, "timeseries" => []}} =
             LabbyConsole.UsageMetrics.fetch(session, "7d")
  end

  test "unknown and revoked callers cannot obtain analytics", %{session: session} do
    assert {:error, :unauthenticated} = LabbyConsole.UsageMetrics.fetch("unknown", "24h")
    Application.put_env(:labby_console, :fixture_usage_metrics, "forbidden")
    assert {:error, :unauthenticated} = LabbyConsole.UsageMetrics.fetch(session, "24h")
  end
end
