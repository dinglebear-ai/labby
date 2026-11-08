defmodule LabbyConsole.ProjectsActionsTest do
  use ExUnit.Case, async: false
  alias LabbyConsole.{Auth, Projects}

  setup do
    owner = self()
    state = start_supervised!({Agent, fn -> %{owner: owner, rows: [], reject: false} end})

    server =
      start_supervised!(
        {Bandit, plug: {LabbyConsole.ProjectsFixture, state: state}, port: 0, ip: {127, 0, 0, 1}}
      )

    {:ok, {_, port}} = ThousandIsland.listener_info(server)
    old = Application.get_env(:labby_console, :gateway_url)
    Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:#{port}")
    on_exit(fn -> Application.put_env(:labby_console, :gateway_url, old) end)
    {:ok, id} = Auth.sign_in("projects")
    %{state: state, id: id}
  end

  test "latest per-project permission prevents a stale managing snapshot from issuing a write", %{
    state: state,
    id: id
  } do
    p = %{
      "project_id" => "p",
      "team_id" => "t",
      "name" => "Project",
      "role" => "admin",
      "status" => "active",
      "policy_epoch" => 4,
      "can_manage" => false
    }

    Agent.update(state, &%{&1 | rows: [p]})

    assert {:error, %{kind: :forbidden}} =
             Projects.mutate(id, "edit", %{
               "team_id" => "t",
               "project_id" => "p",
               "name" => "Wrong"
             })

    assert_receive {:projects_dispatch, "projects.get", _}
    refute_receive {:projects_dispatch, "projects.update", _}
  end

  test "archive lookup restore uses actual action snapshots and increasing revisions", %{id: id} do
    draft = %{"team_id" => "t", "project_id" => "p", "name" => "Project"}
    assert {:ok, %{policy_epoch: 1}} = Projects.mutate(id, "create", draft)
    assert {:ok, %{status: "disabled", policy_epoch: 2}} = Projects.mutate(id, "archive", draft)
    assert {:ok, %{status: "disabled"}} = Projects.get(id, "t", "p")
    assert {:ok, %{status: "active", policy_epoch: 3}} = Projects.mutate(id, "restore", draft)
  end

  test "invalid preflight snapshot fails before dispatch without claiming uncertain write effects",
       %{state: state, id: id} do
    invalid = %{
      "project_id" => "p",
      "team_id" => "t",
      "name" => "Bad snapshot",
      "role" => "admin",
      "status" => "active",
      "policy_epoch" => 1
    }

    Agent.update(state, &%{&1 | rows: [invalid]})

    assert {:error,
            %{kind: :invalid_response, side_effects: "none_expected", action: "projects.get"}} =
             Projects.mutate(id, "edit", %{
               "team_id" => "t",
               "project_id" => "p",
               "name" => "Never sent"
             })

    refute_receive {:projects_dispatch, "projects.update", _}
  end
end
