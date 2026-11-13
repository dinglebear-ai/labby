defmodule LabbyConsole.GatewayCrudFixture do
  @moduledoc false
  use Plug.Router
  plug :match
  plug :dispatch

  post "/v1/gateway" do
    {:ok, body, conn} = read_body(conn, length: 32_768)
    {owner, mode} = Agent.get(conn.private.fixture, &{&1.owner, &1.response})
    send(owner, {:gateway_wire, self(), conn.request_path, conn.req_headers, body})

    case mode do
      :trickle -> trickle(send_chunked(conn, 200), 90)
      {status, response} -> send_resp(conn, status, response)
    end
  end

  match _ do
    LabbyConsole.ActionFixture.call(conn, state: conn.private.fixture)
  end

  defp trickle(conn, 0), do: conn

  defp trickle(conn, remaining) do
    receive do
      :stop -> conn
    after
      400 ->
        case chunk(conn, " ") do
          {:ok, conn} -> trickle(conn, remaining - 1)
          {:error, _} -> conn
        end
    end
  end

  def init(opts), do: opts
  def call(conn, opts), do: super(put_private(conn, :fixture, opts[:state]), opts)
end
