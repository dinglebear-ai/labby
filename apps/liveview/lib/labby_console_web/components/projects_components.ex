defmodule LabbyConsoleWeb.ProjectsComponents do
  use LabbyConsoleWeb, :html
  alias LabbyConsole.Projects, as: Preview
  alias LabbyConsoleWeb.WorkspaceIcons
  alias Phoenix.LiveView.JS

  attr :state, :map, required: true
  attr :streams, :map, required: true
  attr :count, :integer, required: true
  attr :preference_key, :string, default: nil

  def page(assigns) do
    assigns =
      assigns
      |> assign(:selected, Preview.selected(assigns.state))
      |> assign(
        :filter_form,
        to_form(%{"query" => assigns.state.query, "team" => assigns.state.active_team},
          as: :filters
        )
      )
      |> assign(:project_form, to_form(assigns.state.draft, as: :project))

    ~H"""
    <main
      id="projects-main"
      phx-hook="ProjectsPreferences"
      data-preference-key={@preference_key}
      class="ov-main lb-workspace-page lb-operations-page pj-main"
      tabindex="-1"
    >
      <header class="lb-page-heading labby-page-header">
        <div>
          <span class="lb-heading-mark"><WorkspaceIcons.icon name="folder" /></span><div>
            <h1>Projects</h1><p>Team-owned workspaces and their access policies.</p>
          </div>
        </div>
        <div class="pj-heading-actions">
          <button
            id="projects-lookup"
            type="button"
            class="lb-action labby-icon-action"
            phx-click={JS.push_focus() |> JS.push("projects_open")}
            phx-value-mode="lookup"
            aria-label="Find project by ID"
            title="Find project by ID"
          ><WorkspaceIcons.icon name="search" /><span class="labby-action-label">Find by ID</span></button><button
            id="projects-refresh"
            disabled={@state.busy}
            type="button"
            class="lb-action labby-icon-action"
            phx-click="projects_refresh"
            aria-label="Refresh projects"
            title="Refresh projects"
          ><WorkspaceIcons.icon name="refresh" /></button><button
            id="projects-create"
            type="button"
            class="lb-action lb-action-primary labby-icon-action"
            disabled={@state.busy || !Preview.can_create?(@state)}
            phx-click={JS.push_focus() |> JS.push("projects_open")}
            phx-value-mode="create"
            aria-label="Create project"
            title={
              if(Preview.can_create?(@state),
                do: "Create project",
                else: "Select a team you manage to create a project"
              )
            }
          ><WorkspaceIcons.icon name="plus" /><span class="labby-action-label">Create project</span></button>
        </div>
      </header>
      <div :if={@state.notice} id="projects-notice" class="pj-notice" role="status">
        <WorkspaceIcons.icon name="check-circle" /><span>{@state.notice}</span><button
          type="button"
          phx-click="projects_notice"
          aria-label="Dismiss notification"
          title="Dismiss notification"
        ><WorkspaceIcons.icon name="close" /></button>
      </div>
      <p :if={@state.busy} role="status">Loading project authority…</p>
      <p :if={@state.errors["page"]} id="projects-error" role="alert">{@state.errors["page"]}</p>
      <section class="pj-collection" aria-label="Projects" aria-busy={to_string(@state.busy)}>
        <div class="pj-toolbar">
          <.form
            for={@filter_form}
            id="projects-filter"
            phx-change="projects_filter"
            phx-submit="projects_filter"
            class="pj-filter-form"
          >
            <div class="pj-search">
              <WorkspaceIcons.icon name="search" /><.input
                field={@filter_form[:query]}
                type="search"
                placeholder="Search projects or teams…"
                aria-label="Search projects"
                phx-debounce="200"
                class="pj-input"
              />
            </div><button
              type="button"
              id="projects-filter-sheet-toggle"
              class="lb-icon-button declutter-filter-toggle"
              popovertarget="projects-filter-sheet"
              aria-label="Choose project team"
              title="Choose project team"
            ><WorkspaceIcons.icon name="filter" class="ov-small" /></button>
            <div
              id="projects-filter-sheet"
              popover
              class="discover-filter-sheet declutter-filter-sheet"
            >
              <header>
                <strong>Choose project team</strong><button
                  type="button"
                  class="lb-icon-button"
                  popovertarget="projects-filter-sheet"
                  popovertargetaction="hide"
                  aria-label="Close filters"
                ><WorkspaceIcons.icon name="close" class="ov-small" /></button>
              </header><select
                id="projects-workspace-team"
                name="filters[team]"
                aria-label="Active project team"
                class="aurora-select--custom pj-team-select"
              >
                {Phoenix.HTML.Form.options_for_select(
                  Enum.map(@state.teams, &{&1.name, &1.id}),
                  @state.active_team
                )}
              </select>
            </div>
          </.form>
          <div class="pj-layout" aria-label="Project layout">
            <button
              :for={{layout, icon} <- [{"table", "table"}, {"list", "list"}, {"grid", "grid"}]}
              id={"projects-layout-#{layout}"}
              type="button"
              class="lb-action"
              phx-click="projects_layout"
              phx-value-layout={layout}
              aria-pressed={to_string(@state.layout == layout)}
              aria-label={"#{String.capitalize(layout)} view"}
              title={"#{String.capitalize(layout)} view"}
            ><WorkspaceIcons.icon name={icon} /></button>
          </div>
        </div>
        <p class="pj-context">
          Create in <strong>{@state.active_team}</strong>. {if(Preview.can_create?(@state),
            do: "You can create projects in this team.",
            else: "Your team access is read only."
          )}
        </p>
        <nav class="pj-scopes" aria-label="Project status">
          <button
            :for={
              {scope, label, icon} <- [
                {"active", "Accessible", "folder"},
                {"archived", "Known archives", "box"},
                {"all", "All known", "layers"}
              ]
            }
            id={"projects-scope-#{scope}"}
            type="button"
            phx-click="projects_scope"
            phx-value-scope={scope}
            aria-pressed={to_string(@state.scope == scope)}
            class="labby-icon-action"
            aria-label={label}
            title={label}
          ><WorkspaceIcons.icon name={icon} /><span class="labby-action-label">{label}</span><span>{Enum.count(
            Preview.visible(@state),
            &(scope == "all" or (scope == "archived" and &1.status == "disabled") or
                &1.status == scope)
          )}</span></button><span class="pj-visible">{@count} shown</span>
        </nav>
        <p :if={@state.scope in ["archived", "all"]} class="pj-context">
          The API lists active projects only. Known archives are snapshots retained after archiving or finding a project by ID.
        </p>
        <div :if={@count == 0} id="projects-empty" class="pj-empty">
          <WorkspaceIcons.icon name="folder" /><h2>No projects found</h2><p>
            Try another search. To inspect an archived project, find it by team and project ID.
          </p><button type="button" class="lb-action" phx-click="projects_reset">Clear filters</button>
        </div>
        <div id="projects-rows" class={"pj-rows pj-#{@state.layout}"} phx-update="stream">
          <article
            :for={{dom_id, project} <- @streams.projects}
            id={dom_id}
            class="pj-row"
            data-current={to_string(@state.current == project.id)}
            data-team={project.team_id}
            data-project={project.project_id}
          >
            <span class="pj-project-mark"><WorkspaceIcons.icon name={
              if(project.status == "disabled", do: "box", else: "folder")
            } /></span>
            <div class="pj-project-copy">
              <button
                type="button"
                class="pj-project-name"
                phx-click={JS.push_focus() |> JS.push("projects_select")}
                phx-value-id={project.id}
                id={"project-open-#{project.id}"}
              >{project.name}</button><span class="pj-project-id">{project.project_id}</span><div class="pj-project-meta">
                <span><WorkspaceIcons.icon name="user" />{project.team_id}</span><span title="Team project assignment role"><WorkspaceIcons.icon name="shield" />{project.role}</span><span title="Project policy revision">Policy {project.policy_epoch}</span><span
                  :if={project.status == "disabled"}
                  class="pj-muted"
                >Archived</span><span :if={@state.current == project.id} class="pj-current"><WorkspaceIcons.icon name="check" />Current</span><span
                  :if={!project.can_manage}
                  class="pj-muted"
                >Read only</span>
              </div>
            </div>
            <button
              type="button"
              class="lb-action pj-open"
              phx-click={JS.push_focus() |> JS.push("projects_select")}
              phx-value-id={project.id}
              aria-label={"Open #{project.name} for #{project.team_id}"}
              title="Open project"
            ><WorkspaceIcons.icon name="arrow" /></button>
          </article>
        </div>
        <footer class="pj-preview-note">
          <WorkspaceIcons.icon name="info" />Projects and permissions come from the gateway. Workspace preferences are saved for this caller and gateway.
        </footer>
      </section>
      <div
        :if={@selected || @state.dialog}
        id="projects-backdrop"
        class="pj-backdrop"
        phx-window-keydown="projects_close"
        phx-key="Escape"
        phx-remove={JS.pop_focus()}
      >
        <.focus_wrap id="projects-focus" phx-mounted={JS.focus_first(to: "#projects-dialog")}>
          <section
            id="projects-dialog"
            class={["pj-dialog", !@state.dialog && "pj-inspector"]}
            role="dialog"
            aria-modal="true"
            aria-labelledby="projects-dialog-title"
            phx-click-away="projects_close"
          >
            <header>
              <span class="pj-project-mark"><WorkspaceIcons.icon name="folder" /></span><h2 id="projects-dialog-title">
                {cond do
                  @state.dialog == "create" -> "Create project"
                  @state.dialog == "edit" -> "Rename project"
                  @state.dialog == "lookup" -> "Find project"
                  @state.dialog == "archive" -> "Archive project?"
                  @state.dialog == "restore" -> "Reactivate project?"
                  true -> @selected.name
                end}
              </h2><button
                id="projects-close"
                type="button"
                class="lb-action"
                phx-click="projects_close"
                aria-label="Close project details"
                title="Close"
              ><WorkspaceIcons.icon name="close" /></button>
            </header>
            <%= cond do %>
              <% @state.dialog in ["create", "edit", "lookup"] -> %>
                <.form
                  for={@project_form}
                  id="projects-form"
                  phx-submit={
                    if(@state.dialog == "lookup", do: "projects_lookup", else: "projects_save")
                  }
                  class="pj-form"
                >
                  <p :if={@state.dialog == "lookup"}>
                    Read a known project using its team and project ID, including an archived project.
                  </p>
                  <p
                    :if={@state.errors["form"]}
                    id="projects-form-error"
                    class="pj-warning"
                    role="alert"
                  >
                    {@state.errors["form"]}
                  </p>
                  <.input
                    :if={@state.dialog != "lookup"}
                    field={@project_form[:name]}
                    id="projects-name"
                    label="Project name"
                    type="text"
                    required
                    maxlength="128"
                    class="pj-input"
                    aria-invalid={@state.errors["name"] != nil}
                  /><p :if={@state.errors["name"]} class="pj-warning" role="alert">
                    {@state.errors["name"]}
                  </p>
                  <.input
                    field={@project_form[:project_id]}
                    id="projects-id"
                    label="Project ID"
                    type="text"
                    required
                    maxlength="128"
                    readonly={@state.dialog == "edit"}
                    class="pj-input"
                    aria-invalid={@state.errors["project_id"] != nil}
                  /><p :if={@state.errors["project_id"]} class="pj-warning" role="alert">
                    {@state.errors["project_id"]}
                  </p>
                  <p :if={@state.dialog != "lookup"} class="pj-field-note">
                    Names and IDs accept up to 128 UTF-8 bytes, without control characters. Project IDs are unique within the organization.
                  </p>
                  <%= if @state.dialog == "lookup" do %>
                    <div>
                      <label for="projects-team">Team</label><select
                        id="projects-team"
                        name="project[team_id]"
                        class="aurora-select--custom"
                      >
                        {Phoenix.HTML.Form.options_for_select(
                          Enum.map(@state.teams, &{&1.name, &1.id}),
                          @state.draft["team_id"]
                        )}
                      </select>
                    </div>
                  <% else %>
                    <div class="pj-field-note">Team: <strong>{@state.draft["team_id"]}</strong></div><input
                      type="hidden"
                      name="project[team_id]"
                      value={@state.draft["team_id"]}
                    />
                  <% end %>
                  <footer>
                    <span class="pj-field-note">Gateway-owned project</span><button
                      type="button"
                      class="lb-action"
                      phx-click="projects_close"
                    >Cancel</button><button
                      id="projects-save"
                      disabled={@state.busy || (@state.uncertain && @state.dialog != "lookup")}
                      phx-disable-with="Submitting…"
                      type="submit"
                      class="lb-action lb-action-primary"
                    >{case @state.dialog do
                      "edit" -> "Save name"
                      "lookup" -> "Find project"
                      _ -> "Create project"
                    end}</button>
                  </footer>
                </.form>
              <% @state.dialog in ["archive", "restore"] -> %>
                <div class="pj-dialog-body">
                  <p :if={@state.errors["form"]} id="projects-form-error" role="alert">
                    {@state.errors["form"]}
                  </p>
                  <p>
                    <strong>{@selected.name}</strong> {if(@state.dialog == "archive",
                      do: "will be archived and removed from accessible project listings.",
                      else: "will become active and appear in accessible project listings again."
                    )}
                  </p><p class="pj-warning">
                    This changes the project across all team assignments and advances its policy revision.
                  </p><p
                    :if={@state.current == @selected.id && @state.dialog == "archive"}
                    class="pj-field-note"
                  >
                    This is your current project. Archiving clears that selection.
                  </p><p class="pj-field-note">
                    The gateway checks your current permission before applying this change.
                  </p>
                </div><footer>
                  <button type="button" class="lb-action" phx-click="projects_close">Cancel</button><button
                    id={"projects-confirm-#{@state.dialog}"}
                    disabled={@state.busy || @state.uncertain}
                    phx-disable-with="Submitting…"
                    type="button"
                    class="lb-action pj-archive"
                    phx-click={
                      if(@state.dialog == "archive", do: "projects_archive", else: "projects_restore")
                    }
                  ><WorkspaceIcons.icon name={
                    if(@state.dialog == "archive", do: "box", else: "refresh")
                  } />{if(
                    @state.dialog == "archive",
                    do: "Archive project",
                    else: "Reactivate project"
                  )}</button>
                </footer>
              <% true -> %>
                <div class="pj-dialog-body">
                  <div class="pj-inspector-status">
                    <span class={if(@selected.status == "active", do: "pj-current", else: "pj-muted")}>{if(
                      @selected.status == "disabled",
                      do: "Archived",
                      else: "Active"
                    )}</span><span
                      :if={@state.current == @selected.id}
                      class="pj-current"
                    ><WorkspaceIcons.icon name="check" />Current workspace</span>
                  </div>
                  <dl class="pj-details">
                    <div>
                      <dt>Project ID</dt><dd>{@selected.project_id}</dd>
                    </div><div>
                      <dt>Team</dt><dd>{@selected.team_id}</dd>
                    </div><div>
                      <dt>Team assignment role</dt><dd>{@selected.role}</dd>
                    </div><div>
                      <dt>Policy revision</dt><dd id="project-policy-epoch">
                        {@selected.policy_epoch}
                      </dd>
                    </div><div>
                      <dt>API status</dt><dd>{@selected.status}</dd>
                    </div><div>
                      <dt>Manage access</dt><dd>
                        {if(@selected.can_manage, do: "Allowed", else: "Read only")}
                      </dd>
                    </div>
                  </dl>
                  <p class="pj-field-note">
                    Tasks use project context. Stash belongs to the owning team.
                  </p>
                  <p :if={!@selected.can_manage} class="pj-field-note">
                    <WorkspaceIcons.icon name="shield" />Manage access is supplied by the API. The assignment role alone does not grant lifecycle actions.
                  </p>
                </div>
                <footer>
                  <button
                    :if={@selected.status == "active" && @state.current != @selected.id}
                    id="projects-use"
                    type="button"
                    class="lb-action lb-action-primary"
                    phx-click="projects_activate"
                    phx-value-id={@selected.id}
                  ><WorkspaceIcons.icon name="check" />Use project</button><button
                    :if={@selected.can_manage && @selected.status == "active"}
                    id="projects-edit"
                    type="button"
                    class="lb-action"
                    phx-click={JS.push_focus() |> JS.push("projects_open")}
                    phx-value-mode="edit"
                    phx-value-id={@selected.id}
                  ><WorkspaceIcons.icon name="edit" />Rename</button><button
                    :if={@selected.can_manage && @selected.status == "active"}
                    id="projects-archive"
                    type="button"
                    class="lb-action pj-archive"
                    phx-click={JS.push_focus() |> JS.push("projects_open")}
                    phx-value-mode="archive"
                    phx-value-id={@selected.id}
                  ><WorkspaceIcons.icon name="box" />Archive</button><button
                    :if={@selected.can_manage && @selected.status == "disabled"}
                    id="projects-restore"
                    type="button"
                    class="lb-action lb-action-primary"
                    phx-click={JS.push_focus() |> JS.push("projects_open")}
                    phx-value-mode="restore"
                    phx-value-id={@selected.id}
                  ><WorkspaceIcons.icon name="refresh" />Reactivate</button>
                </footer>
            <% end %>
          </section>
        </.focus_wrap>
      </div>
    </main>
    """
  end
end
