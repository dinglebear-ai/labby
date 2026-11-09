defmodule LabbyConsoleWeb.GatewayLive do
  use LabbyConsoleWeb, :live_view
  on_mount {LabbyConsoleWeb.LiveSession, :authenticated}
  alias LabbyConsole.{Auth, GatewayFleet, GatewayMutations, GatewayAction.Error}
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
        storage_warning: false,
        mutation: empty_mutation(),
        mutation_request: nil,
        mutation_receipt: nil,
        mutation_notice: nil,
        mutation_restore: nil,
        mutation_focus_after_load: nil
      )

    {:ok, if(connected?(socket), do: load(socket), else: socket)}
  end

  def handle_params(params, _, socket) do
    socket = if socket.assigns.mutation.dialog, do: close_mutation(socket), else: socket

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

  def handle_event("gateway_close", _, socket), do: {:noreply, close_mutation(socket)}

  def handle_event(
        "gateway_reconcile",
        _,
        %{assigns: %{mutation_request: nil, mutation_receipt: receipt}} = socket
      )
      when not is_nil(receipt),
      do: {:noreply, reconcile_mutation(socket)}

  def handle_event(event, _, %{assigns: %{mutation_request: request}} = socket)
      when not is_nil(request) and
             event in ~w(gateway_add gateway_edit gateway_remove gateway_row_test gateway_test gateway_save gateway_confirm_remove gateway_form_change gateway_json gateway_json_change gateway_apply_json gateway_reconcile),
      do: {:noreply, socket}

  def handle_event(event, _, %{assigns: %{mutation_receipt: receipt}} = socket)
      when not is_nil(receipt) and
             event in ~w(gateway_add gateway_edit gateway_remove gateway_row_test gateway_test gateway_save gateway_confirm_remove),
      do: {:noreply, socket}

  def handle_event("gateway_add", _, socket) do
    {:noreply,
     assign(socket,
       mutation: %{empty_mutation() | dialog: :form},
       mutation_notice: nil,
       mutation_restore: nil
     )}
  end

  def handle_event(event, %{"name" => name} = params, socket)
      when event in ~w(gateway_edit gateway_remove gateway_row_test) do
    row =
      Enum.find(socket.assigns.rows, &(&1["name"] == name and &1["source"] == "custom_gateway"))

    if row do
      mode =
        %{"gateway_edit" => :edit, "gateway_remove" => :remove, "gateway_row_test" => :test}[
          event
        ]

      action = if mode == :remove, do: "remove", else: if(mode == :test, do: "test", else: "edit")
      opener = LabbyConsoleWeb.GatewayMutationComponents.action_id(action, name)

      opener =
        if params["opener"] == opener <> "-inspector", do: opener <> "-inspector", else: opener

      mutation = %{empty_mutation() | dialog: :loading, opener: "#" <> opener}

      {:noreply,
       socket
       |> assign(mutation: mutation, mutation_notice: nil, mutation_restore: nil)
       |> begin_mutation({:open, mode, name}, "gateway.get", %{"name" => name})}
    else
      {:noreply, socket}
    end
  end

  def handle_event("gateway_form_change", %{"server" => params}, socket) do
    {:noreply, change_mutation(socket, params)}
  end

  def handle_event("gateway_json", _, %{assigns: %{mutation: %{dialog: :form}}} = socket) do
    {:noreply, update(socket, :mutation, &%{&1 | json_open: !&1.json_open, errors: []})}
  end

  def handle_event(
        "gateway_json_change",
        %{"config" => %{"json" => json}},
        %{assigns: %{mutation: %{dialog: :form, json_open: true}}} = socket
      )
      when is_binary(json) and byte_size(json) <= 32_768 do
    {:noreply, update(socket, :mutation, &%{&1 | json: json, errors: []})}
  end

  def handle_event(
        "gateway_apply_json",
        %{"config" => %{"json" => json}},
        %{assigns: %{mutation: %{dialog: :form, json_open: true}}} = socket
      ) do
    case GatewayMutations.apply_json(socket.assigns.mutation.model, json) do
      {:ok, model} ->
        {:noreply,
         update(
           socket,
           :mutation,
           &%{&1 | model: model, json: "", json_open: false, errors: [], test_result: nil}
         )}

      {:error, errors} ->
        {:noreply, update(socket, :mutation, &%{&1 | errors: errors})}
    end
  end

  def handle_event(
        "gateway_save",
        %{"server" => params},
        %{assigns: %{mutation: %{dialog: :form}}} = socket
      ),
      do: {:noreply, socket |> change_mutation(params) |> submit_mutation(:save)}

  def handle_event(
        "gateway_test",
        _,
        %{assigns: %{mutation: %{dialog: :form, json_open: false}}} = socket
      ),
      do: {:noreply, submit_mutation(socket, :test)}

  def handle_event(
        "gateway_confirm_remove",
        %{"confirmation" => %{"name" => name}},
        %{assigns: %{mutation: %{dialog: :remove, model: model}}} = socket
      ) do
    if name == model.original do
      {:noreply, begin_mutation(socket, :write, "gateway.remove", %{"name" => model.original})}
    else
      {:noreply,
       update(
         socket,
         :mutation,
         &%{&1 | errors: ["The confirmation must match the exact selected server ID."]}
       )}
    end
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

    restore = socket.assigns.mutation_focus_after_load || socket.assigns.mutation_restore

    restore =
      if restore,
        do:
          LabbyConsoleWeb.GatewayMutationComponents.restore_target(
            GatewayFleet.visible(rows, socket.assigns.preferences),
            selected,
            restore
          )

    {:noreply,
     assign(socket,
       rows: rows,
       selected: selected,
       error: nil,
       mutation_restore: restore,
       mutation_focus_after_load: nil
     )}
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

  def handle_info(
        {:gateway_action, ref, result},
        %{assigns: %{mutation_request: %{ref: ref} = request}} = socket
      ) do
    socket = assign(socket, mutation_request: nil)

    if Auth.matches?(socket.assigns.session_id, request.generation) do
      {:noreply, finish_mutation(socket, request, result)}
    else
      {:noreply, expire(socket)}
    end
  end

  def handle_info({:gateway_action, _, _}, socket), do: {:noreply, socket}

  def handle_info(:session_expired, socket), do: {:noreply, expire(socket)}
  def handle_info(_, socket), do: {:noreply, socket}

  defp expire(socket) do
    if socket.assigns.mutation_request,
      do: Auth.cancel_action(socket.assigns.session_id, socket.assigns.mutation_request.ref)

    socket =
      assign(socket,
        mutation: empty_mutation(),
        mutation_request: nil,
        mutation_receipt: nil,
        mutation_notice: nil,
        mutation_restore: nil,
        mutation_focus_after_load: nil
      )

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

  defp empty_mutation do
    %GatewayMutations.Dialog{model: GatewayMutations.new()}
  end

  defp change_mutation(socket, params) do
    update(socket, :mutation, fn mutation ->
      if mutation.dialog == :form do
        model = GatewayMutations.change(mutation.model, params)

        %{
          mutation
          | model: model,
            test_result: if(model == mutation.model, do: mutation.test_result),
            errors: []
        }
      else
        mutation
      end
    end)
  end

  defp submit_mutation(socket, purpose) do
    case GatewayMutations.build_request(socket.assigns.mutation.model, purpose) do
      {:ok, action, params} ->
        begin_mutation(socket, if(purpose == :save, do: :write, else: :test), action, params)

      {:error, errors} ->
        update(socket, :mutation, &%{&1 | errors: errors})
    end
  end

  defp begin_mutation(socket, purpose, action, params) do
    request = %{
      purpose: purpose,
      action: action,
      generation: socket.assigns.session_generation,
      opener: socket.assigns.mutation.opener,
      name: params["name"] || get_in(params, ["spec", "name"])
    }

    request =
      if purpose == :write,
        do:
          Map.put(request, :receipt, %{mutation_receipt(action, params) | opener: request.opener}),
        else: request

    socket = update(socket, :mutation, &%{&1 | errors: [], test_result: nil})

    case Auth.start_action(socket.assigns.session_id, "gateway", action, params) do
      {:ok, ref} ->
        request =
          if purpose == :write,
            do: Map.update!(request, :receipt, &%{&1 | request_id: ref}),
            else: request

        assign(socket, mutation_request: Map.put(request, :ref, ref))

      {:error, error} ->
        finish_mutation(socket, request, {:error, error})
    end
  end

  defp finish_mutation(socket, _request, {:error, %Error{kind: :unauthenticated}}),
    do: expire(socket)

  defp finish_mutation(socket, %{purpose: {:open, mode, name}}, {:ok, data}) do
    case GatewayMutations.draft_from_view(data, name) do
      {:ok, model} ->
        socket =
          update(
            socket,
            :mutation,
            &%{&1 | dialog: if(mode == :remove, do: :remove, else: :form), model: model}
          )

        if mode == :test, do: submit_mutation(socket, :test), else: socket

      {:error, errors} ->
        update(socket, :mutation, &%{&1 | errors: errors})
    end
  end

  defp finish_mutation(socket, %{purpose: :test}, {:ok, data}) do
    model = socket.assigns.mutation.model
    name = model.original || model.draft["name"]

    case GatewayMutations.test_result(data, name) do
      {:ok, _} ->
        update(
          socket,
          :mutation,
          &%{
            &1
            | test_result:
                "Probe connected and observed capabilities. This temporary connection does not save the draft or establish a retained connection."
          }
        )

      {:error, message} ->
        update(socket, :mutation, &%{&1 | errors: [message]})
    end
  end

  defp finish_mutation(socket, %{purpose: :write, receipt: receipt} = request, {:ok, data}) do
    name =
      if request.action == "gateway.remove", do: hd(receipt.names), else: List.last(receipt.names)

    case GatewayMutations.draft_from_view(data, name) do
      {:ok, _} ->
        # Accepted responses can contain backend-derived public fields. Keep the
        # original uncertainty expectations until this name-bound decode passes.
        expectations =
          case receipt.expectations[name] do
            {:present, _} ->
              Map.put(receipt.expectations, name, {:present, digests(data["config"])})

            :absent ->
              receipt.expectations
          end

        socket
        |> assign(
          mutation: empty_mutation(),
          mutation_receipt: %{receipt | outcome: :accepted, expectations: expectations},
          mutation_notice: "Gateway change accepted. Reading affected configuration."
        )
        |> reconcile_mutation()

      _ ->
        mutation_failure(
          socket,
          request,
          Error.new(:invalid_response, "gateway", request.action, side_effects: "unknown")
        )
    end
  end

  defp finish_mutation(socket, %{purpose: {:reconcile, name}}, result) do
    receipt = socket.assigns.mutation_receipt

    observation =
      case result do
        {:ok, data} ->
          case GatewayMutations.draft_from_view(data, name) do
            {:ok, _} -> {:present, digests(data["config"])}
            _ -> :unavailable
          end

        {:error, %Error{kind: :not_found}} ->
          :absent

        _ ->
          :unavailable
      end

    receipt = %{receipt | observed: Map.put(receipt.observed, name, observation)}
    socket = assign(socket, mutation_receipt: receipt)
    remaining = Enum.reject(receipt.names, &Map.has_key?(receipt.observed, &1))

    case remaining do
      [next | _] -> begin_mutation(socket, {:reconcile, next}, "gateway.get", %{"name" => next})
      [] -> finish_reconciliation(socket)
    end
  end

  defp finish_mutation(socket, request, {:error, %Error{} = error}),
    do: mutation_failure(socket, request, error)

  defp finish_mutation(socket, request, _),
    do:
      mutation_failure(
        socket,
        request,
        Error.new(:unavailable, "gateway", request.action,
          side_effects:
            if(request.purpose in [:test, :write], do: "unknown", else: "none_expected")
        )
      )

  defp mutation_failure(socket, request, error) do
    uncertain = error.side_effects in ~w(possible unknown)

    message =
      if uncertain,
        do:
          "Outcome uncertain (#{error.kind}). Inspect the affected configuration before another operation. The request has not been retried.",
        else: "Gateway operation failed (#{error.kind}). The draft is retained."

    socket = update(socket, :mutation, &%{&1 | errors: [message]})

    cond do
      request.purpose == :write and uncertain ->
        assign(socket, mutation_receipt: request.receipt, mutation_notice: message)

      request.purpose == :test and uncertain ->
        # A saved read cannot prove that an ephemeral upstream probe has drained.
        # Retain a probe hold for this mounted view instead of inviting a replay.
        name = request.name

        assign(socket,
          mutation_receipt: %{
            action: request.action,
            names: [name],
            outcome: :uncertain,
            expectations: %{},
            private?: true,
            observed: %{},
            request_id: request[:ref]
          },
          mutation_notice:
            "Probe wait ended (#{error.kind}). External work may continue. Configuration inspection cannot confirm that the temporary probe has drained."
        )

      true ->
        socket
    end
  end

  defp mutation_receipt(action, params) do
    {names, expectations, private?} =
      case action do
        "gateway.add" ->
          spec = params["spec"]

          {[spec["name"]], %{spec["name"] => {:present, digests(spec)}},
           private_write?(params, spec)}

        "gateway.update" ->
          patch = params["patch"]
          original = params["name"]
          name = patch["name"] || original
          expected = %{name => {:present, digests(Map.put(patch, "name", name))}}
          expected = if name != original, do: Map.put(expected, original, :absent), else: expected
          {Enum.uniq([original, name]), expected, private_write?(params, patch)}

        "gateway.remove" ->
          {[params["name"]], %{params["name"] => :absent}, false}
      end

    %{
      action: action,
      names: names,
      outcome: :uncertain,
      expectations: expectations,
      private?: private?,
      observed: %{},
      opener: nil,
      request_id: nil
    }
  end

  defp private_write?(params, config),
    do:
      Map.has_key?(params, "bearer_token_value") or
        Enum.any?(~w(env headers headers_env display_name), &Map.has_key?(config, &1))

  defp digests(config) do
    config
    |> Map.take(
      ~w(name enabled url command args bearer_token_env proxy_resources proxy_prompts proxy_skills expose_tools expose_resources expose_prompts expose_skills)
    )
    |> Map.new(fn {key, value} -> {key, :crypto.hash(:sha256, Jason.encode!(value))} end)
  end

  defp reconcile_mutation(socket) do
    receipt = %{socket.assigns.mutation_receipt | observed: %{}}
    socket = assign(socket, mutation_receipt: receipt, mutation_restore: nil)
    [name | _] = receipt.names
    begin_mutation(socket, {:reconcile, name}, "gateway.get", %{"name" => name})
  end

  defp finish_reconciliation(socket) do
    receipt = socket.assigns.mutation_receipt

    verified =
      receipt.action != "gateway.test" and
        Enum.all?(receipt.expectations, fn {name, expected} ->
          case {expected, receipt.observed[name]} do
            {:absent, :absent} ->
              true

            {{:present, wanted}, {:present, actual}} ->
              Enum.all?(wanted, fn {key, hash} -> actual[key] == hash end)

            _ ->
              false
          end
        end)

    verified = verified and (receipt.outcome == :accepted or not receipt.private?)

    if verified do
      opener = receipt.opener

      notice =
        if receipt.outcome == :accepted,
          do:
            "Gateway change accepted. Affected configuration read back. Connection status is shown separately.",
          else:
            "Affected configuration now matches the submitted change. The request has not been replayed."

      socket
      |> assign(
        mutation: empty_mutation(),
        mutation_receipt: nil,
        mutation_notice: notice,
        mutation_restore: nil
      )
      |> load()
      |> assign(mutation_focus_after_load: opener)
    else
      assign(socket,
        mutation_notice:
          "Affected configuration inspection is incomplete or cannot verify this operation. Its receipt is retained. Another write remains blocked."
      )
    end
  end

  defp close_mutation(socket) do
    request = socket.assigns.mutation_request
    socket = assign(socket, mutation: empty_mutation())

    if request do
      Auth.cancel_action(socket.assigns.session_id, request.ref)
      result = LabbyConsole.GatewayAction.Lifecycle.terminal_result(request.ref)
      socket = assign(socket, mutation_request: nil)

      case result do
        {:error, %Error{side_effects: risk} = error} when risk in ~w(possible unknown) ->
          mutation_failure(socket, request, error)

        {:ok, data} when request.purpose == :write ->
          finish_mutation(socket, request, {:ok, data})

        _ ->
          socket
      end
    else
      socket
    end
  end
end
