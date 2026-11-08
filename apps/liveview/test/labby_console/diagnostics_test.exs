defmodule LabbyConsole.DiagnosticsTest do
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
      Application.delete_env(:labby_console, :fixture_diagnostics)
    end)

    {:ok, id} = LabbyConsole.Auth.sign_in("fixture-only")
    %{id: id}
  end

  test "authenticated checks project the real report contract", %{id: id} do
    assert {:ok,
            [
              %{"check" => "disk:space", "severity" => "warn", "message" => "Disk space is low"} =
                finding
            ]} = LabbyConsole.Diagnostics.checks(id)

    assert Enum.sort(Map.keys(finding)) == ~w(check message service severity)
  end

  test "invalid reports fail closed and anonymous callers have no findings", %{id: id} do
    for mode <- ["malformed", "invalid", "oversized"] do
      Application.put_env(:labby_console, :fixture_diagnostics, mode)
      assert {:error, :unavailable} = LabbyConsole.Diagnostics.checks(id)
    end

    Application.put_env(:labby_console, :fixture_diagnostics, "empty")
    assert {:ok, []} = LabbyConsole.Diagnostics.checks(id)
    assert {:error, :unauthenticated} = LabbyConsole.Diagnostics.checks("another-caller")
    Application.put_env(:labby_console, :fixture_diagnostics, "forbidden")
    assert {:error, :unauthenticated} = LabbyConsole.Diagnostics.checks(id)
  end
end
