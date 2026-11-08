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

  slot :inner_block, required: true

  def app(assigns) do
    ~H"""
    <a class="console-skip" href="#console-integration">Skip to content</a>
    <div class="console-frame">
      <aside class="console-sidebar">
        <a href="/" class="console-brand" aria-label="Labby Console home">
          <svg viewBox="0 0 32 32" aria-hidden="true" data-labby-mark="fold"><path
            d="M5 8.5Q5 6.3 7 5.2L14 1.5V22H22V30H14Q5 30 5 21Z"
            fill="var(--aurora-accent-primary)"
          /><path d="M22 22H25Q31 22 31 28V30H22Z" fill="var(--aurora-accent-pink)" /></svg>
          <span class="console-wordmark">lab<span>by</span></span>
        </a>
        <p class="console-nav-label">Gateway</p>
        <nav aria-label="Primary navigation">
          <a
            :for={{label, path, icon} <- navigation(@authenticated)}
            href={path}
            aria-current={if @page == label, do: "page"}
            title={label}
            aria-label={label}
          ><svg
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="1.75"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
          ><path d={icon} /></svg><span>{label}</span></a>
        </nav>
        <p id="console-scope">
          {if @authenticated, do: "Authenticated session", else: "Public probes"}
        </p>
      </aside>
      <div class="console-workspace">
        <header class="console-topbar">
          <span>Workspace / <strong>{@page}</strong></span><span class="console-runtime-label">LiveView Console</span>
        </header>
        {render_slot(@inner_block)}
      </div>
    </div>

    <.flash_group flash={@flash} />
    """
  end

  defp navigation(false), do: [{"Gateway", "/", "M4 5h16v6H4zM4 15h16v5H4zM7 8h1M7 18h1"}]

  defp navigation(true),
    do: [
      {"Overview", "/overview", "M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM14 14h7v7h-7z"},
      {"Gateway", "/gateway", "M4 5h16v6H4zM4 15h16v5H4zM7 8h1M7 18h1"},
      {"Runtime", "/runtime", "M3 12h4l3-8 4 16 3-8h4"},
      {"Clients", "/clients", "M8 3v4M16 3v4M5 7h14v13H5zM9 12h.01M15 12h.01M9 16h6"},
      {"Diagnostics", "/diagnostics", "M12 3 3 20h18L12 3zM12 9v5M12 17h.01"},
      {"Usage", "/usage", "M4 20V10M10 20V4M16 20v-7M22 20H2"},
      {"Activity", "/activity", "M3 12h4l3-8 4 16 3-8h4"},
      {"Logs", "/logs", "M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01"}
    ]

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
