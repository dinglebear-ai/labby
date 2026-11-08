defmodule LabbyConsoleWeb.DiagnosticsLive do
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
            page_title: "Diagnostics",
            session_id: id,
            subject: user["user"]["sub"],
            rows: [],
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
    do: {:noreply, load(socket)}

  def handle_event("refresh", _, socket), do: {:noreply, socket}

  defp load(socket) do
    id = socket.assigns.session_id

    socket
    |> assign(loading: true, error: nil, rows: [])
    |> start_async(:checks, fn -> LabbyConsole.Diagnostics.checks(id) end)
  end

  def handle_async(:checks, {:ok, {:ok, rows}}, socket),
    do: {:noreply, assign(socket, rows: rows, loading: false, error: nil)}

  def handle_async(:checks, {:ok, {:error, :unauthenticated}}, socket),
    do: {:noreply, socket |> assign(rows: []) |> redirect(to: "/sign-in")}

  def handle_async(:checks, _, socket),
    do:
      {:noreply,
       assign(socket,
         rows: [],
         loading: false,
         error: "System checks are unavailable. Try again."
       )}

  def render(assigns) do
    ~H"""
    <Layouts.app flash={@flash} authenticated={true} page="Diagnostics">
      <main id="console-integration" tabindex="-1">
        <h1>Diagnostics</h1>
        <p>System checks from your gateway. Read-only findings; no repairs are performed.</p>
        <div class="console-session">
          <span id="identity">{@subject}</span><.form for={%{}} action="/sign-out" method="post">
            <button>Sign out</button>
          </.form>
        </div>
        <section class="console-panel" aria-busy={to_string(@loading)}>
          <header class="console-inventory-header">
            <h2>System findings</h2><Layouts.refresh_action
              id="diagnostics-refresh"
              label="Refresh"
              event="refresh"
              disabled={@loading}
            />
          </header>
          <p :if={@loading} role="status">Loading system checks…</p>
          <p :if={@error} id="diagnostics-error" role="alert">{@error}</p>
          <p :if={!@loading && !@error && @rows == []} id="diagnostics-empty">
            No system findings were returned.
          </p>
          <ul id="diagnostics-findings">
            <li :for={{row, index} <- Enum.with_index(@rows)} id={"diagnostic-#{index}"}>
              <div>
                <strong>{row["check"]}</strong><p>{row["message"]}</p><small>{row["service"]}</small>
              </div>
              <span class="console-status" data-state={row["severity"]}>{%{
                "ok" => "Passed",
                "warn" => "Warning",
                "fail" => "Failed"
              }[row["severity"]]}</span>
            </li>
          </ul>
        </section>
      </main>
    </Layouts.app>
    """
  end
end
