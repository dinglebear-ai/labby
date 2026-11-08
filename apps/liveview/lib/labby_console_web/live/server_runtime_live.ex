defmodule LabbyConsoleWeb.ServerRuntimeLive do
  use LabbyConsoleWeb, :live_view
  on_mount {LabbyConsoleWeb.LiveSession, :authenticated}
  alias LabbyConsole.{ServerRuntime}

  def mount(%{"name" => server_id}, _session, socket) do
    socket =
      assign(socket,
        page_title: "Runtime snapshot",
        server_id: server_id,
        row: nil,
        loading: false,
        error: nil
      )

    {:ok, if(connected?(socket), do: load(socket), else: socket)}
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
    |> start_async(:runtime, fn -> ServerRuntime.fetch(session_id, server_id) end)
  end

  def handle_async(:runtime, {:ok, {:ok, row}}, socket),
    do: {:noreply, assign(socket, row: row, loading: false)}

  def handle_async(:runtime, {:ok, {:error, :unauthenticated}}, socket),
    do: {:noreply, expire(socket)}

  def handle_async(:runtime, _, socket),
    do:
      {:noreply,
       assign(socket,
         row: nil,
         loading: false,
         error: "Runtime snapshot is unavailable. Try again."
       )}

  defp expire(socket), do: socket |> assign(row: nil) |> redirect(to: "/sign-in")

  def render(assigns) do
    ~H"""
    <Layouts.app
      flash={@flash}
      authenticated={true}
      subject={@subject}
      preference_key={@preference_key}
    >
      <main id="console-integration" tabindex="-1">
        <.link navigate="/gateway">Back to servers</.link>
        <h1>{"Runtime snapshot"}</h1>
        <section class="console-panel" id="server-runtime" aria-busy={to_string(@loading)}>
          <header class="console-inventory-header">
            <h2>Runtime snapshot</h2>
            <Layouts.refresh_action
              id="server-runtime-refresh"
              label="Refresh runtime"
              event="refresh"
              disabled={@loading}
            />
          </header>
          <p :if={@loading} id="server-runtime-loading" role="status">Loading runtime snapshot…</p>
          <p :if={!@loading && !@error && !@row} id="server-runtime-empty">
            No visible runtime snapshot for this upstream.
          </p>
          <p :if={@error} id="server-runtime-error" role="alert">{@error}</p>
          <div :if={@row} id="server-runtime-content">
            <h3>{@row["name"]}</h3>
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
            <dl id="server-runtime-process">
              <div :for={
                {label, key} <- [
                  {"Process ID", "pid"},
                  {"Process group", "pgid"},
                  {"Age · seconds", "age_seconds"},
                  {"Likely stale processes", "likely_stale_count"},
                  {"Transport", "transport"},
                  {"Server version", "server_version"},
                  {"MCP protocol", "protocol_version"}
                ]
              }>
                <dt>{label}</dt><dd>{Map.get(@row, key) || "Not reported"}</dd>
              </div>
            </dl>
            <h3>Capability inventory</h3>
            <p>Exposed / discovered</p>
            <dl id="server-runtime-capabilities">
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
        <p>
          Read-only runtime snapshot. Process observations can be stale. Refresh does not connect, restart, or discover upstreams. Gateway administrator access is required.
        </p>
      </main>
    </Layouts.app>
    """
  end
end
