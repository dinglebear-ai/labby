defmodule LabbyConsole.GatewayFixture do
  use Plug.Router
  plug :match
  plug :dispatch

  get "/ready" do
    {status, body} = Agent.get(conn.private.fixture, & &1)

    conn =
      if status == 302, do: Plug.Conn.put_resp_header(conn, "location", "/unexpected"), else: conn

    if body == "timeout",
      do:
        (receive do
           :release -> :ok
         after
           3_000 -> :ok
         end)

    conn |> Plug.Conn.put_resp_content_type("application/json") |> send_resp(status, body)
  end

  get "/health" do
    conn
    |> Plug.Conn.put_resp_content_type("application/json")
    |> send_resp(200, ~s({"status":"ok"}))
  end

  match _ do
    send_resp(conn, 404, "")
  end

  def init(opts), do: opts
  def call(conn, opts), do: super(Plug.Conn.put_private(conn, :fixture, opts[:state]), opts)
end
