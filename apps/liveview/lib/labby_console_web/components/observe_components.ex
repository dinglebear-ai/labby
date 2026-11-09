defmodule LabbyConsoleWeb.ObserveComponents do
  use LabbyConsoleWeb, :html
  alias LabbyConsoleWeb.WorkspaceIcons, as: Icons
  alias LabbyConsole.Observe
  attr :state, :map, required: true

  def page(assigns) do
    assigns =
      assign(
        assigns,
        :selected,
        Enum.find(assigns.state.rows, &(to_string(&1["id"]) == assigns.state.selected))
      )

    ~H"""
    <main
      id="observe-main"
      class="observe-main"
      phx-hook="ObservePreferences"
      data-preference-key={@state.preference_key}
      tabindex="-1"
    >
      <header class="ob-heading">
        <div class="ob-title">
          <span class="ob-page-glyph"><Icons.icon name="pulse" /></span><div>
            <h1>Observe</h1><p>Retained requests and server signals.</p>
          </div>
        </div>
        <button :if={@state.following} id="observe-pause" phx-click="pause" aria-label="Pause follow"><Icons.icon name="pause" /><span>Pause follow</span></button>
        <button
          :if={!@state.following}
          id="observe-resume"
          phx-click="resume"
          aria-label="Resume follow"
        ><Icons.icon name="play" /><span>Resume follow</span></button>
      </header>
      <div class="ob-summary" role="status">
        <span>Loaded snapshot</span><span :if={@state.loaded}>{@state.rows |> length()} retained records</span><span :if={
          !@state.loaded
        }>Awaiting authorized snapshot</span><span :if={@state.loading}>Refreshing…</span><span :if={
          @state.following
        }>Following every 5 seconds</span><span :if={!@state.following}>Follow paused</span>
      </div>
      <section class="ob-panel" aria-label="Observe signals">
        <nav class="ob-tabs" aria-label="Observe views">
          <button
            :for={
              {tab, label, icon} <- [
                {"activity", "Activity", "pulse"},
                {"logs", "Logs", "list"},
                {"traces", "Traces", "branch"}
              ]
            }
            id={"observe-tab-#{tab}"}
            phx-click="tab"
            phx-value-tab={tab}
            aria-current={if @state.tab == tab, do: "page"}
          ><Icons.icon name={icon} />{label}</button>
        </nav>
        <form id="observe-filter-form" phx-change="filter" phx-submit="filter" class="ob-filters">
          <label class="ob-search">Search
          <input
            type="search"
            name="filters[query]"
            value={@state.filters["query"] || ""}
            maxlength="256"
            placeholder={
              if @state.tab == "logs", do: "Search server logs…", else: "Search retained calls…"
            }
            phx-debounce="300"
          /></label>
          <label :if={@state.tab != "logs"}>Server
          <input
            name="filters[upstream]"
            value={@state.filters["upstream"] || ""}
            maxlength="256"
            placeholder="Exact upstream name"
            phx-debounce="300"
          /></label>
          <label :if={@state.tab != "logs"}>Outcome
          <select name="filters[outcome]"><option
            :for={
              {label, value} <- [
                {"All outcomes", ""},
                {"Succeeded", "ok"},
                {"Timeout", "timeout"},
                {"Upstream error", "upstream_error"},
                {"Tool error", "tool_error"}
              ]
            }
            value={value}
            selected={(@state.filters["outcome"] || "") == value}
          >
            {label}
          </option></select></label>
          <label :if={@state.tab != "logs"}>Window
          <select name="filters[window]"><option
            :for={window <- ~w(1h 24h 7d)}
            value={window}
            selected={@state.window == window}
          >
            {window}
          </option></select></label>
          <label :if={@state.tab == "logs"}>Level
          <select name="filters[level]"><option
            :for={level <- ["", "TRACE", "DEBUG", "INFO", "WARN", "ERROR"]}
            value={level}
            selected={(@state.filters["level"] || "") == level}
          >
            {if level == "", do: "All levels", else: level}
          </option></select></label>
          <button type="submit">Apply filters</button>
        </form>
        <div :if={@state.tab == "traces"} class="ob-notice" id="observe-trace-contract">
          Retained call metadata is available below. Trace graph, request/response IO, and log correlation require a caller-authorized backend trace contract.
        </div>
        <div :if={@state.error} class="ob-notice" role="alert" id="observe-error">
          {error(@state.error)} <button phx-click="refresh">Retry</button>
        </div>
        <div :if={@state.storage_warning} class="ob-notice" role="status">
          Browser preferences are unavailable. This view still works.
        </div>
        <div class="ob-results-heading">
          <strong>{String.capitalize(@state.tab)}</strong><span id="observe-result-count">{if @state.loaded,
            do: "#{length(@state.rows)} loaded records",
            else: "Awaiting snapshot"}</span>
        </div>
        <p :if={@state.rows == [] && !@state.loading && is_nil(@state.error)} class="ob-empty">
          {if @state.loaded,
            do: "No retained records match these filters.",
            else: "Loading authorized records…"}
        </p>
        <div :if={@state.tab != "logs" && @state.rows != []} class="ob-table-wrap">
          <table class="ob-table">
            <thead>
              <tr>
                <th>Operation</th><th>Source</th><th>Caller</th><th>Outcome</th><th>Duration</th><th>
                  Time · UTC
                </th>
              </tr>
            </thead><tbody>
              <tr :for={row <- @state.rows} id={"observe-row-#{row["id"]}"}>
                <td data-label="Operation">
                  <button
                    id={"observe-inspect-#{row["id"]}"}
                    phx-click="inspect"
                    phx-value-id={row["id"]}
                    aria-haspopup="dialog"
                  ><Icons.icon name="bolt" /><span><strong>{row["tool"]}</strong><small>{row[
                    "capability"
                  ]}/{row["operation"]}</small></span></button>
                </td>
                <td data-label="Source">{row["upstream"]}</td><td data-label="Caller">
                  {Observe.attribution(row)}
                </td><td data-label="Outcome">
                  <span class="ob-outcome" data-outcome={row["outcome"]}>{row["outcome"]}</span>
                </td><td data-label="Duration">{row["elapsed_ms"]} ms</td><td data-label="Time · UTC">
                  <time>{timestamp(row["ts_unix"])}</time>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
        <ol :if={@state.tab == "logs"} class="ob-logs" aria-label="Server log records">
          <li :for={row <- @state.rows} id={"observe-row-#{row["id"]}"}>
            <button
              id={"observe-inspect-#{row["id"]}"}
              phx-click="inspect"
              phx-value-id={row["id"]}
              aria-haspopup="dialog"
            ><time>{row["timestamp"] || "Unknown time"}</time><span class="ob-level">{row["level"] ||
              "Unknown level"}</span><span>{row["service"] || row["target"] || "Unknown source"}</span><pre>{row["message"] || ""}</pre></button>
          </li>
        </ol>
        <footer class="ob-footer">
          <span :if={@state.truncated}>Bounded snapshot. More records may exist.</span><span :if={
            !@state.truncated
          }>Loaded records only.</span><button phx-click="refresh" disabled={@state.loading}>Refresh</button><button
            :if={@state.cursor}
            id="observe-next"
            phx-click="next"
            disabled={@state.loading || @state.following}
          >Load next 25</button><button
            id="observe-export"
            phx-click="export"
            disabled={@state.loading || !@state.loaded || !is_nil(@state.error)}
          >Export loaded snapshot</button>
        </footer>
        <p :if={@state.export_error} role="alert" class="ob-notice">
          Export unavailable. Refresh or narrow the loaded snapshot.
        </p>
      </section>
      <section
        :if={@selected}
        id="observe-inspector"
        class="ob-inspector"
        role="dialog"
        aria-modal="true"
        aria-labelledby="observe-inspector-title"
        data-selected={@state.selected}
      >
        <header>
          <h2 id="observe-inspector-title">Retained record</h2><button
            id="observe-close"
            phx-click="close_inspector"
            aria-label="Close inspector"
          >Close</button>
        </header>
        <dl>
          <div :for={{key, value} <- details(@selected)}>
            <dt>{key}</dt><dd>{value}</dd>
          </div>
        </dl>
        <.link :if={Observe.server_link(@selected)} navigate={Observe.server_link(@selected)}>Open server runtime</.link>
        <p :if={@state.tab != "logs"}>
          Trace graph, request/response IO, and cross-record correlation are unavailable in the current backend contract.
        </p>
      </section>
    </main>
    """
  end

  defp error(:invalid_filters),
    do: "Filters must use supported values and at most 256 bytes per field."

  defp error(:forbidden), do: "Permission denied. This login cannot read these records."

  defp error(:cursor_repeated),
    do: "The gateway repeated a page cursor. Refresh to restart pagination."

  defp error(_),
    do:
      "Refresh unavailable. Retained records have unknown freshness. Resume manually after repeated failures."

  defp timestamp(unix) when is_integer(unix) do
    case DateTime.from_unix(unix) do
      {:ok, value} -> Calendar.strftime(value, "%Y-%m-%d %H:%M:%S")
      _ -> "Unknown time"
    end
  end

  defp timestamp(_), do: "Unknown time"

  defp details(row) do
    row
    |> Map.drop(~w(actor))
    |> Enum.sort_by(&elem(&1, 0))
    |> Enum.map(fn {key, value} ->
      {key, if(is_map(value), do: Jason.encode!(value), else: to_string(value))}
    end)
  end
end
