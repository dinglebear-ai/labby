# Pure rendering only. No application, endpoint, or runtime service is started.
defmodule LabbyAccessOfflineMarkup do
  import Phoenix.LiveViewTest
  alias LabbyConsole.Access
  alias LabbyConsoleWeb.AccessComponents

  def render do
    id = "access-offline-rendered-team-0123456789ab"
    name = "Acceptance rendered name 0123456789ab"
    base = Access.new()

    markup =
      Map.new(~w(active suspended), fn status ->
        team = %{
          "team_id" => id,
          "name" => name,
          "role" => "owner",
          "status" => status,
          "policy_epoch" => 1,
          "membership_epoch" => 1,
          "global_revision" => 1
        }

        state =
          base
          |> Map.put(:team, id)
          |> Map.put(:capabilities, MapSet.new(Enum.map(Access.specs(), & &1.action)))
          |> put_in([:cards, :teams], %{base.cards.teams | rows: [team], status: :loaded})

        html =
          render_component(&AccessComponents.page/1,
            state: state,
            scope: %{"is_admin" => true, "capabilities" => ["platform.manage"]},
            busy: false
          )

        {status, html}
      end)

    Jason.encode!(%{team: %{id: id, name: name}, markup: markup})
  end
end

IO.puts(LabbyAccessOfflineMarkup.render())
