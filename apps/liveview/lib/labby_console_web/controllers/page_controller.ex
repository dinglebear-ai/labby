defmodule LabbyConsoleWeb.PageController do
  use LabbyConsoleWeb, :controller

  def home(conn, _params) do
    render(conn, :home)
  end
end
