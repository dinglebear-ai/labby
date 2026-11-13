defmodule LabbyConsoleWeb.WorkspaceShellTest do
  use LabbyConsoleWeb.ConnCase, async: true
  import Phoenix.LiveViewTest

  test "shell exposes implemented navigation and actual identity without invented counts" do
    html =
      render_component(&LabbyConsoleWeb.WorkspaceShell.shell/1,
        id: "shell",
        page: "projects",
        main_id: "projects-main",
        subject: "operator-7",
        routes: %{"projects" => "/projects", "gateway" => "/gateway"},
        inner_block: [%{inner_block: fn _, _ -> "body" end}]
      )

    assert html =~ "operator-7"
    assert html =~ ~s(href="/projects")
    assert html =~ ~s(aria-current="page")
    assert html =~ ~s(href="#projects-main")
    refute html =~ ~s(href="/tasks")
    refute html =~ "DEV PREVIEW"
    refute html =~ "ov-nav-count"
    refute html =~ "Personal"
  end

  test "identity fallback is honest and preferences require a caller namespace" do
    html =
      render_component(&LabbyConsoleWeb.WorkspaceShell.shell/1,
        id: "shell",
        page: "gateway",
        main_id: "main",
        routes: %{"gateway" => "/gateway"},
        inner_block: [%{inner_block: fn _, _ -> "body" end}]
      )

    assert html =~ "Authenticated session"
    refute html =~ "data-preference-key="
  end
end
