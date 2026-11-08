defmodule LabbyConsole.OverviewDetailsTest do
  use ExUnit.Case
  alias LabbyConsole.{Overview, Auth}

  setup do
    state = start_supervised!({Agent, fn -> true end})
    owner = self()

    data =
      start_supervised!(
        Supervisor.child_spec({Agent, fn -> %{owner: owner, mode: :normal} end},
          id: :overview_data
        )
      )

    server =
      start_supervised!(
        {Bandit,
         plug: {LabbyConsole.OverviewFixture, state: state, data_state: data},
         port: 0,
         ip: {127, 0, 0, 1}}
      )

    {:ok, {_, port}} = ThousandIsland.listener_info(server)
    previous = Application.get_env(:labby_console, :gateway_url)
    Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:#{port}")
    on_exit(fn -> Application.put_env(:labby_console, :gateway_url, previous) end)
    {:ok, id} = Auth.sign_in("fixture-only")
    %{session: id}
  end

  test "detail scope is applied to real metrics and retained calls with honest pagination", %{
    session: id
  } do
    filters = %{
      "upstream" => "files",
      "tool" => "read_file",
      "capability" => "tool",
      "operation" => "call",
      "subject_scoped" => true
    }

    assert {:ok, details} = Overview.details(id, filters, 100, 200)
    assert details.metrics["total_calls"] == 8
    assert hd(details.calls)["tool"] == "read_file"
    assert details.next_cursor == "opaque-next"

    assert_receive {:overview_metrics_params,
                    %{"upstream" => "files", "since_unix" => 100, "until_unix" => 200}}

    assert_receive {:overview_calls_params,
                    %{
                      "upstream" => "files",
                      "since_unix" => 100,
                      "until_unix" => 200,
                      "limit" => 25
                    }}

    refute inspect(details) =~ "private-payload"

    assert {:error, :invalid_filters} =
             Overview.details(id, %{"principal_id" => "forged"}, 100, 200)

    assert {:error, :invalid_filters} = Overview.details(id, %{}, 200, 100)
  end
end
