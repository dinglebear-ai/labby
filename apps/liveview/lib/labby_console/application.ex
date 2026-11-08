defmodule LabbyConsole.Application do
  # See https://elixir.hexdocs.pm/Application.html
  # for more information on OTP Applications
  @moduledoc false

  use Application

  @impl true
  def start(_type, _args) do
    children = [
      LabbyConsoleWeb.Telemetry,
      LabbyConsole.Auth,
      LabbyConsole.GatewayAction.Lifecycle,
      {Task.Supervisor, name: LabbyConsole.GatewayAction.Tasks, max_children: 32},
      LabbyConsole.GatewayAction,
      {DNSCluster, query: Application.get_env(:labby_console, :dns_cluster_query) || :ignore},
      {Phoenix.PubSub, name: LabbyConsole.PubSub},
      # Start a worker by calling: LabbyConsole.Worker.start_link(arg)
      # {LabbyConsole.Worker, arg},
      # Start to serve requests, typically the last entry
      LabbyConsoleWeb.Endpoint
    ]

    # See https://elixir.hexdocs.pm/Supervisor.html
    # for other strategies and supported options
    opts = [strategy: :one_for_one, name: LabbyConsole.Supervisor]
    Supervisor.start_link(children, opts)
  end

  # Tell Phoenix to update the endpoint configuration
  # whenever the application is updated.
  @impl true
  def config_change(changed, _new, removed) do
    LabbyConsoleWeb.Endpoint.config_change(changed, removed)
    :ok
  end
end
