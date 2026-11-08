defmodule LabbyConsole.ServerRuntimeTest do
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
      Application.delete_env(:labby_console, :fixture_server_runtime)
    end)

    {:ok, session} = LabbyConsole.Auth.sign_in("fixture-only")
    %{session: session}
  end

  test "runtime exposes a bounded matching process snapshot without private fields", %{
    session: session
  } do
    assert {:ok, row} = LabbyConsole.ServerRuntime.fetch(session, "fixture-server")
    assert row["pid"] == 4821
    assert row["exposed_tool_count"] == 3
    refute Map.has_key?(row, "target")
    refute Map.has_key?(row, "owner")
  end

  test "an unknown caller cannot read server runtime" do
    assert {:error, :unauthenticated} =
             LabbyConsole.ServerRuntime.fetch("unknown", "fixture-server")
  end

  test "invalid or unrelated snapshots and gateway errors fail closed", %{session: session} do
    for mode <- ["malformed", "wrong_id", "unavailable"] do
      Application.put_env(:labby_console, :fixture_server_runtime, mode)
      assert {:error, :unavailable} = LabbyConsole.ServerRuntime.fetch(session, "fixture-server")
    end

    Application.put_env(:labby_console, :fixture_server_runtime, "forbidden")

    assert {:error, :unauthenticated} =
             LabbyConsole.ServerRuntime.fetch(session, "fixture-server")

    assert {:error, :unauthenticated} =
             LabbyConsole.ServerRuntime.fetch(session, "fixture-server")
  end

  test "empty runtime is distinct from an unavailable process and absent process IDs stay absent",
       %{session: session} do
    Application.put_env(:labby_console, :fixture_server_runtime, "empty")
    assert {:ok, nil} = LabbyConsole.ServerRuntime.fetch(session, "fixture-server")
    Application.put_env(:labby_console, :fixture_server_runtime, "disconnected")
    assert {:ok, row} = LabbyConsole.ServerRuntime.fetch(session, "fixture-server")
    refute row["connected"]
    assert is_nil(row["pid"])
    assert row["likely_stale_count"] == 2
  end
end
