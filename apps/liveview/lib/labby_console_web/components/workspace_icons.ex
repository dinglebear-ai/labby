defmodule LabbyConsoleWeb.WorkspaceIcons do
  @moduledoc "Native Labby presentation glyphs."
  use Phoenix.Component
  attr :name, :string, required: true
  attr :class, :any, default: "size-4"

  def icon(assigns) do
    paths = %{
      "grid" => "M3 3h7v7H3z M14 3h7v7h-7z M3 14h7v7H3z M14 14h7v7h-7z",
      "table" => "M3 3h18v18H3z M3 9h18 M9 9v12",
      "file" =>
        "M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z M14 2v6h6 M8 13h8 M8 17h6",
      "branch" =>
        "M6 3v12 M6 15a3 3 0 1 0 0 6 3 3 0 0 0 0-6 M6 3a3 3 0 1 0 0 6 3 3 0 0 0 0-6 M18 3a3 3 0 1 0 0 6 3 3 0 0 0 0-6 M18 9a9 9 0 0 1-9 9",
      "library" => "M4 3h4v18H4z M8 3h4v18H8z M15 3l5-1 4 18-5 1z",
      "compass" => "M22 12a10 10 0 1 1-20 0a10 10 0 0 1 20 0 M16.2 7.8l-2.8 5.6-5.6 2.8 2.8-5.6z",
      "layers" => "M12 2L2 7l10 5 10-5z M2 12l10 5 10-5 M2 17l10 5 10-5",
      "user" => "M20 21v-2a7 7 0 0 0-14 0v2 M16 7a4 4 0 1 1-8 0a4 4 0 0 1 8 0",
      "chevrons" => "M8 8l4-4 4 4 M8 16l4 4 4-4",
      "bookmark" => "M5 21V3h14v18l-7-4z",
      "package" => "M12 2L2 7v10l10 5 10-5V7z M2 7l10 5 10-5 M12 12v10 M7 4.5l10 5",
      "sparkles" =>
        "M12 3l2.5 6.5L21 12l-6.5 2.5L12 21l-2.5-6.5L3 12l6.5-2.5z M4 2v4 M2 4h4 M20 18v4 M18 20h4",
      "power" => "M12 2v10 M18.4 5.6a9 9 0 1 1-12.8 0",
      "eye" =>
        "M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z M15 12a3 3 0 1 1-6 0a3 3 0 0 1 6 0",
      "eye-off" =>
        "M3 3l18 18 M10.6 10.6a2 2 0 0 0 2.8 2.8 M9.9 5.2A11 11 0 0 1 12 5c6.5 0 10 7 10 7a18 18 0 0 1-3 3.9 M6.5 6.5A19 19 0 0 0 2 12s3.5 7 10 7a11 11 0 0 0 5.5-1.5",
      "server" => "M4 3h16v7H4z M4 14h16v7H4z M7 6.5h.1 M7 17.5h.1 M11 6.5h6 M11 17.5h6",
      "search" => "M21 21l-5-5 M18 10a8 8 0 1 1-16 0a8 8 0 0 1 16 0",
      "code" => "M8 5L1 12l7 7 M16 5l7 7-7 7 M14 3l-4 18",
      "pulse" => "M2 12h4l3-8 5 16 3-8h5",
      "globe" =>
        "M21 12a9 9 0 1 1-18 0a9 9 0 0 1 18 0 M3 12h18 M12 3c5 5 5 13 0 18c-5-5-5-13 0-18",
      "bot" => "M5 7h14v13H5z M12 3v4 M9 12h.1 M15 12h.1 M9 16h6 M2 11v5 M22 11v5",
      "check" => "M5 12l4 4L19 6",
      "check-circle" => "M22 11.1V12a10 10 0 1 1-5.93-9.14 M22 4L12 14.01l-3-3",
      "clock" => "M22 12a10 10 0 1 1-20 0a10 10 0 0 1 20 0 M12 6v6l4 2",
      "alert" =>
        "M10.3 3.9L1.8 18.6a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0 M12 9v4 M12 17h.01",
      "terminal" => "M4 4h16v16H4z M7 8l4 4-4 4 M13 16h4",
      "database" =>
        "M20 6c0 2.2-3.6 4-8 4S4 8.2 4 6s3.6-4 8-4 8 1.8 8 4Z M4 6v12c0 2.2 3.6 4 8 4s8-1.8 8-4V6 M4 12c0 2.2 3.6 4 8 4s8-1.8 8-4",
      "messages" =>
        "M21 11.5a8.4 8.4 0 0 1-.9 3.8 8.5 8.5 0 0 1-7.6 4.7 8.4 8.4 0 0 1-3.8-.9L3 21l1.9-5.7a8.4 8.4 0 0 1-.9-3.8 8.5 8.5 0 0 1 4.7-7.6 8.4 8.4 0 0 1 3.8-.9h.5a8.5 8.5 0 0 1 8 8v.5Z",
      "cpu" =>
        "M6 6h12v12H6z M9 9h6v6H9z M9 2v4 M15 2v4 M9 18v4 M15 18v4 M2 9h4 M2 15h4 M18 9h4 M18 15h4",
      "memory" => "M3 7h18v10H3z M7 10v4 M11 10v4 M15 10v4 M6 17v3 M10 17v3 M14 17v3 M18 17v3",
      "network" => "M9 2h6v6H9z M2 16h6v6H2z M16 16h6v6h-6z M12 8v4 M5 16v-4h14v4",
      "refresh" => "M20 7A9 9 0 0 0 4 8 M4 3v5h5 M4 17a9 9 0 0 0 16-1 M20 21v-5h-5",
      "pause" => "M8 4v16 M16 4v16",
      "unplug" => "M8 3v4 M16 3v4 M5 7h14v4a7 7 0 0 1-7 7v4 M4 4l16 16",
      "info" => "M22 12a10 10 0 1 1-20 0a10 10 0 0 1 20 0 M12 11v6 M12 7h.01",
      "box" => "M12 2L2 7v10l10 5 10-5V7z M2 7l10 5 10-5 M12 12v10 M7 4.5l10 5",
      "folder" => "M3 5h7l2 3h9v12H3z",
      "settings" =>
        "M9 3h6l1 4 4 1 2 5-3 3 0 4-5 2-3-3-4 0-2-5 3-3z M15 12a3 3 0 1 1-6 0a3 3 0 0 1 6 0",
      "chevron" => "M9 5l7 7-7 7",
      "plus" => "M12 4v16 M4 12h16",
      "arrow" => "M4 12h16 M14 6l6 6-6 6",
      "command" => "M9 9V6a3 3 0 1 0-3 3h12a3 3 0 1 0-3-3v12a3 3 0 1 0 3-3H6a3 3 0 1 0 3 3z",
      "close" => "M5 5l14 14 M5 19L19 5",
      "bolt" => "M13 2L4 14h7l-1 8 10-13h-8z",
      "more" => "M5 12h.1 M12 12h.1 M19 12h.1",
      "shield" => "M12 2l9 4v6c0 5-9 10-9 10S3 17 3 12V6z M8 12l3 3 5-6",
      "play" => "M7 4l14 8-14 8z",
      "copy" => "M8 8h13v13H8z M4 16H2V2h14v2",
      "save" =>
        "M19 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h12l4 4v12a2 2 0 0 1-2 2z M7 3v6h10V3 M7 21v-8h10v8",
      "download" => "M12 3v12 M6 9l6 6 6-6 M4 16v5h16v-5",
      "upload" => "M12 16V4 M6 10l6-6 6 6 M4 16v5h16v-5",
      "edit" => "M14 5l5 5 M3 21l5-1L21 7l-6-6L2 14z",
      "trash" => "M3 6h18 M9 6V3h6v3 M5 6l1 15h12l1-15 M10 10v7 M14 10v7",
      "sort" => "M3 16l4 4 4-4 M7 20V4 M13 4h8 M13 8h6 M13 12h4",
      "list" => "M8 6h13 M8 12h13 M8 18h13 M3 6h.01 M3 12h.01 M3 18h.01",
      "filter" => "M3 5h18 M6 12h12 M10 19h4",
      "link" =>
        "M10 13l4-4 M8 15l-2 2a4 4 0 0 1-6-6l5-5a4 4 0 0 1 6 0 M13 8l2-2a4 4 0 0 1 6 6l-5 5a4 4 0 0 1-6 0"
    }

    assigns = assign(assigns, :path, Map.get(paths, assigns.name, paths["grid"]))

    ~H"""
    <svg
      class={["glyph", @class]}
      width="20"
      height="20"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="1.65"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    ><path d={@path} /></svg>
    """
  end
end
