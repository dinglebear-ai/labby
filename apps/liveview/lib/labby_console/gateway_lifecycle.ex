defmodule LabbyConsole.GatewayLifecycle do
  @moduledoc """
  Upstream MCP lifecycle requests and bounded result projection.

  Rust owns enablement, process cleanup and reconnection. This module only
  builds the three allowlisted lifecycle requests and decides whether a
  response proves the named upstream reached the requested state. A response
  that cannot be verified for that exact name is never reported as settled.
  """

  # The restart backend waits inline. Keep its wait inside the console's
  # bounded Gateway write budget so the local wait ends before the operation
  # deadline rather than being cut off by it.
  @restart_wait_ms 15_000
  @cleanup_counts ~w(gateway local aggressive)
  @name ~r/\A[A-Za-z0-9_.-]+\z/

  @doc "Builds an allowlisted lifecycle request, or `:error` for an unsupported mode or name."
  def request(mode, name) do
    if valid_name?(name), do: build(mode, name), else: :error
  end

  defp build(:enable, name), do: {:ok, "gateway.mcp.enable", %{"name" => name}}

  defp build({:disable, cleanup}, name) when is_boolean(cleanup),
    do:
      {:ok, "gateway.mcp.disable", %{"name" => name, "cleanup" => cleanup, "aggressive" => false}}

  defp build(:restart, name),
    do:
      {:ok, "gateway.mcp.restart",
       %{"name" => name, "aggressive" => false, "wait_ms" => @restart_wait_ms}}

  defp build(_, _), do: :error

  @doc "Projects a lifecycle response for `name`, or `:error` when it proves nothing."
  def result("gateway.mcp.enable", data, name), do: settled(data, name, true, nil)

  def result("gateway.mcp.disable", %{"gateway" => view} = data, name)
      when is_map_key(data, "cleanup"),
      do: settled(view, name, false, data["cleanup"])

  def result("gateway.mcp.restart", %{"completed" => true} = data, name),
    do: settled(data["gateway"], name, true, data["cleanup"])

  def result("gateway.mcp.restart", %{"completed" => false, "in_flight" => true}, _),
    do: {:ok, %{state: :already_running, enabled: nil, connected: nil, cleanup: nil}}

  def result("gateway.mcp.restart", %{"completed" => false}, _),
    do: {:ok, %{state: :in_flight, enabled: nil, connected: nil, cleanup: nil}}

  def result(_, _, _), do: :error

  defp settled(view, name, expected, cleanup) do
    with %{"config" => %{"name" => ^name, "enabled" => ^expected}, "runtime" => runtime} <- view,
         %{"name" => ^name, "connected" => connected} when is_boolean(connected) <- runtime,
         {:ok, cleanup} <- cleanup(cleanup, name) do
      {:ok, %{state: :settled, enabled: expected, connected: connected, cleanup: cleanup}}
    else
      _ -> :error
    end
  end

  defp cleanup(nil, _), do: {:ok, nil}

  defp cleanup(%{"upstream" => upstream, "dry_run" => dry_run} = report, name)
       when upstream == name and is_boolean(dry_run) do
    keys = Enum.flat_map(@cleanup_counts, &["#{&1}_matched", "#{&1}_killed"])
    counts = Enum.map(keys, &Map.get(report, &1))

    if Enum.all?(counts, &(is_integer(&1) and &1 >= 0)) do
      sum = fn suffix ->
        Enum.reduce(@cleanup_counts, 0, &(&2 + report["#{&1}_#{suffix}"]))
      end

      {:ok, %{matched: sum.("matched"), killed: sum.("killed"), dry_run: dry_run}}
    else
      :error
    end
  end

  defp cleanup(_, _), do: :error

  defp valid_name?(name),
    do:
      is_binary(name) and byte_size(name) in 1..128 and String.valid?(name) and
        Regex.match?(@name, name)
end
