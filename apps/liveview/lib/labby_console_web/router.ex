defmodule LabbyConsoleWeb.Router do
  use LabbyConsoleWeb, :router

  pipeline :browser do
    plug :accepts, ["html"]
    plug :fetch_session
    plug :fetch_live_flash
    plug :put_root_layout, html: {LabbyConsoleWeb.Layouts, :root}
    plug :protect_from_forgery
    plug :put_secure_browser_headers
    plug :no_store
  end

  defp no_store(conn, _), do: Plug.Conn.put_resp_header(conn, "cache-control", "no-store")

  pipeline :api do
    plug :accepts, ["json"]
  end

  scope "/", LabbyConsoleWeb do
    pipe_through :browser

    live "/", IntegrationLive, :index
    live "/connection", ConnectionLive, :index
    get "/sign-in", SessionController, :new
    post "/sign-in", SessionController, :create
    post "/sign-in/provider/start", SessionController, :provider_start
    post "/sign-in/provider/poll", SessionController, :provider_poll
    post "/sign-in/provider/cancel", SessionController, :provider_cancel
    post "/sign-out", SessionController, :delete
    live "/overview", OverviewLive, :index
    live "/observe", ObserveLive, :index
    live "/gateway", GatewayLive, :index
    live "/gateway/servers/:id", ServerDetailsLive, :show
    live "/gateway/servers/:id/catalog", ServerCatalogLive, :show
    live "/diagnostics", DiagnosticsLive, :index
    live "/runtime", RuntimeLive, :index
    live "/clients", ClientsLive, :index
    live "/usage", UsageMetricsLive, :index
    live "/activity", UsageCallsLive, :index
    live "/logs", ServerLogsLive, :index
    live "/gateway/upstreams/:name/runtime", ServerRuntimeLive, :show
    live "/projects", ProjectsLive, :index
  end

  # Other scopes may use custom stacks.
  # scope "/api", LabbyConsoleWeb do
  #   pipe_through :api
  # end
end
