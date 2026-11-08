defmodule LabbyConsoleWeb.OverviewComponents do
  @moduledoc "Native Overview presentation backed only by validated gateway snapshots."
  use LabbyConsoleWeb, :html
  alias LabbyConsole.Overview
  alias LabbyConsoleWeb.WorkspaceIcons, as: Icons
  attr :state, :map, required: true

  def page(assigns) do
    cards = Map.new(Overview.cards(), &{&1.id, &1})
    assigns = assign(assigns, :cards, Enum.map(assigns.state.card_layout.order, &cards[&1]))

    ~H"""
    <main
      id="console-integration"
      class="ov-main ov-compact-page"
      tabindex="-1"
      phx-hook="OverviewLayout"
      data-preference-key={@state.preference_key}
      data-editing={to_string(@state.layout_editing)}
    >
      <div class="ov-page-heading ov-compact-heading labby-page-header">
        <div class="ov-page-title">
          <span class="ov-title-icon"><Icons.icon name="grid" /></span><div>
            <h1>Overview</h1><p>Usage, health, and runtime at a glance.</p>
          </div>
        </div>
        <div class="ov-page-actions">
          <button
            :if={issues(@state.inventory) != []}
            id="ov-attention-toggle"
            type="button"
            class="ov-button labby-icon-action"
            phx-click="toggle_attention"
            aria-label="Gateway notifications"
            title="Gateway notifications"
            aria-expanded={to_string(!@state.attention_dismissed)}
          ><Icons.icon name="alert" class="ov-small" /><span>{length(issues(@state.inventory))}</span></button>
          <button
            id="overview-refresh"
            type="button"
            class="ov-button labby-icon-action"
            phx-click="refresh"
            aria-label="Refresh overview"
            title="Refresh overview"
            disabled={@state.pending != []}
          ><Icons.icon name="refresh" class="ov-small" /></button>
          <details
            id="ov-view-options"
            class="ov-view-options"
            phx-mounted={JS.ignore_attributes("open")}
          >
            <summary
              class="ov-button labby-icon-action"
              aria-label="Overview options"
              title="Overview options"
            >
              <Icons.icon name="settings" class="ov-small" />
            </summary><div>
              <button
                id="ov-customize"
                type="button"
                class="ov-button"
                phx-click="overview_customize"
                aria-pressed={to_string(@state.layout_editing)}
              ><Icons.icon name="grid" class="ov-small" />{if @state.layout_editing,
                do: "Finish arranging cards",
                else: "Arrange cards"}</button>
              <button id="ov-layout-reset" type="button" class="ov-button" phx-click="overview_reset"><Icons.icon
                name="refresh"
                class="ov-small"
              />Reset card layout</button>
              <p>Tokens and cost are not reported by this gateway.</p>
            </div>
          </details>
          <.link
            id="ov-manage-gateway"
            navigate="/gateway"
            class="ov-button ov-primary labby-icon-action"
            aria-label="Open Gateway"
            title="Open Gateway"
          ><Icons.icon name="network" class="ov-small" /></.link>
        </div>
      </div>
      <div class="ov-toolbar">
        <div class="ov-periods" role="group" aria-label="Usage time window">
          <button
            :for={{value, label} <- windows()}
            id={"ov-window-#{value}"}
            type="button"
            class="ov-period"
            phx-click="window"
            phx-value-window={value}
            aria-label={label}
            title={label}
            aria-pressed={to_string(@state.window == value)}
          ><span class="labby-time-label">{label}</span><span class="labby-time-short">{value}</span></button>
        </div><span class="ov-toolbar-note"><span class="ov-window-context">Usage window · UTC</span><span class="ov-snapshot"><Icons.icon
          name="clock"
          class="ov-small"
        />Snapshot · {time(@state.snapshot)} UTC</span></span>
      </div>
      <p :if={@state.pending != []} class="ov-state-banner" role="status">
        Loading current gateway measurements…
      </p>
      <p :if={:usage in @state.errors} id="overview-usage-error" class="ov-state-banner" role="alert">
        Usage is unavailable. Refresh to retry.
      </p>
      <p
        :if={:inventory in @state.errors}
        id="overview-inventory-error"
        class="ov-state-banner"
        role="alert"
      >
        Inventory is unavailable. Refresh to retry.
      </p>
      <.metrics usage={@state.usage} inventory={@state.inventory} />
      <p :if={@state.storage_warning} class="ov-state-banner" role="status">
        Card changes work for this session; this browser could not save the layout.
      </p>
      <p
        id="ov-layout-status"
        class={if @state.layout_editing, do: "ov-layout-help", else: "ov-sr-only"}
        role="status"
        aria-live="polite"
      >
        {if @state.layout_notice == "",
          do:
            "Drag card handles within a lane, use Alt+Arrow Up/Down, or use the move buttons. Width and order are saved on this device.",
          else: @state.layout_notice}
      </p>
      <div
        :if={!@state.layout_editing && !@state.customized}
        id="ov-operational-layout"
        class="ov-operational-layout"
      >
        <section class="ov-telemetry-group" aria-label="Traffic and reliability">
          <.traffic state={@state} /><.latency usage={@state.usage} />
        </section>
        <.rankings state={@state} />
        <div class="ov-system-group"><.runtime state={@state} /><.inventory state={@state} /></div>
        <.recent_calls state={@state} />
      </div>
      <div
        :if={@state.layout_editing || @state.customized}
        id="ov-card-grid"
        class="ov-card-grid"
        data-editing={to_string(@state.layout_editing)}
      >
        <div
          :for={card <- @cards}
          id={"ov-card-#{card.id}"}
          class="ov-layout-card"
          data-overview-card={card.id}
          data-lane={card.lane}
          data-wide={to_string(Map.get(@state.card_layout.widths, card.id, false))}
          tabindex={if card.lane != "fixed", do: "0"}
          aria-label={card.label}
          style={"--overview-span: #{if Map.get(@state.card_layout.widths,card.id,false),do: 6,else: card.span}"}
        >
          <div :if={@state.layout_editing && card.lane != "fixed"} class="ov-card-controls">
            <span draggable="true" data-overview-handle={card.id} title="Drag within this lane"><Icons.icon
              name="grid"
              class="ov-small"
            />{card.label}</span>
            <button
              :for={direction <- ~w(earlier later)}
              id={"ov-card-#{direction}-#{card.id}"}
              type="button"
              class="ov-icon-button"
              phx-click="overview_move"
              phx-value-id={card.id}
              phx-value-direction={direction}
              aria-label={"Move #{card.label} #{direction}"}
            >{if direction == "earlier", do: "↑", else: "↓"}</button>
            <button
              :if={card.lane == "telemetry"}
              type="button"
              class="ov-icon-button"
              phx-click="overview_width"
              phx-value-id={card.id}
              aria-label={"Resize #{card.label}"}
              aria-pressed={to_string(Map.get(@state.card_layout.widths, card.id, false))}
            >↔</button>
          </div>
          <%= case card.id do %>
            <% "traffic" -> %>
              <.traffic state={@state} />
            <% "latency" -> %>
              <.latency usage={@state.usage} />
            <% "rankings" -> %>
              <.rankings state={@state} />
            <% "runtime" -> %>
              <.runtime state={@state} />
            <% "inventory" -> %>
              <.inventory state={@state} />
            <% "clients" -> %>
              <.clients clients={@state.clients} />
            <% "fleet" -> %>
              <.fleet inventory={@state.inventory} />
            <% "callers" -> %>
              <.leader_list state={@state} tab="callers" id="ov-callers" />
            <% "upstreams" -> %>
              <.leader_list state={@state} tab="servers" id="ov-upstreams" />
            <% "attention" -> %>
              <.attention inventory={@state.inventory} />
            <% "calls" -> %>
              <.recent_calls state={@state} />
          <% end %>
        </div>
      </div>
      <aside
        :if={
          issues(@state.inventory) != [] && !@state.attention_dismissed && !@state.layout_editing &&
            !@state.customized
        }
        id="ov-corner-attention"
        class="ov-corner-attention"
        aria-label="Gateway notifications"
      >
        <button
          class="ov-attention-dismiss"
          type="button"
          phx-click="dismiss_attention"
          aria-label="Dismiss notifications"
        ><Icons.icon name="close" class="ov-small" /></button><.attention inventory={@state.inventory} />
      </aside>
      <footer class="ov-page-footer">
        <span>Labby / Overview</span><span>Caller-scoped usage · current runtime snapshots</span>
      </footer>
      <.inspector :if={@state.selected} selected={@state.selected} />
    </main>
    """
  end

  attr :usage, :any, required: true
  attr :inventory, :any, required: true

  def metrics(assigns) do
    usage = assigns.usage

    assigns =
      assign(assigns, :cards, [
        %{
          id: "calls",
          label: "Total calls",
          value: number(usage && usage["window_total_calls"]),
          suffix: "",
          icon: "pulse",
          note:
            if(usage,
              do: "#{number(usage["distinct_tools"])} distinct tool identities",
              else: "Usage not available"
            ),
          tone: "neutral"
        },
        %{
          id: "success",
          label: "Success rate",
          value: decimal(Overview.success_rate(usage)),
          suffix: if(Overview.success_rate(usage), do: "%", else: ""),
          icon: "check-circle",
          note:
            if(usage,
              do: "#{number(usage["error_calls"])} failed calls",
              else: "Usage not available"
            ),
          tone: "green"
        },
        %{
          id: "p95",
          label: "P95 latency",
          value:
            if(usage && usage["total_calls"] > 0, do: number(usage["p95_elapsed_ms"]), else: "—"),
          suffix: if(usage && usage["total_calls"] > 0, do: "ms", else: ""),
          icon: "clock",
          note: "95% of calls complete within this time",
          tone: "neutral"
        },
        %{
          id: "connected",
          label: "Connected servers",
          value:
            if(assigns.inventory,
              do: number(Enum.count(assigns.inventory, & &1["connected"])),
              else: "—"
            ),
          suffix: if(assigns.inventory, do: "/ #{length(assigns.inventory)}", else: ""),
          icon: "server",
          note: "Enabled transports · current snapshot",
          tone: "neutral"
        }
      ])

    ~H"""
    <div id={if @usage, do: "overview-usage", else: "ov-metrics-unavailable"} class="ov-metrics">
      <section :for={card <- @cards} class="ov-metric" data-metric={card.id}>
        <div class="ov-metric-label">
          {card.label}<Icons.icon name={card.icon} class="ov-small" />
        </div><p class={"ov-metric-value ov-tone-#{card.tone}"}>
          <span id={"ov-#{card.id}-value"}>{card.value}</span><span class="ov-metric-suffix">{card.suffix}</span>
        </p><p class="ov-metric-note">{card.note}</p>
      </section>
    </div>
    """
  end

  attr :state, :map, required: true

  def traffic(assigns) do
    usage = assigns.state.usage
    series = (usage && usage["timeseries"]) || []

    maximum =
      Enum.max(
        Enum.map(
          series,
          &if(assigns.state.chart == "errors", do: &1["failed"], else: &1["calls"])
        ),
        fn -> 1 end
      )
      |> max(1)

    assigns = assign(assigns, series: series, maximum: maximum)

    ~H"""
    <section
      id="ov-traffic"
      class="ov-panel"
      data-mode={@state.chart_mode}
      aria-labelledby="ov-traffic-title"
    >
      <div class="ov-panel-heading">
        <div>
          <h2 id="ov-traffic-title" class="ov-sr-only">Gateway traffic</h2><.form
            for={%{}}
            phx-change="chart_mode"
            id="ov-chart-mode-form"
          >
            <label for="ov-chart-mode" class="ov-sr-only">Chart mode</label><select
              id="ov-chart-mode"
              name="chart_mode[mode]"
              class="ov-chart-mode"
            ><option
              :for={{value, label} <- chart_modes()}
              value={value}
              selected={value == @state.chart_mode}
            >
              {label}
            </option></select>
          </.form><p>Every capability. One view.</p>
        </div><div class="ov-segments" role="group" aria-label="Chart metric">
          <button
            :for={
              {value, label, icon} <- [
                {"calls", "All calls", "pulse"},
                {"errors", "Failures", "alert"}
              ]
            }
            id={"ov-chart-#{value}"}
            class="labby-icon-action"
            type="button"
            phx-click="chart"
            phx-value-chart={value}
            aria-label={label}
            title={label}
            aria-pressed={to_string(@state.chart == value)}
          ><Icons.icon name={icon} class="ov-small" /></button>
        </div>
      </div>
      <%= if @state.usage do %>
        <div class="ov-chart-summary">
          <strong>{number(
            @state.usage[if(@state.chart == "errors", do: "error_calls", else: "total_calls")]
          )}</strong><span>{if @state.chart == "errors",
            do: "failed calls",
            else: "calls in this window"}</span><span class="ov-chart-peak"><Icons.icon
            name="pulse"
            class="ov-small"
          />Peak <b>{number(@state.usage["peak_per_min"])}</b>
          / min</span>
        </div>
        <p
          :if={@state.chart_mode == "servers" && !Map.has_key?(@state.usage, "upstream_timeseries")}
          class="ov-panel-note"
        >
          Per-server bucket detail is not reported. Total volume remains visible.
        </p>
        <div class="ov-chart" data-chart={@state.chart}>
          <div class="ov-chart-axis">
            <span>{number(@maximum)}</span><span>{number(div(@maximum, 2))}</span><span>0</span>
          </div><div class="ov-chart-plot">
            <div class="ov-chart-guides"><i></i><i></i><i></i></div><div
              class="ov-chart-bars"
              role="group"
              aria-label="Calls per time bucket; tabular values follow"
            >
              <button
                :for={{bucket, index} <- Enum.with_index(@series)}
                id={"ov-bucket-#{index}"}
                type="button"
                class="ov-chart-column"
                phx-click={JS.push_focus() |> JS.push("inspect_bucket")}
                phx-value-index={index}
                aria-label={"#{time(bucket["ts_unix"])} UTC: #{bucket["calls"]} calls, #{bucket["failed"]} failed. Open time bucket."}
              >
                <span
                  class={[
                    "ov-chart-bar",
                    @state.chart == "errors" && "ov-chart-bar-error",
                    @state.chart_mode in ~w(servers outcomes errors) && "ov-chart-stack"
                  ]}
                  style={"--bar-height: #{percent(bucket[if(@state.chart=="errors",do: "failed",else: "calls")],@maximum)}%"}
                >
                  <span
                    :for={{segment, n} <- Enum.with_index(segments(bucket, @state))}
                    class="ov-series-segment"
                    data-series={n}
                    data-outcome={segment.kind}
                    style={"height: #{percent(segment.count,bucket["calls"])}%"}
                    title={"#{segment.label}: #{segment.count} calls"}
                  ></span>
                </span><span class="ov-chart-tooltip" aria-hidden="true"><strong>{time(
                  bucket["ts_unix"]
                )} UTC</strong><span>{number(bucket["calls"])} calls</span><small>{number(
                  bucket["failed"]
                )} failed</small></span>
              </button>
            </div>
          </div>
        </div>
        <p :if={@series == []} class="ov-empty">No retained calls in this window.</p>
        <div class="ov-chart-labels">
          <span>{bucket_date(List.first(@series), @state.window)}</span><span>{window_label(
            @state.window
          )} · UTC</span><span>{bucket_date(List.last(@series), @state.window)}</span>
        </div>
        <details id="ov-chart-data" class="ov-chart-data" phx-mounted={JS.ignore_attributes("open")}>
          <summary>
            <Icons.icon name="info" class="ov-small" />View Chart Data<Icons.icon
              name="chevron"
              class="ov-small ov-chart-disclosure"
            />
          </summary><div class="ov-table-scroll">
            <table class="ov-table">
              <caption>Calls per time bucket · UTC</caption><thead>
                <tr>
                  <th>Bucket start</th><th>Calls</th><th>Failures</th>
                </tr>
              </thead><tbody>
                <tr :for={bucket <- @series}>
                  <td>{time(bucket["ts_unix"])}</td><td>{number(bucket["calls"])}</td><td>
                    {number(bucket["failed"])}
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
        </details>
      <% else %>
        <p class="ov-empty">Usage measurements are unavailable.</p>
      <% end %>
    </section>
    """
  end

  attr :usage, :any, required: true

  def latency(assigns) do
    ~H"""
    <section id="ov-latency" class="ov-panel" aria-labelledby="ov-latency-title">
      <div class="ov-panel-heading">
        <div>
          <h2 id="ov-latency-title">Latency &amp; failures</h2><p>
            Across retained calls in the window.
          </p>
        </div><Icons.icon name="pulse" class="ov-small" />
      </div>
      <%= if @usage do %>
        <div class="ov-percentiles">
          <div :for={key <- ~w(p50 p95 p99)} data-percentile={String.upcase(key)}>
            <span>{String.upcase(key)}</span><strong>{if @usage["total_calls"] > 0,
              do: number(@usage[key <> "_elapsed_ms"]),
              else: "—"}<small :if={@usage["total_calls"] > 0}>ms</small></strong>
          </div>
        </div>
        <div class="ov-outcome-actions">
          <button
            :for={
              {outcome, label, count} <- [
                {"ok", "successful", @usage["total_calls"] - @usage["error_calls"]},
                {"failed", "failed", @usage["error_calls"]}
              ]
            }
            type="button"
            phx-click={JS.push_focus() |> JS.push("inspect_usage")}
            phx-value-outcome={outcome}
          >{number(count)} {label}</button>
        </div>
        <p class="ov-mean">
          Mean elapsed
          <b>{if @usage["total_calls"] > 0, do: decimal(@usage["avg_elapsed_ms"]) <> " ms", else: "—"}</b>
        </p><div class="ov-error-list">
          <div :for={row <- @usage["errors"] || []}>
            <span><Icons.icon
              name={if row["kind"] == "timeout", do: "clock", else: "alert"}
              class="ov-small"
            />{String.replace(row["kind"], "_", " ")}</span><strong>{number(row["calls"])}</strong>
          </div><p :if={@usage["errors"] == []} class="ov-empty">No recorded failures.</p><p
            :if={!Map.has_key?(@usage, "errors")}
            class="ov-empty"
          >
            Failure kinds not reported.
          </p>
        </div>
      <% else %>
        <p class="ov-empty">Usage measurements are unavailable.</p>
      <% end %>
      <p class="ov-panel-note">Percentiles describe elapsed time, not a latency histogram.</p>
    </section>
    """
  end

  attr :state, :map, required: true

  def rankings(assigns) do
    ~H"""
    <section
      id="ov-activity-hub"
      class="ov-activity-hub ov-panel"
      aria-labelledby="ov-activity-hub-title"
    >
      <header class="ov-hub-heading">
        <h2 id="ov-activity-hub-title">Activity leaders</h2><div
          class="ov-hub-tabs"
          role="group"
          aria-label="Activity ranking"
        >
          <button
            :for={
              {tab, label, icon} <- [
                {"tools", "Tools", "bolt"},
                {"servers", "Servers", "server"},
                {"callers", "Callers", "user"}
              ]
            }
            id={"ov-activity-#{tab}"}
            type="button"
            phx-click="activity_tab"
            phx-value-tab={tab}
            aria-pressed={to_string(@state.activity_tab == tab)}
          ><Icons.icon name={icon} class="ov-small" />{label}</button>
        </div>
      </header><.leader_list state={@state} tab={@state.activity_tab} id="ov-rankings" />
    </section>
    """
  end

  attr :state, :map, required: true
  attr :tab, :string, required: true
  attr :id, :string, required: true

  def leader_list(assigns) do
    rows = Overview.rankings(assigns.state.usage, assigns.tab, assigns.state.ranking)

    maximum =
      Enum.max(Enum.map(rows || [], &value(&1, assigns.state.ranking, assigns.tab)), fn -> 1 end)
      |> max(1)

    assigns = assign(assigns, rows: rows, maximum: maximum)

    ~H"""
    <section
      id={@id}
      class="ov-panel"
      aria-label={
        if @tab == "tools",
          do: "Tool activity",
          else: if(@tab == "callers", do: "Who's calling", else: "Most active servers")
      }
    >
      <div :if={@tab == "tools"} class="ov-ranking-tabs" role="group" aria-label="Tool ranking">
        <button
          :for={
            {rank, label, icon} <- [
              {"top", "Most used", "bolt"},
              {"least", "Least used", "pause"},
              {"slowest", "Slowest", "clock"}
            ]
          }
          id={"ov-ranking-#{rank}"}
          class="ov-ranking-control"
          type="button"
          phx-click="ranking"
          phx-value-ranking={rank}
          aria-pressed={to_string(@state.ranking == rank)}
        ><Icons.icon name={icon} class="ov-small" />{label}</button>
      </div>
      <p :if={@tab == "tools" && @state.ranking == "slowest"} class="ov-panel-note">
        Mean elapsed time per capability identity; this ranking is not P95.
      </p>
      <div class="ov-rank-list">
        <div
          :for={{row, index} <- Enum.with_index(@rows || [])}
          class="ov-rank-row"
          style={"--ov-rank-share: #{percent(value(row,@state.ranking,@tab),@maximum)}%"}
        >
          <span class="ov-rank-index">{String.pad_leading(Integer.to_string(index + 1), 2, "0")}</span><div>
            <button
              id={"#{@id}-open-#{index}"}
              type="button"
              class="ov-drill-link"
              phx-click={JS.push_focus() |> JS.push("inspect_leader")}
              phx-value-kind={@tab}
              phx-value-index={index}
            >{leader_label(row, @tab)}</button><small>{if @tab == "tools",
              do: row["upstream"],
              else: if(@tab == "callers", do: row["actor"], else: "Gateway server")}</small>
          </div><strong class="ov-mono">{if @tab == "tools" && @state.ranking == "slowest",
            do: decimal(row["avg_elapsed_ms"]) <> " ms",
            else: number(row["calls"]) <> " calls"}</strong>
        </div>
      </div>
      <p :if={is_nil(@rows)} class="ov-empty">Ranking measurements are not reported.</p><p
        :if={@rows == []}
        class="ov-empty"
      >
        {if @tab == "tools",
          do: "No tool activity in this window.",
          else: "No activity in this window."}
      </p>
      <p :if={@tab == "callers"} class="ov-panel-note">
        Caller identities are not unique people. Shared or unattributed actors remain distinct labels.
      </p>
    </section>
    """
  end

  attr :state, :map, required: true

  def runtime(assigns) do
    ~H"""
    <section id="ov-runtime" class="ov-panel" aria-labelledby="ov-runtime-title">
      <div class="ov-panel-heading">
        <div>
          <h2 id="ov-runtime-title">Runtime &amp; readiness</h2><p :if={@state.host}>
            {@state.host["hostname"] || "Host not reported"} · {@state.host["platform"]} · {number(
              @state.host["cpu_cores"]
            )} cores
          </p>
        </div><span id="ov-readiness" class="ov-status" data-state={readiness(@state.readiness)}>{String.replace(
          readiness(@state.readiness),
          "_",
          " "
        )}</span>
      </div>
      <%= if @state.host && @state.host["available"] do %>
        <div class="ov-runtime-stats">
          <div data-runtime="compute">
            <span><Icons.icon name="cpu" class="ov-small" />Labby CPU</span><strong>{decimal(
              @state.host["cpu_percent"]
            )}<small :if={!is_nil(@state.host["cpu_percent"])}>%</small></strong><small>Daemon + descendants</small>
          </div><div data-runtime="memory">
            <span><Icons.icon name="memory" class="ov-small" />Process memory</span><strong>{bytes(
              @state.host["memory_used_bytes"]
            )}</strong><small>of {bytes(@state.host["memory_total_bytes"])} {if @state.host[
                                                                                  "memory_limit_is_cgroup"
                                                                                ],
                                                                                do: "cgroup limit",
                                                                                else: "host memory"}</small>
          </div><div data-runtime="processes">
            <span><Icons.icon name="server" class="ov-small" />Child processes</span><strong>{number(
              @state.host["child_process_count"]
            )}</strong><small>{bytes(@state.host["child_rss_bytes"])} child RSS</small>
          </div>
        </div>
        <div class="ov-runtime-secondary">
          <div>
            <span><Icons.icon name="database" class="ov-small" />Data filesystem</span><strong>{bytes(
              @state.host["disk_used_bytes"]
            )} / {bytes(@state.host["disk_total_bytes"])}</strong>
          </div><div>
            <span><Icons.icon name="network" class="ov-small" />Network namespace</span><strong>↓ {bytes(
              @state.host["network_rx_bytes_per_second"]
            )}/s · ↑ {bytes(@state.host["network_tx_bytes_per_second"])}/s</strong>
          </div>
        </div>
      <% else %>
        <p class="ov-empty">Runtime measurements are unavailable. Missing values are not zero.</p>
      <% end %>
      <details id="ov-runtime-details" class="ov-section-disclosure">
        <summary>
          <Icons.icon name="cpu" class="ov-small" /><span>Runtime details</span><Icons.icon
            name="chevron"
            class="ov-small"
          />
        </summary><p class="ov-panel-note">
          {if @state.host,
            do: "#{number(@state.host["sample_ms"])} ms sample · #{@state.host["scope"]}",
            else: "No runtime sample available."} · Readiness is separate from upstream health.
        </p><.link navigate="/runtime" class="ov-panel-footer">Open runtime</.link>
      </details>
    </section>
    """
  end

  attr :state, :map, required: true

  def inventory(assigns) do
    assigns =
      assign(
        assigns,
        :families,
        Enum.map(
          [
            {"tools", "Tools", "bolt"},
            {"resources", "Resources", "folder"},
            {"prompts", "Prompts", "command"},
            {"skills", "Skills", "code"}
          ],
          fn {key, label, icon} ->
            {key, label, icon,
             if(assigns.state.inventory,
               do: Overview.capability_total(assigns.state.inventory, key),
               else: %{exposed: nil, discovered: nil, unknown: 0, stale: 0, failed: 0}
             )}
          end
        )
      )

    ~H"""
    <section
      id={if @state.inventory, do: "overview-inventory", else: "ov-inventory-unavailable"}
      class="ov-discovery-group"
      aria-label="Inventory and observed clients"
    >
      <div class="ov-discovery-heading">
        <p class="ov-section-eyebrow">Discovery &amp; connections</p><h2>Gateway inventory</h2>
      </div><div class="ov-capabilities">
        <div
          :for={{key, label, icon, totals} <- @families}
          id={"ov-capability-#{key}"}
          class="ov-capability"
          data-capability={key}
        >
          <span class="ov-cap-icon"><Icons.icon name={icon} /></span><div>
            <span class="ov-cap-label">{label}</span><p>
              <strong>{if totals.unknown > 0 && !is_nil(totals.exposed), do: "≥", else: ""}{number(
                totals.exposed
              )}</strong><span>exposed / {number(totals.discovered)} discovered</span>
            </p><small>{observation_note(totals)}</small>
          </div>
        </div>
      </div>
      <div class="ov-gateway-summary">
        <span>Gateway health</span><span>Configured {if @state.inventory,
          do: length(@state.inventory),
          else: "—"}</span><span>{if @state.inventory,
          do: "#{Enum.count(@state.inventory, & &1["connected"])} connected",
          else: "Unavailable"}</span><.link navigate="/gateway">View servers
        <Icons.icon name="arrow" class="ov-small" /></.link>
      </div>
      <details
        class="ov-section-disclosure"
        id="ov-observed-clients"
        phx-mounted={JS.ignore_attributes("open")}
      >
        <summary>
          <Icons.icon name="command" class="ov-small" /><span>Observed clients</span><small>{if @state.clients,
            do: length(@state.clients),
            else: "—"}</small><Icons.icon name="chevron" class="ov-small" />
        </summary><.clients clients={@state.clients} id="ov-inventory-clients" />
      </details>
      <details
        class="ov-section-disclosure"
        id="ov-configured-servers"
        phx-mounted={JS.ignore_attributes("open")}
      >
        <summary>
          <Icons.icon name="server" class="ov-small" /><span>Server connections</span><small>{if @state.inventory,
            do: length(@state.inventory),
            else: "—"}</small><Icons.icon name="chevron" class="ov-small" />
        </summary><.fleet inventory={@state.inventory} id="ov-inventory-fleet" />
      </details>
    </section>
    """
  end

  attr :clients, :any, required: true
  attr :id, :string, default: "ov-clients"

  def clients(assigns) do
    ~H"""
    <section id={@id} class="ov-panel" aria-label="Observed MCP clients">
      <div class="ov-panel-heading">
        <h2>Observed MCP clients</h2>
      </div><div class="ov-client-list">
        <div :for={client <- @clients || []} class="ov-client-row">
          <span class="ov-caller-avatar"><Icons.icon name="command" class="ov-small" /></span><div>
            <strong>{client["client_name"] || "Unreported client"}<small>{client["client_version"]}</small></strong><small>{client[
              "transport"
            ]} · {client["observation_count"]} observations</small>
          </div><span class="ov-client-seen">{client["last_seen_at"] || "Not reported"}<small>last seen</small></span>
        </div>
      </div><p :if={is_nil(@clients)} class="ov-empty">Client observations are unavailable.</p><p
        :if={@clients == []}
        class="ov-empty"
      >
        No client observations retained.
      </p><p class="ov-panel-note">
        Observation history does not guarantee that a client is still connected.
      </p>
    </section>
    """
  end

  attr :inventory, :any, required: true

  attr :id, :string, default: "ov-fleet"

  def fleet(assigns) do
    ~H"""
    <section id={@id} class="ov-panel" aria-label="Server connections">
      <div class="ov-server-list">
        <.link
          :for={row <- @inventory || []}
          navigate={"/gateway/servers/"<>URI.encode(row["id"],&URI.char_unreserved?/1)}
          class="ov-server-row"
        ><Icons.icon name="server" class="ov-small" /><strong>{row["display_name"] || row["name"]}</strong><span>{cond do
          row["connected"] -> "Connected"
          !row["enabled"] -> "Disabled"
          true -> "Disconnected"
        end}</span><Icons.icon name="chevron" class="ov-small" /></.link>
      </div><p :if={@inventory == []} class="ov-empty">No servers configured.</p><p
        :if={is_nil(@inventory)}
        class="ov-empty"
      >
        Server inventory is unavailable.
      </p>
    </section>
    """
  end

  attr :inventory, :any, required: true

  def attention(assigns) do
    assigns = assign(assigns, :issues, issues(assigns.inventory))

    ~H"""
    <section class="ov-panel" id="ov-attention">
      <div class="ov-panel-heading">
        <h2>Needs attention <span class="ov-count">{length(@issues)}</span></h2>
      </div><.link
        :for={row <- @issues}
        navigate={"/gateway/servers/"<>URI.encode(row["id"],&URI.char_unreserved?/1)}
        class="ov-issue"
      ><Icons.icon name="alert" class="ov-small" /><div>
        <strong>{row["display_name"] || row["name"]} is disconnected</strong><span>Enabled transport · inspect its current state</span>
      </div><Icons.icon name="arrow" class="ov-small" /></.link><p
        :if={@issues == []}
        class="ov-panel-note"
      >
        No disconnected enabled server in the current snapshot.
      </p>
    </section>
    """
  end

  attr :state, :map, required: true

  def recent_calls(assigns) do
    calls =
      Enum.filter(assigns.state.calls || [], fn row ->
        assigns.state.call_filter == "all" ||
          (assigns.state.call_filter == "ok" && row["outcome"] == "ok") ||
          (assigns.state.call_filter == "failed" && row["outcome"] != "ok")
      end)

    assigns = assign(assigns, :rows, calls)

    ~H"""
    <section id="ov-recent-calls" class="ov-panel">
      <div class="ov-panel-heading">
        <h2>Recent calls <span class="ov-count">{length(@rows)}</span></h2><div
          class="ov-segments"
          role="group"
          aria-label="Recent call outcome"
        >
          <button
            :for={{filter, label} <- [{"all", "All"}, {"ok", "Successful"}, {"failed", "Failed"}]}
            id={"ov-call-filter-#{filter}"}
            type="button"
            phx-click="call_filter"
            phx-value-filter={filter}
            aria-pressed={to_string(@state.call_filter == filter)}
          >{label}</button><.link navigate="/activity" aria-label="Open all retained calls"><Icons.icon
            name="grid"
            class="ov-small"
          /></.link>
        </div>
      </div><.call_table rows={@rows} prefix="ov-call" />
      <p :if={is_nil(@state.calls)} class="ov-empty">Retained calls are unavailable.</p><p
        :if={@state.calls != nil && @rows == []}
        class="ov-empty"
      >
        No matching retained calls in this loaded page.
      </p><p class="ov-panel-note">
        Latest 25 retained calls in this window. Filters apply to this loaded page; open Activity for further pages.
      </p>
    </section>
    """
  end

  attr :rows, :list, required: true
  attr :prefix, :string, required: true

  def call_table(assigns) do
    ~H"""
    <div class="ov-table-scroll">
      <table class="ov-table ov-calls-table">
        <caption class="ov-sr-only">Retained gateway calls</caption><thead>
          <tr>
            <th>Capability / operation</th><th>Server</th><th>Caller</th><th>Outcome</th><th>
              Duration
            </th><th>Time · UTC</th>
          </tr>
        </thead><tbody id={if @prefix == "ov-call", do: "ov-call-rows", else: @prefix <> "-rows"}>
          <tr :for={row <- @rows} id={"#{@prefix}-#{row["id"]}"}>
            <td>
              <button
                :if={@prefix == "ov-call"}
                id={"ov-call-open-#{row["id"]}"}
                type="button"
                class="ov-call-name"
                phx-click={JS.push_focus() |> JS.push("inspect_call")}
                phx-value-id={row["id"]}
              ><Icons.icon name="bolt" class="ov-small" />{row["tool"]}</button><strong :if={
                @prefix != "ov-call"
              }>{row["tool"]}</strong><small class="ov-call-kind">{row["capability"]} / {row[
                "operation"
              ]}{if row["subject_scoped"], do: " · subject scoped", else: ""}</small>
            </td><td>{row["upstream"]}</td><td>
              {get_in(row, ["attribution", "client_name"]) || row["actor"]}
            </td><td>
              <span :if={row["outcome"] == "ok"} class="ov-call-success" aria-label="Succeeded"><Icons.icon
                name="check"
                class="ov-small"
              /></span><span :if={row["outcome"] != "ok"} class="ov-outcome">{row["outcome"]}</span>
            </td><td class="ov-mono">{number(row["elapsed_ms"])} ms</td><td class="ov-mono ov-muted">
              {time(row["ts_unix"])}
            </td>
          </tr>
        </tbody>
      </table>
    </div>
    """
  end

  attr :selected, :map, required: true

  def inspector(assigns) do
    ~H"""
    <div
      id="ov-inspector-backdrop"
      class="ov-modal-backdrop"
      phx-remove={JS.pop_focus()}
      phx-window-keydown="close_inspector"
      phx-key="Escape"
    >
      <.focus_wrap id="ov-inspector-focus" phx-mounted={JS.focus_first(to: "#ov-inspector")}>
        <section
          id="ov-inspector"
          class="ov-inspector ov-activity-inspector ov-call-inspector"
          role="dialog"
          aria-modal="true"
          aria-labelledby="ov-inspector-title"
          phx-click-away="close_inspector"
        >
          <header>
            <div>
              <p class="ov-eyebrow">Caller-scoped usage</p><h2 id="ov-inspector-title">
                {@selected.title}
              </h2>
            </div><button
              id="ov-inspector-close"
              type="button"
              class="ov-icon-button"
              phx-click="close_inspector"
              aria-label="Close inspector"
            ><Icons.icon name="close" /></button>
          </header>
          <p :if={@selected.loading} role="status" class="ov-empty">Loading scoped usage…</p><p
            :if={@selected.error}
            role="alert"
            class="ov-state-banner"
          >
            {@selected.error}
          </p>
          <%= if @selected.call do %>
            <dl id="ov-call-attribution" class="ov-detail-list">
              <div :for={
                {label, key} <- [
                  {"Capability", "capability"},
                  {"Operation", "operation"},
                  {"Server", "upstream"},
                  {"Caller identity", "actor"},
                  {"Outcome", "outcome"},
                  {"Elapsed (ms)", "elapsed_ms"},
                  {"Response size (bytes)", "response_bytes"}
                ]
              }>
                <dt>{label}</dt><dd>{Map.get(@selected.call, key) || "Not reported"}</dd>
              </div><div>
                <dt>Recorded at (UTC)</dt><dd>{stamp(@selected.call["ts_unix"])}</dd>
              </div>
            </dl>
          <% end %>
          <%= if @selected.detail do %>
            <p class="ov-panel-note">
              {stamp(@selected.first)}–{stamp(@selected.last)} UTC · Filters: {filter_label(
                @selected.filters
              )}
            </p><div class="ov-percentiles">
              <div>
                <span>Scoped calls</span><strong>{number(@selected.detail.metrics["total_calls"])}</strong>
              </div><div>
                <span>Failed</span><strong>{number(@selected.detail.metrics["error_calls"])}</strong>
              </div><div>
                <span>Mean elapsed</span><strong>{if @selected.detail.metrics["total_calls"] > 0,
                  do: decimal(@selected.detail.metrics["avg_elapsed_ms"]),
                  else: "—"}<small>ms</small></strong>
              </div>
            </div><h3>Retained calls</h3><.call_table
              rows={@selected.detail.calls}
              prefix="ov-scoped-call"
            /><p :if={@selected.detail.calls == []} class="ov-empty">No matching retained calls.</p><p class="ov-panel-note">
              {if @selected.detail.next_cursor,
                do:
                  "Showing the first 25 matching retained calls; more pages are available in Activity.",
                else: "All returned retained calls are shown."}
            </p>
          <% end %>
          <nav class="ov-inspector-actions" aria-label="Related gateway views">
            <.link navigate="/activity" class="ov-button">Open Activity</.link><.link
              navigate="/usage"
              class="ov-button"
            >Open usage</.link>
          </nav>
        </section>
      </.focus_wrap>
    </div>
    """
  end

  defp windows,
    do: [{"1h", "Last hour"}, {"24h", "24 hours"}, {"7d", "7 days"}, {"30d", "30 days"}]

  defp window_label(value),
    do: Enum.find_value(windows(), value, fn {key, label} -> if key == value, do: label end)

  defp chart_modes,
    do: [
      {"volume", "Call volume"},
      {"servers", "By server"},
      {"outcomes", "By outcome"},
      {"errors", "Success & failures"},
      {"failures", "Failure volume"}
    ]

  defp issues(nil), do: []
  defp issues(rows), do: Enum.filter(rows, &(&1["enabled"] && !&1["connected"]))
  defp readiness(nil), do: "unavailable"
  defp readiness(value), do: value.status
  defp leader_label(row, "tools"), do: row["tool"]

  defp leader_label(row, "callers"),
    do: get_in(row, ["attribution", "client_name"]) || row["actor"]

  defp leader_label(row, _), do: row["upstream"]
  defp value(row, "slowest", "tools"), do: row["avg_elapsed_ms"]
  defp value(row, _, _), do: row["calls"]
  defp percent(_, 0), do: 0
  defp percent(value, total), do: min(100, max(0, value / total * 100))
  defp observation_note(%{exposed: nil}), do: "Not observed"

  defp observation_note(totals) do
    cond do
      totals.failed > 0 -> "#{totals.failed} failed observations"
      totals.stale > 0 -> "#{totals.stale} stale observations"
      totals.unknown > 0 -> "#{totals.unknown} unobserved servers"
      true -> "Current observations"
    end
  end

  defp segments(bucket, %{chart_mode: "errors"}),
    do: [
      %{kind: "succeeded", label: "Succeeded", count: bucket["calls"] - bucket["failed"]},
      %{kind: "upstream_error", label: "Failed", count: bucket["failed"]}
    ]

  defp segments(bucket, %{chart_mode: "outcomes"}) do
    rows =
      Enum.map(bucket["outcomes"] || [], fn row ->
        %{kind: row["kind"], label: row["kind"], count: row["calls"]}
      end)

    missing = max(0, bucket["calls"] - Enum.sum(Enum.map(rows, & &1.count)))

    if missing > 0,
      do: rows ++ [%{kind: "unknown", label: "Unclassified", count: missing}],
      else: rows
  end

  defp segments(bucket, %{chart_mode: "servers", usage: usage}) do
    rows =
      Enum.map(usage["upstream_timeseries"] || %{}, fn {name, series} ->
        row = Enum.find(series, &(&1["ts_unix"] == bucket["ts_unix"]))
        %{kind: "server", label: name, count: if(row, do: row["calls"], else: 0)}
      end)

    missing = max(0, bucket["calls"] - Enum.sum(Enum.map(rows, & &1.count)))

    if missing > 0,
      do: rows ++ [%{kind: "unknown", label: "Other or unreported servers", count: missing}],
      else: rows
  end

  defp segments(_, _), do: []
  defp bucket_date(nil, _), do: "—"

  defp bucket_date(row, window),
    do:
      DateTime.from_unix!(row["ts_unix"])
      |> Calendar.strftime(if window in ~w(1h 24h), do: "%H:%M", else: "%b %d")

  defp filter_label(filters) when map_size(filters) == 0,
    do: "All visible calls in this time range"

  defp filter_label(filters),
    do: Enum.map_join(filters, ", ", fn {key, value} -> "#{key}=#{value}" end)

  def number(nil), do: "—"

  def number(value) when is_integer(value),
    do: value |> Integer.to_string() |> String.replace(~r/\B(?=(\d{3})+(?!\d))/, ",")

  def decimal(nil), do: "—"
  def decimal(value), do: :erlang.float_to_binary(value / 1, decimals: 1)
  def bytes(nil), do: "—"
  def bytes(value) when value >= 1_073_741_824, do: decimal(value / 1_073_741_824) <> " GiB"
  def bytes(value) when value >= 1_048_576, do: number(round(value / 1_048_576)) <> " MiB"
  def bytes(value) when value >= 1024, do: number(round(value / 1024)) <> " KiB"
  def bytes(value), do: number(round(value)) <> " B"
  def time(value), do: DateTime.from_unix!(value) |> Calendar.strftime("%H:%M:%S")
  defp stamp(value), do: DateTime.from_unix!(value) |> Calendar.strftime("%b %d %H:%M:%S")
end
