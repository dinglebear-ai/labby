# Controlled source-UI fixture only. No native gateway or provider is started.
{:ok, state} =
  Agent.start_link(fn ->
    state = LabbyConsole.AccessUIFixture.state(nil)

    %{
      state
      | members:
          state.members ++
            [
              Map.put(
                hd(state.members),
                "principal_id",
                String.pad_trailing("principal-", 128, "x")
              )
            ]
    }
  end)

{:ok, _} =
  Bandit.start_link(
    plug: {LabbyConsole.AccessUIFixture, state: state},
    port: 4199,
    ip: {127, 0, 0, 1}
  )

Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:4199")

Application.put_env(
  :labby_console,
  LabbyConsoleWeb.Endpoint,
  Keyword.merge(Application.get_env(:labby_console, LabbyConsoleWeb.Endpoint),
    server: true,
    http: [ip: {127, 0, 0, 1}, port: 4198],
    check_origin: ["http://127.0.0.1:4198"]
  )
)

{:ok, _} = Application.ensure_all_started(:labby_console)
Process.sleep(:infinity)
