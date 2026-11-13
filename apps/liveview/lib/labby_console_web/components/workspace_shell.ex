defmodule LabbyConsoleWeb.WorkspaceShell do
  @moduledoc "Native Labby chrome with an explicit implemented route map."
  use LabbyConsoleWeb, :html
  alias LabbyConsoleWeb.WorkspaceIcons

  @doc "Labby's Fold mark, drawn for compact navigation and wordmark lockups."
  def brand_mark(assigns) do
    ~H"""
    <svg viewBox="0 0 32 32" fill="none" aria-hidden="true" focusable="false" data-labby-mark="fold">
      <path d="M5 8.5Q5 6.3 7 5.2L14 1.5V22H22V30H14Q5 30 5 21Z" fill="var(--aurora-accent-strong)" />
      <path d="M14 22H22V30H14Q10 30 7.5 27.5Q14 29 14 22Z" fill="var(--aurora-accent-strong)" />
      <path d="M22 22H25Q31 22 31 28V30H22Z" fill="var(--aurora-accent-pink)" />
    </svg>
    """
  end

  attr :id, :string, required: true

  attr :page, :string, required: true
  attr :main_id, :string, required: true
  attr :server_count, :integer, default: nil
  attr :subject, :string, default: nil
  attr :authenticated, :boolean, default: true
  attr :preference_key, :string, default: nil
  attr :routes, :map, required: true
  slot :inner_block, required: true

  def shell(assigns) do
    assigns = assign(assigns, :identity, assigns.subject || "Authenticated session")

    ~H"""
    <div
      id={@id}
      class="labby-overview"
      data-preference-key={@preference_key}
      phx-hook="WorkspaceChrome"
    >
      <a href={"##{@main_id}"} class="ov-skip">Skip to content</a>
      <aside class="ov-sidebar" aria-label="Labby navigation">
        <.link
          navigate={Map.get(@routes, "overview", Map.get(@routes, "gateway", "/"))}
          class="ov-brand"
          aria-label="Labby overview"
        ><span class="ov-brand-mark"><.brand_mark /></span><span class="labby-wordmark"><span class="labby-wordmark-prefix">lab</span><span class="labby-wordmark-suffix">by</span></span></.link>
        <button
          id="labby-sidebar-toggle"
          type="button"
          class="ov-icon-button lb-sidebar-toggle"
          aria-label="Collapse navigation"
          aria-expanded="true"
          title="Collapse navigation"
        ><WorkspaceIcons.icon name="chevron" class="ov-small" /></button>
        <div class="lb-sidebar-scroll">
          <div :for={{group, items} <- groups(@routes)}>
            <p class="ov-nav-label">{group}</p>
            <nav aria-label={if group == "Gateway", do: "Primary navigation", else: group}>
              <.nav_item
                :for={{page, label, icon} <- items}
                page={page}
                label={label}
                icon={icon}
                current={@page}
                routes={@routes}
                count={if page == "gateway", do: @server_count}
              />
            </nav>
          </div>
        </div>
        <p :if={!@authenticated} id="console-scope" class="lb-profile-note">Public probes</p>
        <.profile :if={@authenticated} id="labby-workspace" identity={@identity} routes={@routes} />
      </aside>
      <div class="ov-body">
        <header class="ov-topbar">
          <div>
            <span class="ov-breadcrumb">Workspace</span><span class="ov-slash">/</span><strong>{page_label(
              @page
            )}</strong>
          </div>
          <div class="labby-header-status">
            <span
              id="labby-connection"
              class="labby-connection"
              data-state="connecting"
              role="status"
              aria-live="polite"
              title="LiveView connection; gateway readiness is reported in page content"
              aria-label="Console connecting"
              phx-connected={
                JS.set_attribute({"data-state", "connected"})
                |> JS.set_attribute({"aria-label", "Console connected"})
              }
              phx-disconnected={
                JS.set_attribute({"data-state", "disconnected"})
                |> JS.set_attribute({"aria-label", "Console reconnecting"})
              }
            >
              <span class="labby-connection-dot" aria-hidden="true"></span>
              <span class="labby-connection-message labby-connection-connecting">Connecting…</span>
              <span class="labby-connection-message labby-connection-connected">Connected</span>
              <span class="labby-connection-message labby-connection-disconnected">Reconnecting…</span>
            </span>
            <.profile
              :if={@authenticated}
              id="labby-mobile-profile"
              identity={@identity}
              routes={@routes}
              mobile
            />
          </div>
          <div id="labby-request-indicator" class="labby-request-indicator" aria-hidden="true">
            <span></span>
          </div>
        </header>
        <nav class="labby-mobile-nav" aria-label="Labby pages">
          <.link
            :for={
              {page, label, icon} <-
                Enum.filter(pages(@routes), fn {page, _, _} ->
                  page in ~w(overview gateway observe projects)
                end)
            }
            id={"labby-mobile-#{page}"}
            class="labby-icon-action"
            aria-label={label}
            title={label}
            aria-current={if(@page == page, do: "page")}
            {navigation(@routes, page)}
          ><WorkspaceIcons.icon name={icon} class="ov-small" /><span class="labby-action-label">{label}</span></.link>
          <button
            id="labby-mobile-more"
            type="button"
            popovertarget="labby-mobile-more-menu"
            aria-label="More pages"
            title="More pages"
            class={[
              "labby-icon-action",
              @page not in ~w(overview gateway observe projects) &&
                "labby-more-current"
            ]}
          ><WorkspaceIcons.icon name="more" class="ov-small" /></button>
        </nav>
        <div id="labby-mobile-more-menu" popover class="labby-mobile-more-menu">
          <header>
            <strong>Workspace</strong><button
              type="button"
              popovertarget="labby-mobile-more-menu"
              popovertargetaction="hide"
              id="labby-more-close"
              aria-label="Close more pages"
            ><WorkspaceIcons.icon name="close" class="ov-small" /></button>
          </header><nav aria-label="More Labby pages">
            <.link
              :for={
                {page, label, icon} <-
                  Enum.reject(pages(@routes), fn {page, _, _} ->
                    page in ~w(overview gateway observe projects)
                  end)
              }
              id={"labby-mobile-#{page}"}
              aria-label={label}
              title={label}
              phx-click={JS.dispatch("click", to: "#labby-more-close")}
              aria-current={if @page == page, do: "page"}
              {navigation(@routes, page)}
            ><WorkspaceIcons.icon name={icon} class="ov-small" />{label}</.link>
          </nav>
        </div>
        {render_slot(@inner_block)}
      </div>
    </div>
    """
  end

  attr :page, :string, required: true
  attr :label, :string, required: true
  attr :icon, :string, required: true
  attr :current, :string, required: true
  attr :count, :any, default: nil
  attr :routes, :map, required: true

  defp nav_item(assigns) do
    ~H"""
    <.link
      id={"labby-nav-#{@page}"}
      class={["ov-nav-item", "lb-nav-#{@page}", @page == @current && "ov-nav-active"]}
      aria-current={if(@page == @current, do: "page")}
      title={@label}
      {navigation(@routes, @page)}
    >
      <WorkspaceIcons.icon name={@icon} /><span class="lb-nav-label">{@label}</span><span
        :if={@count}
        class="ov-nav-count"
      >{@count}</span>
    </.link>
    """
  end

  attr :id, :string, required: true
  attr :identity, :string, required: true
  attr :routes, :map, required: true
  attr :mobile, :boolean, default: false

  defp profile(assigns) do
    ~H"""
    <details
      id={@id}
      class={["lb-profile", @mobile && "lb-profile-mobile"]}
      phx-hook=".ProfileMenu"
      phx-mounted={JS.ignore_attributes("open")}
    >
      <summary aria-label="Session menu" title={@identity}>
        <span class="lb-profile-avatar"><WorkspaceIcons.icon name="user" /></span>
        <span class="lb-profile-text"><strong>{@identity}</strong></span>
        <WorkspaceIcons.icon name="chevrons" class="ov-small lb-profile-chevron" />
      </summary>
      <div id={"#{@id}-menu"} class="lb-profile-menu" popover>
        <p class="lb-profile-menu-label">Signed in as</p>
        <p class="lb-profile-note">{@identity}</p>
        <.link :for={{page, label, icon} <- pages(@routes)} {navigation(@routes, page)}>
          <WorkspaceIcons.icon name={icon} class="ov-small" />{label}
        </.link>
        <.form for={%{}} action="/sign-out" method="post" id={"#{@id}-sign-out"}>
          <button type="submit" class="labby-icon-action">
            <WorkspaceIcons.icon name="user" class="ov-small" />Sign out
          </button>
        </.form>
      </div>
    </details>
    <script :type={Phoenix.LiveView.ColocatedHook} name=".ProfileMenu">
      export default {
        mounted() {
          this.menu = this.el.querySelector(".lb-profile-menu")
          this.sync = () => {
            if (this.el.open && !this.menu.matches(":popover-open")) this.menu.showPopover()
            if (!this.el.open && this.menu.matches(":popover-open")) this.menu.hidePopover()
          }
          this.menuToggle = () => {
            if (!this.menu.matches(":popover-open")) this.el.removeAttribute("open")
          }
          this.el.addEventListener("toggle", this.sync)
          this.menu.addEventListener("toggle", this.menuToggle)
          this.close = event => {
            if (this.el.open && (!this.el.contains(event.target) || event.target.closest("a[href]"))) this.el.removeAttribute("open")
          }
          this.escape = event => {
            if (event.key === "Escape" && this.el.open && this.el.contains(event.target)) {
              event.preventDefault(); this.el.removeAttribute("open"); this.el.querySelector("summary").focus()
            }
          }
          window.addEventListener("click", this.close, true)
          window.addEventListener("keydown", this.escape)
        },
        destroyed() { this.el.removeEventListener("toggle", this.sync); this.menu.removeEventListener("toggle", this.menuToggle); window.removeEventListener("click", this.close, true); window.removeEventListener("keydown", this.escape) }
      }
    </script>
    """
  end

  defp page_label("access"), do: "People & teams"
  defp page_label("tools"), do: "Workspace tools"
  defp page_label(page), do: String.capitalize(page)

  defp pages(routes) do
    [
      {"overview", "Overview", "grid"},
      {"gateway", "Gateway", "server"},
      {"observe", "Observe", "pulse"},
      {"discover", "Discover", "compass"},
      {"create", "Create", "plus"},
      {"library", "Library", "library"},
      {"projects", "Projects", "folder"},
      {"access", "People & teams", "shield"},
      {"tasks", "Tasks", "clock"},
      {"agents", "Agents", "bot"},
      {"stash", "Stash", "database"},
      {"runtime", "Runtime", "pulse"},
      {"clients", "Clients", "bot"},
      {"diagnostics", "Diagnostics", "alert"},
      {"usage", "Usage", "table"},
      {"activity", "Activity", "pulse"},
      {"logs", "Logs", "list"}
    ]
    |> Enum.filter(fn {page, _, _} -> Map.has_key?(routes, page) end)
  end

  defp groups(routes) do
    items = pages(routes)

    [
      {"Gateway", ~w(overview gateway observe)},
      {"Build", ~w(discover create library)},
      {"Workspace", ~w(projects access tasks agents stash)},
      {"Operations", ~w(runtime clients diagnostics usage activity logs)}
    ]
    |> Enum.map(fn {label, keys} ->
      {label, Enum.filter(items, fn {key, _, _} -> key in keys end)}
    end)
    |> Enum.reject(fn {_, items} -> items == [] end)
  end

  defp navigation(routes, target), do: %{navigate: Map.fetch!(routes, target)}
end
