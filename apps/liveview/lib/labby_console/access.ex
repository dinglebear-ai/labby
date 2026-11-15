defmodule LabbyConsole.Access do
  @moduledoc "Finite native Access forms, read projections and nonsecret reconciliation."
  alias LabbyConsole.GatewayAction.Error

  @specs [
    {"access.team.create", "Create Team", ~w(team_id name), :platform},
    {"access.team.member.add", "Add member", ~w(team_id principal_id role), :manager},
    {"access.team.member.role.set", "Change member role", ~w(team_id principal_id role),
     :manager},
    {"access.team.member.suspend", "Suspend member", ~w(team_id principal_id), :manager},
    {"access.team.member.remove", "Remove member", ~w(team_id principal_id), :manager},
    {"access.team.member.activate", "Restore member", ~w(team_id principal_id role), :manager},
    {"access.team.suspend", "Suspend Team", ~w(team_id), :manager},
    {"access.team.activate", "Activate Team", ~w(team_id), :manager},
    {"access.team_invitation.create", "Invite teammate",
     ~w(team_id principal_id role token ttl_seconds), :manager},
    {"access.team_invitation.accept", "Accept invitation", ~w(token), :caller},
    {"access.team_project.assign", "Assign or change Project role", ~w(team_id project_id role),
     :manager},
    {"access.team_project.unassign", "Remove Project assignment", ~w(team_id project_id),
     :manager},
    {"access.platform_admin.grant", "Grant platform administrator", ~w(principal_id), :platform},
    {"access.platform_admin.revoke", "Revoke platform administrator", ~w(principal_id),
     :platform},
    {"access.gateway_credential.bind", "Bind or replace credential reference",
     ~w(team_id upstream_name binding_id), :manager},
    {"access.gateway_credential.revoke", "Revoke credential reference", ~w(team_id upstream_name),
     :manager}
  ]
  @reads %{
    teams: {"access.team.list", "teams"},
    effective: {"access.project.effective.list", "projects"},
    credentials: {"access.gateway_credential.list", "bindings"},
    members: {"access.team.member.list", "members"},
    assignments: {"access.team_project.list", "assignments"},
    invitations: {"access.team_invitation.list", "invitations"},
    admins: {"access.platform_admin.list", "administrators"}
  }
  @paged [:members, :assignments, :invitations, :admins]
  @scoped [:members, :assignments, :invitations, :credentials]
  @states ~w(active suspended revoked)
  @team_roles ~w(owner admin member)
  @project_roles ~w(owner admin member viewer)

  def specs,
    do:
      Enum.map(@specs, fn {a, label, fields, authority} ->
        %{action: a, label: label, fields: fields, authority: authority}
      end)

  def spec(action), do: Enum.find(specs(), &(&1.action == action))
  def read_action(card), do: elem(Map.fetch!(@reads, card), 0)
  def paged?(card), do: card in @paged
  def scoped?(card), do: card in @scoped
  def scoped_cards, do: @scoped
  def roles("access.team_project.assign"), do: @project_roles
  def roles(_), do: @team_roles

  def new do
    %{
      team: nil,
      capabilities: nil,
      help_error: nil,
      dialog: nil,
      draft: %{},
      errors: %{},
      receipt: nil,
      cards: Map.new(@reads, fn {key, _} -> {key, empty_card()} end)
    }
  end

  defp empty_card,
    do: %{
      status: :idle,
      rows: [],
      metadata: %{},
      revision: nil,
      next_offset: nil,
      offset: 0,
      limit: 50,
      error: nil
    }

  def writable?(state), do: is_struct(state.capabilities, MapSet) && !held?(state)
  def held?(state), do: state.receipt && state.receipt.status in [:pending, :accepted, :uncertain]
  def selected(state), do: Enum.find(state.cards.teams.rows, &(&1["team_id"] == state.team))

  def platform?(scope),
    do:
      is_map(scope) && scope["is_admin"] == true && is_list(scope["capabilities"]) &&
        "platform.manage" in scope["capabilities"]

  def availability(state, action, scope) do
    spec = spec(action)
    team = selected(state)
    platform = platform?(scope) && state.cards.admins.status != :forbidden

    cond do
      !spec -> :unavailable
      !is_struct(state.capabilities, MapSet) -> :unknown
      !MapSet.member?(state.capabilities, action) -> :unavailable
      held?(state) -> :held
      spec.authority == :caller -> :available
      platform -> :available
      spec.authority == :platform -> :forbidden
      !team || state.cards.teams.status not in [:loaded, :partial] -> :forbidden
      team["status"] != "active" -> :forbidden
      spec.authority == :manager && team["role"] in ~w(owner admin) -> :available
      true -> :forbidden
    end
  end

  def decode_help(%{"service" => "access", "actions" => actions})
      when is_list(actions) and length(actions) <= 64 do
    known =
      MapSet.new(Enum.map(@specs, &elem(&1, 0)) ++ Enum.map(@reads, fn {_, {a, _}} -> a end))

    if Enum.all?(actions, &(is_map(&1) && text?(&1["name"], 128))) do
      {:ok, MapSet.intersection(known, MapSet.new(Enum.map(actions, & &1["name"])))}
    else
      invalid("help")
    end
  end

  def decode_help(_), do: invalid("help")

  def prepare(action, params) when is_map(params) do
    with %{fields: fields} <- spec(action),
         true <- Map.keys(params) -- fields == [],
         true <- safe_size?(params),
         {:ok, clean} <- validate_fields(action, fields, params) do
      intent = %{
        action: action,
        params: Map.delete(clean, "token"),
        affected: affected(action),
        observed: MapSet.new()
      }

      {:ok, clean, intent}
    else
      _ -> {:error, %{"form" => "Check the required fields and their limits."}}
    end
  end

  def prepare(_, _), do: {:error, %{"form" => "Invalid Access form."}}

  defp validate_fields(action, fields, params) do
    Enum.reduce_while(fields, {:ok, %{}}, fn field, {:ok, acc} ->
      value = params[field]

      result =
        case field do
          "token" ->
            if is_binary(value) && Regex.match?(~r/\A[0-9a-fA-F]{64}\z/, value) &&
                 value != String.duplicate("0", 64), do: {:ok, value}, else: :error

          "ttl_seconds" ->
            ttl(value)

          "role" ->
            if value in roles(action), do: {:ok, value}, else: :error

          _ ->
            if text?(value, if(field in ~w(binding_id upstream_name), do: 256, else: 128)),
              do: {:ok, value},
              else: :error
        end

      case result do
        {:ok, valid} -> {:cont, {:ok, Map.put(acc, field, valid)}}
        _ -> {:halt, {:error, field}}
      end
    end)
  end

  defp ttl(value) when is_binary(value) do
    case Integer.parse(value) do
      {n, ""} -> ttl(n)
      _ -> :error
    end
  end

  defp ttl(n) when is_integer(n) and n in 1..604_800, do: {:ok, n}
  defp ttl(_), do: :error

  defp safe_size?(params) do
    case Jason.encode(params) do
      {:ok, json} -> byte_size(json) <= 32_768
      _ -> false
    end
  end

  def select(state, team) do
    if Enum.any?(state.cards.teams.rows, &(&1["team_id"] == team)) && !held?(state) do
      change_scope(state, team)
    else
      state
    end
  end

  def change_scope(state, team) do
    %{
      close(state)
      | team: team,
        cards: Enum.reduce(@scoped, state.cards, &Map.put(&2, &1, empty_card()))
    }
  end

  def lose_scope(state) do
    receipt = state.receipt

    receipt =
      if receipt && receipt.params["team_id"] == state.team &&
           receipt.status in [:accepted, :uncertain, :reconciled] do
        uncertain_invitation =
          receipt.status == :uncertain &&
            String.starts_with?(receipt.action, "access.team_invitation.")

        %{
          receipt
          | status: if(uncertain_invitation, do: :uncertain, else: :visibility_lost),
            message:
              "The affected Team is no longer visible. Its lifecycle state cannot be inspected."
        }
      else
        receipt
      end

    %{
      change_scope(state, nil)
      | receipt: receipt,
        errors: %{"page" => "The selected Team is no longer visible. Choose an available Team."}
    }
  end

  def capture_intent(state, %{action: "access.gateway_credential.bind"} = intent) do
    card = state.cards.credentials

    row =
      Enum.find(
        card.rows,
        &(&1["team_id"] == intent.params["team_id"] &&
            &1["upstream_name"] == intent.params["upstream_name"])
      )

    before = if card.status == :loaded, do: if(row, do: row["generation"], else: 0), else: nil
    Map.put(intent, :binding_generation_before, before)
  end

  def capture_intent(_, intent), do: intent

  def open(state, action, scope, values \\ %{}) do
    if availability(state, action, scope) == :available do
      fields = spec(action).fields -- ["token"]

      draft =
        Map.merge(
          %{"team_id" => state.team || "", "role" => "member", "ttl_seconds" => "86400"},
          Map.take(values, fields)
        )
        |> Map.take(fields)

      %{state | dialog: action, draft: draft, errors: %{}}
    else
      %{
        state
        | errors: %{
            "page" => "This action is unavailable for the current Team, account, or gateway."
          }
      }
    end
  end

  def close(state), do: %{state | dialog: nil, draft: %{}, errors: %{}}

  def params(state, card, mode \\ :refresh) do
    old = state.cards[card]
    scope = if scoped?(card), do: %{"team_id" => state.team}, else: %{}

    if paged?(card) do
      page =
        if mode == :next,
          do: %{"offset" => old.next_offset, "snapshot_revision" => old.revision},
          else: %{"offset" => 0}

      Map.merge(scope, Map.put(page, "limit", old.limit))
    else
      scope
    end
  end

  def loading(state, card),
    do: put_in(state.cards[card], %{state.cards[card] | status: :loading, error: nil})

  def unavailable(state, card),
    do: put_in(state.cards[card], %{empty_card() | status: :unavailable})

  def decode(card, data, params) when is_map(data) do
    {action, root} = Map.fetch!(@reads, card)
    rows = data[root]

    with true <-
           is_list(rows) &&
             length(rows) <= if(card in @paged, do: params["limit"] || 50, else: 500),
         true <- !scoped?(card) || card == :credentials || data["team_id"] == params["team_id"],
         true <- Enum.all?(rows, &valid_row?(card, &1, params)),
         true <- valid_page?(card, data, params),
         true <- valid_metadata?(card, data) do
      keys = row_keys(card)

      {:ok,
       %{
         rows: Enum.map(rows, &Map.take(&1, keys)),
         revision: data["global_revision"],
         next_offset: data["next_offset"],
         metadata:
           Map.take(
             data,
             ~w(team_id team_status team_policy_epoch team_membership_epoch observed_at global_revision)
           )
       }}
    else
      _ -> invalid(action)
    end
  end

  def decode(card, _, _), do: invalid(read_action(card))

  defp valid_metadata?(card, data) when card in [:members, :assignments],
    do:
      data["team_status"] in ~w(active suspended) &&
        (card != :members ||
           (epoch?(data["team_policy_epoch"]) && epoch?(data["team_membership_epoch"])))

  defp valid_metadata?(:invitations, data),
    do: epoch?(data["team_membership_epoch"]) && epoch?(data["observed_at"])

  defp valid_metadata?(_, _), do: true

  defp valid_page?(card, data, params) when card in @paged do
    revision = data["global_revision"]
    next = data["next_offset"]
    offset = params["offset"] || 0

    epoch?(revision) && (!params["snapshot_revision"] || params["snapshot_revision"] == revision) &&
      (is_nil(next) ||
         (is_integer(next) && next > offset &&
            next == offset + length(data[elem(@reads[card], 1)])))
  end

  defp valid_page?(_, _, _), do: true

  defp valid_row?(:teams, row, _) when is_map(row),
    do:
      text?(row["team_id"], 128) && text?(row["name"], 128) &&
        row["status"] in ~w(active suspended) && row["role"] in [nil | @team_roles] &&
        Enum.all?(~w(policy_epoch membership_epoch global_revision), &epoch?(row[&1]))

  defp valid_row?(:members, row, _) when is_map(row),
    do:
      text?(row["principal_id"], 128) && row["role"] in @team_roles && row["status"] in @states &&
        epoch?(row["membership_epoch"])

  defp valid_row?(:assignments, row, _) when is_map(row),
    do:
      text?(row["project_id"], 128) && row["role"] in @project_roles && row["status"] in @states &&
        row["project_status"] in ~w(active suspended disabled) && epoch?(row["assignment_epoch"]) &&
        epoch?(row["project_policy_epoch"])

  defp valid_row?(:invitations, row, _) when is_map(row),
    do:
      text?(row["principal_id"], 128) && row["role"] in @team_roles &&
        row["status"] in ~w(pending accepted revoked expired) &&
        text?(row["acceptance_state"], 64) &&
        Enum.all?(~w(team_membership_epoch created_at expires_at), &epoch?(row[&1])) &&
        Enum.all?(~w(accepted_at revoked_at), &(is_nil(row[&1]) || epoch?(row[&1])))

  defp valid_row?(:admins, row, _) when is_map(row),
    do:
      text?(row["principal_id"], 128) && row["status"] in ~w(active revoked) &&
        epoch?(row["authority_epoch"])

  defp valid_row?(:effective, row, _) when is_map(row),
    do:
      text?(row["project_id"], 128) && row["role"] in @project_roles && is_boolean(row["direct"]) &&
        is_boolean(row["team_derived"]) && epoch?(row["global_revision"])

  defp valid_row?(:credentials, row, params) when is_map(row),
    do:
      row["team_id"] == params["team_id"] && text?(row["upstream_name"], 256) &&
        text?(row["binding_id"], 256) && text?(row["custodian_principal_id"], 256) &&
        row["status"] in ~w(active revoked) && is_integer(row["generation"]) &&
        row["generation"] > 0 &&
        epoch?(row["rotated_at_millis"])

  defp valid_row?(_, _, _), do: false

  defp row_keys(:teams),
    do: ~w(team_id name role status policy_epoch membership_epoch global_revision)

  defp row_keys(:members), do: ~w(principal_id role status membership_epoch)

  defp row_keys(:assignments),
    do: ~w(project_id role status assignment_epoch project_status project_policy_epoch)

  defp row_keys(:invitations),
    do:
      ~w(principal_id role status team_membership_epoch created_at expires_at accepted_at revoked_at acceptance_state)

  defp row_keys(:admins), do: ~w(principal_id status authority_epoch)
  defp row_keys(:effective), do: ~w(project_id role direct team_derived global_revision)

  defp row_keys(:credentials),
    do:
      ~w(team_id upstream_name binding_id custodian_principal_id status generation rotated_at_millis)

  def decode_write(_intent, {:error, %Error{}} = error), do: error

  def decode_write(intent, {:ok, data}) when is_map(data) do
    p = intent.params
    action = intent.action

    valid =
      case action do
        "access.team.create" ->
          valid_row?(:teams, data, %{}) && data["team_id"] == p["team_id"] &&
            data["name"] == p["name"]

        "access.team.member.add" ->
          membership_result?(data, p)

        "access.team.member.activate" ->
          membership_result?(data, p) && data["status"] == "active"

        "access.team_invitation.accept" ->
          membership_result?(data, %{}) && data["status"] == "active"

        "access.team_invitation.create" ->
          data["team_id"] == p["team_id"] && data["role"] == p["role"] &&
            data["status"] == "pending" && epoch?(data["team_membership_epoch"]) &&
            epoch?(data["expires_at"])

        "access.team_project.unassign" ->
          data["team_id"] == p["team_id"] && data["project_id"] == p["project_id"] &&
            data["role"] in @project_roles && data["status"] == "revoked" &&
            epoch?(data["assignment_epoch"])

        "access.team_project.assign" ->
          data["team_id"] == p["team_id"] && data["project_id"] == p["project_id"] &&
            data["role"] == p["role"] && epoch?(data["assignment_epoch"])

        "access.gateway_credential.bind" ->
          valid_row?(:credentials, data, p) && data["upstream_name"] == p["upstream_name"] &&
            data["binding_id"] == p["binding_id"] && data["status"] == "active"

        "access.gateway_credential.revoke" ->
          is_map(data["binding"]) && valid_row?(:credentials, data["binding"], p) &&
            data["binding"]["upstream_name"] == p["upstream_name"] &&
            data["binding"]["status"] == "revoked"

        _ ->
          !is_nil(spec(action)) && data["ok"] == true
      end

    if valid do
      {:ok,
       case action do
         "access.team_invitation.accept" -> Map.take(data, ~w(team_id principal_id))
         "access.gateway_credential.bind" -> Map.take(data, ~w(generation))
         _ -> %{}
       end}
    else
      invalid_write(action)
    end
  end

  def decode_write(intent, _), do: invalid_write(intent.action)

  defp membership_result?(data, params) do
    valid_row?(:members, data, %{}) && text?(data["team_id"], 128) &&
      Enum.all?(Map.take(params, ~w(team_id principal_id role)), fn {key, value} ->
        data[key] == value
      end)
  end

  defp invalid_write(action),
    do: {:error, Error.new(:invalid_response, "access", action, side_effects: "unknown")}

  def write_result(state, intent, {:ok, data}) do
    %{
      close(state)
      | receipt:
          Map.merge(intent, %{
            status: :accepted,
            binding_generation_after: data["generation"],
            message: "Change accepted. Read-back is pending."
          })
    }
  end

  def write_result(state, intent, {:error, %Error{} = error}) do
    status = if error.side_effects in ~w(unknown possible), do: :uncertain, else: :failed

    %{
      close(state)
      | receipt:
          Map.merge(intent, %{
            status: status,
            message:
              if(status == :uncertain,
                do: "Outcome uncertain. Do not submit again. Refresh affected records only.",
                else: message(error)
              ),
            error: error
          })
    }
  end

  def read_result(state, card, {:ok, page}, params) do
    state =
      put_in(state.cards[card], %{
        status: if(page.next_offset, do: :partial, else: :loaded),
        rows: page.rows,
        metadata: page.metadata,
        revision: page.revision,
        next_offset: page.next_offset,
        offset: params["offset"] || 0,
        limit: params["limit"] || 50,
        error: nil
      })

    reconcile(state, card, params)
  end

  def read_result(state, card, {:error, %Error{} = error}, params) do
    old = state.cards[card]

    status =
      cond do
        error.kind == :forbidden -> :forbidden
        error.kind == :unknown_action -> :unavailable
        old.rows != [] -> :stale
        true -> :error
      end

    rows = if status in [:forbidden, :unavailable], do: [], else: old.rows
    state = put_in(state.cards[card], %{old | status: status, rows: rows, error: error})
    receipt = state.receipt

    if held?(state) && card in receipt.affected && affected_scope?(receipt, card, params) &&
         error.kind == :forbidden do
      uncertain_invitation =
        receipt.status == :uncertain &&
          String.starts_with?(receipt.action, "access.team_invitation.")

      status = if uncertain_invitation, do: :uncertain, else: :visibility_lost

      message =
        if uncertain_invitation,
          do:
            "Invitation outcome remains uncertain. Inventory access was denied. Do not submit again.",
          else:
            "Access to affected records was denied. Their lifecycle state cannot be inspected."

      %{state | receipt: %{receipt | status: status, message: message}}
    else
      state
    end
  end

  defp affected_scope?(receipt, card, params),
    do:
      !receipt.params["team_id"] || receipt.params["team_id"] == params["team_id"] ||
        card in [:teams, :effective, :admins]

  defp reconcile(%{receipt: nil} = state, _, _), do: state

  defp reconcile(state, card, params) do
    receipt = state.receipt
    same_team = affected_scope?(receipt, card, params)

    if held?(state) && card in receipt.affected && same_team &&
         proves?(receipt, card, state.cards[card]) do
      observed = MapSet.put(receipt.observed, card)
      complete = Enum.all?(receipt.affected, &MapSet.member?(observed, &1))

      %{
        state
        | receipt: %{
            receipt
            | observed: observed,
              status: if(complete, do: :reconciled, else: receipt.status),
              message:
                if(complete, do: "Affected gateway records reconciled.", else: receipt.message)
          }
      }
    else
      state
    end
  end

  defp proves?(
         %{action: "access.team_invitation.accept", status: :accepted} = receipt,
         :members,
         card
       ) do
    Enum.any?(
      card.rows,
      &(&1["principal_id"] == receipt.params["principal_id"] && &1["status"] == "active")
    )
  end

  defp proves?(%{action: "access.team_invitation.accept", status: :accepted}, :teams, card),
    do: card.status in [:loaded, :partial]

  defp proves?(%{action: "access.team_invitation." <> _}, _, _), do: false

  defp proves?(%{action: "access.gateway_credential.bind"} = receipt, :credentials, card) do
    before = receipt[:binding_generation_before]
    minimum = receipt[:binding_generation_after] || if(is_integer(before), do: before + 1)

    is_integer(minimum) &&
      Enum.any?(card.rows, fn row ->
        row["team_id"] == receipt.params["team_id"] &&
          row["upstream_name"] == receipt.params["upstream_name"] &&
          row["binding_id"] == receipt.params["binding_id"] && row["status"] == "active" &&
          row["generation"] >= minimum
      end)
  end

  defp proves?(%{action: action}, card, data)
       when action in ["access.team.suspend", "access.team.activate"] and
              card in [:members, :assignments, :invitations],
       do: data.status in [:loaded, :partial]

  defp proves?(%{action: "access.team.member." <> _}, :teams, card),
    do: card.status in [:loaded, :partial]

  defp proves?(receipt, :effective, card),
    do: card.status in [:loaded, :partial] && receipt.status == :accepted

  defp proves?(receipt, card, data) do
    p = receipt.params

    {key, id} =
      case card do
        :teams -> {"team_id", p["team_id"]}
        :members -> {"principal_id", p["principal_id"]}
        :assignments -> {"project_id", p["project_id"]}
        :admins -> {"principal_id", p["principal_id"]}
        :credentials -> {"upstream_name", p["upstream_name"]}
      end

    row = Enum.find(data.rows, &(&1[key] == id))

    desired =
      cond do
        String.ends_with?(receipt.action, ".suspend") ->
          "suspended"

        String.ends_with?(receipt.action, ".remove") ||
          String.ends_with?(receipt.action, ".revoke") ||
            String.ends_with?(receipt.action, ".unassign") ->
          "revoked"

        String.ends_with?(receipt.action, ".role.set") ->
          nil

        true ->
          "active"
      end

    row && (!desired || row["status"] == desired) && (!p["role"] || row["role"] == p["role"]) &&
      (!p["name"] || row["name"] == p["name"])
  end

  def accepted_observed(state, card) do
    # Visible invitation metadata cannot identify the submitted secret. Only a
    # known creation success may release this inventory hold.
    receipt = state.receipt

    if receipt && receipt.status == :accepted && card in receipt.affected do
      observed = MapSet.put(receipt.observed, card)
      complete = Enum.all?(receipt.affected, &MapSet.member?(observed, &1))

      %{
        state
        | receipt: %{
            receipt
            | observed: observed,
              status: if(complete, do: :accepted_observed, else: :accepted),
              message:
                if(complete,
                  do:
                    "Change accepted. Invitation records refreshed. Token identity is unavailable in this inventory.",
                  else: receipt.message
                )
          }
      }
    else
      state
    end
  end

  def affected("access.team.create"), do: [:teams]
  def affected("access.team.member." <> _), do: [:members, :teams]
  def affected("access.team_invitation.create"), do: [:invitations]
  def affected("access.team_invitation.accept"), do: [:members, :teams]
  def affected("access.team_project." <> _), do: [:assignments, :effective]
  def affected("access.platform_admin." <> _), do: [:admins]
  def affected("access.gateway_credential." <> _), do: [:credentials]
  def affected(_), do: [:teams, :members, :assignments, :invitations]

  def message(%Error{kind: :conflict, metadata: %{"existing_id" => "team_owner"}}),
    do: "Keep an active owner before changing this member. The last owner is protected."

  def message(%Error{kind: :conflict, metadata: %{"existing_id" => "access_inventory"}}),
    do: "This inventory changed. Refresh from the first page."

  def message(%Error{kind: :response_too_large, metadata: %{"budget_terminal" => true}}),
    do: "One record exceeds the response budget. Stop paging and contact an administrator."

  def message(%Error{kind: :response_too_large}),
    do: "This page exceeds the response budget. Choose a smaller page size and refresh."

  def message(%Error{kind: :forbidden}),
    do: "Access denied. Check your current Team and Project authority."

  def message(%Error{kind: kind}),
    do: "Access unavailable (#{kind}). Refresh to check current gateway state."

  defp text?(value, max),
    do:
      is_binary(value) && String.valid?(value) && byte_size(value) in 1..max &&
        value == String.trim(value) && !Regex.match?(~r/\p{Cc}/u, value)

  defp epoch?(value), do: is_integer(value) && value >= 0
  defp invalid(action), do: {:error, Error.new(:invalid_response, "access", action)}
end
