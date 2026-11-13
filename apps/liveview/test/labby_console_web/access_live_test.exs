defmodule LabbyConsoleWeb.AccessLiveTest do
  use LabbyConsoleWeb.ConnCase
  import Phoenix.LiveViewTest

  setup do
    owner = self()
    state = start_supervised!({Agent, fn -> LabbyConsole.AccessUIFixture.state(owner) end})

    server =
      start_supervised!(
        {Bandit, plug: {LabbyConsole.AccessUIFixture, state: state}, port: 0, ip: {127, 0, 0, 1}}
      )

    {:ok, {_, port}} = ThousandIsland.listener_info(server)
    old = Application.get_env(:labby_console, :gateway_url)
    Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:#{port}")
    on_exit(fn -> Application.put_env(:labby_console, :gateway_url, old) end)
    %{state: state}
  end

  test "normal Access entry requires a caller session", %{conn: conn} do
    assert {:error, {:redirect, %{to: "/sign-in"}}} = live(conn, "/access")
  end

  test "normal route independently loads all five workflow groups", %{conn: conn} do
    signed = post(conn, "/sign-in", %{credential: "access-ui"})
    {:ok, view, _} = live(recycle(signed), "/access")
    wait(view, "#access-card-members[data-state=loaded]")

    for key <- ~w(invitations admins credentials assignments) do
      wait(view, "#access-card-#{key}[data-state=loaded]")
    end

    assert has_element?(view, "h1", "People & teams")
    refute has_element?(view, "#access-create-team[disabled]")
    assert has_element?(view, "a[href='/access'][aria-current=page]")
  end

  test "accepted mutation with failed affected read stays held across unrelated refresh", %{
    conn: conn,
    state: state
  } do
    signed = post(conn, "/sign-in", %{credential: "access-ui"})
    {:ok, view, _} = live(recycle(signed), "/access")
    wait(view, "#access-card-members[data-state=loaded]")

    Agent.update(
      state,
      &put_in(
        &1.errors["access.team.member.list"],
        {503, "service_unavailable", "none_expected", %{}}
      )
    )

    open(view, "access.team.member.suspend")

    view
    |> form("#access-action-form",
      access_request: %{team_id: "t", principal_id: "p"},
      confirmation: "yes"
    )
    |> render_submit()

    wait(view, "#access-receipt[data-status=accepted]")
    wait(view, "#access-card-members[data-state=stale]")
    view |> element("#access-refresh-admins") |> render_click()
    wait(view, "#access-card-admins[data-state=loaded]")
    assert has_element?(view, "#access-receipt[data-status=accepted]")
    assert has_element?(view, "#access-create-team[disabled]")

    assert Enum.count(Agent.get(state, & &1.calls), fn {a, _} ->
             a == "access.team.member.suspend"
           end) == 1
  end

  test "missing finite backend features stay disabled and unknown discovery entries never appear",
       %{conn: conn, state: state} do
    Agent.update(
      state,
      &%{
        &1
        | absent: [
            "access.team.member.activate",
            "access.team_project.unassign",
            "access.gateway_credential.bind"
          ]
      }
    )

    signed = post(conn, "/sign-in", %{credential: "access-ui"})
    {:ok, view, _} = live(recycle(signed), "/access")
    wait(view, "#access-card-members[data-state=loaded]")

    for name <-
          ~w(access-team-member-activate access-team_project-unassign access-gateway_credential-bind) do
      assert has_element?(view, "#access-action-#{name}[disabled]")
    end
  end

  test "last-owner conflict is typed and retains the valid caller session", %{
    conn: conn,
    state: state
  } do
    signed = post(conn, "/sign-in", %{credential: "access-ui"})
    {:ok, view, _} = live(recycle(signed), "/access")
    wait(view, "#access-card-members[data-state=loaded]")

    Agent.update(
      state,
      &put_in(
        &1.errors["access.team.member.remove"],
        {409, "conflict", "none_expected", %{existing_id: "team_owner"}}
      )
    )

    open(view, "access.team.member.remove")

    view
    |> form("#access-action-form",
      access_request: %{team_id: "t", principal_id: "p"},
      confirmation: "yes"
    )
    |> render_submit()

    wait(view, "#access-receipt[data-status=failed]")
    assert render(view) =~ "last owner is protected"
    assert has_element?(view, "#access-main")
  end

  test "manual inventory pages preserve revision, conflict refresh starts at zero, and terminal overflow stops",
       %{conn: conn, state: state} do
    Agent.update(state, fn s ->
      %{s | members: for(id <- ~w(p q r), do: Map.put(hd(s.members), "principal_id", id))}
    end)

    signed = post(conn, "/sign-in", %{credential: "access-ui"})
    {:ok, view, _} = live(recycle(signed), "/access")
    wait(view, "#access-card-members[data-state=loaded]")

    view
    |> form("#access-paging-members", paging: %{limit: "1", card: "members"})
    |> render_submit()

    wait(view, "#access-card-members[data-state=partial]")
    view |> element("#access-next-members") |> render_click()
    wait(view, "#access-card-members[data-state=partial]")

    assert_receive {:access_ui_dispatch, "access.team.member.list",
                    %{"offset" => 1, "limit" => 1, "snapshot_revision" => 1}}

    Agent.update(
      state,
      &put_in(
        &1.errors["access.team.member.list"],
        {409, "conflict", "none_expected", %{existing_id: "access_inventory"}}
      )
    )

    view |> element("#access-next-members") |> render_click()
    wait(view, "#access-card-members[data-state=stale]")
    assert render(view) =~ "Refresh from the first page"
    Agent.update(state, &%{&1 | errors: %{}})
    view |> element("#access-refresh-members") |> render_click()
    wait(view, "#access-card-members[data-state=partial]")

    assert_receive {:access_ui_dispatch, "access.team.member.list",
                    %{"offset" => 0, "limit" => 1}}

    Agent.update(
      state,
      &put_in(
        &1.errors["access.team.member.list"],
        {413, "response_too_large", "none_expected", %{budget_terminal: true}}
      )
    )

    view |> element("#access-refresh-members") |> render_click()
    wait(view, "#access-card-members[data-state=stale]")
    assert has_element?(view, "#access-refresh-members[disabled]")
    before = length(Agent.get(state, & &1.calls))
    render_click(view, "access_refresh", %{"card" => "members"})
    Process.sleep(30)
    assert length(Agent.get(state, & &1.calls)) == before
  end

  test "secret validation failure sends a nonsecret result so the browser can submit a correction",
       %{conn: conn} do
    signed = post(conn, "/sign-in", %{credential: "access-ui"})
    {:ok, view, _} = live(recycle(signed), "/access")
    wait(view, "#access-card-members[data-state=loaded]")
    open(view, "access.team_invitation.accept")

    render_submit(view, "access_submit", %{
      "access_request" => %{"token" => String.duplicate("0", 64)},
      "confirmation" => "yes",
      "nonce" => "correction"
    })

    assert_push_event(view, "access-secret-result", %{nonce: "correction", accepted: false})
    assert has_element?(view, "#access-dialog")
  end

  test "delayed replaced reads cannot cross Team scope and pending operations stay bounded", %{
    conn: conn,
    state: state
  } do
    Agent.update(state, fn s ->
      %{
        s
        | teams:
            s.teams ++ [Map.merge(hd(s.teams), %{"team_id" => "second", "name" => "Second Team"})],
          hold: "access.team.member.list"
      }
    end)

    signed = post(conn, "/sign-in", %{credential: "access-ui"})
    {:ok, view, _} = live(recycle(signed), "/access")
    wait(view, "#access-card-members[data-state=loading]")
    refs = :sys.get_state(view.pid).socket.assigns.access_requests
    assert map_size(refs) <= 2
    view |> form("#access-team-form", scope: %{team: "second"}) |> render_change()
    for {ref, _} <- refs, do: send(view.pid, {:gateway_action, ref, {:ok, %{"teams" => []}}})
    wait(view, "#access-card-members[data-state=loaded]")
    assert :sys.get_state(view.pid).socket.assigns.access.team == "second"
    assert has_element?(view, "#access-team option[value=second][selected]")
  end

  test "forbidden write removes affected permission snapshots while preserving login", %{
    conn: conn,
    state: state
  } do
    signed = post(conn, "/sign-in", %{credential: "access-ui"})
    {:ok, view, _} = live(recycle(signed), "/access")
    wait(view, "#access-card-members[data-state=loaded]")

    Agent.update(
      state,
      &put_in(&1.errors["access.team.member.remove"], {403, "forbidden", "none_expected", %{}})
    )

    open(view, "access.team.member.remove")

    view
    |> form("#access-action-form",
      access_request: %{team_id: "t", principal_id: "p"},
      confirmation: "yes"
    )
    |> render_submit()

    wait(view, "#access-receipt[data-status=failed]")
    assert :sys.get_state(view.pid).socket.assigns.access.cards.members.rows == []
    assert :sys.get_state(view.pid).socket.assigns.access.cards.teams.rows == []
    assert has_element?(view, "#access-main")
  end

  test "malformed client field containers cannot crash the native view or dispatch a write", %{
    conn: conn,
    state: state
  } do
    signed = post(conn, "/sign-in", %{credential: "access-ui"})
    {:ok, view, _} = live(recycle(signed), "/access")
    wait(view, "#access-card-members[data-state=loaded]")
    open(view, "access.team.member.add")
    render_submit(view, "access_submit", %{"access_request" => [], "confirmation" => "yes"})
    render_submit(view, "access_limit", %{"paging" => %{"card" => "members", "limit" => []}})
    assert has_element?(view, "#access-main")

    refute Enum.any?(Agent.get(state, & &1.calls), fn {action, _} ->
             action == "access.team.member.add"
           end)
  end

  test "a write cancels unrelated pending reads into a retryable card instead of a jobless spinner",
       %{conn: conn, state: state} do
    Agent.update(state, &%{&1 | hold: "access.platform_admin.list", hold_ms: 1000})
    signed = post(conn, "/sign-in", %{credential: "access-ui"})
    {:ok, view, _} = live(recycle(signed), "/access")
    wait(view, "#access-card-members[data-state=loaded]")
    assert has_element?(view, "#access-card-admins[data-state=loading]")
    open(view, "access.team.member.suspend")

    view
    |> form("#access-action-form",
      access_request: %{team_id: "t", principal_id: "p"},
      confirmation: "yes"
    )
    |> render_submit()

    wait(view, "#access-receipt[data-status=reconciled]")
    assert :sys.get_state(view.pid).socket.assigns.access.cards.admins.status in [:error, :stale]
    refute has_element?(view, "#access-refresh-admins[disabled]")
    Agent.update(state, &%{&1 | hold: nil})
    view |> element("#access-refresh-admins") |> render_click()
    wait(view, "#access-card-admins[data-state=loaded]")
  end

  for {card, key, action} <- [
        {"members", :members, "access.team.member.list"},
        {"assignments", :assignments, "access.team_project.list"},
        {"invitations", :invitations, "access.team_invitation.list"},
        {"admins", :admins, "access.platform_admin.list"}
      ] do
    @page_card card
    @page_key key
    @page_action action
    test "R1 #{@page_card} sends snapshot_revision and rejects changed inventory revision", %{
      conn: conn,
      state: state
    } do
      Agent.update(state, fn s ->
        row =
          if @page_key == :invitations,
            do: %{
              "principal_id" => "p",
              "role" => "member",
              "status" => "pending",
              "team_membership_epoch" => 1,
              "created_at" => 1,
              "expires_at" => 200,
              "accepted_at" => nil,
              "revoked_at" => nil,
              "acceptance_state" => "pending"
            },
            else: hd(s[@page_key])

        Map.put(s, @page_key, List.duplicate(row, 3))
      end)

      signed = post(conn, "/sign-in", %{credential: "access-ui"})
      {:ok, view, _} = live(recycle(signed), "/access")
      wait(view, "#access-card-#{@page_card}[data-state=loaded]")

      view
      |> form("#access-paging-#{@page_card}", paging: %{card: @page_card, limit: "1"})
      |> render_submit()

      wait(view, "#access-card-#{@page_card}[data-state=partial]")
      view |> element("#access-next-#{@page_card}") |> render_click()
      wait(view, "#access-card-#{@page_card}[data-state=partial]")

      assert_receive {:access_ui_dispatch, @page_action,
                      %{"offset" => 1, "snapshot_revision" => 1}}

      Agent.update(state, &%{&1 | revision: 2})
      view |> element("#access-next-#{@page_card}") |> render_click()
      wait(view, "#access-card-#{@page_card}[data-state=stale]")
      assert render(view) =~ "Refresh from the first page"
    end
  end

  test "R2 accepting into another Team clears every old scoped card and updates LiveSession", %{
    conn: conn,
    state: state
  } do
    Agent.update(state, fn s ->
      %{
        s
        | accepted_team: "b",
          visible_team_ids: ["t"],
          teams: s.teams ++ [Map.merge(hd(s.teams), %{"team_id" => "b", "name" => "Team B"})],
          scope_rows: %{
            "t" => %{assignments: [Map.put(hd(s.assignments), "project_id", "project-A")]},
            "b" => %{assignments: [Map.put(hd(s.assignments), "project_id", "project-B")]}
          }
      }
    end)

    signed = post(conn, "/sign-in", %{credential: "access-ui"})
    {:ok, view, _} = live(recycle(signed), "/access")
    wait(view, "#access-card-assignments[data-state=loaded]")
    assert render(view) =~ "project-A"
    open(view, "access.team_invitation.accept")

    render_submit(view, "access_submit", %{
      "access_request" => %{"token" => String.duplicate("ab", 32)},
      "confirmation" => "yes",
      "nonce" => "other-team"
    })

    wait(view, "#access-receipt[data-status=reconciled]")
    wait(view, "#access-card-assignments[data-state=loaded]")
    assigns = :sys.get_state(view.pid).socket.assigns
    assert assigns.workspace_scope == %{team_id: "b"}
    assert assigns.access.team == "b"
    assert render(view) =~ "project-B"
    refute render(view) =~ "project-A"

    for card <- ~w(members assignments invitations credentials)a,
        do: assert(assigns.access.cards[card].status == :loaded)
  end

  test "R2 authoritative selected Team loss clears scoped data and fences late old results", %{
    conn: conn,
    state: state
  } do
    signed = post(conn, "/sign-in", %{credential: "access-ui"})
    {:ok, view, _} = live(recycle(signed), "/access")
    wait(view, "#access-card-assignments[data-state=loaded]")
    Agent.update(state, &%{&1 | visible_team_ids: [], hold: "access.team_project.list"})
    view |> element("#access-refresh-assignments") |> render_click()
    old = :sys.get_state(view.pid).socket.assigns.access_requests
    render_click(view, "access_refresh", %{"card" => "teams"})
    wait(view, "#access-card-teams[data-state=loaded]")
    assigns = :sys.get_state(view.pid).socket.assigns
    assert is_nil(assigns.access.team)
    assert assigns.workspace_scope == %{}

    for card <- ~w(members assignments invitations credentials)a,
        do: assert(assigns.access.cards[card].rows == [])

    for {ref, _} <- old,
        do:
          send(view.pid, {:gateway_action, ref, {:ok, %{"team_id" => "t", "assignments" => []}}})

    assert is_nil(:sys.get_state(view.pid).socket.assigns.access.team)
  end

  test "R5 nonplatform administrator can suspend an active Team but cannot restore it", %{
    conn: conn,
    state: state
  } do
    Agent.update(state, &%{&1 | platform: false, teams: [Map.put(hd(&1.teams), "role", "admin")]})
    signed = post(conn, "/sign-in", %{credential: "access-ui"})
    {:ok, view, _} = live(recycle(signed), "/access")
    wait(view, "#access-card-members[data-state=loaded]")
    refute has_element?(view, "#access-action-access-team-suspend[disabled]")
    open(view, "access.team.suspend")

    view
    |> form("#access-action-form", access_request: %{team_id: "t"}, confirmation: "yes")
    |> render_submit()

    wait(view, "#access-card-teams[data-state=loaded]")

    assert LabbyConsole.Access.selected(:sys.get_state(view.pid).socket.assigns.access)["status"] ==
             "suspended"

    assert has_element?(view, "#access-action-access-team-activate[disabled]")
    assert_receive {:access_ui_dispatch, "access.team.suspend", %{"team_id" => "t"}}
    refute Enum.any?(Agent.get(state, & &1.calls), fn {a, _} -> a == "access.team.activate" end)
  end

  test "R6 a write leaves never-started scoped cards manually retryable", %{
    conn: conn,
    state: state
  } do
    Agent.update(
      state,
      &%{&1 | hold: ~w(access.project.effective.list access.platform_admin.list), hold_ms: 1000}
    )

    signed = post(conn, "/sign-in", %{credential: "access-ui"})
    {:ok, view, _} = live(recycle(signed), "/access")
    wait(view, "#access-card-admins[data-state=loading]")
    assert :sys.get_state(view.pid).socket.assigns.access.cards.invitations.status == :idle
    open(view, "access.team.member.suspend")

    view
    |> form("#access-action-form",
      access_request: %{team_id: "t", principal_id: "p"},
      confirmation: "yes"
    )
    |> render_submit()

    wait(view, "#access-receipt[data-status=reconciled]")

    for card <- ~w(invitations credentials assignments),
        do: refute(has_element?(view, "#access-refresh-#{card}[disabled]"))
  end

  test "R6 manual Team switch keeps canceled unscoped cards retryable", %{
    conn: conn,
    state: state
  } do
    Agent.update(
      state,
      &%{
        &1
        | hold: ~w(access.project.effective.list access.platform_admin.list),
          hold_ms: 1000,
          teams: &1.teams ++ [Map.merge(hd(&1.teams), %{"team_id" => "b", "name" => "Team B"})]
      }
    )

    signed = post(conn, "/sign-in", %{credential: "access-ui"})
    {:ok, view, _} = live(recycle(signed), "/access")
    wait(view, "#access-card-admins[data-state=loading]")
    view |> form("#access-team-form", scope: %{team: "b"}) |> render_change()
    wait(view, "#access-card-members[data-state=loaded]")
    assigns = :sys.get_state(view.pid).socket.assigns
    assert assigns.workspace_scope == %{team_id: "b"}

    for card <- [:admins, :effective],
        do: assert(assigns.access.cards[card].status in [:error, :stale])
  end

  test "authorized credential reference and custodian metadata survive reload and changed identities",
       %{conn: conn, state: state} do
    binding = %{
      "team_id" => "t",
      "upstream_name" => "up",
      "binding_id" => "reference-a",
      "custodian_principal_id" => "custodian-id",
      "generation" => 4,
      "rotated_at_millis" => 5,
      "status" => "active"
    }

    Agent.update(state, &%{&1 | credentials: [binding]})
    signed = post(conn, "/sign-in", %{credential: "access-ui"})
    {:ok, view, _} = live(recycle(signed), "/access")
    wait(view, "#access-card-credentials[data-state=loaded]")
    assert has_element?(view, "#access-card-credentials", "reference-a")
    assert has_element?(view, "#access-card-credentials", "custodian-id")

    Agent.update(
      state,
      &%{
        &1
        | credentials: [Map.merge(binding, %{"binding_id" => "reference-b", "generation" => 5})]
      }
    )

    view |> element("#access-refresh-credentials") |> render_click()
    wait(view, "#access-card-credentials[data-state=loaded]")
    assert has_element?(view, "#access-card-credentials", "reference-b")
    refute has_element?(view, "#access-card-credentials", "reference-a")
  end

  test "R2 self-removal preserves accepted receipt and clears every lost Team projection", %{
    conn: conn,
    state: state
  } do
    Agent.update(state, &%{&1 | platform: false, teams: [Map.put(hd(&1.teams), "role", "admin")]})
    signed = post(conn, "/sign-in", %{credential: "access-ui"})
    {:ok, view, _} = live(recycle(signed), "/access")
    wait(view, "#access-card-assignments[data-state=loaded]")
    open(view, "access.team.member.remove")

    view
    |> form("#access-action-form",
      access_request: %{team_id: "t", principal_id: "p"},
      confirmation: "yes"
    )
    |> render_submit()

    wait(view, "#access-receipt[data-status=visibility_lost]")
    wait(view, "#access-card-teams[data-state=loaded]")
    assigns = :sys.get_state(view.pid).socket.assigns
    assert is_nil(assigns.access.team)
    assert assigns.workspace_scope == %{}
    assert assigns.access.receipt.action == "access.team.member.remove"

    for card <- ~w(members assignments invitations credentials)a,
        do: assert(assigns.access.cards[card].rows == [])

    assert render(view) =~ "cannot be inspected"
  end

  for {label, failure} <- [
        {"unavailable", {503, "service_unavailable", "none_expected", %{}}},
        {"timeout", {504, "timeout", "none_expected", %{}}},
        {"malformed success", {200, "invalid_param", "none_expected", %{}}}
      ] do
    @team_failure failure
    test "R2 accepted cross-Team scope survives #{label} Team inventory", %{
      conn: conn,
      state: state
    } do
      Agent.update(state, fn s ->
        %{
          s
          | accepted_team: "b",
            visible_team_ids: ["t"],
            teams: s.teams ++ [Map.merge(hd(s.teams), %{"team_id" => "b", "name" => "Team B"})]
        }
      end)

      signed = post(conn, "/sign-in", %{credential: "access-ui"})
      {:ok, view, _} = live(recycle(signed), "/access")
      wait(view, "#access-card-members[data-state=loaded]")
      Agent.update(state, &put_in(&1.errors["access.team.list"], @team_failure))
      open(view, "access.team_invitation.accept")

      render_submit(view, "access_submit", %{
        "access_request" => %{"token" => String.duplicate("ab", 32)},
        "confirmation" => "yes",
        "nonce" => "pending-team"
      })

      wait(view, "#access-card-teams[data-state=stale]")
      assigns = :sys.get_state(view.pid).socket.assigns
      assert assigns.access.team == "b"
      assert assigns.workspace_scope == %{team_id: "b"}
      assert assigns.access.receipt.status == :accepted
      assert has_element?(view, "#access-team option[value=b][selected]", "b")
      refute has_element?(view, "#access-team option[value=b]", "Team B")
      Agent.update(state, &%{&1 | errors: %{}})
      render_click(view, "access_refresh", %{"card" => "affected"})
      wait(view, "#access-receipt[data-status=reconciled]")
    end
  end

  @forms [
    {"access.team.create", %{"team_id" => "new-team", "name" => "Created Team"}, :reconciled},
    {"access.team.member.add",
     %{"team_id" => "t", "principal_id" => "new-principal", "role" => "member"}, :reconciled},
    {"access.team.member.role.set", %{"team_id" => "t", "principal_id" => "p", "role" => "admin"},
     :reconciled},
    {"access.team.member.suspend", %{"team_id" => "t", "principal_id" => "p"}, :reconciled},
    {"access.team.member.remove", %{"team_id" => "t", "principal_id" => "p"}, :reconciled},
    {"access.team.member.activate",
     %{"team_id" => "t", "principal_id" => "p", "role" => "member"}, :reconciled},
    {"access.team.suspend", %{"team_id" => "t"}, :reconciled},
    {"access.team.activate", %{"team_id" => "t"}, :reconciled},
    {"access.team_invitation.create",
     %{"team_id" => "t", "principal_id" => "p", "role" => "member", "ttl_seconds" => "60"},
     :accepted_observed},
    {"access.team_invitation.accept", %{}, :reconciled},
    {"access.team_project.assign",
     %{"team_id" => "t", "project_id" => "project", "role" => "viewer"}, :reconciled},
    {"access.team_project.unassign", %{"team_id" => "t", "project_id" => "project"}, :reconciled},
    {"access.platform_admin.grant", %{"principal_id" => "p"}, :reconciled},
    {"access.platform_admin.revoke", %{"principal_id" => "p"}, :reconciled},
    {"access.gateway_credential.bind",
     %{"team_id" => "t", "upstream_name" => "up", "binding_id" => "host-ref"}, :reconciled},
    {"access.gateway_credential.revoke", %{"team_id" => "t", "upstream_name" => "up"},
     :reconciled}
  ]
  for {action, params, status} <- @forms do
    @action action
    @params params
    @status status
    test "normal form dispatches and reads back #{@action}", %{conn: conn, state: state} do
      signed = post(conn, "/sign-in", %{credential: "access-ui"})
      {:ok, view, _} = live(recycle(signed), "/access")
      wait(view, "#access-card-members[data-state=loaded]")
      open(view, @action)
      assert has_element?(view, "#access-dialog")

      if @action in ~w(access.team_invitation.create access.team_invitation.accept) do
        assert has_element?(view, "#access-field-token:not([name]):not([value])")

        render_submit(view, "access_submit", %{
          "access_request" => Map.put(@params, "token", String.duplicate("cd", 32)),
          "confirmation" => "yes",
          "nonce" => "nonsecret-confirmation"
        })
      else
        view
        |> form("#access-action-form", access_request: @params, confirmation: "yes")
        |> render_submit()
      end

      wait(view, "#access-receipt[data-status=#{@status}]")
      assert Enum.count(Agent.get(state, & &1.calls), fn {a, _} -> a == @action end) == 1
      refute inspect(:sys.get_state(view.pid)) =~ String.duplicate("cd", 32)
      refute inspect(Agent.get(state, & &1)) =~ String.duplicate("cd", 32)
    end
  end

  defp open(view, action) do
    view |> element("#access-action-" <> String.replace(action, ".", "-")) |> render_click()
  end

  defp wait(view, selector, tries \\ 50)

  defp wait(view, selector, 0),
    do:
      assert(
        has_element?(view, selector),
        "Expected #{selector}. Receipt #{inspect(:sys.get_state(view.pid).socket.assigns.access.receipt)}"
      )

  defp wait(view, selector, tries) do
    if has_element?(view, selector) do
      :ok
    else
      Process.sleep(20)
      wait(view, selector, tries - 1)
    end
  end
end
