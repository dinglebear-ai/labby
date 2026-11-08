defmodule LabbyConsoleWeb.OverviewComponentsTest do
  use LabbyConsoleWeb.ConnCase, async: true
  import Phoenix.LiveViewTest
  alias LabbyConsoleWeb.OverviewComponents

  defp call(overrides) do
    Map.merge(
      %{
        "tool" => "read_file",
        "capability" => "tool",
        "operation" => "call",
        "upstream" => "files",
        "actor" => "actual-caller-identity",
        "outcome" => "ok",
        "elapsed_ms" => 53,
        "response_bytes" => 0,
        "ts_unix" => 1_793_600_000
      },
      overrides
    )
  end

  defp inspector(row) do
    render_component(&OverviewComponents.inspector/1,
      selected: %{title: row["tool"], loading: false, error: nil, detail: nil, call: row}
    )
  end

  test "call inspector renders safe reported attribution independently of caller identity" do
    html =
      inspector(
        call(%{
          "subject_scoped" => true,
          "attribution" => %{
            "client_name" => "client-attribution-marker",
            "client_version" => "version-attribution-marker",
            "surface" => "surface-attribution-marker",
            "agent_id" => "agent-attribution-marker",
            "task_id" => "task-attribution-marker",
            "authorization" => "private-provider-secret"
          }
        })
      )

    for marker <-
          ~w(client-attribution-marker version-attribution-marker surface-attribution-marker agent-attribution-marker task-attribution-marker) do
      assert html =~ marker
    end

    assert html =~ "actual-caller-identity"
    assert html =~ ~r/<dt>Subject scoped<\/dt>\s*<dd>Yes<\/dd>/
    assert html =~ ~r/<dt>Response size \(bytes\)<\/dt>\s*<dd>0<\/dd>/
    refute html =~ "private-provider-secret"
  end

  test "missing attribution is explicit and false subject scope is retained" do
    html = inspector(call(%{"subject_scoped" => false}))

    for label <- ["Client", "Client version", "Surface", "Agent", "Task"] do
      assert html =~ "<dt>#{label}</dt>"
    end

    assert html =~ ~r/<dt>Client<\/dt>\s*<dd>Not reported<\/dd>/
    assert html =~ ~r/<dt>Subject scoped<\/dt>\s*<dd>No<\/dd>/
    assert html =~ "actual-caller-identity"
  end
end
