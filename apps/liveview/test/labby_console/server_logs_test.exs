defmodule LabbyConsole.ServerLogsTest do
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
      Application.delete_env(:labby_console, :fixture_server_logs)
    end)

    {:ok, id} = LabbyConsole.Auth.sign_in("fixture-only")
    %{id: id}
  end

  test "bounded query projects log data", %{id: id} do
    Application.put_env(:labby_console, :fixture_server_logs_owner, self())

    assert {:ok, %{entries: [entry], truncated: true}} =
             LabbyConsole.ServerLogs.query(id, %{"level" => "ERROR", "query" => "boom"})

    assert_receive {:logs_query,
                    %{
                      "action" => "server_logs.query",
                      "params" => %{
                        "limit" => 200,
                        "max_scan_bytes" => 32_768,
                        "stop_after_limit" => true,
                        "level" => "ERROR",
                        "query" => "boom"
                      }
                    }}

    Application.delete_env(:labby_console, :fixture_server_logs_owner)
    assert entry["message"] == "<script>boom</script>"
    refute Map.has_key?(entry, "fields")
  end

  test "invalid, oversized and unauthorized responses fail closed", %{id: id} do
    for mode <- ["malformed", "invalid", "oversized", "too_many"] do
      Application.put_env(:labby_console, :fixture_server_logs, mode)
      assert {:error, :unavailable} = LabbyConsole.ServerLogs.query(id)
    end

    assert {:error, :unavailable} = LabbyConsole.ServerLogs.query(id, %{"level" => "BOGUS"})
    Application.put_env(:labby_console, :fixture_server_logs, "empty")
    assert {:ok, %{entries: [], truncated: false}} = LabbyConsole.ServerLogs.query(id)
    assert {:error, :unauthenticated} = LabbyConsole.ServerLogs.query("stranger")
    Application.put_env(:labby_console, :fixture_server_logs, "forbidden")
    assert {:error, :unauthenticated} = LabbyConsole.ServerLogs.query(id)
  end

  test "ANSI styling is stripped instead of interpreted", %{id: id} do
    Application.put_env(:labby_console, :fixture_server_logs, "ansi")
    assert {:ok, %{entries: [%{"message" => "error"}]}} = LabbyConsole.ServerLogs.query(id)
  end
end
