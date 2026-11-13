defmodule LabbyConsoleWeb.WorkspaceIntegrationTest do
  use LabbyConsoleWeb.ConnCase, async: true
  import Phoenix.LiveViewTest

  test "authenticated layouts expose Projects, its main target and CSRF-protected sign-out" do
    html =
      render_component(&LabbyConsoleWeb.Layouts.app/1,
        flash: %{},
        authenticated: true,
        page: "Projects",
        subject: "verified-caller",
        preference_key: String.duplicate("a", 64),
        main_id: "projects-main",
        inner_block: [%{inner_block: fn _, _ -> "<main id=\"projects-main\">Content</main>" end}]
      )

    assert html =~ ~s(href="#projects-main")
    assert html =~ ~s(href="/projects")
    assert html =~ ~s(action="/sign-out")
    assert html =~ ~s(method="post")
    assert html =~ ~s(name="_csrf_token")
    assert html =~ "Sign out"
    assert html =~ "verified-caller"
  end

  test "public layouts have no session sign-out control" do
    html =
      render_component(&LabbyConsoleWeb.Layouts.app/1,
        flash: %{},
        inner_block: [%{inner_block: fn _, _ -> "Public content" end}]
      )

    refute html =~ ~s(action="/sign-out")
    refute html =~ ~s(href="/projects")
  end
end
