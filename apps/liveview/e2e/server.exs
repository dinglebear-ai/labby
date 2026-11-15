# Test fixture runs only in MIX_ENV=test and is never included in a release.
{:ok, state} = Agent.start_link(fn -> true end)
{:ok, _} = Bandit.start_link(plug: {LabbyConsole.AuthFixture, state: state}, port: 4199, ip: {127,0,0,1})
Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:4199")
Application.put_env(:labby_console, LabbyConsoleWeb.Endpoint, Keyword.merge(Application.get_env(:labby_console, LabbyConsoleWeb.Endpoint), server: true, http: [ip: {127,0,0,1}, port: 4198], check_origin: ["http://127.0.0.1:4198"]))
{:ok, _} = Application.ensure_all_started(:labby_console)
Process.sleep(:infinity)
