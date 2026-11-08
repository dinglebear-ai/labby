defmodule LabbyConsoleWeb.ProjectsLive do
  use LabbyConsoleWeb, :live_view
  on_mount {LabbyConsoleWeb.LiveSession, :authenticated}
  alias LabbyConsole.{Auth, Projects, GatewayAction.Error}
  alias LabbyConsoleWeb.ProjectsComponents, as: UI

  def mount(_, _, socket) do
    socket =
      assign(socket,
        page_title: "Projects",
        projects: Projects.new(),
        request: nil,
        pending_preferences: nil,
        preferences_ready: false
      )

    {:ok, if(connected?(socket), do: refresh(socket), else: present(socket))}
  end

  def render(assigns) do
    ~H"""
    <Layouts.app
      flash={@flash}
      authenticated={true}
      page="Projects"
      main_id="projects-main"
      subject={@subject}
      preference_key={@preference_key}
    >
      <UI.page state={@projects} streams={@streams} count={@count} preference_key={@preference_key} />
    </Layouts.app>
    """
  end

  def handle_event("projects_preferences", %{"preferences" => preferences}, socket)
      when is_map(preferences) do
    preferences = Map.take(preferences, ~w(layout query team current))

    if socket.assigns.preferences_ready && !socket.assigns.projects.busy do
      reply(socket, &Projects.restore_preferences(&1, preferences))
    else
      {:noreply, assign(socket, pending_preferences: preferences)}
    end
  end

  def handle_event("projects_notice", _, socket), do: reply(socket, &%{&1 | notice: nil})
  # A submitted write cannot be canceled by changing scope and then mistaken for a
  # completed operation. UI scope controls are fenced until its bounded result.
  def handle_event(_, _, %{assigns: %{projects: %{busy: true}}} = socket), do: {:noreply, socket}

  def handle_event("projects_filter", %{"filters" => params}, socket),
    do: reply(socket, &Projects.filter(&1, params))

  def handle_event("projects_scope", %{"scope" => value}, socket),
    do: reply(socket, &Projects.filter(&1, %{"scope" => value}))

  def handle_event("projects_layout", %{"layout" => value}, socket),
    do: reply(socket, &Projects.filter(&1, %{"layout" => value}))

  def handle_event("projects_reset", _, socket),
    do: reply(socket, &Projects.filter(&1, %{"query" => "", "scope" => "all"}))

  def handle_event("projects_open", %{"mode" => mode} = params, socket),
    do: reply(socket, &Projects.open(&1, mode, params["id"]))

  def handle_event("projects_select", %{"id" => id}, socket),
    do:
      reply(socket, fn state ->
        if Projects.find(state, id), do: %{state | selected: id}, else: state
      end)

  def handle_event("projects_close", _, socket),
    do: reply(socket, &%{&1 | selected: nil, dialog: nil, errors: %{}})

  def handle_event("projects_activate", %{"id" => id}, socket),
    do: reply(socket, &Projects.select_workspace(&1, id))

  def handle_event("projects_refresh", _, socket), do: {:noreply, refresh(socket)}

  def handle_event(event, _, %{assigns: %{projects: %{uncertain: true}}} = socket)
      when event in ~w(projects_save projects_archive projects_restore),
      do: {:noreply, socket}

  def handle_event("projects_save", %{"project" => params}, socket) do
    state = socket.assigns.projects

    case state.dialog do
      "create" ->
        if Projects.can_create?(state) do
          draft =
            Map.merge(Map.take(params, ~w(name project_id)), %{"team_id" => state.active_team})

          {:noreply, mutate(socket, "create", draft)}
        else
          denied(socket)
        end

      "edit" ->
        case Projects.selected(state) do
          %{can_manage: true, status: "active"} = p ->
            {:noreply,
             mutate(socket, "edit", %{
               "name" => params["name"] || "",
               "team_id" => p.team_id,
               "project_id" => p.project_id
             })}

          _ ->
            denied(socket)
        end

      _ ->
        denied(socket)
    end
  end

  def handle_event("projects_lookup", %{"project" => params}, socket) do
    state = socket.assigns.projects

    if state.dialog == "lookup" do
      draft = Map.merge(%{"name" => ""}, Map.take(params, ~w(team_id project_id)))
      id = socket.assigns.session_id
      socket = update(socket, :projects, &%{&1 | draft: draft})

      {:noreply,
       begin(socket, :lookup, fn ->
         Projects.get(id, draft["team_id"] || "", draft["project_id"] || "")
       end)}
    else
      denied(socket)
    end
  end

  def handle_event(event, _, socket) when event in ~w(projects_archive projects_restore) do
    state = socket.assigns.projects
    mode = if event == "projects_archive", do: "archive", else: "restore"

    case Projects.selected(state) do
      %{can_manage: true} = p when state.dialog == mode ->
        {:noreply,
         mutate(socket, mode, %{
           "team_id" => p.team_id,
           "project_id" => p.project_id,
           "name" => p.name
         })}

      _ ->
        denied(socket)
    end
  end

  def handle_event(_, _, socket), do: {:noreply, socket}

  defp mutate(socket, mode, draft) do
    id = socket.assigns.session_id

    socket
    |> update(:projects, &%{&1 | draft: draft})
    |> begin({:write, mode}, fn -> Projects.mutate(id, mode, draft) end)
  end

  defp refresh(socket) do
    state = socket.assigns.projects
    # Every known alias is re-read separately; another Team's row never inherits
    # the initiating alias's manage permission.
    known =
      Enum.filter(state.projects, &(&1.status == "disabled" || &1.id in state.known_archived))
      |> Enum.take(20)

    id = socket.assigns.session_id
    scope = socket.assigns.current_scope

    socket
    |> update(:projects, &%{&1 | projects: [], selected: nil, teams: []})
    |> begin(:refresh, fn -> Projects.refresh(id, scope, known) end)
  end

  defp begin(socket, purpose, fun) do
    ref = make_ref()

    fence =
      {socket.assigns.session_id, socket.assigns.current_scope,
       socket.assigns.projects.active_team}

    Process.send_after(self(), {:projects_deadline, ref}, 12_000)

    socket
    |> assign(request: {ref, purpose, fence})
    |> update(:projects, &%{&1 | busy: true, errors: %{}, notice: nil})
    |> start_async({:projects, ref}, fun)
    |> present()
  end

  def handle_async(
        {:projects, ref},
        {:ok, result},
        %{assigns: %{request: {ref, purpose, fence}}} = socket
      ) do
    current =
      {socket.assigns.session_id, socket.assigns.current_scope,
       socket.assigns.projects.active_team}

    if fence == current && Auth.capture(socket.assigns.session_id) do
      socket = socket |> assign(request: nil) |> update(:projects, &%{&1 | busy: false})
      complete(socket, purpose, result)
    else
      {:noreply, socket |> assign(request: nil, projects: Projects.new()) |> present()}
    end
  end

  def handle_async(
        {:projects, ref},
        {:exit, _},
        %{assigns: %{request: {ref, purpose, _}}} = socket
      ),
      do:
        complete(
          socket |> assign(request: nil) |> update(:projects, &%{&1 | busy: false}),
          purpose,
          {:error,
           Error.new(:unavailable, "projects", "projects.list",
             side_effects: if(match?({:write, _}, purpose), do: "unknown", else: "none_expected")
           )}
        )

  def handle_async({:projects, _}, _, socket), do: {:noreply, socket}

  def handle_info({:projects_deadline, ref}, %{assigns: %{request: {ref, purpose, _}}} = socket) do
    socket =
      cancel_async(socket, {:projects, ref})
      |> assign(request: nil)
      |> update(:projects, &%{&1 | busy: false})

    complete(
      socket,
      purpose,
      {:error,
       Error.new(:timeout, "projects", "projects.list",
         side_effects: if(match?({:write, _}, purpose), do: "unknown", else: "none_expected")
       )}
    )
  end

  def handle_info({:projects_deadline, _}, socket), do: {:noreply, socket}

  defp complete(socket, _, {:error, %Error{kind: :unauthenticated}}),
    do: {:noreply, socket |> assign(projects: Projects.new()) |> redirect(to: "/sign-in")}

  defp complete(socket, purpose, {:error, %Error{} = error}) do
    location =
      if purpose == :refresh || match?({:reconcile, _}, purpose), do: "page", else: "form"

    message =
      if error.side_effects in ["unknown", "possible"],
        do:
          "Outcome uncertain. Do not submit again. Refresh or find this project by ID to reconcile gateway state.",
        else:
          "Project unavailable (#{error.kind}). Check your access and the team and project ID."

    reply(socket, fn state ->
      state = %{
        state
        | uncertain: error.side_effects in ["unknown", "possible"] || state.uncertain
      }

      # A failed permission check cannot leave an old managing row actionable.
      if error.kind in [:forbidden, :authority_changed] do
        %{
          state
          | errors: %{location => message},
            projects: [],
            current: nil,
            selected: nil,
            dialog: if(state.dialog in ~w(create edit lookup), do: state.dialog, else: nil)
        }
      else
        %{state | errors: %{location => message}}
      end
    end)
  end

  defp complete(socket, :refresh, {:ok, data}) do
    preferences = socket.assigns.pending_preferences
    socket = assign(socket, pending_preferences: nil, preferences_ready: true)

    reply(socket, fn state ->
      reconciled = Projects.reconcile(state, data.active, data.archived)

      team =
        if Enum.any?(data.teams, &(&1.id == state.active_team)),
          do: state.active_team,
          else: (List.first(data.teams) || %{id: ""}).id

      state = %{reconciled | teams: data.teams, active_team: team, uncertain: false}
      if preferences, do: Projects.restore_preferences(state, preferences), else: state
    end)
  end

  defp complete(socket, :lookup, {:ok, p}) do
    reply(socket, fn state ->
      known =
        if p.status == "disabled",
          do: Enum.uniq([p.id | state.known_archived]) |> Enum.take(20),
          else: state.known_archived

      %{
        state
        | projects: [p | Enum.reject(state.projects, &(&1.id == p.id))],
          known_archived: known,
          selected: p.id,
          dialog: nil,
          scope: if(p.status == "disabled", do: "archived", else: "active"),
          query: ""
      }
    end)
  end

  defp complete(socket, {:write, mode}, {:ok, p}) do
    state = socket.assigns.projects
    # Lifecycle changes are organization-wide. Retain identity only and fetch each
    # alias through Rust instead of copying the mutating row's permission.
    known =
      Enum.filter(state.projects, &(&1.status == "disabled" || &1.project_id == p.project_id))

    known = Enum.uniq_by([p | known], & &1.id) |> Enum.take(20)
    id = socket.assigns.session_id
    scope = socket.assigns.current_scope

    socket =
      update(
        socket,
        :projects,
        &%{
          &1
          | dialog: nil,
            selected: nil,
            projects: [],
            scope: "active",
            query: ""
        }
      )

    {:noreply, begin(socket, {:reconcile, mode}, fn -> Projects.refresh(id, scope, known) end)}
  end

  defp complete(socket, {:reconcile, mode}, result) do
    case complete(socket, :refresh, result) do
      {:noreply, updated} when is_tuple(result) and elem(result, 0) == :ok ->
        reply(
          updated,
          &%{
            &1
            | notice:
                "Project #{%{"create" => "created", "edit" => "renamed", "archive" => "archived", "restore" => "reactivated"}[mode]}. Gateway state reconciled."
          }
        )

      other ->
        other
    end
  end

  defp denied(socket),
    do:
      reply(
        socket,
        &%{&1 | errors: %{"form" => "Project management is unavailable for this assignment."}}
      )

  defp reply(socket, fun), do: {:noreply, socket |> update(:projects, fun) |> present()}

  defp present(socket) do
    rows = Projects.rows(socket.assigns.projects)
    socket = socket |> assign(:count, length(rows)) |> stream(:projects, rows, reset: true)

    if socket.assigns.preferences_ready && !socket.assigns.projects.busy,
      do:
        push_event(socket, "projects-preferences-state", %{
          key: socket.assigns.preference_key,
          preferences: Projects.preferences(socket.assigns.projects)
        }),
      else: socket
  end
end
