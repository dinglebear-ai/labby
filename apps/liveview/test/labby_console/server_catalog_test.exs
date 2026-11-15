defmodule LabbyConsole.ServerCatalogTest do
  use ExUnit.Case, async: false
  alias LabbyConsole.{Auth, ServerCatalog}

  setup do
    state = start_supervised!({Agent, fn -> true end})

    server =
      start_supervised!(
        {Bandit, plug: {LabbyConsole.AuthFixture, state: state}, port: 0, ip: {127, 0, 0, 1}}
      )

    {:ok, {_, port}} = ThousandIsland.listener_info(server)
    old = Application.get_env(:labby_console, :gateway_url)
    Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:#{port}")
    Application.put_env(:labby_console, :fixture_server_catalog, "normal")

    on_exit(fn ->
      Application.put_env(:labby_console, :gateway_url, old)
      Application.delete_env(:labby_console, :fixture_server_catalog)
    end)

    {:ok, id} = Auth.sign_in("fixture-only")
    %{id: id, state: state}
  end

  test "reads retained tools, resource URIs and prompt names without exposing extra fields", %{
    id: id
  } do
    assert {:ok,
            %{
              tools: [%{"name" => "fetch", "exposed" => true} = tool],
              resources: ["file:///README.md"],
              prompts: ["review"]
            }} = ServerCatalog.load(id, "fixture-server")

    refute Map.has_key?(tool, "private")
  end

  test "rejects malformed, invalid and oversized snapshots without partial results", %{id: id} do
    for mode <- ["malformed", "invalid", "oversized"] do
      Application.put_env(:labby_console, :fixture_server_catalog, mode)
      assert {:error, :unavailable} = ServerCatalog.load(id, "fixture-server")
    end
  end

  test "empty snapshots remain empty and inaccessible servers do not leak capabilities", %{id: id} do
    Application.put_env(:labby_console, :fixture_server_catalog, "empty")

    assert {:ok, %{tools: [], resources: [], prompts: []}} =
             ServerCatalog.load(id, "fixture-server")

    assert {:error, :unavailable} = ServerCatalog.load(id, "unknown")
    assert {:error, :unauthenticated} = ServerCatalog.load("another-caller", "fixture-server")
  end

  test "revoked sessions cannot retain a catalog", %{id: id, state: state} do
    Agent.update(state, fn _ -> false end)
    assert {:error, :unauthenticated} = ServerCatalog.load(id, "fixture-server")
  end

  test "accepts empty optional tool descriptions", %{id: id} do
    Application.put_env(:labby_console, :fixture_server_catalog, "empty_description")

    assert {:ok, %{tools: [%{"description" => "", "exposed" => false}]}} =
             ServerCatalog.load(id, "fixture-server")
  end
end
