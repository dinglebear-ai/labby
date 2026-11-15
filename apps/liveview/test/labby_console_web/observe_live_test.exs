defmodule LabbyConsoleWeb.ObserveLiveTest do
  use ExUnit.Case, async: true
  import Phoenix.LiveViewTest
  alias LabbyConsoleWeb.ObserveLive

  defp socket do
    socket = %Phoenix.LiveView.Socket{
      endpoint: LabbyConsoleWeb.Endpoint,
      assigns: %{
        __changed__: %{},
        flash: %{},
        subject: "test",
        preference_key: String.duplicate("a", 64),
        session_id: "private",
        session_generation: 1
      }
    }

    {:ok, socket} = ObserveLive.mount(%{}, %{}, socket)
    socket
  end

  test "disconnected mount renders honest native empty states without a request" do
    socket = socket()
    refute socket.assigns.loading
    refute socket.assigns.following
    html = render_component(&LabbyConsoleWeb.ObserveComponents.page/1, state: socket.assigns)
    assert html =~ "Observe"
    assert html =~ "Activity"
    assert html =~ "Loaded snapshot"
    refute html =~ "Release readiness"
  end

  test "untrusted inspection cannot select a record outside loaded scope" do
    {:noreply, state} = ObserveLive.handle_event("inspect", %{"id" => "42"}, socket())
    assert state.assigns.selected == nil
  end

  test "stale load and timeout completions are fenced" do
    initial = socket()

    {:noreply, state} =
      ObserveLive.handle_async(
        {:observe, make_ref(), 1},
        {:ok, {:ok, %{calls: [%{"id" => 1}], next_cursor: nil}}},
        initial
      )

    assert state.assigns.rows == []
    {:noreply, state} = ObserveLive.handle_info({:observe_timeout, make_ref()}, state)
    assert state.assigns.rows == []
  end

  test "pause cancels follow and clears observed pending without inventing counts" do
    {:noreply, state} = ObserveLive.handle_event("pause", %{}, socket())
    refute state.assigns.following
    assert state.assigns.timer == nil
    assert state.assigns.rows == []
  end

  test "forbidden clears retained rows and selection without redirecting valid login" do
    socket = Phoenix.Component.assign(socket(), rows: [%{"id" => 1}], selected: "1")
    state = ObserveLive.apply_result(socket, {:error, :forbidden}, false)
    assert state.assigns.rows == []
    assert state.assigns.selected == nil
    assert state.assigns.error == :forbidden
    assert state.redirected == nil
  end

  test "transient refresh failure retains rows as unknown and stops after three failures" do
    socket =
      Phoenix.Component.assign(socket(),
        rows: [%{"id" => 1}],
        selected: "1",
        following: true,
        failures: 2
      )

    state = ObserveLive.apply_result(socket, {:error, :unavailable}, false)
    assert state.assigns.rows == [%{"id" => 1}]
    assert state.assigns.selected == "1"
    assert state.assigns.error == :unavailable
    refute state.assigns.following
  end

  test "refresh snapshot replaces rows without duplicate append and invalidates missing selection" do
    socket = Phoenix.Component.assign(socket(), rows: [%{"id" => 1}], selected: "1")

    state =
      ObserveLive.apply_result(
        socket,
        {:ok, %{calls: [%{"id" => 2}], next_cursor: "opaque"}},
        false
      )

    assert state.assigns.rows == [%{"id" => 2}]
    assert state.assigns.selected == nil
    assert state.assigns.cursor == "opaque"
  end

  test "pagination rejects a repeated cursor and bounds retained state" do
    socket =
      Phoenix.Component.assign(socket(), rows: [%{"id" => 1}], cursors: ["same"], cursor: "same")

    state =
      ObserveLive.apply_result(socket, {:ok, %{calls: [%{"id" => 2}], next_cursor: "same"}}, true)

    assert state.assigns.cursor == nil
    assert state.assigns.error == :cursor_repeated
  end

  test "first next page cannot repeat its request cursor" do
    socket = Phoenix.Component.assign(socket(), cursor: "same", cursors: [])

    state =
      ObserveLive.apply_result(socket, {:ok, %{calls: [%{"id" => 2}], next_cursor: "same"}}, true)

    assert state.assigns.cursor == nil
    assert state.assigns.error == :cursor_repeated
  end

  test "log selection survives insertion ahead of the actual selected retained record" do
    socket = socket()
    first = %{"timestamp" => "2026-11-09T23:59:59Z", "message" => "first"}
    second = %{"timestamp" => "2026-11-09T23:59:58Z", "message" => "second"}

    state =
      ObserveLive.apply_result(
        socket,
        {:ok, %{entries: [first, second], truncated: false}},
        false
      )

    selected = List.last(state.assigns.rows)["id"]
    state = Phoenix.Component.assign(state, selected: selected)

    state =
      ObserveLive.apply_result(
        state,
        {:ok, %{entries: [%{"message" => "new"}, first, second], truncated: false}},
        false
      )

    assert state.assigns.selected == selected
    assert Enum.find(state.assigns.rows, &(&1["id"] == selected))["message"] == "second"
  end

  test "oversized filter events do not retain unbounded UI values" do
    {:noreply, state} =
      ObserveLive.handle_event(
        "filter",
        %{"filters" => %{"query" => String.duplicate("x", 3000)}},
        socket()
      )

    assert state.assigns.filters == %{}
    assert state.assigns.error == :invalid_filters
  end
end
