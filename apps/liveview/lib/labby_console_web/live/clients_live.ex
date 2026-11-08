defmodule LabbyConsoleWeb.ClientsLive do
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
            page_title: "Clients",
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
    |> start_async(:clients, fn -> LabbyConsole.Clients.list(id) end)
  end

  def handle_async(:clients, {:ok, {:ok, rows}}, socket),
    do: {:noreply, assign(socket, rows: rows, loading: false, error: nil)}

  def handle_async(:clients, {:ok, {:error, :unauthenticated}}, socket),
    do: {:noreply, socket |> assign(rows: []) |> redirect(to: "/sign-in")}

  def handle_async(:clients, _, socket),
    do:
      {:noreply,
       assign(socket,
         rows: [],
         loading: false,
         error: "Client observations are unavailable. Try again."
       )}

  def render(assigns) do
    ~H"""
    <Layouts.app flash={@flash} authenticated={true} page="Clients">
      <main id="console-integration" tabindex="-1">
        <h1>Observed MCP clients</h1>
        <p>Observation history, not connection liveness or a count of unique people.</p>
        <div class="console-session">
          <span id="identity">{@subject}</span><.form for={%{}} action="/sign-out" method="post">
            <button>Sign out</button>
          </.form>
        </div>
        <section class="console-panel" aria-busy={to_string(@loading)}>
          <header class="console-inventory-header">
            <h2>Client observations <small :if={!@loading && !@error}>{length(@rows)}</small></h2>
            <Layouts.refresh_action
              id="clients-refresh"
              label="Refresh"
              event="refresh"
              disabled={@loading}
            />
          </header>
          <p :if={@loading} role="status">Loading client observations…</p>
          <p :if={@error} id="clients-error" role="alert">{@error}</p>
          <p :if={!@loading && !@error && @rows == []} id="clients-empty">
            No client observations were returned.
          </p>
          <ul id="clients-list">
            <li :for={{row, index} <- Enum.with_index(@rows)} id={"client-#{index}"}>
              <div>
                <strong>{row["client_name"] || "Unnamed client"}</strong>
                <small :if={row["client_version"]}>{row["client_version"]}</small>
                <p>{row["transport"]} · {row["observation_count"]} observations</p>
                <details :if={row["subject"] || row["authorized_client_id"]}>
                  <summary>Identity details</summary>
                  <p :if={row["subject"]}>Subject tag: {row["subject"]}</p>
                  <p :if={row["authorized_client_id"]}>
                    Authorized client: {row["authorized_client_id"]}
                  </p>
                </details>
              </div>
              <div>
                <small>First observed</small>
                <time datetime={row["connected_at"]}>{row["connected_at"]}</time>
                <small>Last observed</small>
                <time :if={row["last_seen_at"]} datetime={row["last_seen_at"]}>{row["last_seen_at"]}</time>
                <span :if={!row["last_seen_at"]}>Not reported</span>
              </div>
            </li>
          </ul>
          <p class="console-panel-note">
            A recent observation does not guarantee the client is still connected. Dates are reported by the gateway.
          </p>
        </section>
      </main>
    </Layouts.app>
    """
  end
end
