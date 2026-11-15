defmodule LabbyConsoleWeb.GatewayLifecycleComponents do
  @moduledoc "Native confirmation, result notice and retained receipt for upstream lifecycle operations."
  use LabbyConsoleWeb, :html
  alias LabbyConsoleWeb.WorkspaceIcons, as: Icons

  attr :state, :map, required: true

  def status(assigns) do
    ~H"""
    <div :if={@state.lifecycle_notice || @state.lifecycle_receipt} class="gl-status">
      <p :if={@state.lifecycle_notice} id="gl-notice" role="status">{@state.lifecycle_notice}</p>
      <section
        :if={@state.lifecycle_receipt}
        id="gl-receipt"
        aria-label="Retained lifecycle receipt"
      >
        <p>
          Operation {@state.lifecycle_receipt.action} affects {@state.lifecycle_receipt.name}. Outcome {@state.lifecycle_receipt.outcome}. Receipt {inspect(
            @state.lifecycle_receipt.request_id
          )}.
        </p>
        <p>
          The local wait can end while backend work continues. Another Gateway change stays blocked until this receipt is cleared.
        </p>
        <button
          :if={@state.lifecycle_receipt.verifiable}
          id="gl-reconcile"
          type="button"
          phx-click="gateway_lifecycle_reconcile"
          disabled={!is_nil(@state.mutation_request)}
        >Inspect affected enablement</button>
        <button
          :if={!@state.lifecycle_receipt.verifiable}
          id="gl-acknowledge"
          type="button"
          phx-click="gateway_lifecycle_acknowledge"
          disabled={!is_nil(@state.mutation_request)}
        >Acknowledge without verification</button>
      </section>
    </div>
    """
  end

  attr :state, :map, required: true

  def dialog(assigns) do
    assigns =
      assign(assigns,
        lifecycle: assigns.state.lifecycle,
        busy: !is_nil(assigns.state.mutation_request)
      )

    ~H"""
    <div
      :if={@lifecycle}
      id="gl-backdrop"
      class="gm-backdrop"
      phx-remove={JS.focus(to: @lifecycle.opener)}
      phx-window-keydown="gateway_lifecycle_cancel"
      phx-key="Escape"
    >
      <.focus_wrap id="gl-focus" phx-mounted={JS.focus_first(to: "#gl-dialog")}>
        <section
          id="gl-dialog"
          class="gm-dialog"
          role="dialog"
          aria-modal="true"
          aria-labelledby="gl-title"
          aria-describedby="gl-description"
          aria-busy={to_string(@busy)}
        >
          <header>
            <h2 id="gl-title">{title(@lifecycle.mode)}</h2>
            <button
              id="gl-close"
              type="button"
              phx-click="gateway_lifecycle_cancel"
              aria-label="Close lifecycle dialog"
            ><Icons.icon name="close" /></button>
          </header>
          <p id="gl-description">{description(@lifecycle.mode, @lifecycle.name)}</p>
          <ul :if={@lifecycle.errors != []} id="gl-errors" class="gm-errors" role="alert">
            <li :for={error <- @lifecycle.errors}>{error}</li>
          </ul>
          <p :if={@busy} id="gl-pending" role="status">
            Waiting for the bounded Gateway operation. Closing ends this local wait. Backend work may continue.
          </p>
          <.form
            for={%{}}
            id="gl-form"
            phx-change="gateway_lifecycle_change"
            phx-submit="gateway_lifecycle_confirm"
            class="gm-form"
          >
            <label :if={@lifecycle.mode == :stop} class="gm-toggle" for="gl-cleanup"><input
              type="hidden"
              name="lifecycle[cleanup]"
              value="false"
            /><input
              id="gl-cleanup"
              type="checkbox"
              name="lifecycle[cleanup]"
              value="true"
              checked={@lifecycle.cleanup}
              disabled={@busy}
            />Also clean up this upstream's leftover host processes</label>
            <p :if={@lifecycle.mode == :stop}>
              Disabling stops new sessions from discovering this upstream. Cleanup additionally terminates its matching host processes.
            </p>
            <footer class="gm-footer">
              <button id="gl-cancel" type="button" phx-click="gateway_lifecycle_cancel">Cancel</button><button
                id="gl-confirm"
                type="button"
                class="gm-primary"
                phx-click="gateway_lifecycle_confirm"
                disabled={@busy}
              >{confirm_label(@lifecycle.mode)}</button>
            </footer>
          </.form>
        </section>
      </.focus_wrap>
    </div>
    """
  end

  @doc "Lifecycle controls available for one loaded fleet row."
  def controls(row) do
    cond do
      row["source"] != "custom_gateway" -> []
      row["enabled"] -> [{"stop", "Stop"}, {"reload", "Reload"}]
      true -> [{"start", "Start"}]
    end
  end

  def mode("start"), do: :start
  def mode("stop"), do: :stop
  def mode("reload"), do: :reload
  def mode(_), do: nil

  defp title(:start), do: "Start server"
  defp title(:stop), do: "Stop server"
  defp title(:reload), do: "Reload connection"

  defp confirm_label(:start), do: "Start server"
  defp confirm_label(:stop), do: "Stop server"
  defp confirm_label(:reload), do: "Reload connection"

  defp description(:start, name),
    do:
      "Enable #{name} so new sessions discover and proxy it again. Connection status is reported separately."

  defp description(:stop, name),
    do: "Disable #{name}. Its saved configuration is preserved."

  defp description(:reload, name),
    do:
      "Replace the current #{name} connection, clean up its stale processes and reconnect it. The console waits a bounded time for the Gateway to report completion."
end
