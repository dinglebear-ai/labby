defmodule LabbyConsoleWeb.UsageCallsLive do
  use LabbyConsoleWeb, :live_view
  alias LabbyConsole.Auth

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
            page_title: "Activity",
            session_id: id,
            subject: user["user"]["sub"],
            rows: [],
            window: "24h",
            outcome: "all",
            until_unix: System.system_time(:second),
            next_cursor: nil,
            loading: false,
            error: nil
          )

        {:ok, if(connected?(socket), do: load(socket), else: socket)}

      _ ->
        {:ok, redirect(socket, to: "/sign-in")}
    end
  end

  def handle_info(:session_expired, socket),
    do: {:noreply, socket |> assign(rows: []) |> redirect(to: "/sign-in")}

  def handle_event("refresh", _, %{assigns: %{loading: false}} = socket),
    do: {:noreply, socket |> assign(until_unix: System.system_time(:second)) |> load()}

  def handle_event("refresh", _, socket), do: {:noreply, socket}

  def handle_event("filters", %{"window" => window, "outcome" => outcome}, socket)
      when window in ["1h", "24h", "7d"] and outcome in ["all", "ok", "timeout", "upstream_error"] do
    if socket.assigns.loading do
      {:noreply, socket}
    else
      {:noreply,
       socket
       |> assign(window: window, outcome: outcome, until_unix: System.system_time(:second))
       |> load()}
    end
  end

  def handle_event("filters", _, socket),
    do: {:noreply, put_flash(socket, :error, "Choose valid activity filters.")}

  def handle_event("next", _, %{assigns: %{loading: false, next_cursor: cursor}} = socket)
      when is_binary(cursor), do: {:noreply, load(socket, cursor)}

  def handle_event("next", _, socket), do: {:noreply, socket}

  defp load(socket, cursor \\ nil) do
    id = socket.assigns.session_id

    seconds = %{"1h" => 3600, "24h" => 86400, "7d" => 604_800}[socket.assigns.window]

    params = %{
      since_unix: socket.assigns.until_unix - seconds,
      until_unix: socket.assigns.until_unix
    }

    params =
      if socket.assigns.outcome == "all",
        do: params,
        else: Map.put(params, :outcome, socket.assigns.outcome)

    params = if cursor, do: Map.put(params, :cursor, cursor), else: params

    socket
    |> assign(loading: true, error: nil, rows: [], next_cursor: nil)
    |> start_async(:checks, fn -> LabbyConsole.UsageCalls.list(id, params) end)
  end

  def handle_async(:checks, {:ok, {:ok, page}}, socket),
    do:
      {:noreply,
       assign(socket, rows: page.calls, next_cursor: page.next_cursor, loading: false, error: nil)}

  def handle_async(:checks, {:ok, {:error, :unauthenticated}}, socket),
    do: {:noreply, socket |> assign(rows: []) |> redirect(to: "/sign-in")}

  def handle_async(:checks, _, socket),
    do:
      {:noreply,
       assign(socket,
         rows: [],
         loading: false,
         error: "Retained calls are unavailable. Try again."
       )}

  defp time(ts), do: DateTime.from_unix!(ts) |> Calendar.strftime("%b %d %H:%M:%S")

  def render(assigns) do
    ~H"""
    <Layouts.app flash={@flash} authenticated={true} page="Activity">
      <main id="console-integration" tabindex="-1">
        <h1>Activity</h1><p>Retained gateway calls visible to your session.</p>
        <section class="console-panel" aria-busy={to_string(@loading)}>
          <header class="console-inventory-header">
            <h2>Recent calls</h2><Layouts.refresh_action
              id="usage-calls-refresh"
              label="Refresh"
              event="refresh"
              disabled={@loading}
            />
          </header>
          <form id="usage-calls-filters" phx-change="filters" class="console-activity-filters">
            <label>Usage window<select aria-label="Usage window" name="window" disabled={@loading}><option
              :for={
                {label, value} <- [
                  {"Last hour", "1h"},
                  {"Last 24 hours", "24h"},
                  {"Last 7 days", "7d"}
                ]
              }
              value={value}
              selected={@window == value}
            >
              {label}
            </option></select></label>
            <label>Outcome<select aria-label="Outcome" name="outcome" disabled={@loading}><option
              :for={
                {label, value} <- [
                  {"All outcomes", "all"},
                  {"Succeeded", "ok"},
                  {"Timeout", "timeout"},
                  {"Upstream error", "upstream_error"}
                ]
              }
              value={value}
              selected={@outcome == value}
            >
              {label}
            </option></select></label>
          </form>
          <p :if={@loading} role="status">Loading retained calls…</p>
          <p :if={@error} id="usage-calls-error" role="alert">{@error}</p>
          <p :if={!@loading && !@error && @rows == []} id="usage-calls-empty">
            No retained calls match this window and outcome.
          </p>
          <div class="console-table-wrap">
            <table id="usage-calls-table" class="console-call-table">
              <caption class="sr-only">Retained calls; this page is not an aggregate total.</caption>
              <thead>
                <tr>
                  <th>Operation</th><th>Server</th><th>Actor</th><th>Outcome</th><th>Duration</th><th>
                    Time · UTC
                  </th>
                </tr>
              </thead>
              <tbody>
                <tr :for={row <- @rows}>
                  <td>
                    <strong>{row["tool"]}</strong><small>{row["capability"]} / {row["operation"]}</small>
                  </td><td>{row["upstream"]}</td><td>{row["actor"]}</td><td>
                    <span
                      class="console-status"
                      data-state={if row["outcome"] == "ok", do: "ok", else: "warn"}
                    >{row["outcome"]}</span>
                  </td><td>{row["elapsed_ms"]} ms</td><td>{time(row["ts_unix"])}</td>
                </tr>
              </tbody>
            </table>
          </div>
          <footer class="console-inventory-header">
            <span>{length(@rows)} retained calls on this page. Counts are not aggregate totals.</span><button
              id="usage-calls-next"
              phx-click="next"
              disabled={@loading || is_nil(@next_cursor)}
            >Next page</button>
          </footer>
        </section>
      </main>
    </Layouts.app>
    """
  end
end
