defmodule LabbyConsoleWeb.ObserveRouteTest do
  use LabbyConsoleWeb.ConnCase, async: true

  test "normal Observe entry point requires authenticated session", %{conn: conn} do
    conn = get(conn, "/observe")
    assert redirected_to(conn) =~ "/sign-in"
  end
end
