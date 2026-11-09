defmodule LabbyConsoleWeb.GatewayFleetComponents do
  @moduledoc "Native fleet inventory, presentation controls and read inspector."
  use LabbyConsoleWeb, :html
  alias LabbyConsole.GatewayFleet, as: Fleet
  alias LabbyConsoleWeb.WorkspaceIcons, as: Icons

  attr :state, :map, required: true

  def page(assigns) do
    assigns = assign(assigns, :rows, Fleet.visible(assigns.state.rows, assigns.state.preferences))

    assigns =
      assign(
        assigns,
        :selected,
        Enum.find(assigns.state.rows, &(&1["id"] == assigns.state.selected))
      )

    ~H"""
    <main id="console-integration" class="gateway-fleet" tabindex="-1">
      <span id="identity" class="sr-only">{@state.subject}</span>
      <header class="gf-heading">
        <div class="gf-title">
          <span class="gf-title-icon"><Icons.icon name="server" /></span><div>
            <h1>Gateway</h1><p>Manage your connected MCP servers.</p>
          </div>
        </div>
        <.link navigate="/overview" class="gf-overview">Usage in Overview <Icons.icon name="arrow" /></.link>
      </header>
      <section class="gf-summary" aria-label="Loaded fleet summary">
        <span><i class="gf-dot"></i>{Enum.count(@state.rows, & &1["connected"])} connected</span>
        <span>{Enum.count(@state.rows, &(!&1["enabled"]))} disabled</span>
        <span class="gf-attention">{Enum.count(@state.rows, &Fleet.attention?/1)} need attention</span>
        <span class="gf-summary-total">{length(@state.rows)} loaded servers</span>
      </section>
      <section
        id="gateway-inventory"
        class="gf-inventory console-panel"
        data-layout={@state.preferences["layout"]}
        aria-busy={to_string(@state.loading)}
        aria-labelledby="gateway-inventory-title"
      >
        <h2 id="gateway-inventory-title" class="sr-only">Server inventory</h2>
        <div
          id="gateway-preferences"
          phx-hook="GatewayPreferences"
          data-preference-key={@state.preference_key}
        >
          <div class="gf-toolbar">
            <.form for={%{}} id="servers-search" phx-change="search" role="search">
              <label for="servers-query" class="sr-only">Search servers</label>
              <Icons.icon name="search" />
              <input
                id="servers-query"
                type="search"
                name="query"
                value={@state.preferences["query"]}
                placeholder="Search servers…"
                aria-label="Search servers"
                phx-debounce="150"
              />
            </.form>
            <details class="gf-menu" id="gateway-filter-menu">
              <summary><Icons.icon name="filter" /><span>Filters</span></summary>
              <div class="gf-popover">
                <.form for={%{}} id="gateway-filters" phx-change="filter">
                  <label for="gateway-lens">Fleet</label>
                  <select id="gateway-lens" name="lens"><option
                    :for={
                      {value, label} <- [
                        {"all", "All servers"},
                        {"enabled", "Enabled"},
                        {"disabled", "Disabled"},
                        {"connected", "Connected"},
                        {"disconnected", "Disconnected"},
                        {"attention", "Needs attention"}
                      ]
                    }
                    value={value}
                    selected={@state.preferences["lens"] == value}
                  >
                    {label}
                  </option></select>
                  <label for="gateway-source">Source</label>
                  <select id="gateway-source" name="source"><option
                    :for={
                      {value, label} <- [
                        {"all", "All sources"},
                        {"custom_gateway", "Custom"},
                        {"in_process", "Lab"},
                        {"unknown", "Unknown"}
                      ]
                    }
                    value={value}
                    selected={@state.preferences["source"] == value}
                  >
                    {label}
                  </option></select>
                </.form>
              </div>
            </details>
            <details class="gf-menu" id="gateway-view-menu">
              <summary><Icons.icon name="grid" /><span>View</span></summary>
              <div class="gf-popover gf-view-options">
                <div class="gf-layouts" role="group" aria-label="Inventory layout">
                  <button
                    :for={layout <- ~w(table cards list)}
                    type="button"
                    phx-click="layout"
                    phx-value-layout={layout}
                    aria-pressed={to_string(@state.preferences["layout"] == layout)}
                  >{String.capitalize(layout)}</button>
                </div>
                <.form for={%{}} id="gateway-sort" phx-change="sort">
                  <label for="gateway-sort-field">Sort by</label><select
                    id="gateway-sort-field"
                    name="sort"
                  ><option
                    :for={field <- ~w(name source enabled connected tools resources prompts skills)}
                    value={field}
                    selected={@state.preferences["sort"] == field}
                  >
                    {String.capitalize(field)}
                  </option></select>
                  <label for="gateway-sort-direction">Direction</label><select
                    id="gateway-sort-direction"
                    name="direction"
                  ><option
                    :for={direction <- ~w(ascending descending)}
                    value={direction}
                    selected={@state.preferences["direction"] == direction}
                  >
                    {String.capitalize(direction)}
                  </option></select>
                </.form>
                <fieldset>
                  <legend>Columns</legend><div
                    :for={
                      column <-
                        @state.preferences["columns"] ++
                          (Fleet.columns() -- @state.preferences["columns"])
                    }
                    class="gf-column-option"
                  >
                    <button
                      type="button"
                      phx-click="column"
                      phx-value-column={column}
                      aria-pressed={to_string(column in @state.preferences["columns"])}
                    >{String.capitalize(column)}</button>
                    <button
                      :if={column in @state.preferences["columns"]}
                      type="button"
                      phx-click="column_move"
                      phx-value-column={column}
                      phx-value-direction="earlier"
                      aria-label={"Move #{column} earlier"}
                    >↑</button>
                    <button
                      :if={column in @state.preferences["columns"]}
                      type="button"
                      phx-click="column_move"
                      phx-value-column={column}
                      phx-value-direction="later"
                      aria-label={"Move #{column} later"}
                    >↓</button>
                  </div>
                </fieldset>
              </div>
            </details>
            <Layouts.refresh_action
              id="servers-refresh"
              label="Refresh"
              event="refresh"
              disabled={@state.loading}
            />
            <span class="gf-row-count" aria-live="polite">{length(@rows)} / {length(@state.rows)}</span>
          </div>
        </div>
        <p :if={@state.storage_warning} role="status" class="gf-notice">
          View preferences cannot be saved in this browser.
        </p>
        <p :if={@state.loading} id="servers-loading" role="status" class="gf-notice">
          Loading servers…
        </p>
        <p :if={@state.error == :forbidden} id="servers-error" role="alert" class="gf-notice">
          <span id="servers-forbidden">Server inventory is unavailable with your current permissions.</span>
        </p>
        <p :if={@state.error == :unavailable} id="servers-error" role="alert" class="gf-notice">
          Gateway inventory is unavailable. Try Refresh again.
        </p>
        <p
          :if={!@state.loading && !@state.error && @state.rows == []}
          id="servers-empty"
          class="gf-notice"
        >
          No servers are visible to this session.
        </p>
        <p
          :if={!@state.loading && !@state.error && @state.rows != [] && @rows == []}
          id="servers-no-matches"
          class="gf-notice"
        >
          No servers match your search or filters.
        </p>
        <div class="gf-table-head" aria-hidden="true" style={grid(@state.preferences)}>
          <span>Server</span><span :for={column <- @state.preferences["columns"]}>{String.capitalize(
            column
          )}</span><span>Inspect</span>
        </div>
        <ul id="server-list" class="gf-rows">
          <li
            :for={row <- @rows}
            id={"server-" <> Base.url_encode64(row["id"], padding: false)}
            class={[@selected && @selected["id"] == row["id"] && "gf-selected"]}
            style={grid(@state.preferences)}
          >
            <div class="gf-server">
              <span class="gf-server-icon"><Icons.icon name={
                if row["source"] == "in_process", do: "server", else: "globe"
              } /></span><div>
                <strong><.link navigate={details_path(row)}>{Fleet.label(row)}</.link></strong><small>{row[
                  "name"
                ]}</small>
              </div>
            </div>
            <div
              :for={column <- @state.preferences["columns"]}
              class={"gf-cell gf-" <> column}
              data-column={column}
            >
              <%= case column do %>
                <% "source" -> %>
                  <span class="sr-only gf-cell-label">Source</span><span
                    class="gf-mobile-label"
                    aria-hidden="true"
                  >Source</span><span>{Fleet.source_label(row["source"])}</span>
                <% "status" -> %>
                  <span
                    class="gf-status"
                    data-state={if row["connected"], do: "connected", else: "disconnected"}
                  >{if row["connected"], do: "Connected", else: "Disconnected"}</span><small
                    :if={!row["enabled"]}
                    class="gf-disabled"
                    data-enabled="false"
                    data-state="disabled"
                  >Disabled</small>
                <% family -> %>
                  <span class="sr-only gf-cell-label">{String.capitalize(family)}</span><span
                    class="gf-mobile-label"
                    aria-hidden="true"
                  >{String.capitalize(family)}</span><span data-observation={
                    row["observations"][family]["state"]
                  }>{Fleet.measurement(row, family)}</span><small
                    :if={row["observations"][family]["state"] in ~w(stale failed)}
                    class="gf-attention"
                  >{String.capitalize(row["observations"][family]["state"])}</small>
              <% end %>
            </div>
            <div class="gf-row-actions">
              <button
                type="button"
                phx-click={inspect_action(@state.selected, row["id"])}
                phx-value-id={row["id"]}
                aria-label={"Quick inspect " <> Fleet.label(row)}
                aria-pressed={to_string(@state.selected == row["id"])}
              ><Icons.icon name="info" /><span>Inspect</span></button>
            </div>
          </li>
        </ul>
        <footer class="gf-inventory-footer">
          <span>{length(@rows)} of {length(@state.rows)} servers shown</span><span>Exposed / discovered</span>
        </footer>
      </section>
      <section
        :if={@selected}
        id="gateway-inspector"
        class="gf-inspector"
        aria-labelledby="gateway-inspector-title"
        phx-mounted={JS.focus(to: "#gateway-inspector-title")}
        phx-remove={JS.pop_focus()}
        phx-window-keydown="close_inspector"
        phx-key="Escape"
      >
        <header>
          <div>
            <h2 id="gateway-inspector-title" tabindex="-1">{Fleet.label(@selected)}</h2><p>
              {@selected["name"]}
            </p>
          </div><button type="button" phx-click="close_inspector" aria-label="Close server inspector"><Icons.icon name="close" /></button>
        </header>
        <dl>
          <div>
            <dt>Source</dt><dd>{Fleet.source_label(@selected["source"])}</dd>
          </div><div>
            <dt>Enabled</dt><dd>{if @selected["enabled"], do: "Yes", else: "No"}</dd>
          </div><div>
            <dt>Connection</dt><dd>
              {if @selected["connected"], do: "Connected", else: "Disconnected"}
            </dd>
          </div><div :for={family <- ~w(tools resources prompts skills)}>
            <dt>{String.capitalize(family)}</dt><dd>
              {Fleet.measurement(@selected, family)}
              <small>{String.capitalize(@selected["observations"][family]["state"])}</small>
            </dd>
          </div>
        </dl>
        <nav aria-label="Selected server sections">
          <.link navigate={details_path(@selected)}>Open server</.link><.link navigate={"/gateway/servers/" <> segment(@selected["name"]) <> "/catalog"}>Catalog</.link><.link navigate={"/gateway/upstreams/" <> segment(@selected["name"]) <> "/runtime"}>Runtime</.link>
        </nav>
      </section>
    </main>
    """
  end

  defp segment(value), do: URI.encode(value, &URI.char_unreserved?/1)
  defp details_path(row), do: "/gateway/servers/" <> segment(row["id"])

  defp grid(prefs),
    do:
      "--gf-columns: minmax(160px, 2fr) repeat(#{length(prefs["columns"])}, minmax(70px, 1fr)) 76px"

  defp inspect_action(nil, id), do: JS.push_focus() |> JS.push("inspect", value: %{id: id})
  defp inspect_action(_, id), do: JS.push("inspect", value: %{id: id})
end
