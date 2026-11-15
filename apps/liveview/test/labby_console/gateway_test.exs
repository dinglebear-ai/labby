defmodule LabbyConsole.GatewayTest do
  use ExUnit.Case, async: true
  alias LabbyConsole.Gateway

  setup do
    state = start_supervised!({Agent, fn -> {200, ~s({"status":"ready"})} end})

    server =
      start_supervised!(
        {Bandit, plug: {LabbyConsole.GatewayFixture, state: state}, port: 0, ip: {127, 0, 0, 1}}
      )

    {:ok, {_, port}} = ThousandIsland.listener_info(server)
    %{url: "http://127.0.0.1:#{port}", state: state}
  end

  test "reports readiness from the configured HTTP gateway", %{url: url} do
    assert {:ok, %{status: "ready"}} = Gateway.readiness(url)
  end

  test "preserves not-ready and degraded states", %{url: url, state: state} do
    Agent.update(state, fn _ ->
      {503, ~s({"status":"not_ready","pending":["File Stash is not ready"]})}
    end)

    assert {:ok, %{status: "not_ready", details: ["File Stash is not ready"]}} =
             Gateway.readiness(url)

    Agent.update(state, fn _ -> {200, ~s({"status":"degraded","degraded":["upstreams"]})} end)
    assert {:ok, %{status: "degraded", details: ["upstreams"]}} = Gateway.readiness(url)
  end

  test "rejects missing and unsafe target configuration" do
    for target <- [
          nil,
          "",
          "file:///etc/passwd",
          "http://user:secret@localhost",
          "http://localhost/?token=secret",
          "http://localhost/#fragment"
        ] do
      assert {:error, :invalid_target} = Gateway.readiness(target)
    end
  end

  test "fails closed for malformed, oversized, redirected and failed responses", %{
    url: url,
    state: state
  } do
    for response <- [
          {200, "not json"},
          {200, ~s({"status":"invented"})},
          {200, String.duplicate("x", 70_000)},
          {302, ""},
          {500, "private error"}
        ] do
      Agent.update(state, fn _ -> response end)
      assert {:error, :unavailable} = Gateway.readiness(url)
    end
  end

  test "times out rather than hanging the caller", %{url: url, state: state} do
    Agent.update(state, fn _ -> {200, "timeout"} end)
    assert {:error, :unavailable} = Gateway.readiness(url)
  end

  test "liveness is separate from readiness", %{url: url, state: state} do
    Agent.update(state, fn _ -> {503, ~s({"status":"not_ready","pending":[]})} end)
    assert {:ok, %{status: "ok"}} = Gateway.liveness(url)
    assert {:ok, %{status: "not_ready", details: []}} = Gateway.readiness(url)
  end
end
