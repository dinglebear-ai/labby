defmodule LabbyConsoleWeb.ProjectsLiveTest do
  use LabbyConsoleWeb.ConnCase
  import Phoenix.LiveViewTest

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
    %{state: state}
  end

  test "role-null platform manager creates durable project and selection never invokes restore",
       %{conn: conn} do
    signed = post(conn, "/sign-in", %{credential: "projects"})
    {:ok, view, _} = live(recycle(signed), "/projects")
    render_async(view)
    refute has_element?(view, "#projects-create[disabled]")
    view |> element("#projects-create") |> render_click()

    view
    |> form("#projects-form", project: %{name: "Created", project_id: "p", team_id: "t"})
    |> render_submit()

    render_async(view, 2000)
    assert render_async(view, 2000) =~ "Created"
    assert_receive {:projects_dispatch, "projects.create", %{"project_id" => "p"}}
    id = LabbyConsole.Projects.key("t", "p")
    view |> element("#project-open-#{id}") |> render_click()
    view |> element("#projects-use") |> render_click()
    assert has_element?(view, "article[data-current=true]")
    refute_receive {:projects_dispatch, "projects.activate", _}
    view |> element("#projects-close") |> render_click()
    view |> element("#projects-refresh") |> render_click()
    render_async(view, 2000)
    assert has_element?(view, "article[data-current=true]")
  end

  test "forbidden mutation retains draft and valid login", %{conn: conn, state: state} do
    signed = post(conn, "/sign-in", %{credential: "projects"})
    {:ok, view, _} = live(recycle(signed), "/projects")
    render_async(view)
    view |> element("#projects-create") |> render_click()
    Agent.update(state, &%{&1 | reject: true})

    view
    |> form("#projects-form", project: %{name: "Keep draft", project_id: "denied", team_id: "t"})
    |> render_submit()

    render_async(view)
    assert has_element?(view, "#projects-name[value='Keep draft']")
    assert has_element?(view, "#projects-form-error")
    assert {:ok, _} = LabbyConsole.Auth.current(get_session(signed, :console_session))
  end

  test "read-only project assignment refuses forged rename despite platform Team create permission",
       %{conn: conn, state: state} do
    p = %{
      "project_id" => "readonly",
      "team_id" => "t",
      "name" => "Read only",
      "status" => "active",
      "role" => "admin",
      "policy_epoch" => 1,
      "can_manage" => false
    }

    Agent.update(state, &%{&1 | rows: [p]})
    signed = post(conn, "/sign-in", %{credential: "projects"})
    {:ok, view, _} = live(recycle(signed), "/projects")
    render_async(view, 2000)
    id = LabbyConsole.Projects.key("t", "readonly")
    view |> element("#project-open-#{id}") |> render_click()
    refute has_element?(view, "#projects-edit")
    render_click(view, "projects_open", %{"mode" => "edit", "id" => id})
    render_click(view, "projects_save", %{"project" => %{"name" => "Forged"}})
    refute_receive {:projects_dispatch, "projects.update", _}
    assert {:ok, _} = LabbyConsole.Auth.current(get_session(signed, :console_session))
  end

  test "pending submit rejects duplicate events and scope changes", %{conn: conn, state: state} do
    signed = post(conn, "/sign-in", %{credential: "projects"})
    {:ok, view, _} = live(recycle(signed), "/projects")
    render_async(view, 2000)
    view |> element("#projects-create") |> render_click()
    Agent.update(state, &Map.put(&1, :hold, "projects.create"))
    params = %{"project" => %{"name" => "Once", "project_id" => "once", "team_id" => "t"}}
    render_click(view, "projects_save", params)
    assert_receive {:projects_hold, request}, 1000
    render_click(view, "projects_save", params)
    render_click(view, "projects_filter", %{"filters" => %{"team" => "wrong"}})
    assert has_element?(view, "#projects-save[disabled]")
    send(request, :release_projects)
    render_async(view, 2000)
    render_async(view, 2000)
    assert_receive {:projects_dispatch, "projects.create", _}
    refute_receive {:projects_dispatch, "projects.create", _}
    assert has_element?(view, "article[data-project=once]")
  end

  test "uncertain write keeps draft, never replays and requires explicit reconciliation", %{
    conn: conn,
    state: state
  } do
    signed = post(conn, "/sign-in", %{credential: "projects"})
    {:ok, view, _} = live(recycle(signed), "/projects")
    render_async(view, 2000)
    view |> element("#projects-create") |> render_click()
    Agent.update(state, &Map.put(&1, :malformed, true))

    view
    |> form("#projects-form",
      project: %{name: "Possible write", project_id: "possible", team_id: "t"}
    )
    |> render_submit()

    render_async(view, 2000)
    assert has_element?(view, "#projects-form-error", "Outcome uncertain")
    assert has_element?(view, "#projects-name[value='Possible write']")
    assert has_element?(view, "#projects-save[disabled]")
    assert_receive {:projects_dispatch, "projects.create", _}
    refute_receive {:projects_dispatch, "projects.create", _}
    view |> element("#projects-close") |> render_click()
    view |> element("#projects-refresh") |> render_click()
    render_async(view, 2000)
    assert has_element?(view, "article[data-project=possible]")
  end
end
