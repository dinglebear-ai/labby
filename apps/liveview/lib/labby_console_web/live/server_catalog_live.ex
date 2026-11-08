defmodule LabbyConsoleWeb.ServerCatalogLive do
  use LabbyConsoleWeb, :live_view
  alias LabbyConsole.{Auth, ServerCatalog}

  def mount(%{"id" => name}, session, socket) do
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
            page_title: "Server catalog",
            session_id: id,
            name: name,
            catalog: nil,
            loading: true,
            error: nil
          )

        {:ok, if(connected?(socket), do: load(socket), else: socket)}

      _ ->
        {:ok, redirect(socket, to: "/sign-in")}
    end
  end

  def handle_info(:session_expired, socket),
    do: {:noreply, socket |> assign(catalog: nil) |> redirect(to: "/sign-in")}

  def handle_event("retry", _, %{assigns: %{loading: false}} = socket),
    do: {:noreply, load(socket)}

  def handle_event("retry", _, socket), do: {:noreply, socket}

  defp load(socket) do
    id = socket.assigns.session_id
    name = socket.assigns.name

    socket
    |> assign(loading: true, catalog: nil, error: nil)
    |> start_async(:catalog, fn -> ServerCatalog.load(id, name) end)
  end

  def handle_async(:catalog, {:ok, {:ok, catalog}}, socket),
    do: {:noreply, assign(socket, catalog: catalog, loading: false, error: nil)}

  def handle_async(:catalog, {:ok, {:error, :unauthenticated}}, socket),
    do: {:noreply, socket |> assign(catalog: nil) |> redirect(to: "/sign-in")}

  def handle_async(:catalog, _, socket),
    do:
      {:noreply,
       assign(socket,
         catalog: nil,
         loading: false,
         error: "Server catalog is unavailable. Try again."
       )}

  def render(assigns) do
    ~H"""
    <Layouts.app flash={@flash} authenticated={true}>
      <main id="console-integration" tabindex="-1">
        <.link navigate="/gateway">Back to servers</.link>
        <h1>Server catalog</h1>
        <p id="catalog-server">{@name}</p>
        <section class="console-panel" aria-busy={to_string(@loading)}>
          <header class="console-inventory-header">
            <h2>Retained capabilities</h2>
            <Layouts.refresh_action
              id="catalog-retry"
              label="Reload catalog"
              event="retry"
              disabled={@loading}
            />
          </header>
          <p :if={@loading} id="catalog-loading" role="status">Loading catalog…</p>
          <p :if={@error} id="catalog-error" role="alert">{@error}</p>
          <div :if={@catalog} id="catalog-results">
            <h3>Tools</h3>
            <p :if={@catalog.tools == []} id="catalog-tools-empty">No retained tools.</p>
            <ul id="catalog-tools">
              <li :for={tool <- @catalog.tools}>
                <strong>{tool["name"]}</strong>
                <span>{if tool["exposed"], do: "Exposed", else: "Not exposed"}</span>
                <p :if={tool["description"]}>{tool["description"]}</p>
              </li>
            </ul>
            <h3>Resources</h3>
            <p :if={@catalog.resources == []}>No retained resources.</p>
            <ul id="catalog-resources">
              <li :for={uri <- @catalog.resources}>{uri}</li>
            </ul>
            <h3>Prompts</h3>
            <p :if={@catalog.prompts == []}>No retained prompts.</p>
            <ul id="catalog-prompts">
              <li :for={name <- @catalog.prompts}>{name}</li>
            </ul>
          </div>
        </section>
        <p>
          Retained snapshot. Reloading does not discover or reconnect upstream servers. Resource URIs are shown as text; no resource is fetched.
        </p>
        <p>Skills are not included by these server catalog endpoints.</p>
      </main>
    </Layouts.app>
    """
  end
end
