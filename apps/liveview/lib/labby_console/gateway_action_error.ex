defmodule LabbyConsole.GatewayAction.Error do
  @moduledoc "Bounded action failures. Raw gateway messages and response bodies are never retained."
  @enforce_keys [:kind, :service, :action]
  defstruct [
    :kind,
    :status,
    :service,
    :action,
    :request_id,
    side_effects: "none_expected",
    recovery: %{},
    metadata: %{}
  ]

  @type t :: %__MODULE__{
          kind: atom(),
          status: non_neg_integer() | nil,
          service: String.t() | nil,
          action: String.t() | nil,
          request_id: reference() | nil,
          side_effects: String.t(),
          recovery: map(),
          metadata: map()
        }

  @kinds ~w(auth_failed forbidden oauth_needs_reauth oauth_scope_upgrade_required invalid_param missing_param validation_failed not_found conflict authority_changed rate_limited queue_saturated server_busy service_unavailable upstream_error timeout cancelled unknown_action access_setup_required)
  @kind_map Map.new(@kinds, &{&1, String.to_atom(&1)})
  @origins ~w(validation policy tool_execution upstream_transport discovery runtime budget bridge code_mode)
  @recovery_actions ~w(revise_and_retry retry_later reauthenticate rediscover do_not_retry confirm reduce_work start_dependency inspect_and_escalate)
  @same_arguments ~w(safe conditional discouraged never)

  def new(kind, service, action, opts \\ []) do
    risk = Keyword.get(opts, :side_effects, "none_expected")

    recovery =
      if risk in ["possible", "unknown"],
        do: %{"action" => "do_not_retry", "same_arguments" => "discouraged"},
        else: %{"action" => "revise_and_retry", "same_arguments" => "conditional"}

    struct!(
      __MODULE__,
      Keyword.merge(
        [
          kind: kind,
          status: if(kind == :unauthenticated, do: 401, else: nil),
          service: service,
          action: action,
          side_effects: risk,
          recovery: recovery
        ],
        opts
      )
    )
  end

  def from_response(status, data, service, action, mutation?) do
    valid? = envelope?(data, service, action)
    fallback = if status == 403, do: :forbidden, else: :gateway_error

    kind =
      if valid? and status != 403, do: Map.get(@kind_map, data["kind"], fallback), else: fallback

    risk =
      cond do
        valid? and data["side_effects"] in ~w(none_expected possible unknown) ->
          data["side_effects"]

        status in [401, 403] ->
          "none_expected"

        mutation? ->
          "unknown"

        true ->
          "none_expected"
      end

    error = new(kind, service, action, status: status, side_effects: risk)

    if valid?,
      do: %{
        error
        | recovery: recovery(data["recovery"], error.recovery) |> conservative_retry(risk),
          metadata: metadata(data)
      },
      else: error
  end

  def envelope?(data, service, action) when is_map(data),
    do: data["contract_version"] == 1 and data["service"] == service and data["action"] == action

  def envelope?(_, _, _), do: false

  defp conservative_retry(recovery, risk) when risk in ["possible", "unknown"] do
    if recovery["same_arguments"] == "safe",
      do: Map.merge(recovery, %{"action" => "do_not_retry", "same_arguments" => "discouraged"}),
      else: recovery
  end

  defp conservative_retry(recovery, _), do: recovery

  defp recovery(data, fallback) when is_map(data) do
    fallback
    |> maybe_put("action", data["action"], &(&1 in @recovery_actions))
    |> maybe_put("same_arguments", data["same_arguments"], &(&1 in @same_arguments))
    |> maybe_put(
      "retry_after_ms",
      data["retry_after_ms"],
      &(is_integer(&1) and &1 >= 0 and &1 <= 86_400_000)
    )
  end

  defp recovery(_, fallback), do: fallback

  defp metadata(data) do
    %{"contract_version" => 1}
    |> maybe_put("origin", data["origin"], &(&1 in @origins))
    |> maybe_put("param", data["param"], &identifier?/1)
    |> maybe_put("required_scopes", data["required_scopes"], fn value ->
      is_list(value) and length(value) <= 8 and
        Enum.all?(value, &(&1 in ["lab:read", "lab", "lab:admin"]))
    end)
  end

  defp identifier?(value),
    do:
      is_binary(value) and byte_size(value) <= 64 and Regex.match?(~r/\A[a-zA-Z0-9_.-]+\z/, value)

  defp maybe_put(map, key, value, check),
    do: if(check.(value), do: Map.put(map, key, value), else: map)
end
