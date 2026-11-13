# Real backend run: no HTTP fixture is started or linked into this application.
Code.require_file("console-http-guard.exs", __DIR__)
LabbyConsole.E2E.ConsoleHttpGuard.start_from_env!()
Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:4299")
Application.put_env(:labby_console, LabbyConsoleWeb.Endpoint,
  Keyword.merge(Application.get_env(:labby_console, LabbyConsoleWeb.Endpoint),
    server: true, http: [ip: {127, 0, 0, 1}, port: 4298],
    check_origin: ["http://127.0.0.1:4298"]))
{:ok, _} = Application.ensure_all_started(:labby_console)
Process.sleep(:infinity)
