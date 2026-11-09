defmodule LabbyConsoleWeb.GatewayMutationComponents do
  @moduledoc "Native Aurora server drafts, local JSON and named removal confirmation."
  use LabbyConsoleWeb, :html
  alias LabbyConsoleWeb.WorkspaceIcons, as: Icons

  attr :state, :map, required: true

  def status(assigns) do
    ~H"""
    <div :if={@state.mutation_notice || @state.mutation_receipt} class="gm-status">
      <p :if={@state.mutation_notice} id="gm-notice" role="status">{@state.mutation_notice}</p>
      <section :if={@state.mutation_receipt} id="gm-receipt" aria-label="Retained operation receipt">
        <p>
          Operation {@state.mutation_receipt.action} affects {Enum.join(
            @state.mutation_receipt.names,
            ", "
          )}. Outcome {@state.mutation_receipt.outcome}. Receipt {inspect(
            @state.mutation_receipt.request_id
          )}.
        </p>
        <p>
          The local wait can end while backend work continues. Inspect the affected configuration before another change.
        </p>
        <button
          id="gm-reconcile"
          type="button"
          phx-click="gateway_reconcile"
          disabled={!is_nil(@state.mutation_request)}
        >Inspect affected configuration</button>
      </section>
    </div>
    <span
      :if={@state.mutation_restore}
      id="gm-focus-restore"
      phx-mounted={JS.focus(to: @state.mutation_restore)}
    ></span>
    """
  end

  attr :state, :map, required: true

  def dialog(assigns) do
    mutation = assigns.state.mutation

    opener =
      restore_target(
        LabbyConsole.GatewayFleet.visible(assigns.state.rows, assigns.state.preferences),
        assigns.state.selected,
        mutation.opener
      )

    assigns =
      assign(assigns,
        mutation: mutation,
        busy: !is_nil(assigns.state.mutation_request) || !is_nil(assigns.state.mutation_receipt),
        opener: opener
      )

    ~H"""
    <div
      :if={@mutation.dialog}
      id="gm-backdrop"
      class="gm-backdrop"
      phx-remove={JS.focus(to: @opener)}
      phx-window-keydown="gateway_close"
      phx-key="Escape"
    >
      <.focus_wrap id="gm-focus" phx-mounted={JS.focus_first(to: "#gm-dialog")}>
        <section
          id="gm-dialog"
          class="gm-dialog"
          role="dialog"
          aria-modal="true"
          aria-labelledby="gm-title"
          aria-describedby="gm-description"
          aria-busy={to_string(@busy)}
        >
          <header>
            <h2 id="gm-title">{title(@mutation)}</h2>
            <div class="gm-header-actions">
              <button
                :if={@mutation.dialog == :form}
                id="gm-json"
                type="button"
                phx-click="gateway_json"
                disabled={@busy}
              >JSON config</button>
              <button
                id="gm-close"
                type="button"
                phx-click="gateway_close"
                aria-label="Close server dialog"
              ><Icons.icon name="close" /></button>
            </div>
          </header>
          <p id="gm-description">{description(@mutation)}</p>
          <ul :if={@mutation.errors != []} id="gm-errors" class="gm-errors" role="alert">
            <li :for={error <- @mutation.errors}>{error}</li>
          </ul>
          <p :if={@busy} id="gm-pending" role="status">
            Waiting for the bounded Gateway operation. Close ends this local wait. Backend work may continue.
          </p>
          <%= cond do %>
            <% @mutation.dialog == :loading -> %>
              <p>Reading the selected server configuration…</p>
            <% @mutation.dialog == :remove -> %>
              <.form for={%{}} id="gm-remove-form" phx-submit="gateway_confirm_remove" class="gm-form">
                <p>
                  Remove <strong>{@mutation.model.original}</strong> from the Gateway configuration.
                </p>
                <.field
                  id="gm-confirm-name"
                  name="confirmation[name]"
                  value=""
                  label="Type the exact server ID to confirm"
                  disabled={@busy}
                  autocomplete="off"
                />
                <footer class="gm-footer">
                  <button type="button" phx-click="gateway_close">Cancel</button><button
                    id="gm-remove"
                    type="submit"
                    class="gm-danger"
                    disabled={@busy}
                  >Remove server</button>
                </footer>
              </.form>
            <% @mutation.json_open -> %>
              <.form
                for={%{}}
                id="gm-json-form"
                phx-change="gateway_json_change"
                phx-submit="gateway_apply_json"
                class="gm-form"
              >
                <h3>Client JSON configuration</h3>
                <p>
                  Paste exactly one server, either inside mcpServers, a one-server map, or a configuration with its name. Applying JSON changes this draft locally.
                </p>
                <.field
                  id="gm-json-input"
                  name="config[json]"
                  value={@mutation.json}
                  label="JSON configuration"
                  type="textarea"
                  rows={14}
                  disabled={@busy}
                  autocomplete="off"
                />
                <footer class="gm-footer">
                  <button id="gm-json-back" type="button" phx-click="gateway_json" disabled={@busy}>Back to form</button><button
                    id="gm-json-apply"
                    type="submit"
                    class="gm-primary"
                    disabled={@busy}
                  >Apply JSON to form</button>
                </footer>
              </.form>
            <% true -> %>
              <.server_form mutation={@mutation} busy={@busy || !is_nil(@state.mutation_receipt)} />
          <% end %>
        </section>
      </.focus_wrap>
    </div>
    """
  end

  attr :mutation, :map, required: true
  attr :busy, :boolean, required: true

  def server_form(assigns) do
    assigns =
      assign(assigns,
        draft: assigns.mutation.model.draft,
        editing: assigns.mutation.model.mode == :edit
      )

    ~H"""
    <.form
      for={%{}}
      id="gm-server-form"
      phx-change="gateway_form_change"
      phx-submit="gateway_save"
      class="gm-form"
      autocomplete="off"
    >
      <fieldset disabled={@busy}>
        <legend class="sr-only">Server configuration</legend>
        <section class="gm-section">
          <h3><Icons.icon name="server" />Server identity</h3>
          <div class="gm-grid">
            <.field
              id="gm-name"
              name="server[name]"
              value={@draft["name"]}
              label="ID"
              maxlength={128}
            />
            <div>
              <.field
                :if={@editing}
                id="gm-display-intent"
                name="server[display_name_intent]"
                value={@draft["display_name_intent"]}
                label="Display name update"
                type="select"
                options={[
                  {"Preserve saved label", "preserve"},
                  {"Replace label", "replace"},
                  {"Clear label", "clear"}
                ]}
              />
              <.field
                :if={!@editing || @draft["display_name_intent"] == "replace"}
                id="gm-display-name"
                name="server[display_name]"
                value={@draft["display_name"]}
                label="Display name"
                maxlength={80}
              />
            </div>
          </div>
          <.toggle field="enabled" value={@draft["enabled"]} label="Enable server" />
          <fieldset :if={!@editing} class="gm-segments">
            <legend>Transport</legend><label :for={
              {value, label} <- [{"network", "HTTP / WebSocket"}, {"stdio", "stdio"}]
            }><input
              type="radio"
              name="server[transport]"
              value={value}
              checked={@draft["transport"] == value}
            /><span>{label}</span></label>
          </fieldset>
          <p :if={@editing} id="gm-transport-note">
            The Gateway read view omits explicit transport and socket metadata. Connection fields below preserve that metadata. Changing transport is unavailable here.
          </p>
          <.field
            :if={
              (!@editing && @draft["transport"] == "network") ||
                (@editing && @mutation.model.connection == :network)
            }
            id="gm-url"
            name="server[url]"
            value={@draft["url"]}
            label={if(@editing, do: "Configured endpoint", else: "Endpoint URL")}
            type="url"
          />
          <div :if={
            (!@editing && @draft["transport"] == "stdio") ||
              (@editing && @mutation.model.connection == :stdio)
          }>
            <.field id="gm-command" name="server[command]" value={@draft["command"]} label="Command" />
            <.field
              id="gm-args"
              name="server[args]"
              value={@draft["args"]}
              label="Arguments as a JSON array"
              type="textarea"
              rows={3}
            />
            <p>Each array item is one argument. Spaces and empty arguments are preserved.</p>
          </div>
        </section>
        <details id="gm-authentication" class="gm-section" phx-hook=".Disclosure">
          <summary><Icons.icon name="shield" />Authentication and private configuration</summary>
          <p :if={@mutation.model.oauth}>
            Saved OAuth configuration is preserved. Authorization and OAuth policy changes use the separate OAuth workflow.
          </p>
          <.field
            :if={!@mutation.model.oauth}
            id="gm-bearer-env"
            name="server[bearer_token_env]"
            value={@draft["bearer_token_env"]}
            label="Bearer token environment reference"
          />
          <.field
            :if={!@mutation.model.oauth}
            id="gm-bearer-value"
            name="server[bearer_token_value]"
            value={@draft["bearer_token_value"]}
            label="Write-only bearer token"
            type="password"
            autocomplete="new-password"
          />
          <p>
            Saved private values stay hidden. A blank token preserves the saved value. Pasted tokens can be saved explicitly. Draft probes require an environment reference.
          </p>
          <.private_map
            :for={
              {field, label} <- [
                {"env", "Environment variables"},
                {"headers", "Literal HTTP headers"},
                {"headers_env", "Environment-backed HTTP headers"}
              ]
            }
            field={field}
            label={label}
            draft={@draft}
          />
        </details>
        <details id="gm-exposure" class="gm-section" phx-hook=".Disclosure">
          <summary><Icons.icon name="settings" />Exposure surfaces</summary>
          <.toggle
            :for={
              {field, label} <- [
                {"proxy_resources", "Proxy resources"},
                {"proxy_prompts", "Proxy prompts"},
                {"proxy_skills", "Trust Agent Skills"}
              ]
            }
            field={field}
            value={@draft[field]}
            label={label}
          />
          <.field
            :for={
              {field, label} <- [
                {"expose_tools", "Tool exposure patterns"},
                {"expose_resources", "Resource exposure patterns"},
                {"expose_prompts", "Prompt exposure patterns"},
                {"expose_skills", "Skill exposure patterns"}
              ]
            }
            id={"gm-" <> field}
            name={"server[" <> field <> "]"}
            value={@draft[field]}
            label={label}
          />
          <p>
            Enter patterns as a JSON array of strings. Blank removes the allowlist. An empty array exposes no entries.
          </p>
        </details>
      </fieldset>
      <p :if={@editing} id="gm-test-mode">
        Test saved configuration probes the saved ID. Unsaved edits and hidden saved credentials are preserved on the backend. This probe ignores this draft.
      </p>
      <p :if={@mutation.test_result} id="gm-test-result" role="status">{@mutation.test_result}</p>
      <footer class="gm-footer">
        <button id="gm-test" type="button" phx-click="gateway_test" disabled={@busy}>{if(@editing,
          do: "Test saved configuration",
          else: "Test draft connection"
        )}</button><span class="gm-spacer"></span><button type="button" phx-click="gateway_close">Cancel</button><button
          id="gm-save"
          type="submit"
          class="gm-primary"
          disabled={@busy}
        >Save server</button>
      </footer>
      <p>Saving is independent of testing. A saved change has its own connection status.</p>
    </.form>
    <script :type={Phoenix.LiveView.ColocatedHook} name=".Disclosure">
      export default {
        beforeUpdate() { this.open = this.el.open },
        updated() { this.el.open = this.open }
      }
    </script>
    """
  end

  attr :field, :string, required: true
  attr :label, :string, required: true
  attr :draft, :map, required: true

  def private_map(assigns) do
    ~H"""
    <details id={"gm-" <> @field <> "-disclosure"} class="gm-private-map" phx-hook=".Disclosure">
      <summary>{@label}</summary>
      <.field
        id={"gm-" <> @field <> "-intent"}
        name={"server[" <> @field <> "_intent]"}
        value={@draft[@field <> "_intent"]}
        label={@label <> " update"}
        type="select"
        options={[
          {"Preserve saved map", "preserve"},
          {"Replace entire map", "replace"},
          {"Clear entire map", "clear"}
        ]}
      />
      <.field
        :if={@draft[@field <> "_intent"] == "replace"}
        id={"gm-" <> @field}
        name={"server[" <> @field <> "]"}
        value={@draft[@field]}
        label={@label <> " as a JSON string map"}
        type="textarea"
        rows={4}
        autocomplete="off"
      />
      <p :if={@draft[@field <> "_intent"] == "clear"}>Saving clears this entire map.</p>
    </details>
    """
  end

  attr :field, :string, required: true
  attr :value, :string, required: true
  attr :label, :string, required: true

  def toggle(assigns) do
    ~H"""
    <label class="gm-toggle" for={"gm-" <> @field}><input
      type="hidden"
      name={"server[" <> @field <> "]"}
      value="false"
    /><input
      id={"gm-" <> @field}
      type="checkbox"
      name={"server[" <> @field <> "]"}
      value="true"
      checked={@value == "true"}
    />{@label}</label>
    """
  end

  attr :id, :string, required: true
  attr :name, :string, required: true
  attr :label, :string, required: true
  attr :value, :string, required: true
  attr :type, :string, default: "text"
  attr :options, :list, default: []
  attr :rest, :global, include: ~w(rows maxlength disabled autocomplete)

  def field(assigns) do
    ~H"""
    <div class="gm-field">
      <label for={@id}>{@label}</label>
      <%= case @type do %>
        <% "textarea" -> %>
          <textarea id={@id} name={@name} {@rest}>{@value}</textarea>
        <% "select" -> %>
          <select id={@id} name={@name} {@rest}><option
            :for={{label, value} <- @options}
            value={value}
            selected={@value == value}
          >
            {label}
          </option></select>
        <% _ -> %>
          <input
            id={@id}
            name={@name}
            type={@type}
            value={if(@type == "password", do: nil, else: @value)}
            {@rest}
          />
      <% end %>
    </div>
    """
  end

  def restore_target(rows, selected, opener) do
    Enum.find_value(rows, "#gateway-add", fn row ->
      Enum.find_value(~w(edit remove test), fn action ->
        target = "#" <> action_id(action, row["name"])

        cond do
          opener == target -> target
          opener == target <> "-inspector" and selected == row["id"] -> opener
          opener == target <> "-inspector" -> target
          true -> nil
        end
      end)
    end)
  end

  def action_id(action, name),
    do: "gateway-" <> action <> "-" <> Base.url_encode64(name, padding: false)

  defp title(%{dialog: :remove}), do: "Remove server"
  defp title(%{dialog: :loading}), do: "Read server configuration"
  defp title(%{model: %{mode: :edit}}), do: "Edit server"
  defp title(_), do: "Add server"

  defp description(%{dialog: :remove}),
    do: "Confirm the exact selected server before removing it."

  defp description(%{model: %{mode: :edit}}),
    do:
      "Update this server connection and exposure settings. Untouched settings stay on the Gateway."

  defp description(_), do: "Connect a custom MCP endpoint or a command on the Labby host."
end
