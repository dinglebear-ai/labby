defmodule LabbyConsoleWeb.SignInPage do
  @moduledoc "The public sign-in surface, sharing the workspace's Labby identity."
  use LabbyConsoleWeb, :html

  def render(assigns) do
    ~H"""
    <!doctype html>
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <meta name="viewport" content="width=device-width,initial-scale=1" />
        <title>Sign in · Labby</title>
        <meta name="csrf-token" content={@token} />
        <link rel="stylesheet" href={~p"/vendor/aurora.css"} />
        <link rel="stylesheet" href={~p"/assets/css/app.css"} />
        <link rel="stylesheet" href={~p"/assets/css/auth.css"} />
        <script defer src={~p"/assets/js/app.js"}>
        </script>
      </head>
      <body class="labby-auth-page">
        <main id="console-integration" class="console-auth labby-auth">
          <div class="labby-auth-frame">
            <div class="labby-auth-brand" aria-label="Labby">
              <LabbyConsoleWeb.WorkspaceShell.brand_mark />
              <span aria-hidden="true">lab<span class="labby-auth-wordmark-suffix">by</span></span>
            </div>
            <section class="console-panel labby-auth-panel" aria-labelledby="sign-in-title">
              <header class="labby-auth-header">
                <h1 id="sign-in-title">Sign in to Labby</h1>
                <p>Access your gateway workspace.</p>
              </header>
              <p :if={@message} id="sign-in-error" role="alert" class="labby-auth-error">
                {@message}
              </p>
              <section
                :if={@provider}
                class="labby-auth-provider"
                data-provider-login
                data-flow-id={@flow}
                aria-label="Provider sign-in"
              >
                <button type="button" data-provider-start class="labby-auth-primary">
                  Sign in with provider
                </button>
                <p data-provider-status role="status" aria-live="polite"></p>
                <a data-provider-link hidden target="_blank" rel="noopener noreferrer">Open provider sign-in</a>
                <button type="button" data-provider-cancel hidden>Cancel provider sign-in</button>
              </section>
              <div :if={@provider} class="labby-auth-divider"><span>or use a credential</span></div>
              <form action="/sign-in" method="post" class="labby-auth-form">
                <input type="hidden" name="_csrf_token" value={@token} />
                <label for="credential">Gateway credential</label>
                <input
                  id="credential"
                  name="credential"
                  type="password"
                  autocomplete="off"
                  spellcheck="false"
                  autocapitalize="none"
                  aria-describedby={
                    if @message, do: "credential-help sign-in-error", else: "credential-help"
                  }
                  required
                  maxlength="4096"
                />
                <p id="credential-help">
                  Your bearer credential is exchanged once and never stored in your browser.
                </p>
                <button
                  type="submit"
                  class={if @provider, do: "labby-auth-secondary", else: "labby-auth-primary"}
                >Sign in</button>
              </form>
            </section>
          </div>
        </main>
      </body>
    </html>
    """
  end
end
