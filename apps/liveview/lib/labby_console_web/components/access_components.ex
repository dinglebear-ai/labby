defmodule LabbyConsoleWeb.AccessComponents do
  use LabbyConsoleWeb, :html
  alias LabbyConsole.Access
  alias LabbyConsoleWeb.WorkspaceIcons
  alias Phoenix.LiveView.JS

  attr :state, :map, required: true
  attr :scope, :map, required: true
  attr :busy, :boolean, required: true

  def page(assigns) do
    assigns =
      assign(assigns,
        team: Access.selected(assigns.state),
        specs: Access.specs(),
        form_spec: Access.spec(assigns.state.dialog)
      )

    ~H"""
    <main
      id="access-main"
      phx-hook="AccessSecrets"
      class="ov-main lb-workspace-page ac-main"
      tabindex="-1"
    >
      <header class="lb-page-heading">
        <div>
          <span class="lb-heading-mark"><WorkspaceIcons.icon name="shield" /></span><div>
            <h1>People & teams</h1><p>Manage membership, Project roles, and credential references.</p>
          </div>
        </div>
        <.action_button
          id="access-create-team"
          action="access.team.create"
          label="Create Team"
          state={@state}
          scope={@scope}
        />
      </header>
      <p id="access-connection-status" phx-update="ignore" class="ac-alert" role="status" hidden>
        Connection unavailable. Changes stay disabled until Access availability is checked again.
      </p>
      <p :if={@state.errors["page"]} class="ac-alert" role="alert">{@state.errors["page"]}</p>
      <section :if={!@state.capabilities} class="ac-discovery" aria-label="Access availability">
        <p role="status">
          {if @state.help_error,
            do: Access.message(@state.help_error),
            else: "Checking available Access actions…"}
        </p>
        <button
          :if={@state.help_error}
          class="lb-action"
          phx-click="access_refresh"
          phx-value-card="help"
        >Check availability again</button>
      </section>
      <section
        :if={@state.receipt}
        id="access-receipt"
        class="ac-receipt"
        role="status"
        data-status={@state.receipt.status}
      >
        <strong>{receipt_label(@state.receipt.status)}</strong><p>{@state.receipt.message}</p>
        <p class="ac-meta">
          {Access.spec(@state.receipt.action).label}
          <span :if={@state.receipt.params["team_id"]}>· {@state.receipt.params["team_id"]}</span>
        </p>
        <button
          :if={Access.held?(@state) && !@busy}
          id="access-reconcile"
          class="lb-action"
          phx-click="access_refresh"
          phx-value-card="affected"
        >Refresh affected records</button>
      </section>
      <section
        id="access-one-time"
        class="ac-one-time"
        phx-update="ignore"
        hidden
        data-access-secret-output
      >
        <h2>Invitation created</h2><p>
          Copy this token once and share it privately with the existing principal.
        </p>
        <input
          id="access-created-secret"
          type="password"
          autocomplete="off"
          aria-label="Created invitation token"
          readonly
        />
        <div>
          <button type="button" class="lb-action" data-access-copy>Copy token</button><button
            type="button"
            class="lb-action"
            data-access-reveal
          >Reveal once</button><button type="button" class="lb-action" data-access-forget>Close and forget</button>
        </div>
        <p data-access-copy-status role="status"></p>
      </section>
      <div class="ac-toolbar">
        <.form
          for={to_form(%{"team" => @state.team || ""}, as: :scope)}
          id="access-team-form"
          phx-change="access_team"
        >
          <label for="access-team">Team</label><select
            id="access-team"
            name="scope[team]"
            disabled={Access.held?(@state) || @state.cards.teams.status not in [:loaded, :partial]}
          >
            <option :if={is_nil(@state.team)} value="">No Team selected</option><option
              :if={@state.team && is_nil(@team)}
              value={@state.team}
              selected
            >
              {@state.team} (awaiting Team inventory)
            </option><option
              :for={team <- @state.cards.teams.rows}
              value={team["team_id"]}
              selected={team["team_id"] == @state.team}
            >
              {team["name"]}
            </option>
          </select>
        </.form>
        <details id="access-actions" class="ac-menu">
          <summary class="lb-action">Team actions <WorkspaceIcons.icon name="chevron" /></summary>
          <div role="group" aria-label="Access actions">
            <.action_button
              :for={spec <- @specs}
              id={"access-action-" <> slug(spec.action)}
              action={spec.action}
              label={spec.label}
              state={@state}
              scope={@scope}
            />
          </div>
        </details>
      </div>
      <.card state={@state} card={:teams} title="Teams" />
      <section :if={@state.team} class="ac-panel" aria-labelledby="access-roster-title">
        <header>
          <div>
            <h2 id="access-roster-title">{if @team, do: @team["name"], else: @state.team}</h2><p
              :if={@team}
              class="ac-meta"
            >
              {@state.team}
            </p>
          </div><span :if={@team} class="ac-badge" data-status={@team["status"]}>{@team["status"]}</span>
        </header>
        <.inventory state={@state} card={:members} />
      </section>
      <div class="ac-grid">
        <section class="ac-panel">
          <header>
            <h2>Invitations</h2><.action_button
              id="access-invite"
              action="access.team_invitation.create"
              label="Invite teammate"
              state={@state}
              scope={@scope}
            />
          </header><.inventory state={@state} card={:invitations} />
        </section>
        <section class="ac-panel">
          <header>
            <h2>Platform administrators</h2>
          </header><.inventory state={@state} card={:admins} />
        </section>
        <section class="ac-panel">
          <header>
            <h2>Credential bindings</h2>
          </header><p class="ac-help">
            Host-custodied references only. Secret values stay in the credential store.
          </p><.inventory state={@state} card={:credentials} />
        </section>
        <section class="ac-panel">
          <header>
            <h2>Project roles</h2>
          </header><.inventory state={@state} card={:assignments} />
        </section>
      </div>
      <details class="ac-panel ac-effective">
        <summary>Your effective Project access</summary><.inventory state={@state} card={:effective} />
      </details>
      <div
        :if={@form_spec}
        class="ac-backdrop"
        phx-remove={JS.pop_focus()}
        phx-window-keydown="access_close"
        phx-key="Escape"
      >
        <.focus_wrap id="access-dialog-focus">
          <section
            id="access-dialog"
            class="ac-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="access-dialog-title"
          >
            <header>
              <h2 id="access-dialog-title">{@form_spec.label}</h2><button
                type="button"
                id="access-close"
                class="lb-icon-button"
                phx-click="access_close"
                aria-label="Close access dialog"
              ><WorkspaceIcons.icon name="close" /></button>
            </header>
            <.form
              for={to_form(@state.draft, as: :access_request)}
              id="access-action-form"
              phx-submit="access_submit"
              data-action={@form_spec.action}
            >
              <p class="ac-consequence">{consequence(@form_spec.action)}</p>
              <div :for={field <- @form_spec.fields} class="ac-field">
                <label for={"access-field-" <> field}>{field_label(field)}</label>
                <select
                  :if={field == "role"}
                  id={"access-field-" <> field}
                  name="access_request[role]"
                  required
                >
                  <option
                    :for={role <- Access.roles(@form_spec.action)}
                    value={role}
                    selected={role == @state.draft["role"]}
                  >
                    {role}
                  </option>
                </select>
                <input
                  :if={field not in ~w(role token)}
                  id={"access-field-" <> field}
                  name={"access_request[" <> field <> "]"}
                  value={@state.draft[field]}
                  type={if field == "ttl_seconds", do: "number", else: "text"}
                  min={if field == "ttl_seconds", do: "1"}
                  max={if field == "ttl_seconds", do: "604800"}
                  maxlength={if field in ~w(binding_id upstream_name), do: "256", else: "128"}
                  readonly={field == "team_id" && @form_spec.action != "access.team.create"}
                  required
                  autocomplete="off"
                />
                <div :if={field == "token"} id="access-secret-field" phx-update="ignore">
                  <input
                    id="access-field-token"
                    type="password"
                    minlength="64"
                    maxlength="64"
                    pattern="[0-9a-fA-F]{64}"
                    autocomplete="off"
                    required
                    aria-label="Invitation token"
                  />
                  <button
                    :if={@form_spec.action == "access.team_invitation.create"}
                    type="button"
                    class="lb-action"
                    data-access-generate
                  >Generate token</button>
                  <p data-access-secret-status role="status"></p>
                </div>
              </div>
              <label class="ac-confirm"><input
                id="access-confirm"
                type="checkbox"
                name="confirmation"
                value="yes"
                required
              />I confirm this change to the displayed account and Team.</label>
              <p :for={{_, message} <- @state.errors} class="ac-alert" role="alert">{message}</p>
              <footer>
                <button
                  id="access-submit"
                  type="submit"
                  class="lb-action lb-action-primary"
                  disabled={@busy || !is_struct(@state.capabilities, MapSet)}
                >Confirm change</button>
              </footer>
            </.form>
          </section>
        </.focus_wrap>
      </div>
    </main>
    """
  end

  attr :state, :map, required: true
  attr :scope, :map, required: true
  attr :action, :string, required: true
  attr :label, :string, required: true
  attr :id, :string, required: true

  def action_button(assigns) do
    assigns =
      assign(
        assigns,
        :availability,
        Access.availability(assigns.state, assigns.action, assigns.scope)
      )

    ~H"""
    <button
      id={@id}
      type="button"
      class="lb-action"
      phx-click={
        JS.push_focus(
          to:
            if(String.starts_with?(@id, "access-action-"),
              do: "#access-actions > summary",
              else: "##{@id}"
            )
        )
        |> JS.push("access_open")
      }
      phx-value-action={@action}
      disabled={@availability != :available}
      title={availability_label(@availability)}
    >{@label}<span :if={@availability != :available} class="ac-unavailable">{availability_label(
      @availability
    )}</span></button>
    """
  end

  attr :state, :map, required: true
  attr :card, :atom, required: true
  attr :title, :string, required: true

  def card(assigns) do
    ~H"""
    <details id="access-teams-inventory" class="ac-panel ac-team-disclosure">
      <summary>{@title}</summary><.inventory state={@state} card={@card} />
    </details>
    """
  end

  attr :state, :map, required: true
  attr :card, :atom, required: true

  def inventory(assigns) do
    assigns =
      assign(assigns, data: assigns.state.cards[assigns.card], key: Atom.to_string(assigns.card))

    ~H"""
    <div
      id={"access-card-" <> @key}
      class="ac-inventory"
      data-state={@data.status}
      aria-busy={to_string(@data.status == :loading)}
    >
      <p :if={@data.status == :idle} class="ac-empty">
        {if Access.scoped?(@card),
          do: "Select a Team to inspect these records.",
          else: "Waiting for gateway availability."}
      </p>
      <p :if={@data.status == :loading} class="ac-empty" role="status">Loading records…</p>
      <p :if={@data.status == :unavailable} class="ac-empty">
        This gateway does not advertise this inventory.
      </p>
      <p :if={@data.error} class="ac-alert" role="alert">{Access.message(@data.error)}</p>
      <p :if={@data.status in [:loaded, :partial] && @data.rows == []} class="ac-empty">
        No records in this inventory.
      </p>
      <article
        :for={{row, i} <- Enum.with_index(@data.rows)}
        id={"access-" <> @key <> "-" <> to_string(i)}
        class="ac-row"
      >
        <WorkspaceIcons.icon name={if @card in [:members, :admins], do: "user", else: "shield"} />
        <div class="ac-record">
          <strong>{row["principal_id"] || row["project_id"] || row["name"] || row["upstream_name"]}</strong><span>{row[
            "role"
          ] || row["team_id"] ||
            if(@card == :admins, do: "Platform administrator", else: "Credential reference")}</span><span :if={
            row["acceptance_state"]
          }>{row["acceptance_state"]}</span><span :if={@card == :credentials}>Reference {row[
            "binding_id"
          ]}</span><span :if={@card == :credentials}>Custodian {row["custodian_principal_id"]} · Generation {row[
            "generation"
          ]}</span><span :if={row["membership_epoch"]}>Membership epoch {row["membership_epoch"]}</span><span :if={
            row["assignment_epoch"]
          }>Assignment epoch {row["assignment_epoch"]} · Project {row["project_status"]}</span><span :if={
            row["authority_epoch"]
          }>Authority epoch {row["authority_epoch"]}</span><span :if={row["expires_at"]}>Expires {row[
            "expires_at"
          ]}</span>
        </div>
        <span :if={row["status"]} class="ac-badge" data-status={row["status"]}>{row["status"]}</span>
      </article>
      <p :if={@data.revision} class="ac-meta">
        Revision {@data.revision}<span :if={@data.metadata["team_membership_epoch"]}> · Team membership epoch {@data.metadata[
          "team_membership_epoch"
        ]}</span><span :if={@data.metadata["observed_at"]}> · Observed {@data.metadata["observed_at"]}</span>
      </p>
      <div class="ac-paging">
        <button
          id={"access-refresh-" <> @key}
          class="lb-action"
          phx-click="access_refresh"
          phx-value-card={@key}
          disabled={@data.status in [:idle, :loading, :unavailable] || terminal?(@data)}
        >Refresh from start</button>
        <button
          :if={@data.next_offset}
          id={"access-next-" <> @key}
          class="lb-action"
          phx-click="access_next"
          phx-value-card={@key}
          disabled={@data.status not in [:loaded, :partial]}
        >Next page</button>
        <.form
          :if={Access.paged?(@card)}
          for={to_form(%{}, as: :paging)}
          id={"access-paging-" <> @key}
          phx-submit="access_limit"
        >
          <input type="hidden" name="paging[card]" value={@key} /><label for={"access-limit-" <> @key}>Page size</label><input
            id={"access-limit-" <> @key}
            type="number"
            min="1"
            max="100"
            name="paging[limit]"
            value={@data.limit}
            disabled={@data.status == :loading || terminal?(@data)}
          /><button
            class="lb-action"
            disabled={@data.status in [:idle, :loading, :unavailable] || terminal?(@data)}
          >Apply</button>
        </.form>
      </div>
    </div>
    """
  end

  defp terminal?(%{error: %{metadata: %{"budget_terminal" => true}}}), do: true
  defp terminal?(_), do: false
  defp slug(action), do: String.replace(action, ".", "-")
  defp field_label("principal_id"), do: "Existing principal ID"
  defp field_label("ttl_seconds"), do: "Expires after (seconds)"
  defp field_label("binding_id"), do: "Host credential reference ID"
  defp field_label(field), do: field |> String.replace("_", " ") |> String.capitalize()
  defp availability_label(:available), do: "Apply after confirmation"
  defp availability_label(:unknown), do: "Checking availability"
  defp availability_label(:unavailable), do: "Unavailable on this gateway"
  defp availability_label(:held), do: "Resolve the previous change first"
  defp availability_label(:forbidden), do: "Requires current management authority"
  defp receipt_label(:pending), do: "Submitting"
  defp receipt_label(:accepted), do: "Accepted, read-back pending"
  defp receipt_label(:uncertain), do: "Outcome uncertain"
  defp receipt_label(:failed), do: "Change rejected"
  defp receipt_label(:visibility_lost), do: "Affected access unavailable"
  defp receipt_label(:accepted_observed), do: "Accepted, records refreshed"
  defp receipt_label(:reconciled), do: "Records reconciled"

  defp consequence("access.team_invitation.create"),
    do:
      "Invite an existing active principal. Generate or supply a private 64-character hexadecimal token. Copy it after creation, then share it privately. No delivery link is generated."

  defp consequence("access.team_invitation.accept"),
    do:
      "Accept an invitation for your authenticated identity. Acceptance is single-use. Expired or invalidated invitations can be rejected."

  defp consequence("access.team.member.activate"),
    do:
      "Explicitly restore an inactive member. Revoked Initial Team admission cannot be restored through this action. The gateway checks the member and current authority."

  defp consequence("access.team.member.role.set"),
    do:
      "Change this member's role. An inactive member stays inactive until explicitly restored. Keep an active owner."

  defp consequence("access.team_project." <> _),
    do:
      "Requires management authority for both the Team and Project. Removing an assignment can remove your own Project management access. Recovery can require a retained direct owner or platform administrator."

  defp consequence("access.gateway_credential." <> _),
    do:
      "Change only the host-custodied reference for this Team upstream. A replacement can invalidate existing upstream sessions. The inventory shows reference metadata. Secret values stay in the host credential store."

  defp consequence("access.platform_admin.revoke"),
    do:
      "Revoke this principal's platform administrator grant. The gateway prevents self-revocation."

  defp consequence("access.team.member." <> _),
    do:
      "Change membership for an existing principal. Adding an existing member can conflict. Keep an active owner before suspending, removing, or changing an owner's role."

  defp consequence(_),
    do:
      "The gateway checks your current authority before applying this change. Review the identifiers before confirming."
end
