defmodule LabbyConsoleWeb.IntegrationLive do
  @moduledoc "Read-only public readiness slice. No sample state or implicit gateway authority."
  use LabbyConsoleWeb, :live_view
  alias LabbyConsole.Gateway

  def mount(_params, _session, socket) do
    target = Application.get_env(:labby_console, :gateway_url)

    socket =
      assign(socket, page_title: "Labby Console", target: target, result: nil, checking: false)

    {:ok, if(connected?(socket) && is_binary(target), do: check(socket), else: socket)}
  end

  def handle_event("retry", _, %{assigns: %{checking: false, target: target}} = socket)
      when is_binary(target),
      do: {:noreply, check(socket)}

  def handle_event("retry", _, socket), do: {:noreply, socket}

  defp check(socket) do
    target = socket.assigns.target

    socket
    |> assign(checking: true)
    |> start_async(:readiness, fn -> Gateway.readiness(target) end)
  end

  def handle_async(:readiness, {:ok, result}, socket),
    do: {:noreply, assign(socket, result: result, checking: false)}

  def handle_async(:readiness, {:exit, _}, socket),
    do: {:noreply, assign(socket, result: {:error, :unavailable}, checking: false)}

  def render(assigns) do
    ~H"""
    <Layouts.app flash={@flash}>
      <main id="console-integration" tabindex="-1">
        <h1>Gateway readiness</h1>
        <p>Readiness is separate from upstream server connections.</p>
        <p :if={!@target} id="gateway-configuration" role="status">
          Configure a gateway target to begin integration.
        </p>
        <section
          :if={@target}
          class="console-panel"
          aria-label="Gateway readiness"
          aria-busy={to_string(@checking)}
        >
          <p
            id="gateway-status"
            data-state={if(@checking, do: "checking", else: state(@result))}
            role="status"
          >
            {status_label(@checking, @result)}
          </p>
          <ul :if={match?({:ok, %{details: _}}, @result)} id="gateway-details">
            <li :for={detail <- elem(@result, 1).details}>{detail}</li>
          </ul>
          <button id="gateway-retry" type="button" phx-click="retry" disabled={@checking}>Check again</button>
        </section>
        <p>
          Public readiness only. <.link href="/sign-in">Sign in</.link>
          to view protected gateway features.
        </p>
      </main>
    </Layouts.app>
    """
  end

  defp state({:ok, %{status: status}}), do: status
  defp state(_), do: "unavailable"

  defp status_label(true, _), do: "Checking gateway…"
  defp status_label(_, {:ok, %{status: "ready"}}), do: "Gateway ready"
  defp status_label(_, {:ok, %{status: "degraded"}}), do: "Gateway degraded"
  defp status_label(_, {:ok, %{status: "not_ready"}}), do: "Gateway not ready"
  defp status_label(_, {:error, _}), do: "Gateway unavailable"
  defp status_label(_, _), do: "Waiting to check gateway"
end
