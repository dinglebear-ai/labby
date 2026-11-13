defmodule LabbyConsole.GatewayAction.Supervisor do
  @moduledoc "Owns durable receipts while the gateway transport children restart together."
  use Supervisor
  alias LabbyConsole.GatewayAction

  def start_link(_), do: Supervisor.start_link(__MODULE__, [], name: __MODULE__)

  @impl true
  def init(_) do
    # The application supervisor owns the durable receipt table. This subtree
    # stops all original HTTP children before restarting the recovery guardian.
    Supervisor.init(
      [
        {Task.Supervisor, name: GatewayAction.Tasks, max_children: 32},
        GatewayAction.Lifecycle,
        GatewayAction
      ],
      strategy: :one_for_all
    )
  end
end
