defmodule LabbyConsoleWeb.ServerLogsLive do
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
            page_title: "Server logs",
            session_id: id,
            rows: [],
            truncated: false,
            loading: false,
            following: true,
            timer: nil,
            filters: %{"level" => "", "query" => ""},
            error: nil
          )

        {:ok, if(connected?(socket), do: load(socket), else: socket)}

      _ ->
        {:ok, redirect(socket, to: "/sign-in")}
    end
  end

  def handle_info(:session_expired, socket),
    do: {:noreply, socket |> stop() |> assign(rows: []) |> redirect(to: "/sign-in")}

  def handle_info(:poll, socket) do
    socket = assign(socket, timer: nil)

    {:noreply,
     if(socket.assigns.following && !socket.assigns.loading, do: load(socket), else: socket)}
  end

  def handle_event("follow", _, socket) do
    if socket.assigns.following do
      {:noreply, socket |> stop() |> assign(following: false)}
    else
      {:noreply, socket |> assign(following: true) |> load()}
    end
  end

  def handle_event("refresh", _, socket),
    do: {:noreply, if(socket.assigns.loading, do: socket, else: load(socket))}

  def handle_event("filter", %{"filters" => filters}, socket),
    do:
      {:noreply,
       socket |> stop() |> assign(filters: Map.take(filters, ~w(level query))) |> load()}

  defp stop(socket) do
    if socket.assigns.timer, do: Process.cancel_timer(socket.assigns.timer)
    socket |> cancel_async(:logs) |> assign(timer: nil, loading: false)
  end

  defp load(socket) do
    if socket.assigns.timer, do: Process.cancel_timer(socket.assigns.timer)
    id = socket.assigns.session_id
    filters = socket.assigns.filters

    socket
    |> assign(loading: true, error: nil, timer: nil)
    |> start_async(:logs, fn -> LabbyConsole.ServerLogs.query(id, filters) end)
  end

  def handle_async(:logs, {:ok, {:ok, report}}, socket) do
    timer = if socket.assigns.following, do: Process.send_after(self(), :poll, 2000), else: nil

    {:noreply,
     assign(socket,
       rows: report.entries,
       truncated: report.truncated,
       loading: false,
       error: nil,
       timer: timer
     )}
  end

  def handle_async(:logs, {:ok, {:error, :unauthenticated}}, socket),
    do: {:noreply, socket |> stop() |> assign(rows: []) |> redirect(to: "/sign-in")}

  def handle_async(:logs, _, socket),
    do:
      {:noreply,
       socket
       |> stop()
       |> assign(
         rows: [],
         error: "Logs unavailable. Administrator access is required. Try again.",
         following: false
       )}

  def terminate(_, socket) do
    if socket.assigns[:timer], do: Process.cancel_timer(socket.assigns.timer)
    :ok
  end

  def render(assigns) do
    ~H"""
    <Layouts.app flash={@flash} authenticated={true} page="Logs">
      <main id="console-integration" tabindex="-1">
        <h1>Server logs</h1>
        <p>
          Labby process logs · bounded polling every 2 seconds while following. Administrator access required.
        </p>
        <section class="console-panel" aria-busy={to_string(@loading)}>
          <header class="console-inventory-header">
            <h2>Log viewer</h2>
            <div class="console-actions">
              <button id="logs-follow" phx-click="follow" aria-pressed={to_string(@following)}>{if @following,
                do: "Pause",
                else: "Follow"}</button>
              <Layouts.refresh_action
                id="logs-refresh"
                label="Refresh"
                event="refresh"
                disabled={@loading}
              />
            </div>
          </header>
          <.form for={%{}} as={:filters} phx-submit="filter" id="logs-filters">
            <label>Search logs<input name="filters[query]" value={@filters["query"]} maxlength="256" /></label>
            <label for="logs-level">Level<select
              id="logs-level"
              aria-label="Level"
              name="filters[level]"
            ><option value="">All levels</option><option
              :for={level <- ~w(TRACE DEBUG INFO WARN ERROR)}
              value={level}
              selected={@filters["level"] == level}
            >
              {level}
            </option></select></label>
            <button>Apply filters</button>
          </.form>
          <p :if={@loading} role="status">Loading logs…</p>
          <p :if={@error} id="logs-error" role="alert">{@error}</p>
          <p :if={!@loading && !@error && @rows == []} id="logs-empty">
            No matching log entries in this bounded scan.
          </p>
          <div
            id="server-log-lines"
            class="console-logviewer"
            phx-hook="LogFollow"
            data-following={to_string(@following)}
            role="log"
            aria-label="Server log entries"
            aria-live="off"
            tabindex="0"
          >
            <div
              :for={{row, i} <- Enum.with_index(@rows)}
              id={"log-#{i}"}
              class="console-log-line"
              data-level={row["level"]}
            >
              <time>{row["timestamp"] || "—"}</time><span class="console-log-level">{row["level"] ||
                "—"}</span><span class="console-log-source">{row["service"] || row["target"] || "—"}</span><span class="console-log-message">{row[
                "message"
              ] || "(no message)"}</span>
            </div>
          </div>
          <footer>
            <span>{length(@rows)} / 200 lines</span><span :if={@truncated}> · Bounded scan; additional entries may exist.</span>
          </footer>
        </section>
      </main>
    </Layouts.app>
    """
  end
end
