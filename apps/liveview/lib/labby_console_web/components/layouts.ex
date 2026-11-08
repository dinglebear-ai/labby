defmodule LabbyConsoleWeb.Layouts do
  @moduledoc """
  This module holds layouts and related functionality
  used by your application.
  """
  use LabbyConsoleWeb, :html

  # Embed all files in layouts/* within this module.
  # The default root.html.heex file contains the HTML
  # skeleton of your application, namely HTML headers
  # and other static content.
  embed_templates "layouts/*"

  @doc """
  Renders your app layout.

  This function is typically invoked from every template,
  and it often contains your application menu, sidebar,
  or similar.

  ## Examples

      <Layouts.app flash={@flash}>
        <h1>Content</h1>
      </Layouts.app>

  """
  attr :flash, :map, required: true, doc: "the map of flash messages"

  attr :current_scope, :map,
    default: nil,
    doc: "the current [scope](https://phoenix.hexdocs.pm/scopes.html)"

  attr :page, :string, default: "Gateway"
  attr :authenticated, :boolean, default: false
  attr :subject, :string, default: nil
  attr :preference_key, :string, default: nil

  slot :inner_block, required: true

  def app(assigns) do
    assigns = assign(assigns, :routes, navigation(assigns.authenticated))

    ~H"""
    <LabbyConsoleWeb.WorkspaceShell.shell
      id="labby-workspace-shell"
      page={String.downcase(@page)}
      main_id="console-integration"
      authenticated={@authenticated}
      routes={@routes}
      subject={@subject}
      preference_key={@preference_key}
    >
      {render_slot(@inner_block)}
    </LabbyConsoleWeb.WorkspaceShell.shell>
    <.flash_group flash={@flash} />
    """
  end

  defp navigation(false), do: %{"gateway" => "/"}

  defp navigation(true),
    do: %{
      "overview" => "/overview",
      "gateway" => "/gateway",
      "runtime" => "/runtime",
      "clients" => "/clients",
      "diagnostics" => "/diagnostics",
      "usage" => "/usage",
      "activity" => "/activity",
      "logs" => "/logs"
    }

  attr :name, :string, required: true
  attr :id, :string, required: true

  def server_sections(assigns) do
    ~H"""
    <nav class="console-section-nav" aria-label="Server sections">
      <.link navigate={"/gateway/servers/" <> URI.encode(@id, &URI.char_unreserved?/1)}>Overview</.link>
      <.link navigate={"/gateway/servers/" <> URI.encode(@name, &URI.char_unreserved?/1) <> "/catalog"}>Catalog</.link>
      <.link navigate={"/gateway/upstreams/" <> URI.encode(@name, &URI.char_unreserved?/1) <> "/runtime"}>Runtime</.link>
    </nav>
    """
  end

  attr :label, :string, required: true
  attr :id, :string, required: true
  attr :event, :string, required: true
  attr :disabled, :boolean, default: false

  def refresh_action(assigns) do
    ~H"""
    <button
      id={@id}
      class="console-icon-button"
      phx-click={@event}
      disabled={@disabled}
      aria-label={@label}
      title={@label}
    >
      <svg
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="1.75"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
      ><path d="M20 7v5h-5M4 17v-5h5" /><path d="M6.1 7a7 7 0 0 1 11.6-1L20 9M4 15l2.3 3A7 7 0 0 0 17.9 17" /></svg>
    </button>
    """
  end

  @doc """
  Shows the flash group with standard titles and content.

  ## Examples

      <.flash_group flash={@flash} />
  """
  attr :flash, :map, required: true, doc: "the map of flash messages"
  attr :id, :string, default: "flash-group", doc: "the optional id of flash container"

  def flash_group(assigns) do
    ~H"""
    <div id={@id} aria-live="polite">
      <.flash kind={:info} flash={@flash} />
      <.flash kind={:error} flash={@flash} />

      <.flash
        id="client-error"
        kind={:error}
        title="We can't find the internet"
        phx-disconnected={
          show(".phx-client-error #client-error")
          |> JS.remove_attribute("hidden", to: ".phx-client-error #client-error")
        }
        phx-connected={hide("#client-error") |> JS.set_attribute({"hidden", ""})}
        hidden
      >
        Attempting to reconnect
        <.icon name="hero-arrow-path" class="ml-1 size-3 motion-safe:animate-spin" />
      </.flash>

      <.flash
        id="server-error"
        kind={:error}
        title="Something went wrong!"
        phx-disconnected={
          show(".phx-server-error #server-error")
          |> JS.remove_attribute("hidden", to: ".phx-server-error #server-error")
        }
        phx-connected={hide("#server-error") |> JS.set_attribute({"hidden", ""})}
        hidden
      >
        Attempting to reconnect
        <.icon name="hero-arrow-path" class="ml-1 size-3 motion-safe:animate-spin" />
      </.flash>
    </div>
    """
  end
end
