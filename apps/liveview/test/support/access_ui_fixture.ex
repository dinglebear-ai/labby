defmodule LabbyConsole.AccessUIFixture do
  @moduledoc false
  use Plug.Router
  plug Plug.Parsers, parsers: [:json], json_decoder: Jason, length: 40_000
  plug :match
  plug :dispatch

  def state(owner) do
    %{
      owner: owner,
      platform: true,
      accepted_team: "t",
      visible_team_ids: nil,
      scope_rows: %{},
      errors: %{},
      absent: [],
      hold: nil,
      hold_ms: 300,
      calls: [],
      teams: [
        %{
          "team_id" => "t",
          "name" => "Fixture Team",
          "status" => "active",
          "role" => "owner",
          "policy_epoch" => 1,
          "membership_epoch" => 1,
          "global_revision" => 1
        }
      ],
      members: [
        %{
          "principal_id" => "p",
          "role" => "member",
          "status" => "active",
          "membership_epoch" => 1
        }
      ],
      assignments: [
        %{
          "project_id" => "project",
          "role" => "admin",
          "status" => "active",
          "assignment_epoch" => 1,
          "project_status" => "active",
          "project_policy_epoch" => 1
        }
      ],
      invitations: [],
      admins: [%{"principal_id" => "p", "status" => "active", "authority_epoch" => 1}],
      credentials: [],
      revision: 1
    }
  end

  post "/auth/bearer-session" do
    conn |> put_resp_header("set-cookie", "caller=access-ui; HttpOnly") |> send_resp(200, "{}")
  end

  get "/auth/session" do
    platform = Agent.get(conn.private.fixture, & &1.platform)

    reply(conn, 200, %{
      authenticated: true,
      expires_at: System.system_time(:second) + 600,
      csrf_token: "access-ui-csrf",
      user: %{sub: "access-ui"},
      is_admin: platform,
      capabilities: if(platform, do: ["platform.manage"], else: [])
    })
  end

  post("/auth/logout", do: send_resp(conn, 204, ""))

  post "/v1/access/admin" do
    action = conn.body_params["action"]
    params = conn.body_params["params"] || %{}
    state = conn.private.fixture
    safe = Map.delete(params, "token")
    s = Agent.get_and_update(state, fn s -> {s, %{s | calls: [{action, safe} | s.calls]}} end)
    if is_pid(s.owner), do: send(s.owner, {:access_ui_dispatch, action, safe})
    if action in List.wrap(s.hold), do: Process.sleep(s.hold_ms)

    {status, data} =
      case s.errors[action] do
        nil ->
          case validate_request(s, action, params) do
            :ok -> Agent.get_and_update(state, &dispatch(&1, action, params))
            error -> error
          end

        {status, kind, effects, metadata} ->
          {status,
           Map.merge(
             %{
               kind: kind,
               service: "access",
               action: action,
               contract_version: 1,
               side_effects: effects
             },
             metadata
           )}
      end

    reply(conn, status, data)
  end

  match(_, do: send_resp(conn, 404, "{}"))

  defp scoped_rows(s, key, p), do: get_in(s.scope_rows, [p["team_id"], key]) || s[key]

  defp validate_request(s, action, p) do
    inventories =
      ~w(access.team.member.list access.team_project.list access.team_invitation.list access.platform_admin.list)

    scoped =
      action in (inventories -- ["access.platform_admin.list"]) ||
        action == "access.gateway_credential.list"

    team = Enum.find(s.teams, &(&1["team_id"] == p["team_id"]))

    cond do
      action in inventories && !valid_page_request?(action, p) ->
        fixture_error(action, 400, "invalid_param", %{})

      action in inventories && !is_nil(p["snapshot_revision"]) &&
          p["snapshot_revision"] != s.revision ->
        fixture_error(action, 409, "conflict", %{existing_id: "access_inventory"})

      scoped && is_nil(team) ->
        fixture_error(action, 403, "forbidden", %{})

      !s.platform && action == "access.platform_admin.list" ->
        fixture_error(action, 403, "forbidden", %{})

      !s.platform && scoped &&
          (team["status"] != "active" ||
             (!is_nil(s.visible_team_ids) && p["team_id"] not in s.visible_team_ids)) ->
        fixture_error(action, 403, "forbidden", %{})

      !s.platform && action in ~w(access.team.suspend access.team.activate) &&
          (is_nil(team) || team["status"] != "active" || team["role"] not in ~w(owner admin)) ->
        fixture_error(action, 403, "forbidden", %{})

      action in ~w(access.team.member.role.set access.team.member.suspend access.team.member.remove) &&
          p["principal_id"] == "protected-owner" ->
        fixture_error(action, 409, "conflict", %{existing_id: "team_owner"})

      true ->
        :ok
    end
  end

  defp valid_page_request?(action, p) do
    allowed =
      if action == "access.platform_admin.list",
        do: ~w(limit offset snapshot_revision),
        else: ~w(team_id limit offset snapshot_revision)

    limit = p["limit"] || 50
    offset = p["offset"] || 0
    revision = p["snapshot_revision"]

    Map.keys(p) -- allowed == [] && is_integer(limit) && limit in 1..100 && is_integer(offset) &&
      offset >= 0 &&
      (is_nil(revision) || (is_integer(revision) && revision >= 0)) &&
      (offset == 0 || !is_nil(revision))
  end

  defp fixture_error(action, status, kind, metadata),
    do:
      {status,
       Map.merge(
         %{
           contract_version: 1,
           service: "access",
           action: action,
           kind: kind,
           side_effects: "none_expected"
         },
         metadata
       )}

  defp dispatch(s, "help", _) do
    names =
      ~w(access.team.list access.project.effective.list access.gateway_credential.list access.team.member.list access.team_project.list access.team_invitation.list access.platform_admin.list access.team.create access.team.member.add access.team.member.role.set access.team.member.suspend access.team.member.remove access.team.member.activate access.team.suspend access.team.activate access.team_invitation.create access.team_invitation.accept access.team_project.assign access.team_project.unassign access.platform_admin.grant access.platform_admin.revoke access.gateway_credential.bind access.gateway_credential.revoke)

    {{200, %{service: "access", actions: Enum.map(names -- s.absent, &%{name: &1})}}, s}
  end

  defp dispatch(s, "access.team.list", _) do
    rows =
      if is_nil(s.visible_team_ids),
        do: s.teams,
        else: Enum.filter(s.teams, &(&1["team_id"] in s.visible_team_ids))

    {{200, %{teams: rows}}, s}
  end

  defp dispatch(s, "access.project.effective.list", _), do: {{200, %{projects: []}}, s}

  defp dispatch(s, "access.gateway_credential.list", p),
    do: {{200, %{bindings: scoped_rows(s, :credentials, p)}}, s}

  defp dispatch(s, action, p)
       when action in ~w(access.team.member.list access.team_project.list access.team_invitation.list access.platform_admin.list) do
    {key, root} =
      case action do
        "access.team.member.list" -> {:members, "members"}
        "access.team_project.list" -> {:assignments, "assignments"}
        "access.team_invitation.list" -> {:invitations, "invitations"}
        _ -> {:admins, "administrators"}
      end

    offset = p["offset"] || 0
    limit = p["limit"] || 50
    source = scoped_rows(s, key, p)
    rows = Enum.slice(source, offset, limit)
    next = if offset + length(rows) < length(source), do: offset + length(rows), else: nil
    data = %{root => rows, "global_revision" => s.revision, "next_offset" => next}
    team = Enum.find(s.teams, &(&1["team_id"] == p["team_id"]))

    data =
      case key do
        :members ->
          Map.merge(data, %{
            "team_id" => p["team_id"],
            "team_status" => team["status"],
            "team_policy_epoch" => 1,
            "team_membership_epoch" => 1
          })

        :assignments ->
          Map.merge(data, %{"team_id" => p["team_id"], "team_status" => team["status"]})

        :invitations ->
          Map.merge(data, %{
            "team_id" => p["team_id"],
            "team_membership_epoch" => 1,
            "observed_at" => 100
          })

        :admins ->
          data
      end

    {{200, data}, s}
  end

  defp dispatch(s, "access.team.create", p) do
    row = Map.merge(hd(s.teams), Map.take(p, ~w(team_id name)))
    {{200, row}, %{s | teams: s.teams ++ [row]}}
  end

  defp dispatch(s, "access.team.member." <> suffix, p) do
    old = Enum.find(s.members, &(&1["principal_id"] == p["principal_id"])) || hd(s.members)

    status =
      case suffix do
        "suspend" -> "suspended"
        "remove" -> "revoked"
        "role.set" -> old["status"]
        _ -> "active"
      end

    row = Map.merge(old, Map.take(p, ~w(principal_id role))) |> Map.put("status", status)
    members = [row | Enum.reject(s.members, &(&1["principal_id"] == p["principal_id"]))]

    data =
      if suffix in ["add", "activate"],
        do: Map.put(row, "team_id", p["team_id"]),
        else: %{"ok" => true}

    visible =
      if !s.platform && p["principal_id"] == "p" && suffix in ["remove", "suspend"],
        do: Enum.map(s.teams, & &1["team_id"]) -- [p["team_id"]],
        else: s.visible_team_ids

    {{200, data}, %{s | members: members, revision: s.revision + 1, visible_team_ids: visible}}
  end

  defp dispatch(s, action, _) when action in ~w(access.team.suspend access.team.activate) do
    status = if action == "access.team.suspend", do: "suspended", else: "active"

    {{200, %{"ok" => true}},
     %{s | teams: Enum.map(s.teams, &Map.put(&1, "status", status)), revision: s.revision + 1}}
  end

  defp dispatch(s, "access.team_invitation.create", p) do
    row =
      Map.merge(Map.take(p, ~w(principal_id role)), %{
        "status" => "pending",
        "team_membership_epoch" => 1,
        "created_at" => 100,
        "expires_at" => 100 + p["ttl_seconds"],
        "accepted_at" => nil,
        "revoked_at" => nil,
        "acceptance_state" => "pending"
      })

    data =
      Map.take(row, ~w(role status team_membership_epoch expires_at))
      |> Map.put("team_id", p["team_id"])

    {{200, data}, %{s | invitations: [row | s.invitations], revision: s.revision + 1}}
  end

  defp dispatch(s, "access.team_invitation.accept", _) do
    row = Map.put(hd(s.members), "team_id", s.accepted_team)

    visible =
      if is_nil(s.visible_team_ids),
        do: nil,
        else: Enum.uniq(s.visible_team_ids ++ [s.accepted_team])

    {{200, row}, %{s | visible_team_ids: visible}}
  end

  defp dispatch(s, "access.team_project." <> suffix, p) do
    row =
      Map.merge(hd(s.assignments), Map.take(p, ~w(project_id role)))
      |> Map.put("status", if(suffix == "unassign", do: "revoked", else: "active"))

    data = Map.put(row, "team_id", p["team_id"])
    {{200, data}, %{s | assignments: [row], revision: s.revision + 1}}
  end

  defp dispatch(s, "access.platform_admin." <> suffix, p) do
    row = %{
      "principal_id" => p["principal_id"],
      "status" => if(suffix == "grant", do: "active", else: "revoked"),
      "authority_epoch" => 2
    }

    {{200, %{"ok" => true}}, %{s | admins: [row], revision: s.revision + 1}}
  end

  defp dispatch(s, "access.gateway_credential." <> suffix, p) do
    row =
      Map.merge(
        Map.merge(
          %{"binding_id" => "fixture-reference"},
          Map.take(p, ~w(team_id upstream_name binding_id))
        ),
        %{
          "status" => if(suffix == "bind", do: "active", else: "revoked"),
          "generation" => 2,
          "rotated_at_millis" => 100,
          "custodian_principal_id" => "p"
        }
      )

    {{200, if(suffix == "bind", do: row, else: %{"binding" => row})}, %{s | credentials: [row]}}
  end

  defp dispatch(s, _, _), do: {{400, %{}}, s}

  defp reply(conn, status, data),
    do:
      conn |> put_resp_content_type("application/json") |> send_resp(status, Jason.encode!(data))

  def init(opts), do: opts
  def call(conn, opts), do: super(put_private(conn, :fixture, opts[:state]), opts)
end
