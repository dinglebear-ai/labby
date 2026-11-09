defmodule LabbyConsoleWeb.GatewayLive do
  use LabbyConsoleWeb, :live_view
  on_mount {LabbyConsoleWeb.LiveSession, :authenticated}
  alias LabbyConsole.{Auth, GatewayFleet}
  alias LabbyConsoleWeb.GatewayFleetComponents, as: UI

  def mount(_, _session, socket) do
    socket =
      assign(socket,
        page_title: "Gateway",
        rows: [],
        preferences: GatewayFleet.preferences(%{}),
        loading: false,
        error: nil,
        selected: nil,
        epoch: nil,
        deadline: nil,
        url_preferences: false,
        storage_warning: false
      )

    {:ok, if(connected?(socket), do: load(socket), else: socket)}
  end

  def handle_params(params, _, socket) do
    params =
      case params["columns"] do
        value when is_binary(value) and byte_size(value) <= 256 ->
          Map.put(params, "columns", String.split(value, ","))

        _ ->
          params
      end

    preferences = GatewayFleet.preferences(params)

    {:noreply,
     socket
     |> assign(preferences: preferences, url_preferences: map_size(params) > 0)
     |> preferences_changed()}
  end

  def render(assigns) do
    ~H"""
    <Layouts.app
      flash={@flash}
      authenticated={true}
      subject={@subject}
      preference_key={@preference_key}
      page="Gateway"
      server_count={if @loading || @error, do: nil, else: length(@rows)}
    >
      <UI.page state={assigns} />
    </Layouts.app>
    """
  end

  def handle_event("refresh", _, %{assigns: %{loading: false}} = socket),
    do: {:noreply, load(socket)}

  def handle_event("refresh", _, socket), do: {:noreply, socket}

  def handle_event("search", %{"query" => query}, socket),
    do: update_preferences(socket, %{"query" => query})

  def handle_event("filter", params, socket),
    do: update_preferences(socket, Map.take(params, ~w(lens source)))

  def handle_event("layout", %{"layout" => layout}, socket),
    do: update_preferences(socket, %{"layout" => layout})

  def handle_event("sort", params, socket),
    do: update_preferences(socket, Map.take(params, ~w(sort direction)))

  def handle_event("column", %{"column" => column}, socket) do
    columns = socket.assigns.preferences["columns"]
    columns = if column in columns, do: List.delete(columns, column), else: columns ++ [column]
    update_preferences(socket, %{"columns" => columns})
  end

  def handle_event("column_move", %{"column" => column, "direction" => direction}, socket) do
    prefs = GatewayFleet.move_column(socket.assigns.preferences, column, direction)
    update_preferences(socket, prefs)
  end

  def handle_event("gateway_preferences", %{"preferences" => prefs}, socket) do
    if socket.assigns.url_preferences,
      do: {:noreply, preferences_changed(socket)},
      else: update_preferences(socket, GatewayFleet.preferences(prefs))
  end

  def handle_event("gateway_storage_unavailable", _, socket),
    do: {:noreply, assign(socket, storage_warning: true)}

  def handle_event("inspect", %{"id" => id}, socket) do
    selected = Enum.find_value(socket.assigns.rows, fn row -> if row["id"] == id, do: id end)
    {:noreply, assign(socket, selected: selected)}
  end

  def handle_event("close_inspector", _, socket), do: {:noreply, assign(socket, selected: nil)}
  def handle_event(_, _, socket), do: {:noreply, socket}

  defp update_preferences(socket, changes) do
    preferences = GatewayFleet.preferences(Map.merge(socket.assigns.preferences, changes))
    query = preferences |> Map.update!("columns", &Enum.join(&1, ",")) |> URI.encode_query()

    {:noreply,
     socket
     |> assign(preferences: preferences)
     |> preferences_changed()
     |> push_patch(to: "/gateway?" <> query)}
  end

  defp preferences_changed(socket),
    do:
      push_event(socket, "gateway-preferences-state", %{
        key: socket.assigns.preference_key,
        preferences: socket.assigns.preferences
      })

  defp load(socket) do
    epoch = make_ref()
    id = socket.assigns.session_id
    generation = socket.assigns.session_generation
    timer = Process.send_after(self(), {:gateway_fleet_timeout, epoch}, 12_000)

    socket
    |> assign(rows: [], loading: true, error: nil, epoch: epoch, deadline: timer)
    |> start_async({:servers, epoch, generation}, fn -> GatewayFleet.fetch(id) end)
  end

  def handle_async({:servers, epoch, generation}, result, socket) do
    if socket.assigns.epoch == epoch && Auth.matches?(socket.assigns.session_id, generation) do
      Process.cancel_timer(socket.assigns.deadline)
      complete(result, assign(socket, loading: false, epoch: nil, deadline: nil))
    else
      {:noreply, socket}
    end
  end

  defp complete({:ok, {:ok, rows}}, socket) do
    selected =
      if Enum.any?(rows, &(&1["id"] == socket.assigns.selected)), do: socket.assigns.selected

    {:noreply, assign(socket, rows: rows, selected: selected, error: nil)}
  end

  defp complete({:ok, {:error, :unauthenticated}}, socket), do: {:noreply, expire(socket)}

  defp complete({:ok, {:error, :forbidden}}, socket),
    do: {:noreply, assign(socket, rows: [], selected: nil, error: :forbidden)}

  defp complete(_, socket),
    do: {:noreply, assign(socket, rows: [], selected: nil, error: :unavailable)}

  def handle_info({:gateway_fleet_timeout, epoch}, %{assigns: %{epoch: epoch}} = socket)
      when not is_nil(epoch) do
    socket = cancel_async(socket, {:servers, epoch, socket.assigns.session_generation})

    {:noreply,
     assign(socket,
       rows: [],
       selected: nil,
       epoch: nil,
       deadline: nil,
       loading: false,
       error: :unavailable
     )}
  end

  def handle_info(:session_expired, socket), do: {:noreply, expire(socket)}
  def handle_info(_, socket), do: {:noreply, socket}

  defp expire(socket) do
    if socket.assigns.deadline, do: Process.cancel_timer(socket.assigns.deadline)

    socket =
      if socket.assigns.epoch,
        do:
          cancel_async(
            socket,
            {:servers, socket.assigns.epoch, socket.assigns.session_generation}
          ),
        else: socket

    socket
    |> assign(rows: [], selected: nil, epoch: nil, loading: false)
    |> redirect(to: "/sign-in")
  end
end
