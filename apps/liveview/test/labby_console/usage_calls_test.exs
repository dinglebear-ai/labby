defmodule LabbyConsole.UsageCallsTest do
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
      Application.delete_env(:labby_console, :fixture_usage_calls)
    end)

    {:ok, id} = LabbyConsole.Auth.sign_in("fixture-only")
    %{id: id}
  end

  test "projects retained calls and follows the returned opaque cursor", %{id: id} do
    assert {:ok, %{calls: [row], next_cursor: cursor}} =
             LabbyConsole.UsageCalls.list(id, %{since_unix: 100, until_unix: 200})

    assert row["tool"] == "fetch"
    refute Map.has_key?(row, "payload")

    assert {:ok, %{calls: [next], next_cursor: nil}} =
             LabbyConsole.UsageCalls.list(id, %{since_unix: 100, until_unix: 200, cursor: cursor})

    assert next["tool"] == "fetch_html"
  end

  test "rejects invalid filters locally and malformed or oversized pages", %{id: id} do
    for params <- [
          %{},
          %{since_unix: 200, until_unix: 100},
          %{since_unix: 0, until_unix: 700_000},
          %{since_unix: 100, until_unix: 200, outcome: "forged"},
          %{since_unix: 100, until_unix: 200, cursor: ""}
        ] do
      assert {:error, :invalid_filters} = LabbyConsole.UsageCalls.list(id, params)
    end

    for mode <- ["malformed", "oversized", "invalid", "invalid_cursor", "invalid_time"] do
      Application.put_env(:labby_console, :fixture_usage_calls, mode)

      assert {:error, :unavailable} =
               LabbyConsole.UsageCalls.list(id, %{since_unix: 100, until_unix: 200})
    end

    Application.put_env(:labby_console, :fixture_usage_calls, "empty")

    assert {:ok, %{calls: [], next_cursor: nil}} =
             LabbyConsole.UsageCalls.list(id, %{since_unix: 100, until_unix: 200})

    assert {:error, :unauthenticated} =
             LabbyConsole.UsageCalls.list("stranger", %{since_unix: 100, until_unix: 200})

    Application.put_env(:labby_console, :fixture_usage_calls, "unauthorized")

    assert {:error, :unauthenticated} =
             LabbyConsole.UsageCalls.list(id, %{since_unix: 100, until_unix: 200})
  end
end
