defmodule LabbyConsole.Projects do
  @moduledoc "Strict Rust Projects projections and session-local presentation preferences."
  alias LabbyConsole.{Auth, GatewayAction.Error}

  def new do
    %{
      projects: [],
      known_archived: [],
      query: "",
      scope: "active",
      layout: "list",
      active_team: "",
      teams: [],
      current: nil,
      selected: nil,
      dialog: nil,
      errors: %{},
      notice: nil,
      busy: false,
      uncertain: false,
      draft: %{"name" => "", "project_id" => "", "team_id" => ""}
    }
  end

  def key(team, project), do: Base.url_encode64(team <> <<0>> <> project, padding: false)

  def visible(state),
    do: Enum.filter(state.projects, &(&1.status == "active" or &1.id in state.known_archived))

  def rows(state) do
    query = String.downcase(state.query)

    Enum.filter(visible(state), fn p ->
      (state.scope == "all" or (state.scope == "archived" and p.status == "disabled") or
         p.status == state.scope) and
        String.contains?(
          String.downcase(Enum.join([p.name, p.project_id, p.team_id, p.role], " ")),
          query
        )
    end)
  end

  def selected(state), do: find(state, state.selected)
  def find(state, id), do: Enum.find(state.projects, &(&1.id == id))

  def can_create?(state),
    do: Enum.any?(state.teams, &(&1.id == state.active_team and &1.can_create))

  def filter(state, params) do
    team =
      if params["team"] in Enum.map(state.teams, & &1.id),
        do: params["team"],
        else: state.active_team

    %{
      state
      | query: String.slice(params["query"] || state.query, 0, 200),
        scope: allowed(params["scope"], ~w(active archived all), state.scope),
        layout: allowed(params["layout"], ~w(list grid table), state.layout),
        active_team: team,
        current: if(team != state.active_team, do: nil, else: state.current)
    }
  end

  def open(state, mode, id \\ nil) do
    project = find(state, id)

    cond do
      mode == "create" and can_create?(state) ->
        %{
          state
          | dialog: mode,
            errors: %{},
            draft: %{"name" => "", "project_id" => "", "team_id" => state.active_team}
        }

      mode == "lookup" ->
        %{
          state
          | dialog: mode,
            errors: %{},
            draft: %{"name" => "", "project_id" => "", "team_id" => state.active_team}
        }

      mode in ~w(edit archive restore) and project != nil and project.can_manage and
          ((mode == "restore" and project.status == "disabled") or
             (mode != "restore" and project.status == "active")) ->
        %{
          state
          | dialog: mode,
            selected: id,
            errors: %{},
            draft: %{
              "name" => project.name,
              "project_id" => project.project_id,
              "team_id" => project.team_id
            }
        }

      true ->
        state
    end
  end

  def select_workspace(state, id) do
    case find(state, id) do
      %{status: "active"} = project ->
        %{
          state
          | current: project.id,
            active_team: project.team_id,
            notice:
              "Workspace selected for this caller and gateway. Project lifecycle is unchanged."
        }

      _ ->
        state
    end
  end

  def restore_preferences(state, preferences) when is_map(preferences) do
    team = preferences["team"]
    team = if Enum.any?(state.teams, &(&1.id == team)), do: team, else: state.active_team
    query = preferences["query"]

    query =
      if is_binary(query) && byte_size(query) <= 800 && String.length(query) <= 200 &&
           !Regex.match?(~r/[\x00-\x1f\x7f-\x9f]/u, query), do: query, else: state.query

    layout = allowed(preferences["layout"], ~w(list grid table), state.layout)

    current =
      case find(state, preferences["current"]) do
        %{status: "active", team_id: ^team, id: id} -> id
        _ -> nil
      end

    %{state | active_team: team, query: query, layout: layout, current: current}
  end

  def restore_preferences(state, _), do: state

  def preferences(state),
    do: %{
      layout: state.layout,
      query: state.query,
      team: state.active_team,
      current: state.current
    }

  def reconcile(state, active, archived) do
    projects = Enum.uniq_by(active ++ archived, & &1.id)
    ids = Enum.map(projects, & &1.id)
    current = Enum.find(projects, &(&1.id == state.current && &1.status == "active"))
    selected = if state.selected in ids, do: state.selected

    %{
      state
      | projects: projects,
        known_archived: Enum.map(archived, & &1.id),
        current: current && current.id,
        selected: selected,
        dialog: if(state.selected && is_nil(selected), do: nil, else: state.dialog)
    }
  end

  def decode_list(rows) when is_list(rows) and length(rows) <= 1000 do
    decode_many(rows, fn row ->
      with {:ok, p} <- decode_snapshot(row), true <- p.status == "active", do: {:ok, p}
    end)
  end

  def decode_list(_), do: invalid("projects.list")

  def decode_snapshot(row) when is_map(row) do
    if Enum.sort(Map.keys(row)) ==
         Enum.sort(~w(project_id team_id name status role policy_epoch can_manage)) &&
         Enum.all?(~w(project_id team_id name), &text?(row[&1])) &&
         row["status"] in ~w(active disabled) && row["role"] in ~w(owner admin member viewer) &&
         is_integer(row["policy_epoch"]) && row["policy_epoch"] >= 0 &&
         is_boolean(row["can_manage"]) do
      {:ok,
       %{
         id: key(row["team_id"], row["project_id"]),
         project_id: row["project_id"],
         team_id: row["team_id"],
         name: row["name"],
         status: row["status"],
         role: row["role"],
         policy_epoch: row["policy_epoch"],
         can_manage: row["can_manage"]
       }}
    else
      invalid("projects.get")
    end
  end

  def decode_snapshot(_), do: invalid("projects.get")

  def decode_teams(%{"teams" => rows}, session) when is_list(rows) and length(rows) <= 1000 do
    platform = session["is_admin"] == true && "platform.manage" in (session["capabilities"] || [])

    decode_many(
      rows,
      fn row ->
        if is_map(row) && Enum.all?(~w(team_id name), &text?(row[&1])) &&
             row["status"] in ~w(active disabled) &&
             row["role"] in [nil, "owner", "admin", "member"] &&
             Enum.all?(
               ~w(policy_epoch membership_epoch global_revision),
               &(is_integer(row[&1]) && row[&1] >= 0)
             ) do
          {:ok,
           %{
             id: row["team_id"],
             name: row["name"],
             can_create: row["status"] == "active" && (platform || row["role"] in ~w(owner admin))
           }}
        else
          invalid("access.team.list")
        end
      end,
      "access.team.list"
    )
  end

  def decode_teams(_, _), do: invalid("access.team.list")

  def refresh(session_id, session, known) do
    with {:ok, data} <- Auth.action(session_id, "access", "access.team.list", %{}),
         {:ok, teams} <- decode_teams(data, session),
         {:ok, data} <- Auth.action(session_id, "projects", "projects.list", %{}),
         {:ok, active} <- decode_list(data) do
      # Archived IDs must be re-read: retained permissions are never authority.
      archived =
        Enum.reduce(known, [], fn p, acc ->
          case get(session_id, p.team_id, p.project_id) do
            {:ok, %{status: "disabled"} = fresh} -> [fresh | acc]
            _ -> acc
          end
        end)

      {:ok, %{teams: teams, active: active, archived: archived}}
    end
  end

  def get(session_id, team, project) do
    with {:ok, row} <-
           Auth.action(session_id, "projects", "projects.get", %{
             "team_id" => team,
             "project_id" => project
           }),
         {:ok, p} <- decode_snapshot(row),
         true <- p.team_id == team && p.project_id == project do
      {:ok, p}
    else
      false -> invalid("projects.get")
      error -> error
    end
  end

  def mutate(session_id, mode, draft) when mode in ~w(create edit archive restore) do
    action =
      %{
        "create" => "projects.create",
        "edit" => "projects.update",
        "archive" => "projects.archive",
        "restore" => "projects.activate"
      }[mode]

    params =
      Map.take(
        draft,
        if(mode in ~w(create edit), do: ~w(team_id project_id name), else: ~w(team_id project_id))
      )

    with :ok <- require_current_permission(session_id, mode, params) do
      with {:ok, data} <- Auth.action(session_id, "projects", action, params),
           {:ok, p} <- decode_snapshot(data),
           true <- p.team_id == params["team_id"] && p.project_id == params["project_id"] do
        {:ok, p}
      else
        false ->
          {:error, Error.new(:invalid_response, "projects", action, side_effects: "unknown")}

        {:error, %Error{kind: :invalid_response} = error} ->
          {:error, %{error | action: action, side_effects: "unknown"}}

        error ->
          error
      end
    end
  end

  defp require_current_permission(_, "create", _), do: :ok

  defp require_current_permission(id, mode, params) do
    with {:ok, p} <- get(id, params["team_id"], params["project_id"]),
         true <- p.can_manage && p.status == if(mode == "restore", do: "disabled", else: "active") do
      :ok
    else
      false -> {:error, Error.new(:forbidden, "projects", "projects.get")}
      error -> error
    end
  end

  defp decode_many(rows, fun, action \\ "projects.list") do
    Enum.reduce_while(rows, {:ok, []}, fn row, {:ok, acc} ->
      case fun.(row) do
        {:ok, decoded} -> {:cont, {:ok, [decoded | acc]}}
        _ -> {:halt, invalid(action)}
      end
    end)
    |> case do
      {:ok, values} ->
        if length(Enum.uniq_by(values, & &1.id)) == length(values),
          do: {:ok, Enum.reverse(values)},
          else: invalid(action)

      error ->
        error
    end
  end

  defp text?(value),
    do: is_binary(value) && byte_size(value) in 1..128 && !String.contains?(value, <<0>>)

  defp invalid(action),
    do:
      {:error,
       Error.new(
         :invalid_response,
         if(String.starts_with?(action, "access."), do: "access", else: "projects"),
         action
       )}

  defp allowed(value, options, default), do: if(value in options, do: value, else: default)
end
