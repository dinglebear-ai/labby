defmodule LabbyConsoleWeb.GatewayLive do
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
            page_title: "Gateway",
            session_id: id,
            subject: user["user"]["sub"],
            rows: [],
            query: "",
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

  def handle_event("search", %{"query" => query}, socket),
    do: {:noreply, assign(socket, query: String.slice(query, 0, 256))}

  defp visible_rows(rows, query) do
    query = query |> String.trim() |> String.downcase()

    Enum.filter(rows, fn row ->
      Enum.any?([row["name"], row["display_name"]], fn value ->
        is_binary(value) && String.contains?(String.downcase(value), query)
      end)
    end)
  end

  defp load(socket) do
    id = socket.assigns.session_id

    socket
    |> assign(loading: true, error: nil)
    |> start_async(:servers, fn -> Auth.servers(id) end)
  end

  def handle_async(:servers, {:ok, {:ok, rows}}, socket),
    do: {:noreply, assign(socket, rows: rows, loading: false, error: nil)}

  def handle_async(:servers, {:ok, {:error, :unauthenticated}}, socket),
    do: {:noreply, socket |> assign(rows: []) |> redirect(to: "/sign-in")}

  def handle_async(:servers, _, socket),
    do:
      {:noreply,
       assign(socket,
         rows: [],
         loading: false,
         error: "Gateway inventory is unavailable. Try again."
       )}

  def render(assigns) do
    ~H"""
    <Layouts.app flash={@flash} authenticated={true}>
      <main id="console-integration" tabindex="-1">
        <h1>Gateway</h1><p>Configured servers visible to your gateway session.</p>
        <div class="console-session">
          <span id="identity">{@subject}</span><.form for={%{}} action="/sign-out" method="post">
            <button>Sign out</button>
          </.form>
        </div>
        <section class="console-panel" aria-busy={to_string(@loading)}>
          <header class="console-inventory-header">
            <h2>
              Server inventory <small>{length(visible_rows(@rows, @query))} / {length(@rows)}</small>
            </h2><Layouts.refresh_action
              id="servers-refresh"
              label="Refresh"
              event="refresh"
              disabled={@loading}
            />
          </header>
          <.form for={%{}} id="servers-search" phx-change="search" role="search">
            <label for="servers-query" class="sr-only">Search servers</label>
            <input
              id="servers-query"
              type="search"
              name="query"
              value={@query}
              placeholder="Search servers…"
              aria-label="Search servers"
              phx-debounce="150"
            />
          </.form>
          <p
            :if={!@loading && !@error && @rows != [] && visible_rows(@rows, @query) == []}
            id="servers-no-matches"
          >
            No servers match your search.
          </p>
          <p :if={@loading} role="status">Loading servers…</p>
          <p :if={@error} id="servers-error" role="alert">{@error}</p>
          <p :if={!@loading && !@error && @rows == []} id="servers-empty">
            No servers are visible to this session.
          </p>
          <ul id="server-list">
            <li
              :for={row <- visible_rows(@rows, @query)}
              id={"server-" <> Base.url_encode64(row["id"], padding: false)}
            >
              <strong><.link navigate={"/gateway/servers/" <> URI.encode(row["id"], &URI.char_unreserved?/1)}>{row[
                "display_name"
              ] || row["name"]}</.link></strong><.link navigate={"/gateway/servers/" <> URI.encode(row["name"], &URI.char_unreserved?/1) <> "/catalog"}>Catalog</.link><.link navigate={"/gateway/upstreams/" <> URI.encode(row["name"], &URI.char_unreserved?/1) <> "/runtime"}>Runtime</.link><span
                class="console-status"
                data-state={
                  cond do
                    !row["enabled"] -> "disabled"
                    row["connected"] -> "connected"
                    true -> "disconnected"
                  end
                }
              >{cond do
                !row["enabled"] -> "Disabled"
                row["connected"] -> "Connected"
                true -> "Disconnected"
              end}</span>
            </li>
          </ul>
        </section>
        <p>
          Read-only snapshot. Refresh reloads the inventory without reconnecting upstream servers.
        </p>
      </main>
    </Layouts.app>
    """
  end
end
