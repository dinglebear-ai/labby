defmodule LabbyConsole.ProjectsTest do
  use ExUnit.Case, async: true
  alias LabbyConsole.Projects

  def row,
    do: %{
      "project_id" => "p",
      "team_id" => "t",
      "name" => "Project",
      "status" => "active",
      "role" => "viewer",
      "policy_epoch" => 1,
      "can_manage" => false
    }

  test "Projects page exports its LiveView entry point" do
    assert Code.ensure_loaded?(LabbyConsoleWeb.ProjectsLive)
    assert function_exported?(LabbyConsoleWeb.ProjectsLive, :render, 1)
  end

  test "strict snapshots retain row permissions and unambiguous identity" do
    assert {:ok, [p]} = Projects.decode_list([row()])
    assert p.id == Base.url_encode64("t\0p", padding: false)
    refute p.can_manage
    assert {:error, _} = Projects.decode_list(%{"projects" => [row()]})
    assert {:error, _} = Projects.decode_list([Map.put(row(), "can_manage", "true")])
    assert {:error, _} = Projects.decode_list([Map.put(row(), "status", "disabled")])
  end

  test "platform manager can create in role-null Team but not infer project management" do
    data = %{
      "teams" => [
        %{
          "team_id" => "t",
          "name" => "Team",
          "status" => "active",
          "role" => nil,
          "policy_epoch" => 1,
          "membership_epoch" => 1,
          "global_revision" => 1
        }
      ]
    }

    assert {:ok, [team]} =
             Projects.decode_teams(data, %{
               "capabilities" => ["platform.manage"],
               "is_admin" => true
             })

    assert team.can_create
    assert {:ok, [viewer]} = Projects.decode_teams(data, %{})
    refute viewer.can_create
  end

  test "authoritative reconciliation removes revoked projects and clears selection" do
    {:ok, [p]} = Projects.decode_list([row()])
    state = %{Projects.new() | projects: [p], current: p.id, selected: p.id}
    result = Projects.reconcile(state, [], [])
    assert result.projects == []
    assert result.current == nil
    assert result.selected == nil
  end

  test "refresh preserves caller selection only while its authoritative active row remains" do
    {:ok, [p]} = Projects.decode_list([row()])
    state = %{Projects.new() | projects: [p], current: p.id}
    assert Projects.reconcile(state, [p], []).current == p.id
    {:ok, archived} = Projects.decode_snapshot(Map.put(row(), "status", "disabled"))
    assert Projects.reconcile(state, [], [archived]).current == nil
  end

  test "team aliases keep distinct permissions when lifecycle snapshots reconcile" do
    {:ok, admin} =
      Projects.decode_snapshot(
        row()
        |> Map.put("team_id", "admin-team")
        |> Map.put("can_manage", true)
      )

    {:ok, viewer} = Projects.decode_snapshot(row())
    state = Projects.reconcile(Projects.new(), [admin, viewer], [])
    assert Projects.find(state, admin.id).can_manage
    refute Projects.find(state, viewer.id).can_manage
    assert {:error, _} = Projects.decode_list([row(), row()])
  end

  test "wire roles match Rust project owner and Team role enums" do
    assert {:ok, %{role: "owner"}} = Projects.decode_snapshot(Map.put(row(), "role", "owner"))

    invalid = %{
      "teams" => [
        %{
          "team_id" => "t",
          "name" => "Team",
          "status" => "active",
          "role" => "viewer",
          "policy_epoch" => 1,
          "membership_epoch" => 1,
          "global_revision" => 1
        }
      ]
    }

    assert {:error, %{service: "access", action: "access.team.list"}} =
             Projects.decode_teams(invalid, %{})
  end

  test "restored preferences validate Team and active project without restoring stale authority or clearing uncertainty" do
    {:ok, [p]} = Projects.decode_list([row()])

    state = %{
      Projects.new()
      | projects: [p],
        teams: [%{id: "t", name: "Team", can_create: false}],
        active_team: "t",
        uncertain: true
    }

    restored =
      Projects.restore_preferences(state, %{
        "layout" => "grid",
        "query" => "find",
        "team" => "t",
        "current" => p.id,
        "can_manage" => true
      })

    assert restored.layout == "grid"
    assert restored.query == "find"
    assert restored.current == p.id
    assert restored.uncertain
    refute Projects.find(restored, p.id).can_manage

    invalid =
      Projects.restore_preferences(state, %{
        "team" => "untrusted",
        "current" => "missing",
        "query" => String.duplicate("x", 201)
      })

    assert invalid.active_team == "t"
    assert invalid.current == nil
    assert invalid.query == ""
    {:ok, archived} = Projects.decode_snapshot(Map.put(row(), "status", "disabled"))

    assert Projects.restore_preferences(%{state | projects: [archived]}, %{"current" => p.id}).current ==
             nil
  end
end
