defmodule LabbyConsoleWeb.OverviewLive do
  use LabbyConsoleWeb, :live_view
  on_mount {LabbyConsoleWeb.LiveSession, :authenticated}
  alias LabbyConsole.{Auth, UsageMetrics, Overview, Runtime, Clients, Gateway}
  alias LabbyConsoleWeb.OverviewComponents, as: UI
  @sections [:inventory, :usage, :host, :clients, :calls, :readiness]

  def mount(_, _session, socket) do
    socket =
      assign(socket,
        page_title: "Overview",
        window: "24h",
        snapshot: System.system_time(:second),
        inventory: nil,
        usage: nil,
        host: nil,
        clients: nil,
        calls: nil,
        readiness: nil,
        pending: [],
        errors: [],
        epoch: nil,
        deadline: nil,
        selected: nil,
        detail_epoch: nil,
        ranking: "top",
        activity_tab: "tools",
        chart: "calls",
        chart_mode: "volume",
        call_filter: "all",
        card_layout: Overview.default_layout(),
        layout_editing: false,
        layout_notice: "",
        storage_warning: false,
        customized: false,
        attention_dismissed: false
      )

    {:ok, if(connected?(socket), do: load(socket), else: socket)}
  end

  def render(assigns) do
    ~H"""
    <Layouts.app
      flash={@flash}
      authenticated={true}
      subject={@subject}
      preference_key={@preference_key}
      page="Overview"
    >
      <UI.page state={assigns} />
    </Layouts.app>
    """
  end

  def handle_event("refresh", _, socket), do: {:noreply, load(socket)}

  def handle_event("window", %{"window" => value}, socket) when value in ~w(1h 24h 7d 30d),
    do: {:noreply, socket |> assign(window: value) |> load()}

  def handle_event("ranking", %{"ranking" => value}, socket) when value in ~w(top least slowest),
    do: {:noreply, assign(socket, ranking: value)}

  def handle_event("activity_tab", %{"tab" => value}, socket)
      when value in ~w(tools servers callers), do: {:noreply, assign(socket, activity_tab: value)}

  def handle_event("chart", %{"chart" => value}, socket) when value in ~w(calls errors),
    do:
      {:noreply,
       assign(socket,
         chart: value,
         chart_mode: if(value == "errors", do: "failures", else: "volume")
       )}

  def handle_event("chart_mode", %{"chart_mode" => %{"mode" => value}}, socket)
      when value in ~w(volume servers outcomes errors failures),
      do:
        {:noreply,
         assign(socket,
           chart_mode: value,
           chart: if(value == "failures", do: "errors", else: "calls")
         )}

  def handle_event("call_filter", %{"filter" => value}, socket) when value in ~w(all ok failed),
    do: {:noreply, assign(socket, call_filter: value)}

  def handle_event("overview_customize", _, socket),
    do: {:noreply, update(socket, :layout_editing, &(!&1))}

  def handle_event("overview_preferences", params, socket),
    do:
      {:noreply,
       assign(socket,
         card_layout: Overview.normalize(params),
         customized: Overview.normalize(params) != Overview.default_layout()
       )}

  def handle_event("overview_storage_error", _, socket),
    do: {:noreply, assign(socket, storage_warning: true)}

  def handle_event("overview_reset", _, socket),
    do:
      {:noreply, persist(socket, Overview.default_layout(), "Default card layout restored.", nil)}

  def handle_event("overview_move", %{"id" => id, "direction" => direction}, socket),
    do:
      {:noreply,
       persist(
         socket,
         Overview.move(socket.assigns.card_layout, id, direction),
         "Card position updated.",
         id
       )}

  def handle_event("overview_place", %{"source" => source, "target" => target} = params, socket) do
    current = socket.assigns.card_layout
    layout = Overview.place(current, source, target, params["after"] == true)

    message =
      if layout == current, do: "Cards stay within their lane.", else: "Card position updated."

    {:noreply, persist(socket, layout, message, source)}
  end

  def handle_event("overview_width", %{"id" => id}, socket),
    do:
      {:noreply,
       persist(socket, Overview.widen(socket.assigns.card_layout, id), "Card width updated.", id)}

  def handle_event("dismiss_attention", _, socket),
    do: {:noreply, assign(socket, attention_dismissed: true)}

  def handle_event("toggle_attention", _, socket),
    do: {:noreply, update(socket, :attention_dismissed, &(!&1))}

  def handle_event("inspect_leader", %{"index" => index, "kind" => kind}, socket)
      when kind in ~w(tools servers callers) do
    state = socket.assigns
    rows = Overview.rankings(state.usage, kind, state.ranking) || []

    case integer(index) do
      n when n >= 0 and n < length(rows) ->
        row = Enum.at(rows, n)

        {title, filters} =
          case kind do
            "tools" ->
              {row["tool"], Map.take(row, ~w(tool upstream capability operation subject_scoped))}

            "callers" ->
              {row["actor"], %{"actor" => row["actor"]}}

            _ ->
              {row["upstream"], %{"upstream" => row["upstream"]}}
          end

        {:noreply, inspect_scope(socket, title, filters, first(state), state.snapshot)}

      _ ->
        {:noreply, socket}
    end
  end

  def handle_event("inspect_bucket", %{"index" => index}, socket) do
    state = socket.assigns
    buckets = (state.usage && state.usage["timeseries"]) || []

    case integer(index) do
      n when n >= 0 and n < length(buckets) ->
        bucket = Enum.at(buckets, n)
        next = Enum.at(buckets, n + 1)
        last = if next, do: next["ts_unix"] - 1, else: state.snapshot

        {:noreply,
         inspect_scope(
           socket,
           "Time bucket",
           %{},
           bucket["ts_unix"],
           max(last, bucket["ts_unix"])
         )}

      _ ->
        {:noreply, socket}
    end
  end

  def handle_event("inspect_usage", %{"outcome" => value}, socket) when value in ~w(ok failed),
    do:
      {:noreply,
       inspect_scope(
         socket,
         if(value == "ok", do: "Successful calls", else: "Failed calls"),
         %{"outcome" => value},
         first(socket.assigns),
         socket.assigns.snapshot
       )}

  def handle_event("inspect_call", %{"id" => id}, socket) do
    case Enum.find(socket.assigns.calls || [], &(Integer.to_string(&1["id"]) == id)) do
      nil ->
        {:noreply, socket}

      row ->
        {:noreply,
         socket
         |> close_detail()
         |> assign(
           selected: %{title: row["tool"], loading: false, error: nil, detail: nil, call: row}
         )}
    end
  end

  def handle_event("close_inspector", _, socket),
    do: {:noreply, socket |> close_detail() |> assign(selected: nil)}

  def handle_event(_, _, socket), do: {:noreply, socket}

  def handle_info(:session_expired, socket), do: {:noreply, expire(socket)}

  def handle_info({:overview_deadline, epoch}, %{assigns: %{epoch: epoch}} = socket) do
    socket = cancel_load(socket)

    {:noreply,
     assign(socket,
       epoch: make_ref(),
       errors: Enum.uniq(socket.assigns.errors ++ socket.assigns.pending),
       pending: [],
       deadline: nil
     )}
  end

  def handle_info({:overview_deadline, _}, socket), do: {:noreply, socket}

  def handle_async({:overview, epoch, name}, {:ok, result}, %{assigns: %{epoch: epoch}} = socket) do
    if Auth.matches?(socket.assigns.session_id, socket.assigns.session_generation) do
      case result do
        {:error, :unauthenticated} ->
          {:noreply, expire(socket)}

        _ ->
          socket =
            case result do
              {:ok, value} ->
                assign(socket, [
                  {name, if(name == :calls, do: value.calls, else: value)},
                  {:pending, List.delete(socket.assigns.pending, name)}
                ])

              _ ->
                assign(socket,
                  pending: List.delete(socket.assigns.pending, name),
                  errors: Enum.uniq([name | socket.assigns.errors])
                )
            end

          socket = if name == :usage, do: begin_secondary(socket), else: socket
          {:noreply, finish(socket)}
      end
    else
      {:noreply, expire(socket)}
    end
  end

  def handle_async({:overview, epoch, name}, {:exit, _}, %{assigns: %{epoch: epoch}} = socket) do
    socket =
      assign(socket,
        pending: List.delete(socket.assigns.pending, name),
        errors: Enum.uniq([name | socket.assigns.errors])
      )

    {:noreply, finish(if(name == :usage, do: begin_secondary(socket), else: socket))}
  end

  def handle_async(
        {:overview_detail, epoch},
        result,
        %{assigns: %{detail_epoch: epoch, selected: selected}} = socket
      )
      when not is_nil(selected) do
    if Auth.matches?(socket.assigns.session_id, socket.assigns.session_generation) do
      case result do
        {:ok, {:ok, value}} ->
          {:noreply, assign(socket, selected: %{selected | loading: false, detail: value})}

        {:ok, {:error, :unauthenticated}} ->
          {:noreply, expire(socket)}

        _ ->
          {:noreply,
           assign(socket,
             selected: %{
               selected
               | loading: false,
                 error: "Scoped usage is unavailable. Close and try again."
             }
           )}
      end
    else
      {:noreply, expire(socket)}
    end
  end

  def handle_async(_, _, socket), do: {:noreply, socket}

  defp load(socket) do
    socket = socket |> cancel_load() |> close_detail()
    epoch = make_ref()
    snapshot = System.system_time(:second)
    id = socket.assigns.session_id
    window = socket.assigns.window
    timer = Process.send_after(self(), {:overview_deadline, epoch}, 16_000)

    socket
    |> assign(
      inventory: nil,
      usage: nil,
      host: nil,
      clients: nil,
      calls: nil,
      readiness: nil,
      selected: nil,
      pending: @sections,
      errors: [],
      epoch: epoch,
      snapshot: snapshot,
      deadline: timer
    )
    |> start_async({:overview, epoch, :inventory}, fn -> Overview.inventory(id) end)
    |> start_async({:overview, epoch, :usage}, fn -> UsageMetrics.fetch(id, window, snapshot) end)
    |> start_async({:overview, epoch, :host}, fn -> Runtime.metrics(id) end)
    |> start_async({:overview, epoch, :clients}, fn -> Clients.list(id) end)
  end

  defp begin_secondary(socket) do
    state = socket.assigns
    id = state.session_id
    first = first(state)
    last = state.snapshot

    target =
      case Auth.capture(id) do
        %{target: target} -> target
        _ -> nil
      end

    socket
    |> start_async({:overview, state.epoch, :calls}, fn ->
      Overview.calls(id, %{}, first, last)
    end)
    |> start_async({:overview, state.epoch, :readiness}, fn -> Gateway.readiness(target) end)
  end

  defp first(state), do: max(0, state.snapshot - UsageMetrics.window(state.window))

  defp finish(%{assigns: %{pending: []}} = socket) do
    if socket.assigns.deadline, do: Process.cancel_timer(socket.assigns.deadline)
    assign(socket, deadline: nil)
  end

  defp finish(socket), do: socket

  defp cancel_load(socket) do
    if socket.assigns.deadline, do: Process.cancel_timer(socket.assigns.deadline)

    Enum.reduce(@sections, socket, fn name, acc ->
      cancel_async(acc, {:overview, socket.assigns.epoch, name})
    end)
  end

  defp close_detail(socket) do
    if socket.assigns.detail_epoch,
      do:
        cancel_async(socket, {:overview_detail, socket.assigns.detail_epoch})
        |> assign(detail_epoch: nil),
      else: socket
  end

  defp inspect_scope(socket, title, filters, first, last) do
    socket = close_detail(socket)
    epoch = make_ref()
    id = socket.assigns.session_id

    socket
    |> assign(
      detail_epoch: epoch,
      selected: %{
        title: title,
        filters: filters,
        first: first,
        last: last,
        loading: true,
        error: nil,
        detail: nil,
        call: nil
      }
    )
    |> start_async({:overview_detail, epoch}, fn -> Overview.details(id, filters, first, last) end)
  end

  defp persist(socket, layout, message, id) do
    socket =
      socket
      |> assign(
        card_layout: layout,
        customized: layout != Overview.default_layout(),
        layout_notice: message
      )
      |> push_event("overview-layout-state", %{key: socket.assigns.preference_key, layout: layout})

    if Overview.lane(id), do: push_event(socket, "overview-focus-card", %{id: id}), else: socket
  end

  defp integer(value) when is_binary(value) and byte_size(value) <= 3 do
    case Integer.parse(value) do
      {n, ""} -> n
      _ -> -1
    end
  end

  defp integer(_), do: -1

  defp expire(socket),
    do:
      socket
      |> cancel_load()
      |> close_detail()
      |> assign(inventory: nil, usage: nil, host: nil, clients: nil, calls: nil, selected: nil)
      |> redirect(to: "/sign-in")
end
