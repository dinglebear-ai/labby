defmodule LabbyConsoleWeb.LiveSession do
  @moduledoc "Shared authenticated mounts, local revocation and request-scope fencing."
  import Phoenix.Component, only: [assign: 2, assign: 3]
  import Phoenix.LiveView, only: [connected?: 1, attach_hook: 4, redirect: 2]
  alias LabbyConsole.Auth

  @return_paths %{
    LabbyConsoleWeb.ProjectsLive => "/projects",
    LabbyConsoleWeb.GatewayLive => "/gateway",
    LabbyConsoleWeb.RuntimeLive => "/runtime",
    LabbyConsoleWeb.ClientsLive => "/clients",
    LabbyConsoleWeb.DiagnosticsLive => "/diagnostics",
    LabbyConsoleWeb.UsageMetricsLive => "/usage",
    LabbyConsoleWeb.UsageCallsLive => "/activity",
    LabbyConsoleWeb.ServerLogsLive => "/logs"
  }

  def context(id) do
    with {:ok, session} <- Auth.current(id),
         %{target: target, generation: generation} <- Auth.capture(id) do
      user = Map.take(session["user"], ~w(sub name display_name email iss))

      scope =
        session
        |> Map.take(
          ~w(expires_at capabilities is_admin is_configured_admin authority_state authority owner team_id project_id)
        )
        |> Map.put("user", user)

      namespace =
        :crypto.hash(
          :sha256,
          :erlang.term_to_binary(
            {String.trim_trailing(target, "/"), namespace_identity(session, id)}
          )
        )
        |> Base.encode16(case: :lower)

      {:ok,
       %{
         session_id: id,
         session_generation: generation,
         subject: user["sub"],
         current_scope: scope,
         preference_key: namespace
       }}
    else
      {:error, _} = error -> error
      _ -> {:error, :unauthenticated}
    end
  end

  defp namespace_identity(
         %{
           "authority_state" => "ready",
           "authority" => %{"principal_id" => principal, "organization_id" => organization}
         },
         _id
       )
       when is_binary(principal) and principal != "" and is_binary(organization) and
              organization != "",
       do: {:principal, organization, principal}

  # An unbound external subject does not establish a durable principal identity.
  defp namespace_identity(_, id), do: {:session, id}

  def on_mount(:authenticated, _params, session, socket) do
    case context(session["console_session"]) do
      {:ok, context} ->
        socket =
          socket
          |> assign(context)
          |> assign(workspace_scope: %{}, scope_generation: 0, pending_actions: %{})

        socket =
          if connected?(socket) do
            Phoenix.PubSub.subscribe(
              LabbyConsole.PubSub,
              "console-session:" <> context.session_id
            )

            schedule_expiry(context.session_id, context.current_scope["expires_at"])
            attach_hook(socket, :console_session_lifecycle, :handle_info, &lifecycle/2)
          else
            socket
          end

        {:cont, socket}

      {:error, reason} when reason in [:unavailable, :forbidden] ->
        path = Map.get(@return_paths, socket.view, "/overview")
        {:halt, redirect(socket, to: "/connection?return_to=" <> URI.encode_www_form(path))}

      _ ->
        {:halt, redirect(socket, to: "/sign-in")}
    end
  end

  def track(socket, operation, request_id) do
    previous = socket.assigns.pending_actions[operation]
    if previous && previous.request_id != request_id, do: cancel(socket, previous.request_id)

    ticket = %{
      request_id: request_id,
      scope_generation: socket.assigns.scope_generation,
      session_generation: socket.assigns.session_generation
    }

    assign(socket, :pending_actions, Map.put(socket.assigns.pending_actions, operation, ticket))
  end

  def current?(socket, operation, request_id) do
    case socket.assigns.pending_actions[operation] do
      %{request_id: ^request_id, scope_generation: scope, session_generation: session} ->
        scope == socket.assigns.scope_generation && session == socket.assigns.session_generation &&
          Auth.matches?(socket.assigns.session_id, session)

      _ ->
        false
    end
  end

  def set_scope(socket, scope) when is_map(scope) do
    scope = Map.take(scope, [:team_id, :project_id])

    if scope == socket.assigns.workspace_scope do
      socket
    else
      Enum.each(socket.assigns.pending_actions, fn {_, ticket} ->
        cancel(socket, ticket.request_id)
      end)

      assign(socket,
        workspace_scope: scope,
        scope_generation: socket.assigns.scope_generation + 1,
        pending_actions: %{}
      )
    end
  end

  defp cancel(socket, request_id), do: Auth.cancel_action(socket.assigns.session_id, request_id)

  defp lifecycle({:console_session_revoked, id}, %{assigns: %{session_id: id}} = socket),
    do: {:halt, expire(socket)}

  defp lifecycle({:console_session_expiry, id}, %{assigns: %{session_id: id}} = socket) do
    expiry = socket.assigns.current_scope["expires_at"]

    if expiry <= System.system_time(:second) ||
         !Auth.matches?(id, socket.assigns.session_generation) do
      {:halt, expire(socket)}
    else
      schedule_expiry(id, expiry)
      {:halt, socket}
    end
  end

  defp lifecycle(_, socket), do: {:cont, socket}

  defp expire(socket) do
    Enum.each(socket.assigns.pending_actions, fn {_, ticket} ->
      cancel(socket, ticket.request_id)
    end)

    socket
    |> assign(current_scope: nil, pending_actions: %{})
    |> redirect(to: "/sign-in")
  end

  defp schedule_expiry(id, expiry) do
    remaining = max(0, expiry - System.system_time(:second)) * 1_000
    Process.send_after(self(), {:console_session_expiry, id}, min(remaining, 60_000))
  end
end
