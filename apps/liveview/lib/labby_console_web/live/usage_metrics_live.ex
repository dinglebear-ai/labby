defmodule LabbyConsoleWeb.UsageMetricsLive do
  use LabbyConsoleWeb, :live_view
  alias LabbyConsole.{Auth, UsageMetrics}

  def mount(_, session, socket) do
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
            page_title: "Usage analytics",
            session_id: session_id,
            window: "24h",
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

  def handle_event("window", %{"window" => window}, socket) when window in ["1h", "24h", "7d"] do
    if socket.assigns.loading do
      {:noreply, socket}
    else
      {:noreply, socket |> assign(window: window) |> load()}
    end
  end

  def handle_event("window", _, socket), do: {:noreply, socket}

  def handle_info(:session_expired, socket), do: {:noreply, expire(socket)}

  defp load(socket) do
    session_id = socket.assigns.session_id
    window = socket.assigns.window

    socket
    |> assign(loading: true, row: nil, error: nil)
    |> start_async(:usage, fn -> UsageMetrics.fetch(session_id, window) end)
  end

  def handle_async(:usage, {:ok, {:ok, row}}, socket),
    do: {:noreply, assign(socket, row: row, loading: false)}

  def handle_async(:usage, {:ok, {:error, :unauthenticated}}, socket),
    do: {:noreply, expire(socket)}

  def handle_async(:usage, _, socket),
    do:
      {:noreply,
       assign(socket,
         row: nil,
         loading: false,
         error: "Usage analytics is unavailable. Try again."
       )}

  defp expire(socket), do: socket |> assign(row: nil) |> redirect(to: "/sign-in")

  def render(assigns) do
    ~H"""
    <Layouts.app flash={@flash} authenticated={true} page="Usage">
      <main id="console-integration" tabindex="-1">
        <h1>Usage analytics</h1>
        <p>Exact aggregate calls across your visible gateway upstreams.</p>
        <section class="console-panel" id="usage-metrics" aria-busy={to_string(@loading)}>
          <header class="console-inventory-header">
            <form id="usage-window-form" phx-change="window">
              <label for="usage-window">Usage window</label>
              <select id="usage-window" name="window" disabled={@loading}>
                <option
                  :for={
                    {value, label} <- [
                      {"1h", "Last hour"},
                      {"24h", "Last 24 hours"},
                      {"7d", "Last 7 days"}
                    ]
                  }
                  value={value}
                  selected={@window == value}
                >
                  {label}
                </option>
              </select>
            </form>
            <Layouts.refresh_action
              id="usage-refresh"
              label="Refresh usage"
              event="refresh"
              disabled={@loading}
            />
          </header>
          <p :if={@loading} id="usage-loading" role="status">Loading usage analytics…</p>
          <p :if={@error} id="usage-error" role="alert">{@error}</p>
          <div :if={@row} id="usage-content">
            <p :if={@row["total_calls"] == 0} id="usage-empty">No calls in this window.</p>
            <dl id="usage-summary">
              <div :for={
                {key, label} <- [
                  {"total_calls", "Calls"},
                  {"error_calls", "Failed"},
                  {"distinct_tools", "Distinct tools"},
                  {"distinct_actors", "Distinct callers"},
                  {"peak_per_min", "Peak calls / minute"}
                ]
              }>
                <dt>{label}</dt><dd>{@row[key]}</dd>
              </div>
            </dl>
            <h3>Elapsed time</h3>
            <dl id="usage-latency">
              <div :for={
                {key, label} <- [
                  {"avg_elapsed_ms", "Mean"},
                  {"p50_elapsed_ms", "P50"},
                  {"p95_elapsed_ms", "P95"},
                  {"p99_elapsed_ms", "P99"}
                ]
              }>
                <dt>{label}</dt><dd>{@row[key]} ms</dd>
              </div>
            </dl>
            <h3>Call volume · UTC</h3>
            <div class="console-table-scroll">
              <table id="usage-timeseries">
                <thead>
                  <tr>
                    <th>Bucket start · UTC</th><th>Calls</th><th>Failed</th>
                  </tr>
                </thead>
                <tbody>
                  <tr :for={bucket <- @row["timeseries"]}>
                    <td>
                      {DateTime.from_unix!(bucket["ts_unix"]) |> Calendar.strftime("%b %d %H:%M")}
                    </td><td>{bucket["calls"]}</td><td>{bucket["failed"]}</td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </section>
        <p>
          Read-only aggregate telemetry. Administrator access is required. Queries above 250,000 matching rows are unavailable; reduce the window and retry. Tokens and cost are not reported.
        </p>
      </main>
    </Layouts.app>
    """
  end
end
