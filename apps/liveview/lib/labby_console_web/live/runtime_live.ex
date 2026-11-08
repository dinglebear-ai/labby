defmodule LabbyConsoleWeb.RuntimeLive do
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
            page_title: "Runtime",
            session_id: id,
            subject: user["user"]["sub"],
            metrics: nil,
            loading: false,
            error: nil
          )

        {:ok, if(connected?(socket), do: load(socket), else: socket)}

      _ ->
        {:ok, redirect(socket, to: "/sign-in")}
    end
  end

  def handle_info(:session_expired, socket),
    do: {:noreply, socket |> assign(metrics: nil) |> redirect(to: "/sign-in")}

  def handle_event("refresh", _, %{assigns: %{loading: false}} = socket),
    do: {:noreply, load(socket)}

  def handle_event("refresh", _, socket), do: {:noreply, socket}

  defp load(socket) do
    id = socket.assigns.session_id

    socket
    |> assign(loading: true, error: nil, metrics: nil)
    |> start_async(:metrics, fn -> LabbyConsole.Runtime.metrics(id) end)
  end

  def handle_async(:metrics, {:ok, {:ok, metrics}}, socket),
    do: {:noreply, assign(socket, metrics: metrics, loading: false, error: nil)}

  def handle_async(:metrics, {:ok, {:error, :unauthenticated}}, socket),
    do: {:noreply, socket |> assign(metrics: nil) |> redirect(to: "/sign-in")}

  def handle_async(:metrics, _, socket),
    do:
      {:noreply,
       assign(socket,
         metrics: nil,
         loading: false,
         error: "Runtime sample is unavailable. Try again."
       )}

  def render(assigns) do
    ~H"""
    <Layouts.app flash={@flash} authenticated={true} page="Runtime">
      <main id="console-integration" tabindex="-1">
        <h1>Runtime</h1>
        <p>Daemon resources and its configured data filesystem. Read-only samples.</p>
        <div class="console-session">
          <span id="identity">{@subject}</span><.form for={%{}} action="/sign-out" method="post">
            <button>Sign out</button>
          </.form>
        </div>
        <section class="console-panel" aria-busy={to_string(@loading)}>
          <header class="console-inventory-header">
            <h2>Runtime resources</h2><Layouts.refresh_action
              id="runtime-refresh"
              label="Refresh"
              event="refresh"
              disabled={@loading}
            />
          </header>
          <p :if={@loading} role="status">
            Sampling runtime resources… This can take several seconds.
          </p>
          <p :if={@error} id="runtime-error" role="alert">{@error}</p>
          <div :if={@metrics} id="runtime-metrics">
            <p>
              <strong>{@metrics["hostname"] || "Host unavailable"}</strong>
              · {@metrics["platform"]} · {number(@metrics["cpu_cores"])} cores
            </p>
            <p :if={!@metrics["available"]} id="runtime-unsupported" role="status">
              Sampling is not available on this runtime.
            </p>
            <dl class="console-runtime-grid">
              <div>
                <dt>Labby CPU</dt><dd id="runtime-cpu">{percent(@metrics["cpu_percent"])}</dd><small>Daemon and descendants</small>
              </div>
              <div>
                <dt>Process memory</dt><dd>{bytes(@metrics["memory_used_bytes"])}</dd><small>of {bytes(
                  @metrics["memory_total_bytes"]
                )} {if @metrics["memory_limit_is_cgroup"], do: "cgroup limit", else: "host memory"}</small>
              </div>
              <div>
                <dt>Child processes</dt><dd>{number(@metrics["child_process_count"])}</dd><small>{bytes(
                  @metrics["child_rss_bytes"]
                )} child RSS</small>
              </div>
            </dl>
            <dl class="console-runtime-grid">
              <div>
                <dt>Data filesystem</dt><dd>{bytes(@metrics["disk_used_bytes"])}</dd><small>of {bytes(
                  @metrics["disk_total_bytes"]
                )}</small>
              </div>
              <div>
                <dt>Network namespace</dt><dd>
                  ↓ {rate(@metrics["network_rx_bytes_per_second"])}
                </dd><small>↑ {rate(@metrics["network_tx_bytes_per_second"])}</small>
              </div>
            </dl>
            <details>
              <summary>Sample scope</summary><p>{@metrics["scope"]}</p><p>
                Sample duration: {@metrics["sample_ms"]} ms. Network traffic is shared within the namespace; short-lived processes may be missed.
              </p>
            </details>
          </div>
        </section>
      </main>
    </Layouts.app>
    """
  end

  defp number(nil), do: "Unavailable"
  defp number(value), do: to_string(value)
  defp percent(nil), do: "Unavailable"
  defp percent(value), do: "#{Float.round(value / 1, 1)}%"
  defp rate(nil), do: "Unavailable"
  defp rate(value), do: "#{bytes(value)} /s"

  defp bytes(nil), do: "Unavailable"

  defp bytes(value) when value >= 1_073_741_824,
    do: "#{Float.round(value / 1_073_741_824, 1)} GiB"

  defp bytes(value) when value >= 1_048_576, do: "#{Float.round(value / 1_048_576, 1)} MiB"
  defp bytes(value) when value >= 1024, do: "#{Float.round(value / 1024, 1)} KiB"
  defp bytes(value), do: "#{value} B"
end
