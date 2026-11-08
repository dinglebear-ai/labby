defmodule LabbyConsole.ServerDetailsTest do
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
      Application.delete_env(:labby_console, :fixture_server_details)
    end)

    {:ok, session} = LabbyConsole.Auth.sign_in("fixture-only")
    %{session: session}
  end

  test "details expose capability counts without configuration or private diagnostics", %{
    session: session
  } do
    assert {:ok, row} = LabbyConsole.ServerDetails.fetch(session, "fixture-server")
    assert row["display_name"] == "Fixture Fetch"
    assert row["exposed_tool_count"] == 3
    refute Map.has_key?(row, "config_summary")
    refute Map.has_key?(row, "warnings")
  end

  test "an unknown caller cannot read server details" do
    assert {:error, :unauthenticated} =
             LabbyConsole.ServerDetails.fetch("unknown", "fixture-server")
  end

  test "invalid or unrelated snapshots and gateway errors fail closed", %{session: session} do
    for mode <- ["malformed", "wrong_id", "unavailable"] do
      Application.put_env(:labby_console, :fixture_server_details, mode)
      assert {:error, :unavailable} = LabbyConsole.ServerDetails.fetch(session, "fixture-server")
    end

    Application.put_env(:labby_console, :fixture_server_details, "forbidden")

    assert {:error, :unauthenticated} =
             LabbyConsole.ServerDetails.fetch(session, "fixture-server")

    assert {:error, :unauthenticated} =
             LabbyConsole.ServerDetails.fetch(session, "fixture-server")
  end
end
