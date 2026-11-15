defmodule LabbyConsoleWeb.SessionController do
  use LabbyConsoleWeb, :controller
  alias LabbyConsole.Auth
  plug :no_store
  defp no_store(conn, _), do: put_resp_header(conn, "cache-control", "no-store")

  def new(conn, _) do
    owner =
      get_session(conn, :oauth_owner) ||
        Base.url_encode64(:crypto.strong_rand_bytes(32), padding: false)

    conn
    |> put_session(:oauth_owner, owner)
    |> sign_in_page(Phoenix.Flash.get(conn.assigns.flash, :error))
  end

  defp sign_in_page(conn, message) do
    page =
      LabbyConsoleWeb.SignInPage.render(%{
        token: Plug.CSRFProtection.get_csrf_token(),
        message: message,
        provider: LabbyConsole.OAuthHandoff.configured?(),
        flow: get_session(conn, :oauth_flow_id) || ""
      })

    html(conn, Phoenix.HTML.Safe.to_iodata(page))
  end

  def provider_start(conn, params) when map_size(params) == 0 do
    if get_session(conn, :console_session) do
      conn |> put_status(:conflict) |> json(%{error: "sign_out_required"})
    else
      owner =
        get_session(conn, :oauth_owner) ||
          Base.url_encode64(:crypto.strong_rand_bytes(32), padding: false)

      case Auth.oauth_start(owner) do
        {:ok, result} ->
          conn
          |> put_session(:oauth_owner, owner)
          |> put_session(:oauth_flow_id, result.flow_id)
          |> json(result)

        {:error, :origin_mismatch} ->
          conn |> put_status(:bad_request) |> json(%{error: "provider_origin_mismatch"})

        _ ->
          provider_error(conn)
      end
    end
  end

  def provider_start(conn, _), do: provider_error(conn)

  def provider_poll(conn, %{"flow_id" => flow} = params)
      when map_size(params) == 1 and is_binary(flow) and byte_size(flow) <= 128 do
    if id = get_session(conn, :console_session) do
      case Auth.current(id) do
        {:ok, _} -> json(conn, %{status: "complete"})
        _ -> provider_error(conn)
      end
    else
      case Auth.oauth_poll(get_session(conn, :oauth_owner), flow) do
        {:pending, delay} ->
          json(conn, %{status: "pending", next_poll_ms: delay})

        {:ok, id} ->
          conn
          |> renew_provider_session()
          |> put_session(:console_session, id)
          |> json(%{status: "complete"})

        _ ->
          provider_error(conn)
      end
    end
  end

  def provider_poll(conn, _), do: provider_error(conn)

  def provider_cancel(conn, %{"flow_id" => flow} = params)
      when map_size(params) == 1 and is_binary(flow) and byte_size(flow) <= 128 do
    cond do
      id = get_session(conn, :console_session) ->
        reset_result(conn, Auth.sign_out(id))

      get_session(conn, :oauth_flow_id) == flow and is_binary(get_session(conn, :oauth_owner)) ->
        case Auth.oauth_cancel(get_session(conn, :oauth_owner), flow) do
          {:error, :invalid_flow} -> reset_result(conn, :ok)
          result -> reset_result(conn, result)
        end

      is_nil(get_session(conn, :oauth_owner)) ->
        reset_result(conn, :ok)

      true ->
        provider_error(conn)
    end
  end

  def provider_cancel(conn, params) when map_size(params) == 0 do
    if flow = get_session(conn, :oauth_flow_id) do
      provider_cancel(conn, %{"flow_id" => flow})
    else
      result = if id = get_session(conn, :console_session), do: Auth.sign_out(id), else: :ok
      reset_result(conn, result)
    end
  end

  def provider_cancel(conn, _), do: provider_error(conn)

  defp reset_result(conn, result) when result in [:ok, {:error, :revocation_unconfirmed}] do
    response =
      if result == :ok,
        do: %{status: "cancelled"},
        else: %{status: "cancelled", revocation_unconfirmed: true}

    conn
    |> renew_provider_session()
    |> json(response)
  end

  defp reset_result(conn, _), do: provider_error(conn)

  # The current page keeps its masked token for recovery/reset POSTs.
  # Preserve only its underlying CSRF state while replacing browser authority.
  defp renew_provider_session(conn) do
    csrf = get_session(conn, :_csrf_token)
    conn |> configure_session(renew: true) |> clear_session() |> put_session(:_csrf_token, csrf)
  end

  defp provider_error(conn),
    do: conn |> put_status(:bad_request) |> json(%{error: "provider_sign_in_failed"})

  def create(conn, %{"credential" => credential}) do
    case Auth.sign_in(credential) do
      {:ok, id} ->
        conn
        |> configure_session(renew: true)
        |> clear_session()
        |> put_session(:console_session, id)
        |> redirect(to: "/gateway")

      _ ->
        conn
        |> put_status(:unauthorized)
        |> sign_in_page("Sign-in failed. Check your credential and gateway configuration.")
    end
  end

  def delete(conn, _) do
    result = Auth.sign_out(get_session(conn, :console_session))

    if result == :ok do
      conn |> configure_session(drop: true) |> redirect(to: "/sign-in")
    else
      conn
      |> clear_session()
      |> configure_session(renew: true)
      |> put_flash(
        :error,
        "Signed out locally. Gateway revocation was not confirmed; the remote session may remain active until expiry."
      )
      |> redirect(to: "/sign-in")
    end
  end
end
