defmodule LabbyConsoleWeb.ProjectsComponentsTest do
  use ExUnit.Case, async: true
  import Phoenix.LiveViewTest
  alias LabbyConsole.Projects

  test "uncertain submission cannot be submitted again before reconciliation" do
    state =
      Projects.new()
      |> Map.merge(%{
        dialog: "create",
        uncertain: true,
        teams: [%{id: "t", name: "Team", can_create: true}],
        active_team: "t",
        errors: %{"form" => "Outcome uncertain. Reconcile before submitting again."}
      })

    html =
      render_component(&LabbyConsoleWeb.ProjectsComponents.page/1,
        state: state,
        streams: %{projects: []},
        count: 0
      )

    assert html =~ ~r/id="projects-save"[^>]*disabled/
    assert html =~ "Outcome uncertain"
  end

  test "uncertain write still permits an explicit read-only lookup" do
    state = %{Projects.new() | dialog: "lookup", uncertain: true}

    html =
      render_component(&LabbyConsoleWeb.ProjectsComponents.page/1,
        state: state,
        streams: %{projects: []},
        count: 0
      )

    refute html =~ ~r/id="projects-save"[^>]*disabled/
  end
end
