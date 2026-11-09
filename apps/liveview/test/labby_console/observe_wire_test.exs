defmodule LabbyConsole.ObserveWireTest do
  use ExUnit.Case, async: false
  alias LabbyConsole.{Auth, Observe}

  setup do
    state = start_supervised!({Agent, fn -> true end})

    server =
      start_supervised!(
        {Bandit,
         plug: {LabbyConsole.ObserveFixture, state: state, owner: self()},
         port: 0,
         ip: {127, 0, 0, 1}}
      )

    {:ok, {_, port}} = ThousandIsland.listener_info(server)
    old = Application.get_env(:labby_console, :gateway_url)
    Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:#{port}")
    on_exit(fn -> Application.put_env(:labby_console, :gateway_url, old) end)
    {:ok, id} = Auth.sign_in("fixture-only")
    %{id: id, state: state}
  end

  defp row do
    %{
      "id" => 2,
      "ts_unix" => 3999,
      "elapsed_ms" => 5,
      "upstream" => "actual",
      "tool" => "read",
      "capability" => "tools",
      "operation" => "call",
      "actor" => "private-tag",
      "subject_scoped" => false,
      "outcome" => "ok",
      "response_bytes" => 4,
      "attribution" => %{
        "client_name" => "Actual client",
        "inbound_actor" => "secret",
        "agent_id" => "private"
      }
    }
  end

  test "wire preserves caller cookie bounded exact filters metadata projection and opaque cursor",
       %{id: id, state: state} do
    Agent.update(state, fn _ -> {:response, 200, %{calls: [row()], next_cursor: "opaque"}} end)

    assert {:ok, %{calls: [call], next_cursor: "opaque"}} =
             Observe.fetch(
               id,
               "activity",
               %{"upstream" => "actual", "query" => "read"},
               "1h",
               4000,
               nil
             )

    assert call["attribution"] == %{"client_name" => "Actual client"}
    assert call["response_bytes"] == 4

    assert_receive {:observe_wire,
                    %{
                      "params" => %{
                        "limit" => 25,
                        "upstream" => "actual",
                        "search" => "read",
                        "since_unix" => 400,
                        "until_unix" => 4000
                      }
                    }, ["fixture_session=one"]}

    assert {:error, :unauthenticated} = Observe.fetch("foreign", "activity", %{}, "1h", 4000, nil)
  end

  test "forbidden remains typed on both reads and keeps a valid local login", %{
    id: id,
    state: state
  } do
    Agent.update(state, fn _ -> {:response, 403, %{}} end)
    assert {:error, :forbidden} = Observe.fetch(id, "activity", %{}, "1h", 4000, nil)
    assert {:error, :forbidden} = Observe.fetch(id, "logs", %{}, "1h", 4000, nil)
    assert {:ok, _} = Auth.current(id)
  end

  test "missing backend contracts and malformed optional metadata are visible failures", %{
    id: id,
    state: state
  } do
    Agent.update(state, fn _ -> {:response, 404, %{}} end)
    assert {:error, :unavailable} = Observe.fetch(id, "traces", %{}, "1h", 4000, nil)

    Agent.update(state, fn _ ->
      {:response, 200, %{calls: [Map.put(row(), "attribution", %{"client_name" => 4})]}}
    end)

    assert {:error, :unavailable} = Observe.fetch(id, "activity", %{}, "1h", 4000, nil)
  end

  test "server logs use bounded scan and ANSI safe projection", %{id: id, state: state} do
    Agent.update(state, fn _ ->
      {:response, 200,
       %{
         kind: "server_logs",
         entries: [
           %{
             timestamp: "now",
             level: "ERROR",
             message: "\e[31mreal\e[0m",
             fields: %{secret: "private"}
           }
         ],
         truncated: true
       }}
    end)

    assert {:ok, %{entries: [%{"message" => "real"} = log], truncated: true}} =
             Observe.fetch(id, "logs", %{"level" => "ERROR", "query" => "real"}, "1h", 4000, nil)

    refute Map.has_key?(log, "fields")

    assert_receive {:observe_wire,
                    %{
                      "params" => %{
                        "limit" => 200,
                        "max_scan_bytes" => 32768,
                        "stop_after_limit" => true,
                        "level" => "ERROR",
                        "query" => "real"
                      }
                    }, _}
  end
end
