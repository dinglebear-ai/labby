defmodule LabbyConsole.ProjectsFixture do
  @moduledoc false
  use Plug.Router
  plug Plug.Parsers, parsers: [:json], json_decoder: Jason
  plug :match
  plug :dispatch

  post "/auth/bearer-session" do
    conn |> put_resp_header("set-cookie", "caller=projects; HttpOnly") |> send_resp(200, "{}")
  end

  get "/auth/session" do
    reply(conn, %{
      authenticated: true,
      expires_at: System.system_time(:second) + 60,
      csrf_token: "projects-csrf",
      user: %{sub: "projects"},
      is_admin: true,
      capabilities: ["platform.manage"]
    })
  end

  post "/v1/:service" do
    action = conn.body_params["action"]
    params = conn.body_params["params"]
    state = conn.private.projects_state
    send(Agent.get(state, & &1.owner), {:projects_dispatch, action, params})

    if Agent.get(state, &(Map.get(&1, :hold) == action)) do
      send(Agent.get(state, & &1.owner), {:projects_hold, self()})

      receive do
        :release_projects -> :ok
      after
        2000 -> :ok
      end
    end

    {status, data} =
      Agent.get_and_update(state, fn s ->
        case action do
          "access.team.list" ->
            {{200,
              %{
                teams: [
                  %{
                    team_id: "t",
                    name: "Real contract Team",
                    status: "active",
                    role: nil,
                    policy_epoch: 1,
                    membership_epoch: 1,
                    global_revision: 1
                  }
                ]
              }}, s}

          "projects.list" ->
            {{200, Enum.filter(s.rows, &(&1["status"] == "active"))}, s}

          "projects.get" ->
            p =
              Enum.find(
                s.rows,
                &(&1["project_id"] == params["project_id"] && &1["team_id"] == params["team_id"])
              )

            case Map.get(s, :get_failure) do
              nil -> if p, do: {{200, p}, s}, else: {{403, %{kind: "forbidden"}}, s}
              response -> {response, s}
            end

          _ when s.reject ->
            {{403, %{kind: "forbidden"}}, s}

          "projects.create" ->
            p =
              Map.merge(params, %{
                "status" => "active",
                "role" => "admin",
                "policy_epoch" => 1,
                "can_manage" => true
              })

            {{200, p}, %{s | rows: [p | s.rows]}}

          _ ->
            old = Enum.find(s.rows, &(&1["project_id"] == params["project_id"]))

            if old && old["can_manage"] do
              p =
                old
                |> Map.put("policy_epoch", old["policy_epoch"] + 1)
                |> Map.merge(Map.take(params, ["name"]))
                |> Map.put(
                  "status",
                  case action do
                    "projects.archive" -> "disabled"
                    "projects.activate" -> "active"
                    _ -> old["status"]
                  end
                )

              {{200, p},
               %{
                 s
                 | rows:
                     Enum.map(s.rows, fn r ->
                       if r["project_id"] == p["project_id"], do: p, else: r
                     end)
               }}
            else
              {{403, %{kind: "forbidden"}}, s}
            end
        end
      end)

    if action in ~w(projects.create projects.update projects.archive projects.activate) &&
         status == 200 do
      Agent.update(state, fn s ->
        case Map.get(s, :after_write_get_failure) do
          nil -> s
          failure -> Map.put(s, :get_failure, failure)
        end
      end)
    end

    if action == "projects.create" && Agent.get(state, &Map.get(&1, :malformed, false)),
      do: send_resp(conn, 200, "{"),
      else: send_resp(conn, status, Jason.encode!(data))
  end

  def init(opts), do: opts
  def call(conn, opts), do: super(put_private(conn, :projects_state, opts[:state]), opts)
  defp reply(conn, data), do: send_resp(conn, 200, Jason.encode!(data))
end
