defmodule LabbyConsoleWeb.OverviewLive do
  use LabbyConsoleWeb, :live_view
  alias LabbyConsole.{Auth, UsageMetrics}

  def mount(_, session, socket) do
    id = session["console_session"]

    case Auth.current(id) do
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
            page_title: "Overview",
            session_id: id,
            inventory: nil,
            usage: nil,
            pending: [],
            errors: []
          )

        {:ok, if(connected?(socket), do: load(socket), else: socket)}

      _ ->
        {:ok, redirect(socket, to: "/sign-in")}
    end
  end

  def handle_event("refresh", _, %{assigns: %{pending: []}} = socket),
    do: {:noreply, load(socket)}

  def handle_event("refresh", _, socket), do: {:noreply, socket}
  def handle_info(:session_expired, socket), do: {:noreply, expire(socket)}

  defp load(socket) do
    id = socket.assigns.session_id

    socket
    |> assign(inventory: nil, usage: nil, errors: [], pending: [:inventory, :usage])
    |> start_async(:inventory, fn -> Auth.servers(id) end)
    |> start_async(:usage, fn -> UsageMetrics.fetch(id, "24h") end)
  end

  def handle_async(_, {:ok, {:error, :unauthenticated}}, socket), do: {:noreply, expire(socket)}

  def handle_async(name, {:ok, {:ok, value}}, socket),
    do:
      {:noreply,
       socket |> assign(name, value) |> assign(pending: List.delete(socket.assigns.pending, name))}

  def handle_async(name, _, socket),
    do:
      {:noreply,
       assign(socket,
         pending: List.delete(socket.assigns.pending, name),
         errors: [name | socket.assigns.errors]
       )}

  defp expire(socket),
    do: socket |> assign(inventory: nil, usage: nil) |> redirect(to: "/sign-in")

  def render(assigns) do
    ~H"""
    <Layouts.app flash={@flash} authenticated={true} page="Overview">
      <main id="console-integration" tabindex="-1">
        <header class="console-inventory-header">
          <div>
            <h1>Overview</h1><p>Your gateway at a glance.</p>
          </div>
          <Layouts.refresh_action
            id="overview-refresh"
            label="Refresh overview"
            event="refresh"
            disabled={@pending != []}
          />
        </header>
        <div class="console-overview-grid">
          <section class="console-panel" aria-busy={to_string(:inventory in @pending)}>
            <header class="console-inventory-header">
              <h2>Gateway inventory</h2><.link navigate="/gateway">View servers →</.link>
            </header>
            <div class="console-overview-body">
              <p :if={:inventory in @pending} role="status">Loading inventory…</p>
              <p :if={:inventory in @errors} id="overview-inventory-error" role="alert">
                Inventory is unavailable. Refresh to retry.
              </p>
              <div :if={@inventory != nil} id="overview-inventory">
                <dl class="console-overview-stats">
                  <div>
                    <dt>Configured</dt><dd>{length(@inventory)}</dd>
                  </div>
                  <div>
                    <dt>Connected</dt><dd>{Enum.count(@inventory, & &1["connected"])}</dd>
                  </div>
                  <div>
                    <dt>Disabled</dt><dd>{Enum.count(@inventory, &(!&1["enabled"]))}</dd>
                  </div>
                </dl>
                <p>Current transport snapshot. Connection status is separate from usage.</p>
              </div>
            </div>
          </section>
          <section class="console-panel" aria-busy={to_string(:usage in @pending)}>
            <header class="console-inventory-header">
              <h2>Usage · Last 24 hours</h2><.link navigate="/usage">View usage →</.link>
            </header>
            <div class="console-overview-body">
              <p :if={:usage in @pending} role="status">Loading usage…</p>
              <p :if={:usage in @errors} id="overview-usage-error" role="alert">
                Usage is unavailable. Refresh to retry.
              </p>
              <dl :if={@usage != nil} id="overview-usage" class="console-overview-stats">
                <div>
                  <dt>Total calls</dt><dd>{@usage["window_total_calls"]}</dd>
                </div>
                <div>
                  <dt>Failed calls</dt><dd>{@usage["error_calls"]}</dd>
                </div>
                <div>
                  <dt>P95 elapsed</dt><dd>{@usage["p95_elapsed_ms"]}<small> ms</small></dd>
                </div>
              </dl>
            </div>
          </section>
        </div>
        <section class="console-panel">
          <header class="console-inventory-header">
            <h2>Explore your gateway</h2>
          </header>
          <nav class="console-overview-links" aria-label="Overview details">
            <.link navigate="/activity">Recent calls →</.link>
            <.link navigate="/runtime">Runtime &amp; readiness →</.link>
            <.link navigate="/clients">Observed clients →</.link>
            <.link navigate="/logs">Server logs →</.link>
          </nav>
        </section>
      </main>
    </Layouts.app>
    """
  end
end
