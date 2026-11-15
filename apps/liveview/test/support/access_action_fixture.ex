defmodule LabbyConsole.AccessActionFixture do
  @moduledoc false
  use Plug.Router
  plug Plug.Parsers, parsers: [:json], json_decoder: Jason, length: 100_000
  plug :match
  plug :dispatch

  post "/auth/bearer-session" do
    conn |> put_resp_header("set-cookie", "caller=access; HttpOnly") |> send_resp(200, "{}")
  end

  get "/auth/session" do
    %{owner: owner, preflight: preflight} = Agent.get(conn.private.fixture, & &1)

    body =
      Jason.encode!(%{
        authenticated: true,
        expires_at: System.system_time(:second) + 120,
        user: %{sub: "access"},
        csrf_token: "csrf-access"
      })

    if preflight == :hold do
      send(owner, {:access_preflight, self()})
      preflight(send_chunked(conn, 200), body, 60)
    else
      send_resp(conn, 200, body)
    end
  end

  post("/auth/logout", do: send_resp(conn, 204, ""))

  post("/v1/access/admin", do: action(conn))
  post("/v1/:service", do: action(conn))

  defp action(conn) do
    %{owner: owner, response: response} = Agent.get(conn.private.fixture, & &1)

    send(
      owner,
      {:access_dispatch, self(), conn.request_path, conn.body_params,
       get_req_header(conn, "cookie"), get_req_header(conn, "x-csrf-token"),
       get_req_header(conn, "x-labby-access-read-budget-ms"), System.monotonic_time(:millisecond)}
    )

    {status, body} =
      if response == :hold do
        receive do
          {:respond, status, body} -> {status, body}
        after
          6_000 -> {503, "{}"}
        end
      else
        response
      end

    send_resp(conn, status, body)
  end

  defp preflight(conn, body, 0), do: elem(chunk(conn, body), 1)

  defp preflight(conn, body, remaining) do
    receive do
      :release -> elem(chunk(conn, body), 1)
    after
      100 ->
        case chunk(conn, " ") do
          {:ok, conn} -> preflight(conn, body, remaining - 1)
          {:error, _} -> conn
        end
    end
  end

  def init(opts), do: opts
  def call(conn, opts), do: super(put_private(conn, :fixture, opts[:state]), opts)
end
