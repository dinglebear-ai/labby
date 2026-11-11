defmodule LabbyConsoleWeb.ConnectionLive do
  @moduledoc "Retry an unavailable gateway while preserving the caller's console session."
  use LabbyConsoleWeb, :live_view

  @paths ~w(/overview /gateway /observe /projects /runtime /clients /diagnostics /usage /activity /logs)

  def mount(params, _, socket) do
    path = if params["return_to"] in @paths, do: params["return_to"], else: "/overview"
    {:ok, assign(socket, page_title: "Connection unavailable", return_to: path)}
  end

  def render(assigns) do
    ~H"""
    <Layouts.app flash={@flash}>
      <main id="console-integration" tabindex="-1">
        <section class="console-panel">
          <h1>Gateway connection unavailable</h1>
          <p>Your session was kept. Check the gateway connection, then try again.</p>
          <.link href={@return_to} id="connection-retry">Retry connection</.link>
          <.link href="/sign-in">Sign in with another credential</.link>
        </section>
      </main>
    </Layouts.app>
    """
  end
end
