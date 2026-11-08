defmodule LabbyConsoleWeb.PageControllerTest do
  use LabbyConsoleWeb.ConnCase

  test "GET /", %{conn: conn} do
    conn = get(conn, ~p"/")
    html = html_response(conn, 200)
    assert html =~ "Labby Console"
    assert html =~ "Configure a gateway target"
    refute html =~ "Connected"
  end
end
