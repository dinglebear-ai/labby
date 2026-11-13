defmodule LabbyConsole.RuntimeTest do
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
      Application.delete_env(:labby_console, :fixture_runtime)
    end)

    {:ok, id} = LabbyConsole.Auth.sign_in("fixture-only")
    %{id: id}
  end

  test "runtime projects the daemon sample without exposing unknown fields", %{id: id} do
    assert {:ok, metrics} = LabbyConsole.Runtime.metrics(id)
    assert metrics["cpu_percent"] == 6.4
    assert metrics["memory_used_bytes"] == 356_515_840
    refute Map.has_key?(metrics, "private")
  end

  test "unsupported fields stay null and malformed or revoked samples fail closed", %{id: id} do
    Application.put_env(:labby_console, :fixture_runtime, "unsupported")
    assert {:ok, %{"available" => false, "cpu_percent" => nil}} = LabbyConsole.Runtime.metrics(id)

    for mode <- ["malformed", "invalid", "oversized", "unavailable"] do
      Application.put_env(:labby_console, :fixture_runtime, mode)
      assert {:error, :unavailable} = LabbyConsole.Runtime.metrics(id)
    end

    assert {:error, :unauthenticated} = LabbyConsole.Runtime.metrics("stranger")
    Application.put_env(:labby_console, :fixture_runtime, "unauthorized")
    assert {:error, :unauthenticated} = LabbyConsole.Runtime.metrics(id)
  end

  test "sampling can exceed the ordinary two-second read timeout", %{id: id} do
    Application.put_env(:labby_console, :fixture_runtime, "slow")
    assert {:ok, %{"sample_ms" => 5000}} = LabbyConsole.Runtime.metrics(id)
  end
end
