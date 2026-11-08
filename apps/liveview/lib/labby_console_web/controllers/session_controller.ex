defmodule LabbyConsoleWeb.SessionController do
  use LabbyConsoleWeb, :controller
  alias LabbyConsole.Auth
  plug :no_store
  defp no_store(conn, _), do: put_resp_header(conn, "cache-control", "no-store")

  def new(conn, _), do: sign_in_page(conn, Phoenix.Flash.get(conn.assigns.flash, :error))

  defp sign_in_page(conn, message) do
    token = Plug.CSRFProtection.get_csrf_token()

    html(conn, """
    <!doctype html><html lang="en"><head><meta name="viewport" content="width=device-width,initial-scale=1"><title>Sign in · Labby</title><link rel="stylesheet" href="/vendor/aurora.css"><link rel="stylesheet" href="/assets/css/app.css"></head><body><main id="console-integration" class="console-auth"><section class="console-panel"><h1>Sign in to Labby</h1><p>Use a gateway bearer credential. It is exchanged once and never stored in your browser.</p>#{if message, do: "<p role=\"alert\">#{message}</p>", else: ""}<form action="/sign-in" method="post"><input type="hidden" name="_csrf_token" value="#{token}"><label for="credential">Gateway credential</label><input id="credential" name="credential" type="password" autocomplete="off" required maxlength="4096"><button type="submit">Sign in</button></form></section></main></body></html>
    """)
  end

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
