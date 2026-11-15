defmodule LabbyConsoleWeb.AccessLive do
  use LabbyConsoleWeb, :live_view
  on_mount {LabbyConsoleWeb.LiveSession, :authenticated}
  alias LabbyConsole.{Access, Auth, GatewayAction.Error}
  alias LabbyConsoleWeb.AccessComponents, as: UI
  alias LabbyConsoleWeb.LiveSession

  def mount(_, _, socket) do
    socket =
      assign(socket,
        page_title: "People & teams",
        access: Access.new(),
        access_requests: %{},
        access_queue: []
      )

    {:ok, if(connected?(socket), do: enqueue(socket, [:help]), else: socket)}
  end

  def render(assigns) do
    ~H"""
    <Layouts.app
      flash={@flash}
      authenticated={true}
      page="Access"
      main_id="access-main"
      subject={@subject}
      preference_key={@preference_key}
    >
      <UI.page state={@access} scope={@current_scope} busy={write_pending?(@access_requests)} />
    </Layouts.app>
    """
  end

  def handle_event("access_close", _, socket),
    do:
      {:noreply,
       socket |> update(:access, &Access.close/1) |> push_event("access-secret-clear", %{})}

  def handle_event(_, _, %{assigns: %{access: %{receipt: %{status: :pending}}}} = socket),
    do: {:noreply, socket}

  def handle_event("access_team", %{"scope" => %{"team" => team}}, socket) do
    state = Access.select(socket.assigns.access, team)

    if state.team != socket.assigns.access.team do
      {:noreply, socket |> transition_scope(team) |> enqueue(Access.scoped_cards())}
    else
      {:noreply, socket}
    end
  end

  def handle_event("access_open", %{"action" => action} = values, socket) do
    state =
      Access.open(
        socket.assigns.access,
        action,
        socket.assigns.current_scope,
        Map.take(values, ~w(principal_id project_id upstream_name))
      )

    {:noreply, socket |> assign(access: state) |> push_event("access-secret-clear", %{})}
  end

  def handle_event(
        "access_submit",
        %{"access_request" => params, "confirmation" => "yes"} = payload,
        socket
      )
      when is_map(params) do
    state = socket.assigns.access
    action = state.dialog
    nonce = client_nonce(payload["nonce"])

    if Access.availability(state, action, socket.assigns.current_scope) == :available &&
         valid_scope?(state, params) do
      case Access.prepare(action, params) do
        {:ok, clean, intent} ->
          intent = Access.capture_intent(state, intent)
          socket = cancel_reads(socket)

          case Auth.start_action(socket.assigns.session_id, "access", action, clean) do
            {:ok, ref} ->
              # Only the admitted request owns secret parameters. The view keeps
              # resource keys and a nonsecret client confirmation nonce.
              request = %{kind: :write, intent: intent, nonce: nonce, fence: fence(socket)}

              state =
                Access.close(socket.assigns.access)
                |> Map.put(
                  :receipt,
                  Map.merge(intent, %{status: :pending, message: "Submitting change…"})
                )

              {:noreply, socket |> assign(access: state) |> put_request(ref, request)}

            {:error, %Error{} = error} ->
              {:noreply,
               socket
               |> assign(access: Access.write_result(state, intent, {:error, error}))
               |> push_event("access-secret-result", %{nonce: nonce, accepted: false})}
          end

        {:error, errors} ->
          {:noreply,
           socket
           |> update(:access, &%{&1 | errors: errors})
           |> push_event("access-secret-result", %{nonce: nonce, accepted: false})}
      end
    else
      {:noreply, push_event(socket, "access-secret-result", %{nonce: nonce, accepted: false})}
    end
  end

  def handle_event("access_submit", _, socket),
    do:
      {:noreply,
       update(
         socket,
         :access,
         &%{&1 | errors: %{"form" => "Confirm this change before applying it."}}
       )}

  def handle_event("access_refresh", %{"card" => "help"}, socket) do
    socket =
      socket |> cancel_reads() |> update(:access, &%{&1 | capabilities: nil, help_error: nil})

    {:noreply, enqueue(socket, [:help])}
  end

  def handle_event("access_refresh", %{"card" => "affected"}, socket) do
    cards = if socket.assigns.access.receipt, do: socket.assigns.access.receipt.affected, else: []
    {:noreply, enqueue(socket, cards)}
  end

  def handle_event("access_refresh", %{"card" => name}, socket) do
    case card(name) do
      nil -> {:noreply, socket}
      key -> {:noreply, enqueue(socket, [key])}
    end
  end

  def handle_event("access_next", %{"card" => name}, socket) do
    key = card(name)
    data = socket.assigns.access.cards[key]

    if key && data.next_offset && data.status in [:loaded, :partial] do
      {:noreply, enqueue(socket, [{key, :next}])}
    else
      {:noreply, socket}
    end
  end

  def handle_event("access_limit", %{"paging" => %{"card" => name, "limit" => limit}}, socket)
      when is_binary(limit) do
    key = card(name)

    with true <- key && Access.paged?(key),
         {n, ""} when n in 1..100 <- Integer.parse(limit),
         false <- terminal?(socket.assigns.access.cards[key]) do
      socket = update(socket, :access, &put_in(&1.cards[key].limit, n))
      {:noreply, enqueue(socket, [key])}
    else
      _ -> {:noreply, socket}
    end
  end

  def handle_event(_, _, socket), do: {:noreply, socket}

  def handle_info({:gateway_action, ref, result}, socket) do
    case Map.pop(socket.assigns.access_requests, ref) do
      {nil, _} ->
        {:noreply, socket}

      {request, rest} ->
        socket =
          socket
          |> assign(access_requests: rest)
          |> update(:pending_actions, &Map.delete(&1, {:access, ref}))

        if request.fence == fence(socket) &&
             Auth.matches?(socket.assigns.session_id, socket.assigns.session_generation) do
          case result do
            {:error, %Error{kind: :unauthenticated}} -> {:noreply, expire(socket)}
            _ -> {:noreply, socket |> complete(request, result) |> drain()}
          end
        else
          {:noreply,
           socket
           |> cancel_reads()
           |> assign(access: Access.new())
           |> push_event("access-secret-clear", %{})}
        end
    end
  end

  def handle_info(_, socket), do: {:noreply, socket}

  defp complete(socket, %{kind: :help}, result) do
    result = with {:ok, data} <- result, do: Access.decode_help(data)

    case result do
      {:ok, capabilities} ->
        socket
        |> update(:access, &%{&1 | capabilities: capabilities, help_error: nil})
        |> push_event("access-ready", %{})
        |> enqueue(
          [:teams, :effective, :admins] ++
            if(socket.assigns.access.team, do: Access.scoped_cards(), else: [])
        )

      {:error, %Error{} = error} ->
        update(socket, :access, &%{&1 | capabilities: nil, help_error: error})
    end
  end

  defp complete(socket, %{kind: :read, card: card, params: params}, result) do
    decoded = with {:ok, data} <- result, do: Access.decode(card, data, params)
    state = Access.read_result(socket.assigns.access, card, decoded, params)

    state =
      if match?({:ok, _}, decoded) &&
           String.starts_with?(
             (state.receipt || %{action: ""}).action,
             "access.team_invitation."
           ),
         do: Access.accepted_observed(state, card),
         else: state

    socket = assign(socket, access: state)

    authoritative =
      match?({:ok, _}, decoded) ||
        match?({:error, %Error{kind: :forbidden, status: 403}}, decoded)

    cond do
      card == :teams && authoritative && state.team &&
          !Enum.any?(state.cards.teams.rows, &(&1["team_id"] == state.team)) ->
        socket = cancel_reads(socket)
        state = Access.lose_scope(socket.assigns.access)

        socket
        |> assign(access: state)
        |> LiveSession.set_scope(%{})
        |> push_event("access-secret-clear", %{})

      card == :teams && is_nil(state.team) && state.cards.teams.rows != [] && !Access.held?(state) ->
        socket
        |> transition_scope(hd(state.cards.teams.rows)["team_id"])
        |> enqueue([:effective, :admins] ++ Access.scoped_cards())

      true ->
        socket
    end
  end

  defp complete(socket, %{kind: :write, intent: intent, nonce: nonce}, result) do
    result = Access.decode_write(intent, result)
    state = Access.write_result(socket.assigns.access, intent, result)

    state =
      case result do
        {:ok, %{"team_id" => team, "principal_id" => principal}}
        when intent.action == "access.team_invitation.accept" ->
          put_in(state.receipt.params, %{"team_id" => team, "principal_id" => principal})

        _ ->
          state
      end

    state =
      case result do
        {:error, %Error{kind: :forbidden} = error} ->
          Enum.reduce(intent.affected, state, fn card, current ->
            Access.read_result(current, card, {:error, error}, intent.params)
          end)

        _ ->
          state
      end

    socket =
      socket
      |> assign(access: state)
      |> push_event("access-secret-result", %{nonce: nonce, accepted: match?({:ok, _}, result)})

    case result do
      {:ok, %{"team_id" => team}}
      when intent.action == "access.team_invitation.accept" and team != state.team ->
        socket
        |> transition_scope(team)
        |> enqueue(Enum.uniq(Access.scoped_cards() ++ state.receipt.affected))

      _ ->
        if Access.held?(state), do: enqueue(socket, state.receipt.affected), else: socket
    end
  end

  defp transition_scope(socket, team) do
    socket = cancel_reads(socket)

    socket
    |> update(:access, &Access.change_scope(&1, team))
    |> LiveSession.set_scope(%{team_id: team})
    |> push_event("access-secret-clear", %{})
  end

  defp enqueue(socket, cards) do
    queue = Enum.map(cards, fn item -> if is_tuple(item), do: item, else: {item, :refresh} end)
    assign(socket, access_queue: Enum.uniq(socket.assigns.access_queue ++ queue)) |> drain()
  end

  defp drain(socket) do
    reads = Enum.count(socket.assigns.access_requests, fn {_, r} -> r.kind != :write end)

    case socket.assigns.access_queue do
      [{card, mode} | rest] when reads < 2 ->
        socket = assign(socket, access_queue: rest)
        socket |> begin_read(card, mode) |> drain()

      _ ->
        socket
    end
  end

  defp begin_read(socket, :help, _) do
    start_read(socket, :help, "help", %{})
  end

  defp begin_read(socket, card, mode) do
    state = socket.assigns.access
    action = Access.read_action(card)

    cond do
      Access.scoped?(card) && is_nil(state.team) ->
        socket

      !is_struct(state.capabilities, MapSet) ->
        socket

      !MapSet.member?(state.capabilities, action) ->
        update(socket, :access, &Access.unavailable(&1, card))

      terminal?(state.cards[card]) ->
        socket

      true ->
        start_read(socket, card, action, Access.params(state, card, mode))
    end
  end

  defp start_read(socket, card, action, params) do
    socket = cancel_card(socket, card)

    request =
      if card == :help,
        do: %{kind: :help, fence: fence(socket)},
        else: %{kind: :read, card: card, params: params, fence: fence(socket)}

    case Auth.start_action(socket.assigns.session_id, "access", action, params) do
      {:ok, ref} ->
        socket = put_request(socket, ref, request)
        if card == :help, do: socket, else: update(socket, :access, &Access.loading(&1, card))

      {:error, %Error{kind: :unauthenticated}} ->
        expire(socket)

      {:error, %Error{} = error} ->
        complete(socket, request, {:error, error})
    end
  end

  defp put_request(socket, ref, request) do
    socket
    |> assign(access_requests: Map.put(socket.assigns.access_requests, ref, request))
    |> LiveSession.track({:access, ref}, ref)
  end

  defp cancel_card(socket, card) do
    {cancel, keep} =
      Enum.split_with(socket.assigns.access_requests, fn {_, r} ->
        (r.kind == :help && card == :help) || (r.kind == :read && r.card == card)
      end)

    Enum.each(cancel, fn {ref, _} -> Auth.cancel_action(socket.assigns.session_id, ref) end)

    socket
    |> assign(access_requests: Map.new(keep))
    |> update(
      :pending_actions,
      &Map.drop(&1, Enum.map(cancel, fn {ref, _} -> {:access, ref} end))
    )
  end

  defp cancel_reads(socket) do
    queue = socket.assigns.access_queue

    {cancel, keep} =
      Enum.split_with(socket.assigns.access_requests, fn {_, r} -> r.kind != :write end)

    Enum.each(cancel, fn {ref, _} -> Auth.cancel_action(socket.assigns.session_id, ref) end)

    socket
    |> assign(access_requests: Map.new(keep), access_queue: [])
    |> update(
      :pending_actions,
      &Map.drop(&1, Enum.map(cancel, fn {ref, _} -> {:access, ref} end))
    )
    |> update(:access, fn state ->
      active = for {_, %{kind: :read, card: card}} <- cancel, do: card
      queued = for {card, _} <- queue, card != :help, do: card

      Enum.reduce(Enum.uniq(active ++ queued), state, fn card, current ->
        error = Error.new(:cancelled, "access", Access.read_action(card))
        Access.read_result(current, card, {:error, error}, %{"team_id" => current.team})
      end)
    end)
  end

  defp fence(socket),
    do:
      {socket.assigns.session_id, socket.assigns.session_generation,
       socket.assigns.scope_generation, socket.assigns.access.team}

  defp expire(socket),
    do:
      socket
      |> cancel_reads()
      |> assign(access: Access.new())
      |> push_event("access-secret-clear", %{})
      |> redirect(to: "/sign-in")

  defp client_nonce(nonce) when is_binary(nonce) and byte_size(nonce) <= 64, do: nonce
  defp client_nonce(_), do: nil
  defp valid_scope?(%{dialog: "access.team.create"}, _), do: true

  defp valid_scope?(state, params),
    do: !Map.has_key?(params, "team_id") || params["team_id"] == state.team

  defp card(name),
    do:
      Enum.find(
        [:teams, :effective, :members, :assignments, :invitations, :credentials, :admins],
        &(Atom.to_string(&1) == name)
      )

  defp terminal?(%{error: %Error{metadata: %{"budget_terminal" => true}}}), do: true
  defp terminal?(_), do: false
  defp write_pending?(requests), do: Enum.any?(requests, fn {_, r} -> r.kind == :write end)
end
