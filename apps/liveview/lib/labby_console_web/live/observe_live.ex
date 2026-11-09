defmodule LabbyConsoleWeb.ObserveLive do
  use LabbyConsoleWeb, :live_view
  on_mount {LabbyConsoleWeb.LiveSession, :authenticated}
  alias LabbyConsole.{Auth, Observe}
  alias LabbyConsoleWeb.ObserveComponents, as: UI

  def mount(_, _, socket) do
    socket =
      assign(socket,
        page_title: "Observe",
        tab: "activity",
        window: "24h",
        rows: [],
        selected: nil,
        filters: %{},
        cursor: nil,
        cursors: [],
        snapshot: System.system_time(:second),
        truncated: false,
        loading: false,
        loaded: false,
        error: nil,
        export_error: nil,
        following: false,
        failures: 0,
        timer: nil,
        poll_token: nil,
        epoch: nil,
        deadline: nil,
        append: false,
        storage_warning: false
      )

    {:ok, if(connected?(socket), do: load(socket, false), else: socket)}
  end

  def render(assigns) do
    ~H"""
    <Layouts.app
      flash={@flash}
      authenticated={true}
      subject={@subject}
      preference_key={@preference_key}
      page="Observe"
    >
      <UI.page state={assigns} />
    </Layouts.app>
    """
  end

  def handle_event("tab", %{"tab" => tab}, socket) when tab in ~w(activity logs traces),
    do: {:noreply, socket |> reset() |> assign(tab: tab) |> persist() |> load(false)}

  def handle_event("filter", %{"filters" => filters}, socket) when is_map(filters) do
    window = Observe.preferences(%{"window" => filters["window"]})["window"]
    filters = Map.take(filters, ~w(query upstream outcome level))

    valid =
      Enum.all?(filters, fn {_key, value} ->
        is_binary(value) && String.valid?(value) && byte_size(value) <= 256
      end) && (filters["level"] || "") in ["", "TRACE", "DEBUG", "INFO", "WARN", "ERROR"] &&
        (filters["outcome"] || "") in ["", "ok", "timeout", "upstream_error", "tool_error"]

    if valid do
      {:noreply,
       socket |> reset() |> assign(filters: filters, window: window) |> persist() |> load(false)}
    else
      {:noreply, assign(socket, error: :invalid_filters)}
    end
  end

  def handle_event("refresh", _, socket), do: {:noreply, load(socket, false)}

  def handle_event(
        "next",
        _,
        %{assigns: %{loading: false, cursor: cursor, following: false}} = socket
      )
      when is_binary(cursor),
      do: {:noreply, load(socket, true)}

  def handle_event("pause", _, socket),
    do: {:noreply, socket |> stop() |> assign(following: false)}

  def handle_event("resume", _, socket),
    do: {:noreply, socket |> assign(following: true, failures: 0) |> load(false)}

  def handle_event("inspect", %{"id" => id}, socket),
    do: {:noreply, assign(socket, selected: Observe.selection(socket.assigns.rows, id))}

  def handle_event("close_inspector", _, socket), do: {:noreply, assign(socket, selected: nil)}

  def handle_event("observe_storage_unavailable", _, socket),
    do: {:noreply, assign(socket, storage_warning: true)}

  def handle_event("observe_preferences", %{"preferences" => prefs}, socket) do
    prefs = Observe.preferences(prefs)

    {:noreply,
     socket |> reset() |> assign(tab: prefs["tab"], window: prefs["window"]) |> load(false)}
  end

  def handle_event("export", _, socket) do
    if Auth.matches?(socket.assigns.session_id, socket.assigns.session_generation) &&
         socket.assigns.loaded && is_nil(socket.assigns.error) do
      case Observe.export(socket.assigns.tab, socket.assigns.rows, %{
             truncated: socket.assigns.truncated,
             snapshot: socket.assigns.snapshot
           }) do
        {:ok, data} ->
          {:noreply,
           push_event(socket, "observe-export", %{data: data, key: socket.assigns.preference_key})}

        {:error, reason} ->
          {:noreply, assign(socket, export_error: reason)}
      end
    else
      {:noreply, assign(socket, export_error: :unavailable)}
    end
  end

  def handle_event(_, _, socket), do: {:noreply, socket}

  defp reset(socket),
    do:
      socket
      |> stop()
      |> assign(
        rows: [],
        selected: nil,
        cursor: nil,
        cursors: [],
        loaded: false,
        truncated: false,
        error: nil,
        export_error: nil
      )

  defp persist(socket),
    do:
      push_event(socket, "observe-preferences-state", %{
        key: socket.assigns.preference_key,
        preferences: %{"tab" => socket.assigns.tab, "window" => socket.assigns.window}
      })

  defp load(socket, append) do
    socket = stop(socket)

    if connected?(socket) do
      epoch = make_ref()
      state = socket.assigns
      snapshot = if append, do: state.snapshot, else: System.system_time(:second)
      cursor = if append, do: state.cursor
      deadline = Process.send_after(self(), {:observe_timeout, epoch}, 12_000)
      id = state.session_id
      tab = state.tab
      filters = state.filters
      window = state.window

      socket
      |> assign(
        epoch: epoch,
        deadline: deadline,
        loading: true,
        append: append,
        snapshot: snapshot,
        export_error: nil
      )
      |> start_async({:observe, epoch, state.session_generation}, fn ->
        Observe.fetch(id, tab, filters, window, snapshot, cursor)
      end)
    else
      socket
    end
  end

  def handle_async({:observe, epoch, generation}, result, %{assigns: %{epoch: epoch}} = socket)
      when not is_nil(epoch) do
    if Auth.matches?(socket.assigns.session_id, generation) do
      if socket.assigns.deadline, do: Process.cancel_timer(socket.assigns.deadline)
      append = socket.assigns.append
      socket = assign(socket, epoch: nil, deadline: nil, loading: false)

      value =
        case result do
          {:ok, value} -> value
          _ -> {:error, :unavailable}
        end

      case value do
        {:error, :unauthenticated} -> {:noreply, expire(socket)}
        _ -> {:noreply, socket |> apply_result(value, append) |> schedule()}
      end
    else
      {:noreply, expire(socket)}
    end
  end

  def handle_async(_, _, socket), do: {:noreply, socket}

  @doc false
  def apply_result(socket, {:ok, %{calls: calls, next_cursor: cursor}}, append) do
    state = socket.assigns

    rows =
      if append, do: Observe.merge_calls(state.rows, calls), else: Observe.merge_calls([], calls)

    repeated =
      append && is_binary(cursor) && (cursor in state.cursors || cursor == state.cursor)

    assign(socket,
      rows: rows,
      selected: Observe.selection(rows, state.selected),
      cursor: if(repeated || length(rows) >= 200, do: nil, else: cursor),
      cursors:
        if(append, do: Enum.take(Enum.uniq([state.cursor | state.cursors]), 200), else: []),
      error: if(repeated, do: :cursor_repeated),
      failures: 0,
      loaded: true,
      truncated: length(rows) >= 200
    )
  end

  def apply_result(socket, {:ok, %{entries: entries, truncated: truncated}}, _) do
    {rows, _} =
      Enum.map_reduce(entries, %{}, fn row, seen ->
        count = Map.get(seen, row, 0)
        {Map.put(row, "id", log_id(row, count)), Map.put(seen, row, count + 1)}
      end)

    assign(socket,
      rows: rows,
      selected: Observe.selection(rows, socket.assigns.selected),
      cursor: nil,
      truncated: truncated,
      loaded: true,
      error: nil,
      failures: 0
    )
  end

  def apply_result(socket, {:error, :forbidden}, _),
    do:
      socket
      |> stop()
      |> assign(
        rows: [],
        selected: nil,
        cursor: nil,
        loaded: false,
        error: :forbidden,
        following: false
      )

  def apply_result(socket, _, _) do
    failures = socket.assigns.failures + 1

    assign(socket,
      error: :unavailable,
      failures: failures,
      following: socket.assigns.following && failures < 3
    )
  end

  defp log_id(row, n),
    do: Base.url_encode64(:crypto.hash(:sha256, :erlang.term_to_binary({row, n})), padding: false)

  defp schedule(%{assigns: %{following: true}} = socket) do
    token = make_ref()

    assign(socket,
      poll_token: token,
      timer: Process.send_after(self(), {:observe_poll, token}, 5000)
    )
  end

  defp schedule(socket), do: socket

  def handle_info(
        {:observe_poll, token},
        %{assigns: %{poll_token: token, following: true, loading: false}} = socket
      ),
      do: {:noreply, socket |> assign(timer: nil) |> load(false)}

  def handle_info({:observe_timeout, epoch}, %{assigns: %{epoch: epoch}} = socket)
      when not is_nil(epoch),
      do:
        {:noreply, socket |> stop() |> apply_result({:error, :unavailable}, false) |> schedule()}

  def handle_info(:session_expired, socket), do: {:noreply, expire(socket)}
  def handle_info(_, socket), do: {:noreply, socket}

  defp stop(socket) do
    for timer <- [socket.assigns.timer, socket.assigns.deadline],
        timer,
        do: Process.cancel_timer(timer)

    socket =
      if socket.assigns.epoch,
        do:
          cancel_async(
            socket,
            {:observe, socket.assigns.epoch, socket.assigns.session_generation}
          ),
        else: socket

    assign(socket, timer: nil, poll_token: nil, deadline: nil, epoch: nil, loading: false)
  end

  defp expire(socket),
    do: socket |> reset() |> assign(following: false) |> redirect(to: "/sign-in")

  def terminate(_, socket) do
    for timer <- [socket.assigns[:timer], socket.assigns[:deadline]],
        timer,
        do: Process.cancel_timer(timer)

    :ok
  end
end
