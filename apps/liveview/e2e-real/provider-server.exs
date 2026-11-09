Code.require_file("console-http-guard.exs", __DIR__)
LabbyConsole.E2E.ConsoleHttpGuard.start_from_env!()
Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:4329")
Application.put_env(:labby_console, LabbyConsoleWeb.Endpoint,
  Keyword.merge(Application.get_env(:labby_console, LabbyConsoleWeb.Endpoint),
    server: true, http: [ip: {127, 0, 0, 1}, port: 4328],
    check_origin: ["http://127.0.0.1:4328"]))
{:ok, _} = Application.ensure_all_started(:labby_console)
Process.sleep(:infinity)
