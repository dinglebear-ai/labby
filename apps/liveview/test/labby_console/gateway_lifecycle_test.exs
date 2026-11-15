defmodule LabbyConsole.GatewayLifecycleTest do
  use ExUnit.Case, async: true
  alias LabbyConsole.GatewayLifecycle

  defp view(name, enabled, connected \\ false),
    do: %{
      "config" => %{"name" => name, "enabled" => enabled},
      "runtime" => %{"name" => name, "connected" => connected}
    }

  describe "request building" do
    test "enable requests only the named upstream" do
      assert {:ok, "gateway.mcp.enable", %{"name" => "alpha"}} =
               GatewayLifecycle.request(:enable, "alpha")
    end

    test "disable carries explicit cleanup and non-aggressive matching" do
      assert {:ok, "gateway.mcp.disable",
              %{"name" => "alpha", "cleanup" => false, "aggressive" => false}} =
               GatewayLifecycle.request({:disable, false}, "alpha")

      assert {:ok, "gateway.mcp.disable",
              %{"name" => "alpha", "cleanup" => true, "aggressive" => false}} =
               GatewayLifecycle.request({:disable, true}, "alpha")
    end

    test "restart bounds its backend wait inside the console operation budget" do
      assert {:ok, "gateway.mcp.restart", params} = GatewayLifecycle.request(:restart, "alpha")
      assert params["name"] == "alpha"
      assert params["aggressive"] == false
      assert is_integer(params["wait_ms"])
      assert params["wait_ms"] > 0 and params["wait_ms"] <= 20_000
    end

    test "no request is built for an unsupported mode or invalid upstream name" do
      for name <- ["", "bad name", String.duplicate("a", 129), "a/b", nil, 1] do
        assert :error = GatewayLifecycle.request(:enable, name)
        assert :error = GatewayLifecycle.request(:restart, name)
        assert :error = GatewayLifecycle.request({:disable, false}, name)
      end

      assert :error = GatewayLifecycle.request(:reload, "alpha")
      assert :error = GatewayLifecycle.request({:disable, "yes"}, "alpha")
    end
  end

  describe "enable result" do
    test "settles only when the named upstream reads back enabled" do
      assert {:ok, %{state: :settled, enabled: true, connected: true, cleanup: nil}} =
               GatewayLifecycle.result("gateway.mcp.enable", view("alpha", true, true), "alpha")
    end

    test "a wrong name, wrong enablement or malformed view is not a settled result" do
      for data <- [
            view("other", true),
            view("alpha", false),
            %{"config" => %{"name" => "alpha", "enabled" => true}},
            put_in(view("alpha", true), ["runtime", "connected"], "yes"),
            put_in(view("alpha", true), ["config", "enabled"], "true"),
            %{},
            "alpha",
            nil
          ] do
        assert :error = GatewayLifecycle.result("gateway.mcp.enable", data, "alpha")
      end
    end
  end

  describe "disable result" do
    test "settles with a disabled read-back and an absent cleanup report" do
      assert {:ok, %{state: :settled, enabled: false, connected: false, cleanup: nil}} =
               GatewayLifecycle.result(
                 "gateway.mcp.disable",
                 %{"gateway" => view("alpha", false), "cleanup" => nil},
                 "alpha"
               )
    end

    test "a cleanup report is projected with its matched and killed counts" do
      cleanup = %{
        "upstream" => "alpha",
        "aggressive" => false,
        "dry_run" => false,
        "gateway_matched" => 2,
        "local_matched" => 1,
        "aggressive_matched" => 0,
        "gateway_killed" => 2,
        "local_killed" => 0,
        "aggressive_killed" => 0
      }

      assert {:ok, %{state: :settled, enabled: false, cleanup: projected}} =
               GatewayLifecycle.result(
                 "gateway.mcp.disable",
                 %{"gateway" => view("alpha", false), "cleanup" => cleanup},
                 "alpha"
               )

      assert projected == %{matched: 3, killed: 2, dry_run: false}
    end

    test "an unwrapped view, foreign cleanup upstream or enabled read-back is rejected" do
      for data <- [
            view("alpha", false),
            %{"gateway" => view("alpha", true), "cleanup" => nil},
            %{
              "gateway" => view("alpha", false),
              "cleanup" => %{"upstream" => "other", "gateway_matched" => 0}
            },
            %{"gateway" => view("alpha", false), "cleanup" => %{"upstream" => "alpha"}},
            %{"cleanup" => nil}
          ] do
        assert :error = GatewayLifecycle.result("gateway.mcp.disable", data, "alpha")
      end
    end
  end

  describe "restart result" do
    test "a completed restart settles with the reconnected runtime" do
      assert {:ok, %{state: :settled, enabled: true, connected: true, cleanup: nil}} =
               GatewayLifecycle.result(
                 "gateway.mcp.restart",
                 %{"completed" => true, "gateway" => view("alpha", true, true), "cleanup" => nil},
                 "alpha"
               )
    end

    test "an unfinished restart is reported as continuing backend work, never as finished" do
      assert {:ok, %{state: :in_flight}} =
               GatewayLifecycle.result("gateway.mcp.restart", %{"completed" => false}, "alpha")

      assert {:ok, %{state: :already_running}} =
               GatewayLifecycle.result(
                 "gateway.mcp.restart",
                 %{"completed" => false, "in_flight" => true},
                 "alpha"
               )
    end

    test "a completed restart without a verifiable gateway view is rejected" do
      for data <- [
            %{"completed" => true},
            %{"completed" => true, "gateway" => view("other", true)},
            %{"completed" => true, "gateway" => view("alpha", false)},
            %{"gateway" => view("alpha", true)},
            %{"completed" => "true", "gateway" => view("alpha", true)}
          ] do
        assert :error = GatewayLifecycle.result("gateway.mcp.restart", data, "alpha")
      end
    end
  end

  test "an action outside the lifecycle family has no projection" do
    assert :error =
             GatewayLifecycle.result("gateway.reload", view("alpha", true), "alpha")

    assert :error =
             GatewayLifecycle.result("gateway.mcp.cleanup", view("alpha", true), "alpha")
  end
end
