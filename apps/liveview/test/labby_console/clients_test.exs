defmodule LabbyConsole.ClientsTest do
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
      Application.delete_env(:labby_console, :fixture_clients)
    end)

    {:ok, id} = LabbyConsole.Auth.sign_in("fixture-only")
    %{id: id}
  end

  test "observed clients use authenticated read contract and project safe fields", %{id: id} do
    assert {:ok, [row]} = LabbyConsole.Clients.list(id)
    assert row["client_name"] == "Codex"
    assert row["observation_count"] == 42
    refute Map.has_key?(row, "credential")
    assert {:error, :unauthenticated} = LabbyConsole.Clients.list("another-caller")
  end

  test "invalid and unavailable snapshots fail closed and optional fields remain optional", %{
    id: id
  } do
    for mode <- ["malformed", "invalid", "oversized", "unavailable"] do
      Application.put_env(:labby_console, :fixture_clients, mode)
      assert {:error, :unavailable} = LabbyConsole.Clients.list(id)
    end

    Application.put_env(:labby_console, :fixture_clients, "minimal")

    assert {:ok, [%{"observation_count" => 0, "transport" => "stdio"}]} =
             LabbyConsole.Clients.list(id)

    Application.put_env(:labby_console, :fixture_clients, "empty")
    assert {:ok, []} = LabbyConsole.Clients.list(id)
    Application.put_env(:labby_console, :fixture_clients, "unauthorized")
    assert {:error, :unauthenticated} = LabbyConsole.Clients.list(id)
  end
end
