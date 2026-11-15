defmodule LabbyConsole.ReadActionTest do
  use ExUnit.Case
  alias LabbyConsole.Auth

  test "unknown actions cannot turn the read transport into a mutation proxy" do
    assert {:error, :unavailable} = Auth.read_action("missing", "gateway", "gateway.reload", %{})
    assert {:error, :unavailable} = Auth.read_action("missing", "doctor", "repair", %{})
  end

  test "new slice reads require a caller session" do
    for action <-
          ~w(gateway.host.metrics gateway.clients.list gateway.mcp.list gateway.usage.metrics gateway.usage.calls) do
      assert {:error, :unauthenticated} = Auth.read_action("missing", "gateway", action, %{})
    end
  end

  test "server logs reads require a caller session" do
    assert {:error, :unauthenticated} =
             Auth.read_action("missing", "server_logs", "server_logs.query", %{})
  end

  test "approved reads still require a caller session" do
    assert {:error, :unauthenticated} =
             Auth.read_action("missing", "gateway", "gateway.server.get", %{"id" => "one"})
  end
end
