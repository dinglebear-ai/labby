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
      LabbyConsole.GatewayAction.Supervisor,
      LabbyConsole.OAuthHandoff,
      {DNSCluster, query: Application.get_env(:labby_console, :dns_cluster_query) || :ignore},
      {Phoenix.PubSub, name: LabbyConsole.PubSub},
      # Start a worker by calling: LabbyConsole.Worker.start_link(arg)
      # {LabbyConsole.Worker, arg},
      # Start to serve requests, typically the last entry
      LabbyConsoleWeb.Endpoint
    ]

    # See https://elixir.hexdocs.pm/Supervisor.html
    # for other strategies and supported options
    opts = [name: LabbyConsole.Supervisor]
    Supervisor.start_link(LabbyConsole.Application.Supervisor, children, opts)
  end

  # Tell Phoenix to update the endpoint configuration
  # whenever the application is updated.
  @impl true
  def config_change(changed, _new, removed) do
    LabbyConsoleWeb.Endpoint.config_change(changed, removed)
    :ok
  end
end

defmodule LabbyConsole.Application.Supervisor do
  @moduledoc false
  use Supervisor

  @impl true
  def init(children) do
    # Receipts outlive every transport child, including a transport-supervisor
    # restart-budget escalation. Only stopping the whole application drops them.
    :ets.new(
      LabbyConsole.GatewayAction.Lifecycle,
      [:named_table, :public, :set, write_concurrency: true]
    )

    Supervisor.init(children, strategy: :one_for_one)
  end
end
