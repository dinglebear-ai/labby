defmodule LabbyConsoleWeb.ToolErrorFiltersTest do
  use LabbyConsoleWeb.ConnCase
  import Phoenix.LiveViewTest
  alias LabbyConsole.{Auth, Observe, UsageCalls}

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

    row = %{
      id: 7,
      ts_unix: 3999,
      elapsed_ms: 5,
      upstream: "actual",
      tool: "completed_failure",
      capability: "tools",
      operation: "call",
      actor: "fixture-user",
      subject_scoped: false,
      outcome: "tool_error"
    }

    %{state: state, row: row}
  end

  for adapter <- [:observe, :usage_calls] do
    test "#{adapter} forwards exact tool_error and rejects unknown outcomes before HTTP", %{
      state: state,
      row: row
    } do
      {:ok, id} = Auth.sign_in("fixture-only")
      Agent.update(state, fn _ -> {:response, 200, %{calls: [row]}} end)

      fetch = fn outcome ->
        case unquote(adapter) do
          :observe ->
            Observe.fetch(id, "activity", %{"outcome" => outcome}, "1h", 4000, nil)

          :usage_calls ->
            UsageCalls.list(id, %{since_unix: 400, until_unix: 4000, outcome: outcome})
        end
      end

      assert {:ok, %{calls: [%{"outcome" => "tool_error"}]}} = fetch.("tool_error")

      assert_receive {:observe_wire, %{"params" => %{"outcome" => "tool_error"}},
                      ["fixture_session=one"]}

      assert {:error, :invalid_filters} = fetch.("tool_error_forged")
      refute_receive {:observe_wire, _, _}
    end
  end

  for {route, form, event, params, selected} <- [
        {"/observe", "#observe-filter-form", "filter",
         %{"filters" => %{"window" => "1h", "outcome" => "tool_error"}},
         "select[name='filters[outcome]']"},
        {"/activity", "#usage-calls-filters", "filters",
         %{"window" => "1h", "outcome" => "tool_error"}, "select[name=outcome]"}
      ] do
    test "#{route} form accepts tool_error, reads it back and rejects forged outcomes", %{
      conn: conn,
      state: state,
      row: row
    } do
      signed = post(conn, "/sign-in", %{credential: "fixture-only"})
      Agent.update(state, fn _ -> {:response, 200, %{calls: [row]}} end)
      {:ok, view, _} = live(recycle(signed), unquote(route))
      render_async(view)
      assert_receive {:observe_wire, _, _}
      assert has_element?(view, unquote(selected) <> " option[value=tool_error]", "Tool error")

      view |> element(unquote(form)) |> render_change(unquote(Macro.escape(params)))
      assert render_async(view) =~ "completed_failure"
      assert has_element?(view, unquote(selected) <> " option[value=tool_error][selected]")
      assert render(view) =~ "tool_error"
      assert_receive {:observe_wire, %{"params" => %{"outcome" => "tool_error"}}, _}
      refute :sys.get_state(view.pid).socket.assigns.loading

      forged =
        case unquote(route) do
          "/observe" -> %{"filters" => %{"outcome" => "tool_error_forged"}}
          "/activity" -> %{"window" => "1h", "outcome" => "tool_error_forged"}
        end

      render_hook(view, unquote(event), forged)
      refute_receive {:observe_wire, _, _}
      assert has_element?(view, unquote(selected) <> " option[value=tool_error][selected]")
    end
  end
end
