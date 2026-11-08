defmodule LabbyConsoleWeb.ServerDetailsLive do
  use LabbyConsoleWeb, :live_view
  alias LabbyConsole.{Auth, ServerDetails}

  def mount(%{"id" => server_id}, session, socket) do
    session_id = session["console_session"]

    case Auth.current(session_id) do
      {:ok, user} ->
        if connected?(socket),
          do:
            Process.send_after(
              self(),
              :session_expired,
              max(0, user["expires_at"] - System.system_time(:second)) * 1000
            )

        socket =
          assign(socket,
            page_title: "Server details",
            session_id: session_id,
            server_id: server_id,
            row: nil,
            loading: false,
            error: nil
          )

        {:ok, if(connected?(socket), do: load(socket), else: socket)}

      _ ->
        {:ok, redirect(socket, to: "/sign-in")}
    end
  end

  def handle_event("refresh", _, %{assigns: %{loading: false}} = socket),
    do: {:noreply, load(socket)}

  def handle_event("refresh", _, socket), do: {:noreply, socket}
  def handle_info(:session_expired, socket), do: {:noreply, expire(socket)}

  defp load(socket) do
    session_id = socket.assigns.session_id
    server_id = socket.assigns.server_id

    socket
    |> assign(loading: true, row: nil, error: nil)
    |> start_async(:details, fn -> ServerDetails.fetch(session_id, server_id) end)
  end

  def handle_async(:details, {:ok, {:ok, row}}, socket),
    do: {:noreply, assign(socket, row: row, loading: false)}

  def handle_async(:details, {:ok, {:error, :unauthenticated}}, socket),
    do: {:noreply, expire(socket)}

  def handle_async(:details, _, socket),
    do:
      {:noreply,
       assign(socket,
         row: nil,
         loading: false,
         error: "Server details are unavailable. Try again."
       )}

  defp expire(socket), do: socket |> assign(row: nil) |> redirect(to: "/sign-in")

  def render(assigns) do
    ~H"""
    <Layouts.app flash={@flash} authenticated={true}>
      <main id="console-integration" tabindex="-1">
        <.link navigate="/gateway">Back to servers</.link>
        <h1>{if @row, do: @row["display_name"] || @row["name"], else: "Server details"}</h1>
        <section class="console-panel" id="server-details" aria-busy={to_string(@loading)}>
          <header class="console-inventory-header">
            <h2>Server details</h2>
            <Layouts.refresh_action
              id="server-details-refresh"
              label="Refresh details"
              event="refresh"
              disabled={@loading}
            />
          </header>
          <p :if={@loading} id="server-details-loading" role="status">Loading server details…</p>
          <p :if={@error} id="server-details-error" role="alert">{@error}</p>
          <div :if={@row} id="server-details-content">
            <dl>
              <dt>Identity</dt><dd>{@row["id"]}</dd>
              <dt>Source</dt><dd>
                {if @row["source"] == "custom_gateway", do: "Custom server", else: "Built-in service"}
              </dd>
              <dt>Status</dt><dd>
                <span
                  class="console-status"
                  data-state={
                    cond do
                      !@row["enabled"] -> "disabled"
                      @row["connected"] -> "connected"
                      true -> "disconnected"
                    end
                  }
                >
                  {cond do
                    !@row["enabled"] -> "Disabled"
                    @row["connected"] -> "Connected"
                    true -> "Disconnected"
                  end}
                </span>
              </dd>
            </dl>
            <Layouts.server_sections name={@row["name"]} id={@row["id"]} />
            <h3>Capability inventory</h3>
            <p>Exposed / discovered</p>
            <dl id="server-capabilities">
              <div
                :for={
                  {family, label} <- [
                    {"tool", "Tools"},
                    {"resource", "Resources"},
                    {"prompt", "Prompts"},
                    {"skill", "Skills"}
                  ]
                }
                data-family={family}
              >
                <dt>{label}</dt><dd>
                  {@row["exposed_#{family}_count"]} / {@row["discovered_#{family}_count"]}
                </dd>
              </div>
            </dl>
          </div>
        </section>
        <p>Read-only retained snapshot. Refresh does not reconnect or rediscover this server.</p>
      </main>
    </Layouts.app>
    """
  end
end
