defmodule LabbyConsole.E2E.ConsoleHttpGuard do
  @moduledoc false
  use GenServer
  import Bitwise

  def start_from_env! do
    start!(System.fetch_env!("LABBY_E2E_CONSOLE_HTTP_RECEIPT"))
  rescue
    _ -> halt(:failed)
  catch
    _, _ -> halt(:failed)
  end

  def start!(path, cap \\ 2048) do
    true = Path.type(path) == :absolute
    true = Path.basename(path) == "console-http-counts.json"
    {:ok, %{type: :directory, mode: mode}} = File.lstat(Path.dirname(path))
    true = (mode &&& 0o077) == 0
    true = is_integer(cap) and cap > 0 and cap <= 2048
    {:error, :enoent} = File.lstat(path)
    {:ok, _} = Application.ensure_all_started(:telemetry)
    {:ok, _} = GenServer.start(__MODULE__, {path, cap}, name: __MODULE__)
    :ok = :telemetry.attach(__MODULE__, [:finch, :request, :start], &__MODULE__.admit/4, nil)
    :ok
  rescue
    _ -> halt(:failed)
  catch
    _, _ -> halt(:failed)
  end

  def admit(_, _, metadata, _) do
    # Only the fixed label crosses the process boundary. Request data stays here.
    label = family(metadata.request.method, metadata.request.path)
    :ok = GenServer.call(__MODULE__, label, 5000)
  rescue
    _ -> halt(:failed)
  catch
    _, _ -> halt(:failed)
  end

  @impl true
  def init({path, cap}) do
    counts = %{
      version: 1,
      cap: cap,
      total: 0,
      gatewayPost: 0,
      authPreflight: 0,
      otherServices: 0,
      other: 0
    }

    File.write!(path, Jason.encode!(counts), [:exclusive])
    File.chmod!(path, 0o600)
    {:ok, {path, counts}}
  rescue
    _ -> halt(:failed)
  catch
    _, _ -> halt(:failed)
  end

  @impl true
  def handle_call(label, _, {path, counts}) do
    if counts.total >= counts.cap, do: halt(:exhausted)
    true = label in [:gatewayPost, :authPreflight, :otherServices, :other]
    {:ok, %{type: :regular, size: size, mode: mode}} = File.lstat(path)
    true = size <= 4096 and (mode &&& 0o077) == 0
    counts = counts |> Map.update!(:total, &(&1 + 1)) |> Map.update!(label, &(&1 + 1))
    temporary = path <> ".next"
    File.write!(temporary, Jason.encode!(counts), [:exclusive])
    File.chmod!(temporary, 0o600)
    File.rename!(temporary, path)
    {:reply, :ok, {path, counts}}
  rescue
    _ -> halt(:failed)
  catch
    _, _ -> halt(:failed)
  end

  defp family("POST", "/v1/gateway"), do: :gatewayPost
  defp family("GET", "/auth/session"), do: :authPreflight
  defp family(method, "/v1/" <> _) when is_binary(method), do: :otherServices
  defp family(method, path) when is_binary(method) and is_binary(path), do: :other

  defp halt(kind) do
    {code, message} =
      case kind do
        :exhausted -> {86, "E2E_CONSOLE_HTTP_BUDGET_EXHAUSTED"}
        :failed -> {87, "E2E_CONSOLE_HTTP_GUARD_FAILED"}
      end

    try do
      IO.puts(:stderr, message)
    rescue
      _ -> :ok
    catch
      _, _ -> :ok
    end

    :erlang.halt(code)
  end
end
